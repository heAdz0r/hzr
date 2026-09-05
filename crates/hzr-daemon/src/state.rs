use std::net::{Ipv4Addr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use hzr_context::ContextPlanner;
use hzr_core::Config;
use hzr_exec::{ExecutionPipeline, ForkRuntimePaths, PinnedRtkAdapter, RtkAdapterConfig};
use hzr_memory::{
    IcmConfig, IcmSupervisor, IcmTransport, ServiceStatus, StartOutcome, StopOutcome,
};
use hzr_protocol::DashboardLifecycleKind;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::DaemonError;
use crate::approval::ApprovalStore;
use crate::ledger_writer::LedgerWriter;
use crate::observability::ObservabilityStore;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub started_at: Instant,
    pub approvals: ApprovalStore,
    pub context: Arc<ContextPlanner>,
    pub index_maintenance_stop: Arc<AtomicBool>,
    pub index_maintenance_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    pub memory: Arc<IcmSupervisor>,
    pub memory_start: Arc<RwLock<MemoryStartState>>,
    pub memory_recovery_stop: Arc<AtomicBool>,
    pub memory_recovery_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    pub observability: ObservabilityStore,
    pub rtk: Arc<PinnedRtkAdapter>,
    pub executor: ExecutionPipeline,
    pub ledger: LedgerWriter,
    pub(crate) exec_jobs: crate::exec_jobs::ExecJobs,
    pub(crate) read_costs: crate::read_cost::ReadCosts,
}

#[derive(Clone, Debug)]
pub enum MemoryStartState {
    Disabled,
    Starting,
    Ready(StartOutcome),
    Degraded(String),
}

impl AppState {
    pub async fn initialize(config: Config) -> Result<Self, DaemonError> {
        let started_at = Instant::now();
        config
            .validate()
            .map_err(|error| DaemonError::Config(error.to_string()))?;
        config
            .ensure_layout()
            .map_err(|error| DaemonError::Config(error.to_string()))?;
        let ledger = LedgerWriter::open(&config.data_dir.join("ledger/hzr.sqlite"))
            .map_err(|error| DaemonError::Ledger(error.to_string()))?;

        let icm_config = managed_icm_config(&config)?;
        let memory = Arc::new(IcmSupervisor::new(icm_config).map_err(DaemonError::Memory)?);
        let memory_start = if config.engines.auto_start_icm {
            MemoryStartState::Starting
        } else {
            MemoryStartState::Disabled
        };
        let memory_start = Arc::new(RwLock::new(memory_start));
        let memory_recovery_stop = Arc::new(AtomicBool::new(false));
        let observability = ObservabilityStore::new(ledger.privacy_pseudonymizer());
        observability.record_lifecycle(
            "hzrd",
            DashboardLifecycleKind::Starting,
            None,
            "daemon_initializing",
            None,
        );
        let rtk = PinnedRtkAdapter::detect(RtkAdapterConfig {
            binary: config.engines.binary("rtk"),
            runtime_paths: Some(ForkRuntimePaths::from_data_root(&config.data_dir)),
            ..RtkAdapterConfig::default()
        })
        .await;
        let context = Arc::new(ContextPlanner::from_config(
            &config,
            memory.client(),
            rtk.runner(),
        ));
        let index_maintenance_stop = Arc::new(AtomicBool::new(false));
        let index_maintenance_task = if config.engines.auto_index {
            let background_context = Arc::clone(&context);
            let background_stop = Arc::clone(&index_maintenance_stop);
            let background_observability = observability.clone();
            let sweep_interval = Duration::from_secs(config.engines.grepai_watcher_sweep_seconds);
            Some(tokio::spawn(async move {
                loop {
                    tokio::time::sleep(sweep_interval).await;
                    if background_stop.load(Ordering::Acquire) {
                        return;
                    }
                    match background_context.reap_idle_indexes().await {
                        Ok(reaped) if reaped > 0 => background_observability.record_lifecycle(
                            "grepai",
                            DashboardLifecycleKind::Reaped,
                            None,
                            "idle_watchers_reaped",
                            None,
                        ),
                        Ok(_) => {}
                        Err(error) => {
                            background_observability.record_lifecycle(
                                "grepai",
                                DashboardLifecycleKind::Degraded,
                                None,
                                "watcher_maintenance_failed",
                                None,
                            );
                            tracing::warn!(%error, "grepai watcher maintenance failed");
                        }
                    }
                }
            }))
        } else {
            None
        };
        let memory_recovery_task = if config.engines.auto_start_icm {
            let background_memory = Arc::clone(&memory);
            let background_state = Arc::clone(&memory_start);
            let background_stop = Arc::clone(&memory_recovery_stop);
            let background_observability = observability.clone();
            Some(tokio::spawn(async move {
                supervise_memory(
                    background_memory,
                    background_state,
                    background_stop,
                    background_observability,
                    MemoryRecoveryPolicy::default(),
                )
                .await;
            }))
        } else {
            None
        };
        observability.record_lifecycle(
            "hzrd",
            DashboardLifecycleKind::Ready,
            None,
            "daemon_ready",
            None,
        );
        let exec_jobs =
            crate::exec_jobs::ExecJobs::new(&config.data_dir).map_err(DaemonError::Io)?;
        Ok(Self {
            exec_jobs,
            read_costs: crate::read_cost::ReadCosts::default(),
            config: Arc::new(config),
            started_at,
            approvals: ApprovalStore::default(),
            context,
            index_maintenance_stop,
            index_maintenance_task: Arc::new(Mutex::new(index_maintenance_task)),
            memory,
            memory_start,
            memory_recovery_stop,
            memory_recovery_task: Arc::new(Mutex::new(memory_recovery_task)),
            observability,
            rtk: Arc::new(rtk),
            executor: ExecutionPipeline,
            ledger,
        })
    }
}

