//! HZR-owned lifecycle for the optional agtx observer component.
//!
//! Everything about the helper that could hurt something lives here: which
//! binary is allowed to run, how long it may take, how many may run at once,
//! how a refusal is classified, and how a revoked enrollment discards a result
//! that was already in flight.
//!
//! Two invariants shape the module.
//!
//! **Absent is normal.** Nothing in this file runs unless a user explicitly
//! installed the component *and* explicitly enrolled a project. A daemon with
//! neither behaves exactly as it did before the integration existed, and a
//! daemon with an enrollment but no component reports `missing_component` and
//! degrades the Agents workspace alone.
//!
//! **A public GET never reaches the helper.** Dashboard reads serve durable
//! projections. Only this worker and the authenticated `sync` route spawn a
//! process, so a browser refresh loop cannot turn into a fork bomb against
//! somebody's board.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use hzr_core::{AgentSourceIdentity, AgtxConfig, AgtxProject, Config, privacy_identity_hash};
use hzr_protocol::agents::{
    AGENT_OBSERVER_PATCH_IDENTITY, AGENT_SNAPSHOT_SCHEMA_VERSION, AgentComponentStatus,
    AgentIntegrationState, AgentSnapshotEnvelope, AgentSnapshotFailure, AgentSnapshotRequest,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, RwLock, Semaphore};

use crate::ledger_writer::LedgerWriter;

/// Executable name inside HZR's private engine directory. Never resolved from
/// `PATH`: a component that a stray directory can substitute is not pinned.
pub const OBSERVER_BINARY: &str = "hzr-agtx-observer";

/// One snapshot must answer within this long, including process startup.
pub const HELPER_TIMEOUT: Duration = Duration::from_secs(3);
/// The one-off identity probe gets a larger budget than a poll: it runs at
/// startup and after an install, where a loaded machine's process spawn is slow
/// and a false "component is broken" is worse than waiting.
pub const COMPONENT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// A revoked enrollment's helper must be reaped within this long.
pub const REAP_TIMEOUT: Duration = Duration::from_secs(5);
/// At most this many helpers run across the whole daemon.
pub const MAX_CONCURRENT_HELPERS: usize = 2;
/// Pages one observation cycle will walk before stopping and continuing later.
pub const MAX_PAGES_PER_CYCLE: usize = 5;
/// Response bytes accepted from the helper.
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

const TASKS_PER_PAGE: usize = 200;
const EDGES_PER_PAGE: usize = 1_000;
const BACKOFF_MS: [u64; 5] = [5_000, 10_000, 20_000, 40_000, 60_000];

/// Platforms with a verified observer build. Anything else reports the
/// component as unavailable rather than pretending a build exists.
#[must_use]
pub fn platform_supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "linux"))
}

/// What HZR knows about the installed component.
#[derive(Clone, Debug, Default)]
pub struct ComponentInfo {
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub patch_identity: Option<String>,
    pub protocol_schema_version: Option<u32>,
    /// Reason the component cannot be used, when it cannot.
    pub error_code: Option<String>,
}

impl ComponentInfo {
    #[must_use]
    pub fn usable(&self) -> bool {
        self.path.is_some()
            && self.error_code.is_none()
            && self.patch_identity.as_deref() == Some(AGENT_OBSERVER_PATCH_IDENTITY)
            && self.protocol_schema_version == Some(AGENT_SNAPSHOT_SCHEMA_VERSION)
    }

    #[must_use]
    pub fn to_status(&self) -> AgentComponentStatus {
        AgentComponentStatus {
            installed: self.path.is_some(),
            path: self
                .path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            version: self.version.clone(),
            patch_identity: self.patch_identity.clone(),
            protocol_schema_version: self.protocol_schema_version,
            unsupported_platform: (!platform_supported())
                .then(|| format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)),
        }
    }
}