fn managed_icm_config(config: &Config) -> Result<IcmConfig, DaemonError> {
    // The HTTP transport is private to this daemon. Reserving a fresh loopback port
    // avoids collisions with another isolated daemon, such as release smoke tests,
    // while the per-data-root lock still prevents duplicate writers to one store.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|error| {
        DaemonError::Config(format!(
            "failed to reserve a loopback port for ICM: {error}"
        ))
    })?;
    let bind_addr = listener.local_addr().map_err(|error| {
        DaemonError::Config(format!("failed to resolve the reserved ICM port: {error}"))
    })?;
    drop(listener);

    let mut icm_config =
        IcmConfig::from_data_root(config.engines.binary("icm"), config.data_dir.clone());
    icm_config.bind_addr = bind_addr;
    icm_config.embeddings = config.engines.icm_embeddings;
    icm_config.transport = IcmTransport::Http;
    icm_config.cli_fallback = false;
    Ok(icm_config)
}

#[derive(Clone, Copy, Debug)]
struct MemoryRecoveryPolicy {
    health_poll: Duration,
    stable_health_polls: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
}

impl Default for MemoryRecoveryPolicy {
    fn default() -> Self {
        Self {
            health_poll: Duration::from_secs(1),
            stable_health_polls: 5,
            initial_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_secs(30),
        }
    }
}

impl MemoryRecoveryPolicy {
    fn backoff(self, consecutive_failures: u32) -> Duration {
        let exponent = consecutive_failures.saturating_sub(1).min(16);
        let multiplier = 1_u32.checked_shl(exponent).unwrap_or(u32::MAX);
        self.initial_backoff
            .saturating_mul(multiplier)
            .min(self.max_backoff)
    }
}