/// Where the component would live: HZR's own engine directory, or the data
/// directory's `components/` when no bundle engine directory is configured.
#[must_use]
pub fn component_path(config: &Config) -> PathBuf {
    config
        .engines
        .directory
        .clone()
        .unwrap_or_else(|| config.data_dir.join("components"))
        .join(OBSERVER_BINARY)
}

/// Probe the installed component by running `--version`.
///
/// The probe is the only thing that decides whether a binary is *the* pinned
/// observer: a matching file name proves nothing, and a build with a different
/// patch identity is a different producer.
pub async fn probe_component(config: &Config) -> ComponentInfo {
    probe_agent_component(component_path(config)).await
}

pub async fn probe_agent_component(path: PathBuf) -> ComponentInfo {
    if !platform_supported() {
        return ComponentInfo {
            error_code: Some("unsupported_platform".into()),
            ..Default::default()
        };
    }
    if !path.is_file() {
        return ComponentInfo {
            error_code: Some("missing_component".into()),
            ..Default::default()
        };
    }
    let output =
        bounded_helper_output(&path, &["--version"], &[], 4096, COMPONENT_PROBE_TIMEOUT).await;
    let Ok(output) = output else {
        return ComponentInfo {
            path: Some(path),
            error_code: Some("component_probe_failed".into()),
            ..Default::default()
        };
    };
    // `hzr-agtx-observer <version> schema=<n> patch=<identity>`
    let line = String::from_utf8_lossy(&output).trim().to_string();
    let mut info = ComponentInfo {
        path: Some(path),
        ..Default::default()
    };
    for (index, field) in line.split_whitespace().enumerate() {
        match (index, field) {
            (1, version) => info.version = Some(version.to_string()),
            (_, field) if field.starts_with("schema=") => {
                info.protocol_schema_version = field.trim_start_matches("schema=").parse().ok();
            }
            (_, field) if field.starts_with("patch=") => {
                info.patch_identity = Some(field.trim_start_matches("patch=").to_string());
            }
            _ => {}
        }
    }
    if line.split_whitespace().next() != Some(OBSERVER_BINARY)
        || info.version.as_deref() != Some("1.0.4")
        || info.patch_identity.as_deref() != Some(AGENT_OBSERVER_PATCH_IDENTITY)
        || info.protocol_schema_version != Some(AGENT_SNAPSHOT_SCHEMA_VERSION)
    {
        info.error_code = Some("incompatible".into());
    }
    info
}

/// Live per-enrollment state the dashboard and CLI report.
#[derive(Clone, Debug, Default)]
pub struct EnrollmentRuntime {
    pub state: AgentIntegrationState,
    pub generation: u64,
    pub last_error_code: Option<String>,
    pub last_success_ms: Option<i64>,
    pub source_observed_at_ms: Option<i64>,
    pub consecutive_failures: u32,
}

/// The daemon-owned observer.
#[derive(Clone)]
pub struct AgentObserver {
    inner: Arc<Inner>,
}

struct Inner {
    config: Arc<Config>,
    ledger: LedgerWriter,
    /// The live enrollment set, which `enable`/`disable` mutate at runtime.
    enrollments: RwLock<AgtxConfig>,
    runtime: RwLock<BTreeMap<PathBuf, EnrollmentRuntime>>,
    component: RwLock<ComponentInfo>,
    /// Bumped on every enable/disable so a result from a revoked generation is
    /// discarded rather than written.
    generation: AtomicU64,
    permits: Semaphore,
    project_locks: Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>,
    stop: AtomicBool,
}

/// The outcome of one observation cycle for one project.
#[derive(Clone, Debug, Default)]
pub struct ObservationOutcome {
    pub state: AgentIntegrationState,
    pub pages: u32,
    pub tasks_observed: u64,
    pub events_recorded: u64,
    pub tombstoned: u64,
    pub complete: bool,
    pub warnings: Vec<String>,
    pub error_code: Option<String>,
    pub project_id: Option<String>,
    /// When the source itself reported the observation, distinct from when HZR
    /// received it.
    pub source_observed_at_ms: Option<i64>,
}

impl AgentObserver {
    #[must_use]
    pub fn new(config: Arc<Config>, ledger: LedgerWriter) -> Self {
        let enrollments = config.integrations.agtx.clone();
        Self {
            inner: Arc::new(Inner {
                config,
                ledger,
                enrollments: RwLock::new(enrollments),
                runtime: RwLock::new(BTreeMap::new()),
                component: RwLock::new(ComponentInfo::default()),
                generation: AtomicU64::new(1),
                permits: Semaphore::new(MAX_CONCURRENT_HELPERS),
                project_locks: Mutex::new(BTreeMap::new()),
                stop: AtomicBool::new(false),
            }),
        }
    }

    pub async fn refresh_component(&self) -> ComponentInfo {
        let info = probe_component(&self.inner.config).await;
        *self.inner.component.write().await = info.clone();
        info
    }

    pub async fn component(&self) -> ComponentInfo {
        self.inner.component.read().await.clone()
    }

    pub async fn enrollments(&self) -> AgtxConfig {
        self.inner.enrollments.read().await.clone()
    }

    pub async fn runtime_state(&self) -> BTreeMap<PathBuf, EnrollmentRuntime> {
        self.inner.runtime.read().await.clone()
    }

    /// Adopt a new enrollment set and invalidate every in-flight observation.
    ///
    /// The generation bump is what makes `disable` immediate: a helper that is
    /// still running keeps running until its own timeout, but its result is
    /// dropped instead of being written for an enrollment the user revoked.
    pub async fn set_enrollments(&self, enrollments: AgtxConfig) {
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        let active: Vec<PathBuf> = enrollments
            .active_projects()
            .into_iter()
            .map(|project| project.project_path.clone())
            .collect();
        *self.inner.enrollments.write().await = enrollments;
        let mut runtime = self.inner.runtime.write().await;
        runtime.retain(|path, _| active.contains(path));
        for path in active {
            runtime.entry(path).or_default().state = AgentIntegrationState::Connecting;
        }
    }

    pub fn resume(&self) {
        self.inner.stop.store(false, Ordering::SeqCst);
    }

    pub fn stop(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
    }

    /// Integration state for one enrolled project, or `Disabled`.
    pub async fn state_for(&self, project_path: &Path) -> AgentIntegrationState {
        let enrollments = self.inner.enrollments.read().await;
        if !enrollments.enabled {
            return AgentIntegrationState::Disabled;
        }
        let enabled = enrollments
            .find(project_path)
            .is_some_and(|project| project.enabled);
        drop(enrollments);
        if !enabled {
            return AgentIntegrationState::Disabled;
        }
        let component = self.inner.component.read().await;
        if component.path.is_none() {
            return AgentIntegrationState::MissingComponent;
        }
        if component.error_code.as_deref() == Some("incompatible") {
            return AgentIntegrationState::Incompatible;
        }
        if !component.usable() {
            return AgentIntegrationState::Error;
        }
        drop(component);
        self.inner
            .runtime
            .read()
            .await
            .get(project_path)
            .map_or(AgentIntegrationState::Connecting, |runtime| runtime.state)
    }