async fn supervise_memory(
    memory: Arc<IcmSupervisor>,
    state: Arc<RwLock<MemoryStartState>>,
    stop: Arc<AtomicBool>,
    observability: ObservabilityStore,
    policy: MemoryRecoveryPolicy,
) {
    let mut consecutive_failures = 0_u32;
    while !stop.load(Ordering::Acquire) {
        *state.write().await = MemoryStartState::Starting;
        observability.record_lifecycle(
            "icm",
            DashboardLifecycleKind::Starting,
            None,
            "supervisor_start_attempt",
            None,
        );
        match memory.start_unless_cancelled(&stop).await {
            Ok(Some(outcome)) => {
                *state.write().await = MemoryStartState::Ready(outcome);
                observability.record_lifecycle(
                    "icm",
                    DashboardLifecycleKind::Ready,
                    None,
                    "http_transport_ready",
                    None,
                );
                let mut stable_polls = 0_u32;
                let mut failed_polls = 0_u32;
                loop {
                    tokio::time::sleep(policy.health_poll).await;
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    match memory.status().await {
                        ServiceStatus::Running { .. } | ServiceStatus::Attached { .. } => {
                            failed_polls = 0;
                            stable_polls = stable_polls.saturating_add(1);
                            if stable_polls >= policy.stable_health_polls {
                                consecutive_failures = 0;
                            }
                        }
                        status => {
                            if matches!(status, ServiceStatus::Unready { pid: Some(_), .. }) {
                                stable_polls = 0;
                                failed_polls = failed_polls.saturating_add(1);
                                if failed_polls < 3 {
                                    continue;
                                }
                                if let Err(error) = memory.stop_unready_owned().await {
                                    tracing::warn!(%error, "unable to stop unready owned ICM");
                                }
                            }
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            let delay = policy.backoff(consecutive_failures);
                            record_memory_degraded(
                                &state,
                                format!("post-ready status changed to {status:?}"),
                                delay,
                            )
                            .await;
                            observability.record_lifecycle(
                                "icm",
                                DashboardLifecycleKind::Degraded,
                                None,
                                "post_ready_health_failed",
                                None,
                            );
                            observability.record_lifecycle(
                                "icm",
                                DashboardLifecycleKind::RestartScheduled,
                                None,
                                "bounded_backoff",
                                None,
                            );
                            tokio::time::sleep(delay).await;
                            break;
                        }
                    }
                }
            }
            Ok(None) => return,
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                let delay = policy.backoff(consecutive_failures);
                record_memory_degraded(&state, error.to_string(), delay).await;
                observability.record_lifecycle(
                    "icm",
                    DashboardLifecycleKind::Degraded,
                    None,
                    "startup_failed",
                    None,
                );
                observability.record_lifecycle(
                    "icm",
                    DashboardLifecycleKind::RestartScheduled,
                    None,
                    "bounded_backoff",
                    None,
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
}

pub(crate) async fn stop_memory_supervision(state: &AppState) -> hzr_memory::Result<StopOutcome> {
    state.memory_recovery_stop.store(true, Ordering::Release);
    let outcome = state.memory.stop().await;
    if let Some(task) = state.memory_recovery_task.lock().await.take() {
        task.abort();
        let _ = task.await;
    }
    state.observability.record_lifecycle(
        "icm",
        DashboardLifecycleKind::Stopped,
        None,
        "supervision_stopped",
        None,
    );
    outcome
}

pub(crate) async fn stop_index_maintenance(
    state: &AppState,
) -> Result<(), hzr_context::ContextError> {
    state.index_maintenance_stop.store(true, Ordering::Release);
    let outcome = state.context.shutdown().await;
    if let Some(task) = state.index_maintenance_task.lock().await.take() {
        task.abort();
        let _ = task.await;
    }
    state.observability.record_lifecycle(
        "grepai",
        DashboardLifecycleKind::Stopped,
        None,
        "maintenance_stopped",
        None,
    );
    outcome
}

async fn record_memory_degraded(
    state: &RwLock<MemoryStartState>,
    reason: String,
    retry_delay: Duration,
) {
    let detail = format!(
        "managed ICM HTTP transport unavailable; CLI fallback disabled: {reason}; retry in {} ms",
        retry_delay.as_millis()
    );
    tracing::warn!(reason = %detail, "ICM supervision degraded");
    *state.write().await = MemoryStartState::Degraded(detail);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use axum::routing::{get, post};
    use axum::{Json, Router};
    use hzr_core::Config;
    use hzr_memory::{
        IcmConfig, IcmSupervisor, IcmTransport, MemoryTransport, StopOutcome, StoreRequest,
    };
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use tokio::sync::RwLock;

    use super::{MemoryRecoveryPolicy, MemoryStartState, managed_icm_config, supervise_memory};

    #[test]
    fn daemon_icm_transport_is_typed_and_has_no_cli_fallback() {
        let config = managed_icm_config(&Config::default()).expect("managed ICM config");

        assert_eq!(config.transport, IcmTransport::Http);
        assert!(!config.cli_fallback);
        assert!(config.bind_addr.ip().is_loopback());
        assert_ne!(config.bind_addr.port(), 0);
    }

    #[test]
    fn memory_recovery_backoff_is_exponential_and_bounded() {
        let policy = MemoryRecoveryPolicy {
            health_poll: Duration::from_millis(1),
            stable_health_polls: 2,
            initial_backoff: Duration::from_millis(25),
            max_backoff: Duration::from_millis(100),
        };

        assert_eq!(policy.backoff(1), Duration::from_millis(25));
        assert_eq!(policy.backoff(2), Duration::from_millis(50));
        assert_eq!(policy.backoff(3), Duration::from_millis(100));
        assert_eq!(policy.backoff(u32::MAX), Duration::from_millis(100));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn memory_supervision_recovers_flapping_process_without_cli_or_duplicate_store()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let executable = fake_flapping_icm(&temp)?;
        let starts = temp.path().join("starts");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let stores = Arc::new(AtomicUsize::new(0));
        let server_stores = Arc::clone(&stores);
        let server = tokio::spawn(async move {
            let router = Router::new()
                .route(
                    "/health",
                    get(|| async { Json(json!({"status":"ok","has_embedder":false})) }),
                )
                .route(
                    "/stats",
                    get(|| async {
                        Json(json!({
                            "total_memories": 0,
                            "total_topics": 0,
                            "avg_weight": 0.0
                        }))
                    }),
                )
                .route(
                    "/store",
                    post(move || {
                        let stores = Arc::clone(&server_stores);
                        async move {
                            stores.fetch_add(1, Ordering::AcqRel);
                            Json(json!([memory_record("01HZRRECOVERED", "stored once")]))
                        }
                    }),
                );
            let _ = axum::serve(listener, router).await;
        });

        let mut config = IcmConfig::from_data_root(&executable, temp.path());
        config.bind_addr = address;
        config.transport = IcmTransport::Http;
        config.cli_fallback = false;
        config.startup_timeout = Duration::from_secs(1);
        config.request_timeout = Duration::from_millis(100);
        // 0.8.3: the `--version` probe spawns a process; under parallel test load one second
        // was not enough and the fixture reported a timeout that had nothing to do with recovery.
        config.cli_timeout = Duration::from_secs(5);
        config.shutdown_timeout = Duration::from_secs(1);
        let memory = Arc::new(IcmSupervisor::new(config)?);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if memory.client().readiness().await.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        let state = Arc::new(RwLock::new(MemoryStartState::Starting));
        let stop = Arc::new(AtomicBool::new(false));
        let policy = MemoryRecoveryPolicy {
            health_poll: Duration::from_millis(5),
            stable_health_polls: u32::MAX,
            initial_backoff: Duration::from_millis(20),
            max_backoff: Duration::from_millis(40),
        };
        let task = tokio::spawn(supervise_memory(
            Arc::clone(&memory),
            Arc::clone(&state),
            Arc::clone(&stop),
            crate::observability::ObservabilityStore::new(
                hzr_core::PrivacyPseudonymizer::from_key("22".repeat(32))
                    .expect("valid test privacy key"),
            ),
            policy,
        ));

        let mut degraded = Vec::new();
        // 0.8.3: the window bounds a loaded machine, not the recovery itself, which takes tens
        // of milliseconds; three seconds failed with default test threads on a busy workstation.
        let recovered = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let MemoryStartState::Degraded(reason) = &*state.read().await {
                    if degraded.last() != Some(reason) {
                        degraded.push(reason.clone());
                    }
                }
                let saw_initial_backoff = degraded
                    .iter()
                    .any(|reason| reason.contains("retry in 20 ms"));
                let saw_capped_backoff = degraded
                    .iter()
                    .any(|reason| reason.contains("retry in 40 ms"));
                if start_count(&starts) >= 3
                    && matches!(&*state.read().await, MemoryStartState::Ready(_))
                    && saw_initial_backoff
                    && saw_capped_backoff
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await;
        assert!(
            recovered.is_ok(),
            "supervision did not recover: starts={}, state={:?}, degraded={degraded:?}",
            start_count(&starts),
            *state.read().await
        );

        assert!(
            degraded
                .iter()
                .any(|reason| reason.contains("retry in 20 ms"))
        );
        assert!(
            degraded
                .iter()
                .any(|reason| reason.contains("retry in 40 ms"))
        );
        let receipt = memory
            .client()
            .store(&StoreRequest::new("recovery", "stored once"))
            .await?;
        assert_eq!(receipt.transport, MemoryTransport::Http);
        assert_eq!(
            receipt.memory.as_ref().map(|memory| memory.id.as_str()),
            Some("01HZRRECOVERED")
        );
        assert_eq!(stores.load(Ordering::Acquire), 1);

        stop.store(true, Ordering::Release);
        let _ = memory.stop().await?;
        let starts_at_stop = start_count(&starts);
        tokio::time::timeout(Duration::from_secs(1), task).await??;
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(start_count(&starts), starts_at_stop);
        assert!(matches!(memory.stop().await?, StopOutcome::AlreadyStopped));
        server.abort();
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn memory_supervision_replaces_alive_unready_child()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new()?;
        let starts = temp.path().join("starts-hung");
        let executable = temp.path().join("fake-unready-icm");
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'icm 0.10.61'; exit 0; fi\nprintf 'x\\n' >> '{}'\nexec sleep 60\n",
                starts.display(),
            ),
        )?;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let unhealthy = Arc::new(AtomicBool::new(false));
        let failures = Arc::new(AtomicUsize::new(0));
        let health_flag = Arc::clone(&unhealthy);
        let health_failures = Arc::clone(&failures);
        let health_starts = starts.clone();
        let application = Router::new()
            .route(
                "/health",
                get(move || {
                    let flag = Arc::clone(&health_flag);
                    let failures = Arc::clone(&health_failures);
                    let starts = health_starts.clone();
                    async move {
                        let status = if flag.load(Ordering::Acquire) && start_count(&starts) == 1 {
                            failures.fetch_add(1, Ordering::AcqRel);
                            axum::http::StatusCode::SERVICE_UNAVAILABLE
                        } else {
                            axum::http::StatusCode::OK
                        };
                        (status, Json(json!({"status":"ok","has_embedder":false})))
                    }
                }),
            )
            .route(
                "/stats",
                get(|| async {
                    Json(json!({"total_memories":0,"total_topics":0,"avg_weight":0.0}))
                }),
            );
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, application).await;
        });
        let mut config = IcmConfig::from_data_root(executable, temp.path());
        config.bind_addr = address;
        config.transport = IcmTransport::Http;
        config.cli_fallback = false;
        config.request_timeout = Duration::from_millis(100);
        config.startup_timeout = Duration::from_secs(3);
        config.shutdown_timeout = Duration::from_secs(1);
        let memory = Arc::new(IcmSupervisor::new(config)?);
        let state = Arc::new(RwLock::new(MemoryStartState::Starting));
        let stop = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(supervise_memory(
            Arc::clone(&memory),
            Arc::clone(&state),
            Arc::clone(&stop),
            crate::observability::ObservabilityStore::new(
                hzr_core::PrivacyPseudonymizer::from_key("22".repeat(32))?,
            ),
            MemoryRecoveryPolicy {
                health_poll: Duration::from_millis(20),
                stable_health_polls: 5,
                initial_backoff: Duration::from_millis(20),
                max_backoff: Duration::from_millis(40),
            },
        ));
        tokio::time::timeout(Duration::from_secs(5), async {
            while !matches!(&*state.read().await, MemoryStartState::Ready(_)) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        unhealthy.store(true, Ordering::Release);
        tokio::time::timeout(Duration::from_secs(5), async {
            while start_count(&starts) != 2
                || !matches!(&*state.read().await, MemoryStartState::Ready(_))
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        assert!(
            failures.load(Ordering::Acquire) >= 3,
            "transient failures must not trigger an immediate restart"
        );
        stop.store(true, Ordering::Release);
        assert_eq!(memory.stop().await?, StopOutcome::Stopped);
        tokio::time::timeout(Duration::from_secs(1), task).await??;
        assert_eq!(start_count(&starts), 2);
        server.abort();
        Ok(())
    }

    #[cfg(unix)]
    fn fake_flapping_icm(temp: &TempDir) -> std::io::Result<std::path::PathBuf> {
        use std::os::unix::fs::PermissionsExt;

        let path = temp.path().join("fake-flapping-icm");
        let starts = temp.path().join("starts");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then printf 'icm 0.10.61\\n'; exit 0; fi\n\
             case \" $* \" in\n\
               *\" serve \"*) printf 'x\\n' >> '{starts}'; sleep 0.03 ;;\n\
               *) printf 'unexpected arguments\\n' >&2; exit 2 ;;\n\
             esac\n",
            starts = starts.display()
        );
        std::fs::write(&path, script)?;
        let mut permissions = std::fs::metadata(&path)?.permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions)?;
        Ok(path)
    }

    fn start_count(path: &std::path::Path) -> usize {
        std::fs::read_to_string(path)
            .map(|content| content.lines().count())
            .unwrap_or_default()
    }

    fn memory_record(id: &str, summary: &str) -> Value {
        json!({
            "id": id,
            "created_at": "2026-08-25T00:00:00Z",
            "updated_at": "2026-08-25T00:00:00Z",
            "last_accessed": "2026-08-25T00:00:00Z",
            "access_count": 0,
            "weight": 1.0,
            "topic": "recovery",
            "summary": summary,
            "raw_excerpt": null,
            "keywords": [],
            "importance": "medium",
            "source": {"type":"manual"},
            "related_ids": [],
            "scope": "user"
        })
    }
}