    /// Run one bounded observation cycle for one enrolled project.
    pub async fn observe_once(&self, project_path: &Path) -> ObservationOutcome {
        let project_lock = self
            .inner
            .project_locks
            .lock()
            .await
            .entry(project_path.to_path_buf())
            .or_default()
            .clone();
        let Ok(_project_guard) = project_lock.try_lock() else {
            return ObservationOutcome {
                state: AgentIntegrationState::Connecting,
                error_code: Some("observation_in_progress".into()),
                ..Default::default()
            };
        };
        let enrollments = self.inner.enrollments.read().await.clone();
        let Some(project) = enrollments
            .find(project_path)
            .filter(|project| project.enabled && enrollments.enabled)
            .cloned()
        else {
            return ObservationOutcome {
                state: AgentIntegrationState::Disabled,
                ..Default::default()
            };
        };
        let generation = self.inner.generation.load(Ordering::SeqCst);
        // Re-probe once before declaring anything: a probe that failed under
        // load is a transient error, and reporting it as `incompatible` would
        // accuse the component of being the wrong build.
        let mut component = self.inner.component.read().await.clone();
        if !component.usable() {
            component = self.refresh_component().await;
        }
        if !component.usable() {
            let (state, code) = match component.error_code.as_deref() {
                Some("missing_component") => {
                    (AgentIntegrationState::MissingComponent, "missing_component")
                }
                Some("unsupported_platform") => (
                    AgentIntegrationState::MissingComponent,
                    "unsupported_platform",
                ),
                Some("incompatible") => (AgentIntegrationState::Incompatible, "incompatible"),
                Some(_) | None => (AgentIntegrationState::Error, "component_probe_failed"),
            };
            return self.finish(project_path, state, code).await;
        }
        let binary = component.path.clone().unwrap_or_default();
        let project_hash =
            privacy_identity_hash("project", &project.project_path.to_string_lossy());
        let mut outcome = ObservationOutcome {
            project_id: Some(project_hash.clone()),
            ..Default::default()
        };

        let Ok(_permit) = self.inner.permits.acquire().await else {
            return self
                .finish(
                    project_path,
                    AgentIntegrationState::Error,
                    "observer_shutdown",
                )
                .await;
        };

        let mut cursor: Option<String> = None;
        let mut identity: Option<AgentSourceIdentity> = None;
        for page in 0..MAX_PAGES_PER_CYCLE {
            if self.inner.generation.load(Ordering::SeqCst) != generation
                || self.inner.stop.load(Ordering::SeqCst)
            {
                outcome.state = AgentIntegrationState::Disabled;
                outcome.error_code = Some("enrollment_revoked".into());
                return outcome;
            }
            let request = AgentSnapshotRequest {
                schema_version: AGENT_SNAPSHOT_SCHEMA_VERSION,
                request_id: format!("{project_hash}:{generation}:{page}"),
                data_root: project.data_dir.to_string_lossy().into_owned(),
                project_path: project.project_path.to_string_lossy().into_owned(),
                cursor: cursor.clone(),
                task_limit: TASKS_PER_PAGE,
                edge_limit: EDGES_PER_PAGE,
                // Titles are what make a board readable, so they cross only
                // when the install says they may.
                include_titles: enrollments.publish_task_titles,
                include_hook_status: true,
            };
            let envelope = match run_helper(&binary, &request).await {
                Ok(envelope) => envelope,
                Err(code) => {
                    outcome.error_code = Some(code.clone());
                    let _ = self
                        .inner
                        .ledger
                        .record_agent_gap(
                            project_hash.clone(),
                            identity
                                .as_ref()
                                .map(|identity| identity.source_key.clone())
                                .unwrap_or_else(|| project_hash.clone()),
                            code.clone(),
                            now_ms(),
                        )
                        .await;
                    return self
                        .finish_outcome(project_path, AgentIntegrationState::Error, outcome)
                        .await;
                }
            };

            // A result that belongs to a revoked enrollment is dropped, not
            // written: disabling means monitoring stopped, and a late page must
            // not resurrect it.
            if self.inner.generation.load(Ordering::SeqCst) != generation {
                outcome.error_code = Some("enrollment_revoked".into());
                return self
                    .finish_outcome(project_path, AgentIntegrationState::Disabled, outcome)
                    .await;
            }

            let source = AgentSourceIdentity::new(
                &enrollment_id(&project),
                &envelope.source_instance_id,
                &project_hash,
            );
            identity = Some(source.clone());
            outcome.warnings.extend(
                envelope
                    .warnings
                    .iter()
                    .map(|warning| format!("{}:{}", warning.code, warning.count)),
            );
            outcome.source_observed_at_ms = Some(envelope.observed_at_ms);
            let next_cursor = envelope.next_cursor.clone();
            let complete = envelope.complete;
            match self
                .inner
                .ledger
                .record_agent_snapshot(
                    source,
                    envelope,
                    i64::try_from(enrollments.poll_interval_ms.saturating_mul(3))
                        .unwrap_or(i64::MAX),
                    i64::try_from(enrollments.stale_after_ms).unwrap_or(i64::MAX),
                )
                .await
            {
                Ok(applied) => {
                    outcome.pages += 1;
                    outcome.tasks_observed += applied.tasks_seen;
                    outcome.events_recorded += applied.events_recorded;
                    outcome.tombstoned += applied.tombstoned;
                    outcome.complete = complete;
                }
                Err(error) => {
                    outcome.error_code = Some(format!("ledger_write_failed:{error}"));
                    return self
                        .finish_outcome(project_path, AgentIntegrationState::Error, outcome)
                        .await;
                }
            }
            if complete {
                break;
            }
            cursor = next_cursor;
            if cursor.is_none() {
                break;
            }
        }

        let state = if outcome.complete {
            AgentIntegrationState::Ready
        } else {
            AgentIntegrationState::Partial
        };
        self.finish_outcome(project_path, state, outcome).await
    }

    async fn finish(
        &self,
        project_path: &Path,
        state: AgentIntegrationState,
        code: &str,
    ) -> ObservationOutcome {
        let outcome = ObservationOutcome {
            error_code: Some(code.to_string()),
            ..Default::default()
        };
        self.finish_outcome(project_path, state, outcome).await
    }

    async fn finish_outcome(
        &self,
        project_path: &Path,
        state: AgentIntegrationState,
        mut outcome: ObservationOutcome,
    ) -> ObservationOutcome {
        outcome.state = state;
        let mut runtime = self.inner.runtime.write().await;
        let entry = runtime.entry(project_path.to_path_buf()).or_default();
        entry.state = state;
        entry.last_error_code = outcome.error_code.clone();
        entry.generation = self.inner.generation.load(Ordering::SeqCst);
        if outcome.source_observed_at_ms.is_some() {
            entry.source_observed_at_ms = outcome.source_observed_at_ms;
        }
        if outcome.error_code.is_none() {
            entry.last_success_ms = Some(now_ms());
            entry.consecutive_failures = 0;
        } else {
            entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
        }
        outcome
    }

    /// The background loop. Schedules the next poll *after* the previous one
    /// finishes, so a slow source cannot pile overlapping helpers on itself.
    pub async fn run(self) {
        loop {
            if self.inner.stop.load(Ordering::SeqCst) {
                return;
            }
            let enrollments = self.inner.enrollments.read().await.clone();
            let projects: Vec<AgtxProject> =
                enrollments.active_projects().into_iter().cloned().collect();
            if projects.is_empty() {
                // Nothing enrolled: idle without touching a store or a process.
                tokio::time::sleep(Duration::from_millis(enrollments.poll_interval_ms)).await;
                continue;
            }
            if !self.component().await.usable() {
                self.refresh_component().await;
            }
            let mut longest_backoff = enrollments.poll_interval_ms;
            for project in projects {
                if self.inner.stop.load(Ordering::SeqCst) {
                    return;
                }
                let outcome = self.observe_once(&project.project_path).await;
                if outcome.error_code.is_some() {
                    let failures = self
                        .inner
                        .runtime
                        .read()
                        .await
                        .get(&project.project_path)
                        .map_or(1, |runtime| runtime.consecutive_failures);
                    longest_backoff = longest_backoff.max(backoff_ms(failures));
                }
            }
            let _ = self
                .inner
                .ledger
                .prune_agent_events(enrollments.history_retention_days, now_ms())
                .await;
            tokio::time::sleep(Duration::from_millis(longest_backoff)).await;
        }
    }
}

/// Backoff for consecutive failures, with bounded jitter so a fleet of daemons
/// does not retry a shared source in lockstep.
#[must_use]
pub fn backoff_ms(consecutive_failures: u32) -> u64 {
    let index = (consecutive_failures.max(1) as usize - 1).min(BACKOFF_MS.len() - 1);
    let base = BACKOFF_MS[index];
    let jitter = u64::from(std::process::id() % 500);
    base.saturating_add(jitter)
}

/// Stable per-enrollment identity: HZR's own, independent of the source store.
#[must_use]
pub fn enrollment_id(project: &AgtxProject) -> String {
    privacy_identity_hash(
        "agtx_enrollment",
        &format!(
            "{}\u{0}{}",
            project.project_path.to_string_lossy(),
            project.data_dir.to_string_lossy()
        ),
    )
}

/// Spawn the helper once, by argv, with no shell and no inherited environment
/// that could redirect it.
async fn bounded_helper_output(
    binary: &Path,
    args: &[&str],
    payload: &[u8],
    max_bytes: usize,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let mut child = tokio::process::Command::new(binary)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_remove("AGTX_DATA_DIR")
        .env_remove("AGTX_CONFIG_DIR")
        .env_remove("AGTX_AGENT_HOME")
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "helper_spawn_failed".to_string())?;
    let result = tokio::time::timeout(timeout, async {
        let mut stdin = child.stdin.take().ok_or("helper_io_failed")?;
        let stdout = child.stdout.take().ok_or("helper_io_failed")?;
        let write = async {
            stdin
                .write_all(payload)
                .await
                .map_err(|_| "helper_io_failed")?;
            stdin.shutdown().await.map_err(|_| "helper_io_failed")?;
            drop(stdin);
            Ok::<(), &str>(())
        };
        let read = async {
            let mut output = Vec::new();
            stdout
                .take(max_bytes as u64 + 1)
                .read_to_end(&mut output)
                .await
                .map_err(|_| "helper_io_failed")?;
            if output.len() > max_bytes {
                return Err("helper_response_too_large");
            }
            Ok(output)
        };
        let (_, output) = tokio::try_join!(write, read)?;
        let status = child.wait().await.map_err(|_| "helper_io_failed")?;
        if !status.success() {
            return Err("helper_exit_failed");
        }
        Ok(output)
    })
    .await;
    let result = result
        .unwrap_or(Err("helper_timeout"))
        .map_err(str::to_string);
    if result.is_err() {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(REAP_TIMEOUT, child.wait()).await;
    }
    result
}

async fn run_helper(
    binary: &Path,
    request: &AgentSnapshotRequest,
) -> Result<AgentSnapshotEnvelope, String> {
    let payload = serde_json::to_vec(request).map_err(|_| "request_encode_failed".to_string())?;
    let output = bounded_helper_output(
        binary,
        &["snapshot", "--request-stdin"],
        &payload,
        MAX_RESPONSE_BYTES,
        HELPER_TIMEOUT,
    )
    .await?;
    let stdout = String::from_utf8_lossy(&output);
    let envelope: AgentSnapshotEnvelope = match serde_json::from_str(stdout.trim()) {
        Ok(envelope) => envelope,
        Err(_) => {
            // A refusal is JSON too; report its stable code rather than the
            // parse failure, and never echo the helper's stderr text.
            if let Ok(failure) = serde_json::from_str::<AgentSnapshotFailure>(stdout.trim()) {
                return Err(format!("helper_refused:{}", failure.error));
            }
            return Err("helper_invalid_response".into());
        }
    };
    if envelope.schema_version != AGENT_SNAPSHOT_SCHEMA_VERSION {
        return Err("incompatible_schema_version".into());
    }
    if envelope.patch_identity != AGENT_OBSERVER_PATCH_IDENTITY
        || envelope.upstream_commit != "d307c4c182dff19a65370a50403185cb826f7f49"
        || envelope.upstream_version != "1.0.4"
    {
        return Err("unexpected_producer".into());
    }
    if envelope.request_id != request.request_id {
        return Err("request_id_mismatch".into());
    }
    Ok(envelope)
}

#[must_use]
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
