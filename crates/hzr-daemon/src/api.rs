use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::State;
use hzr_context::{ContextError, PlanRequest, SearchRequest};
use hzr_core::{Ledger, LedgerRecord, locked_engines};
use hzr_exec::{
    CanonicalCommand, CaptureConfig, CaptureOverflow, CapturedContent, ExecutionEnvelope,
    ExecutionOutcome, ForkCoreInvocation, NotStarted, PINNED_RTK_VERSION, RewriteDecision,
    RewriteSource, StdinSpec, TerminationCause,
};
use hzr_index::{
    Deadlines, IndexCoordinatorSnapshot, IndexWatcherState, Workspace, WorkspaceRegistration,
    registered_workspaces,
};
use hzr_memory::{
    GLOBAL_SCOPE_TOKEN, Importance, MemoryNamespace, ProjectMemorySnapshot, RecallRequest,
    ServiceStatus, StoreRequest, global_topic, isolate_memories, merge_memories, namespaced_topic,
    read_project_snapshot, recall_candidate_limit, validate_memory_kind,
};
use hzr_protocol::{
    CodecApiRequest, CommandTermination, ContextPlanApiRequest, ContextPlanApiResponse,
    DashboardEstimatedEfficiency, DashboardHelpCommand, DashboardIndexArtifacts,
    DashboardIndexObservatory, DashboardIndexWatcher, DashboardLocalActivity,
    DashboardLocalOperation, DashboardMemoryEdge, DashboardMemoryObservatory,
    DashboardMemoryRetrieval, DashboardMemoryTopic, DashboardObservedUsage,
    DashboardOperationRoute, DashboardProject, DashboardProjectArtifacts, DashboardProjectState,
    DashboardProviderReceiptState, DashboardProviderReceipts, DashboardResponse,
    DashboardSemanticCanary, DashboardService, DashboardState, EngineHealth, EngineState,
    ExecApiRequest, ExecApprovalApiRequest, ForkRunApiRequest, ForkRunApiResponse, HealthResponse,
    MemoryImportance, MemoryRecallApiRequest, MemoryScopeSelector, MemoryStoreApiRequest,
    MemoryWriteScope, PROTOCOL_VERSION, SearchApiRequest, SearchApiResponse, SearchMode,
    SearchStrategy, TraceId, UsageApiRequest, UsageApiResponse,
};

use crate::approval::PendingApproval;
use crate::error::ApiError;
use crate::state::{AppState, CachedSemanticCanary, MemoryStartState};

pub async fn health(State(state): State<AppState>) -> Result<Json<HealthResponse>, ApiError> {
    let (memory_health, _) = memory_engine_health(&state).await;
    Ok(Json(health_response(&state, memory_health)))
}

fn health_response(state: &AppState, memory_health: EngineHealth) -> HealthResponse {
    let rtk_capabilities = state.rtk.capabilities();
    let rtk_ready = matches!(
        &rtk_capabilities.rewrite,
        hzr_exec::RtkRewriteInterface::ForkCli
    );
    let engines = vec![
        EngineHealth {
            name: "grepai".into(),
            version: Some(hzr_index::SUPPORTED_GREPAI_VERSION.into()),
            state: EngineState::Stopped,
            detail: Some("workspace index is started on demand".into()),
        },
        memory_health,
        EngineHealth {
            name: "rtk".into(),
            version: rtk_capabilities.detected_version.clone(),
            state: if rtk_ready {
                EngineState::Ready
            } else {
                EngineState::Degraded
            },
            detail: Some(format!("{:?}", rtk_capabilities.rewrite)),
        },
        EngineHealth {
            name: "caveman-code".into(),
            version: Some("0.65.2".into()),
            state: EngineState::Stopped,
            detail: Some("managed agent runtime is launched by hzr agent".into()),
        },
    ];
    let overall = if engines
        .iter()
        .any(|engine| engine.state == EngineState::Degraded)
    {
        EngineState::Degraded
    } else {
        EngineState::Ready
    };

    HealthResponse {
        protocol_version: PROTOCOL_VERSION,
        hzr_version: env!("CARGO_PKG_VERSION").into(),
        state: overall,
        workspace_root: None,
        engines,
        capabilities: vec![
            "search".into(),
            "context".into(),
            "memory".into(),
            "exec".into(),
            "codec".into(),
        ],
    }
}

pub async fn dashboard(State(state): State<AppState>) -> Result<Json<DashboardResponse>, ApiError> {
    let (memory_health, memory_retrieval) = memory_engine_health(&state).await;
    let health = health_response(&state, memory_health.clone());
    let registry = registered_workspaces(&state.config.data_dir);
    let registry_warnings = registry.warnings.len();
    let selected = registry.registrations.first().cloned();
    let projects = registry
        .registrations
        .iter()
        .map(dashboard_project)
        .collect::<Result<Vec<_>, ApiError>>()?;
    let ledger_path = state.config.data_dir.join("ledger/hzr.sqlite");
    let selected_path = selected
        .as_ref()
        .and_then(|registration| registration.root.to_str())
        .map(str::to_owned);
    let ledger_project_path = selected_path.clone();
    let ledger = tokio::task::spawn_blocking(move || {
        let summaries = Ledger::summaries_read_only(&ledger_path)?;
        let activity = ledger_project_path.as_deref().map_or_else(
            || Ok(hzr_core::ProjectActivitySummary::default()),
            |project| Ledger::project_activity_read_only(&ledger_path, project),
        )?;
        Ok::<_, hzr_core::LedgerError>((summaries.0, summaries.1, activity))
    })
    .await
    .map_err(|error| ApiError::internal(format!("dashboard ledger task failed: {error}")))?;
    let (observed, estimated, activity, ledger_error) = match ledger {
        Ok((observed, estimated, activity)) => (observed, estimated, activity, None),
        Err(error) => (
            hzr_core::LedgerSummary::default(),
            hzr_core::EfficiencySummary::default(),
            hzr_core::ProjectActivitySummary::default(),
            Some(error.to_string()),
        ),
    };

    let (memory_observatory, index_observatory) = tokio::join!(
        dashboard_memory_observatory(&state, selected.as_ref(), &memory_health, memory_retrieval,),
        dashboard_index_observatory(&state, selected.as_ref()),
    );

    let mut services = Vec::with_capacity(4);
    services.push(DashboardService {
        id: "hzrd".into(),
        name: "HZR daemon".into(),
        version: Some(env!("CARGO_PKG_VERSION").into()),
        state: if ledger_error.is_some() || registry_warnings > 0 {
            DashboardState::Degraded
        } else {
            DashboardState::Ready
        },
        detail: match (&ledger_error, registry_warnings) {
            (Some(_), 0) => "Control plane is ready; the usage ledger is unavailable".into(),
            (None, warnings) if warnings > 0 => {
                format!("Control plane is ready; {warnings} workspace registry warning(s)")
            }
            (Some(_), warnings) => format!(
                "Control plane is ready; usage ledger unavailable and {warnings} registry warning(s)"
            ),
            (None, _) => "Loopback control plane and visualizer are ready".into(),
        },
        command: Some("hzr doctor --workspace .".into()),
    });
    for engine in ["rtk", "icm", "grepai"] {
        if let Some(engine_health) = health.engines.iter().find(|item| item.name == engine) {
            services.push(dashboard_service(engine_health));
        }
    }
    if let Some(grepai) = services.iter_mut().find(|service| service.id == "grepai") {
        grepai.state = index_observatory.state;
        grepai.detail = index_observatory.semantic.detail.clone();
    }
    let overall_state = dashboard_overall_state(&services);
    let reduction_pct = signed_percentage(
        estimated.net_avoided_tokens_estimated,
        estimated.baseline_tokens_estimated,
    );
    let mut notes = vec![
        "Provider-observed usage and UTF-8-byte estimates are displayed separately.".into(),
        "The grepai semantic canary is cached for 30 seconds and never credits the usage ledger."
            .into(),
        "The visualizer is local-only and exposes no engine lifecycle mutations.".into(),
    ];
    if ledger_error.is_some() {
        notes.push(
            "Usage ledger totals are unavailable in this snapshot; no fallback totals were used."
                .into(),
        );
    }

    Ok(Json(DashboardResponse {
        protocol_version: PROTOCOL_VERSION,
        hzr_version: env!("CARGO_PKG_VERSION").into(),
        visualizer_version: env!("CARGO_PKG_VERSION").into(),
        generated_at_ms: now_ms()?,
        uptime_ms: u64::try_from(state.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
        daemon_endpoint: format!("http://{}", state.config.daemon.bind),
        overall_state,
        services,
        projects,
        registry_warnings,
        observed_usage: DashboardObservedUsage {
            tasks: observed.tasks,
            accepted: observed.accepted,
            actual_input_tokens: observed.actual_input_tokens,
            actual_output_tokens: observed.actual_output_tokens,
            estimated_input_tokens: observed.estimated_input_tokens,
            cost_microusd: observed.cost_microusd,
        },
        estimated_efficiency: DashboardEstimatedEfficiency {
            operations: estimated.operations,
            baseline_tokens_estimated: estimated.baseline_tokens_estimated,
            delivered_tokens_estimated: estimated.delivered_tokens_estimated,
            gross_avoided_tokens_estimated: estimated.gross_avoided_tokens_estimated,
            regression_tokens_estimated: estimated.regression_tokens_estimated,
            net_avoided_tokens_estimated: estimated.net_avoided_tokens_estimated,
            reduction_pct,
            total_execution_ms: estimated.total_execution_ms,
            measurement: "estimated_utf8_bytes_div_4_v1".into(),
        },
        memory_observatory,
        index_observatory,
        local_activity: DashboardLocalActivity {
            project: selected.as_ref().map(registration_name),
            operations: activity.operations,
            optimized_operations: activity.optimized_operations,
            raw_operations: activity.raw_operations,
            baseline_tokens_estimated: activity.baseline_tokens_estimated,
            delivered_tokens_estimated: activity.delivered_tokens_estimated,
            gross_avoided_tokens_estimated: activity.gross_avoided_tokens_estimated,
            regression_tokens_estimated: activity.regression_tokens_estimated,
            net_avoided_tokens_estimated: activity.net_avoided_tokens_estimated,
            total_execution_ms: activity.total_execution_ms,
            first_record_at: activity.first_record_at,
            last_record_at: activity.last_record_at,
            unscoped_operations: activity.unscoped_operations,
            measurement: "exact_project_path · estimated_utf8_bytes_div_4_v1".into(),
            recent_operations: activity
                .recent_operations
                .into_iter()
                .map(|operation| DashboardLocalOperation {
                    timestamp: operation.timestamp,
                    operation: operation.operation,
                    route: match operation.route {
                        hzr_core::ProjectOperationRoute::Optimized => {
                            DashboardOperationRoute::Optimized
                        }
                        hzr_core::ProjectOperationRoute::Raw => DashboardOperationRoute::Raw,
                    },
                    baseline_tokens_estimated: operation.baseline_tokens_estimated,
                    delivered_tokens_estimated: operation.delivered_tokens_estimated,
                    net_avoided_tokens_estimated: operation.net_avoided_tokens_estimated,
                    execution_ms: operation.execution_ms,
                    replacement: operation.replacement,
                    rationale: operation.rationale,
                })
                .collect(),
        },
        provider_receipts: DashboardProviderReceipts {
            state: if observed.tasks > 0 {
                DashboardProviderReceiptState::Available
            } else {
                DashboardProviderReceiptState::NoReceipts
            },
            records: observed.tasks,
            accepted: observed.accepted,
            actual_input_tokens: observed.actual_input_tokens,
            actual_output_tokens: observed.actual_output_tokens,
            cost_microusd: observed.cost_microusd,
            detail: if observed.tasks > 0 {
                "Provider-attributed receipts are available in the HZR ledger.".into()
            } else {
                "No provider receipts are connected; HZR does not present missing data as zero usage."
                    .into()
            },
        },
        help: dashboard_help(),
        notes,
    }))
}

async fn dashboard_memory_observatory(
    state: &AppState,
    selected: Option<&WorkspaceRegistration>,
    memory_health: &EngineHealth,
    retrieval: DashboardMemoryRetrieval,
) -> DashboardMemoryObservatory {
    let observed_at_ms = now_ms().unwrap_or_default();
    let started_at = Instant::now();
    let runtime_state = match memory_health.state {
        EngineState::Ready => DashboardState::Ready,
        EngineState::Degraded => DashboardState::Degraded,
        EngineState::Rebuilding => DashboardState::Rebuilding,
        EngineState::Stopped => DashboardState::Stopped,
    };
    let runtime_detail = memory_health
        .detail
        .clone()
        .unwrap_or_else(|| "ICM returned no readiness detail".into());
    let Some(registration) = selected else {
        return DashboardMemoryObservatory {
            state: runtime_state,
            project: None,
            retrieval,
            observed_at_ms,
            latency_ms: 0,
            transport: "json_rpc+sqlite_read_only".into(),
            source: "canonical_icm_store".into(),
            memory_count: 0,
            visible_memory_count: 0,
            hidden_memory_count: 0,
            topics: Vec::new(),
            edges: Vec::new(),
            truncated: false,
            diagnostic_command: "hzr memory status".into(),
            detail: format!("{runtime_detail}; no registered project is selected"),
        };
    };
    if runtime_state != DashboardState::Ready {
        return empty_memory_observatory(
            registration,
            runtime_state,
            retrieval,
            observed_at_ms,
            0,
            runtime_detail,
        );
    }
    let database = state.memory.layout().database.clone();
    let repository_id = registration.repository_id.clone();
    let snapshot =
        tokio::task::spawn_blocking(move || read_project_snapshot(&database, &repository_id)).await;
    match snapshot {
        Ok(Ok(snapshot)) => memory_snapshot_observatory(
            registration,
            retrieval,
            observed_at_ms,
            elapsed_ms(started_at),
            runtime_detail,
            snapshot,
        ),
        Ok(Err(error)) => empty_memory_observatory(
            registration,
            DashboardState::Degraded,
            retrieval,
            observed_at_ms,
            elapsed_ms(started_at),
            format!("ICM is ready, but its read-only project snapshot failed: {error}"),
        ),
        Err(error) => empty_memory_observatory(
            registration,
            DashboardState::Degraded,
            retrieval,
            observed_at_ms,
            elapsed_ms(started_at),
            format!("ICM snapshot task failed: {error}"),
        ),
    }
}

fn memory_snapshot_observatory(
    registration: &WorkspaceRegistration,
    retrieval: DashboardMemoryRetrieval,
    observed_at_ms: u64,
    latency_ms: u64,
    runtime_detail: String,
    snapshot: ProjectMemorySnapshot,
) -> DashboardMemoryObservatory {
    DashboardMemoryObservatory {
        state: DashboardState::Ready,
        project: Some(registration_name(registration)),
        retrieval,
        observed_at_ms,
        latency_ms,
        transport: "json_rpc+sqlite_read_only".into(),
        source: "canonical_icm_store".into(),
        memory_count: snapshot.memory_count,
        visible_memory_count: snapshot.visible_memory_count,
        hidden_memory_count: snapshot.hidden_memory_count,
        topics: snapshot
            .topics
            .into_iter()
            .map(|topic| DashboardMemoryTopic {
                id: topic.id,
                label: topic.label,
                memory_count: topic.memory_count,
                average_weight: topic.average_weight,
                newest_at: topic.newest_at,
            })
            .collect(),
        edges: snapshot
            .edges
            .into_iter()
            .map(|edge| DashboardMemoryEdge {
                source: edge.source,
                target: edge.target,
                relationship_count: edge.relationship_count,
            })
            .collect(),
        truncated: snapshot.truncated,
        diagnostic_command: "hzr memory status".into(),
        detail: format!(
            "{runtime_detail}; snapshot is read-only and positively filtered to this repository"
        ),
    }
}

fn empty_memory_observatory(
    registration: &WorkspaceRegistration,
    state: DashboardState,
    retrieval: DashboardMemoryRetrieval,
    observed_at_ms: u64,
    latency_ms: u64,
    detail: String,
) -> DashboardMemoryObservatory {
    DashboardMemoryObservatory {
        state,
        project: Some(registration_name(registration)),
        retrieval,
        observed_at_ms,
        latency_ms,
        transport: "json_rpc+sqlite_read_only".into(),
        source: "canonical_icm_store".into(),
        memory_count: 0,
        visible_memory_count: 0,
        hidden_memory_count: 0,
        topics: Vec::new(),
        edges: Vec::new(),
        truncated: false,
        diagnostic_command: "hzr memory status".into(),
        detail,
    }
}

async fn dashboard_index_observatory(
    state: &AppState,
    selected: Option<&WorkspaceRegistration>,
) -> DashboardIndexObservatory {
    let observed_at_ms = now_ms().unwrap_or_default();
    let Some(registration) = selected else {
        return empty_index_observatory(
            None,
            DashboardState::Standby,
            observed_at_ms,
            "No registered project is selected".into(),
        );
    };
    let initial = match state.context.index_status(&registration.root).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return empty_index_observatory(
                Some(registration_name(registration)),
                DashboardState::Degraded,
                observed_at_ms,
                format!("grepai index status failed: {error}"),
            );
        }
    };
    let generation = initial
        .index
        .generation
        .as_ref()
        .map(|generation| generation.generation.clone());
    let semantic = semantic_canary(state, registration, generation.clone()).await;
    index_snapshot_observatory(registration, observed_at_ms, initial, semantic)
}

async fn semantic_canary(
    state: &AppState,
    registration: &WorkspaceRegistration,
    generation: Option<String>,
) -> DashboardSemanticCanary {
    const QUERY: &str = "repository architecture and main implementation";
    const CANARY_TIMEOUT: Duration = Duration::from_secs(15);
    const READY_CACHE_TTL: Duration = Duration::from_secs(30);
    const RETRY_CACHE_TTL: Duration = Duration::from_secs(2);

    let mut cache = state.semantic_canary.lock().await;
    if let Some(cached) = cache.as_ref()
        && cached.generation == generation
        && cached.checked_at.elapsed()
            < if cached.snapshot.state == DashboardState::Ready {
                READY_CACHE_TTL
            } else {
                RETRY_CACHE_TTL
            }
    {
        return cached.snapshot.clone();
    }
    let checked_at_ms = now_ms().ok();
    let started_at = Instant::now();
    let result = tokio::time::timeout(
        CANARY_TIMEOUT,
        state.context.search_unaccounted(SearchRequest {
            workspace: registration.root.clone(),
            query: QUERY.into(),
            path: None,
            limit: 3,
            mode: SearchMode::Semantic,
            include_content: false,
        }),
    )
    .await;
    let snapshot = match result {
        Ok(Ok(response)) => {
            let adaptive = response.strategy == SearchStrategy::ForkRgaiAdaptive
                && response.fallback_reason.is_none();
            DashboardSemanticCanary {
                state: if adaptive {
                    DashboardState::Ready
                } else {
                    DashboardState::Degraded
                },
                checked_at_ms,
                latency_ms: elapsed_ms(started_at),
                query: QUERY.into(),
                total_hits: response.total_hits,
                shown_hits: response.shown_hits,
                scanned_files: response.scanned_files,
                strategy: Some(
                    match response.strategy {
                        SearchStrategy::ForkRgaiAdaptive => "fork_rgai_adaptive",
                        SearchStrategy::ForkRgaiBuiltin => "fork_rgai_builtin",
                    }
                    .into(),
                ),
                backend: Some("grepai_semantic".into()),
                generation: response.index_generation,
                detail: response.fallback_reason.unwrap_or_else(|| {
                    format!(
                        "Semantic search executed successfully and returned {} visible hit(s)",
                        response.shown_hits
                    )
                }),
            }
        }
        Ok(Err(error)) => DashboardSemanticCanary {
            state: DashboardState::Degraded,
            checked_at_ms,
            latency_ms: elapsed_ms(started_at),
            query: QUERY.into(),
            total_hits: 0,
            shown_hits: 0,
            scanned_files: 0,
            strategy: None,
            backend: None,
            generation: generation.clone(),
            detail: format!("Semantic canary failed: {error}"),
        },
        Err(_) => DashboardSemanticCanary {
            state: DashboardState::Rebuilding,
            checked_at_ms,
            latency_ms: elapsed_ms(started_at),
            query: QUERY.into(),
            total_hits: 0,
            shown_hits: 0,
            scanned_files: 0,
            strategy: None,
            backend: None,
            generation: generation.clone(),
            detail: "Semantic canary exceeded its 15-second bounded observability budget".into(),
        },
    };
    *cache = Some(CachedSemanticCanary {
        generation,
        checked_at: Instant::now(),
        snapshot: snapshot.clone(),
    });
    snapshot
}

fn index_snapshot_observatory(
    registration: &WorkspaceRegistration,
    observed_at_ms: u64,
    snapshot: IndexCoordinatorSnapshot,
    semantic: DashboardSemanticCanary,
) -> DashboardIndexObservatory {
    let watcher_state = match snapshot.watcher.state {
        IndexWatcherState::Live => DashboardState::Ready,
        IndexWatcherState::Standby => DashboardState::Standby,
        IndexWatcherState::Failed => DashboardState::Degraded,
    };
    let (size_bytes, modified_at_ms) = index_artifact_metadata(&snapshot.workspace);
    let artifacts = DashboardIndexArtifacts {
        initialized: snapshot.index.initialized,
        vectors_present: snapshot.index.vectors_present,
        symbols_present: snapshot.index.symbols_present,
        repository_graph_present: snapshot.index.repository_graph_present,
        size_bytes,
        modified_at_ms,
    };
    let artifact_ready =
        artifacts.initialized && artifacts.vectors_present && artifacts.symbols_present;
    let state = if semantic.state == DashboardState::Degraded
        || watcher_state == DashboardState::Degraded
    {
        DashboardState::Degraded
    } else if semantic.state == DashboardState::Rebuilding {
        DashboardState::Rebuilding
    } else if artifact_ready && semantic.state == DashboardState::Ready {
        DashboardState::Ready
    } else {
        DashboardState::Rebuilding
    };
    DashboardIndexObservatory {
        state,
        project: Some(registration_name(registration)),
        observed_at_ms,
        generation: snapshot
            .index
            .generation
            .as_ref()
            .map(|generation| generation.generation.clone()),
        config_fingerprint: snapshot
            .index
            .generation
            .map(|generation| generation.config_fingerprint),
        artifacts,
        watcher: DashboardIndexWatcher {
            state: watcher_state,
            pid: snapshot.watcher.pid,
            uptime_ms: snapshot.watcher.uptime_ms,
            owned_by_hzr: snapshot.watcher.pid.is_some(),
            ready_marker_observed: snapshot.watcher.ready_marker_observed,
            detail: match snapshot.watcher.state {
                IndexWatcherState::Live => "HZR-owned watcher is live".into(),
                IndexWatcherState::Standby => "Watcher has not started for this daemon".into(),
                IndexWatcherState::Failed => "Managed watcher exited unexpectedly".into(),
            },
        },
        semantic,
        diagnostic_command: "hzr index status --workspace .".into(),
    }
}

fn empty_index_observatory(
    project: Option<String>,
    state: DashboardState,
    observed_at_ms: u64,
    detail: String,
) -> DashboardIndexObservatory {
    DashboardIndexObservatory {
        state,
        project,
        observed_at_ms,
        generation: None,
        config_fingerprint: None,
        artifacts: DashboardIndexArtifacts::default(),
        watcher: DashboardIndexWatcher {
            state: DashboardState::Standby,
            pid: None,
            uptime_ms: None,
            owned_by_hzr: false,
            ready_marker_observed: false,
            detail: detail.clone(),
        },
        semantic: DashboardSemanticCanary {
            state,
            checked_at_ms: None,
            latency_ms: 0,
            query: "repository architecture and main implementation".into(),
            total_hits: 0,
            shown_hits: 0,
            scanned_files: 0,
            strategy: None,
            backend: None,
            generation: None,
            detail,
        },
        diagnostic_command: "hzr index status --workspace .".into(),
    }
}

fn index_artifact_metadata(workspace: &Workspace) -> (u64, Option<u64>) {
    let mut size_bytes = 0_u64;
    let mut modified_at_ms = None;
    for path in [
        &workspace.index.config,
        &workspace.index.vectors,
        &workspace.index.symbols,
        &workspace.index.repository_graph,
    ] {
        let Ok(metadata) = fs::metadata(path) else {
            continue;
        };
        size_bytes = size_bytes.saturating_add(metadata.len());
        let modified = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
        modified_at_ms = modified_at_ms.max(modified);
    }
    (size_bytes, modified_at_ms)
}

fn elapsed_ms(started_at: Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn registration_name(registration: &WorkspaceRegistration) -> String {
    registration
        .root
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("workspace")
        .to_owned()
}

fn dashboard_service(health: &EngineHealth) -> DashboardService {
    let (name, command) = match health.name.as_str() {
        "rtk" => ("RTK fork-core", "hzr rtk -- --version"),
        "icm" => ("ICM memory", "hzr memory status"),
        "grepai" => ("grepai index", "hzr index status --workspace ."),
        other => (other, "hzr engines status"),
    };
    DashboardService {
        id: health.name.clone(),
        name: name.into(),
        version: health.version.clone(),
        state: match health.state {
            EngineState::Ready => DashboardState::Ready,
            EngineState::Degraded => DashboardState::Degraded,
            EngineState::Rebuilding => DashboardState::Rebuilding,
            EngineState::Stopped if health.name == "grepai" => DashboardState::Standby,
            EngineState::Stopped => DashboardState::Stopped,
        },
        detail: health.detail.clone().unwrap_or_else(|| "No detail".into()),
        command: Some(command.into()),
    }
}

fn dashboard_overall_state(services: &[DashboardService]) -> DashboardState {
    if services.iter().any(|service| {
        matches!(
            service.state,
            DashboardState::Degraded | DashboardState::Stopped | DashboardState::Unknown
        )
    }) {
        DashboardState::Degraded
    } else if services
        .iter()
        .any(|service| service.state == DashboardState::Rebuilding)
    {
        DashboardState::Rebuilding
    } else {
        DashboardState::Ready
    }
}

fn dashboard_project(registration: &WorkspaceRegistration) -> Result<DashboardProject, ApiError> {
    let root = registration.root.to_str().ok_or_else(|| {
        ApiError::internal("registered workspace path is not valid UTF-8".to_owned())
    })?;
    let root_available = fs::symlink_metadata(&registration.root)
        .map(|metadata| metadata.file_type().is_dir())
        .unwrap_or(false);
    let index_directory_is_real = fs::symlink_metadata(&registration.index_directory)
        .map(|metadata| metadata.file_type().is_dir())
        .unwrap_or(false);
    let artifacts = dashboard_artifacts(&registration.index_directory);
    let state = if !root_available {
        DashboardProjectState::Unavailable
    } else if !index_directory_is_real {
        DashboardProjectState::Degraded
    } else if artifacts.config_present && artifacts.vectors_present && artifacts.symbols_present {
        DashboardProjectState::Ready
    } else if artifacts.config_present {
        DashboardProjectState::Warming
    } else {
        DashboardProjectState::Registered
    };
    let name = registration
        .root
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(root)
        .to_owned();
    Ok(DashboardProject {
        name,
        root: root.to_owned(),
        repository_id: registration.repository_id.clone(),
        worktree_id: registration.worktree_id.clone(),
        git_backed: registration.git_backed,
        linked_worktree: registration.linked_worktree,
        state,
        registered_at_ms: registration.registered_at_ms,
        last_seen_at_ms: registration.last_seen_at_ms,
        artifacts,
        command: format!("hzr index status --workspace {}", shell_quote(root)),
    })
}

fn dashboard_artifacts(directory: &Path) -> DashboardProjectArtifacts {
    let config = artifact_metadata(&directory.join("config.yaml"));
    let vectors = artifact_metadata(&directory.join("index.gob"));
    let symbols = artifact_metadata(&directory.join("symbols.gob"));
    let graph = artifact_metadata(&directory.join("rpg.gob"));
    let entries = [&config, &vectors, &symbols, &graph];
    DashboardProjectArtifacts {
        config_present: config.is_some(),
        vectors_present: vectors.is_some(),
        symbols_present: symbols.is_some(),
        repository_graph_present: graph.is_some(),
        size_bytes: entries
            .iter()
            .filter_map(|entry| entry.as_ref().map(fs::Metadata::len))
            .fold(0_u64, u64::saturating_add),
        modified_at_ms: entries
            .iter()
            .filter_map(|entry| entry.as_ref().and_then(metadata_modified_ms))
            .max(),
    }
}

fn artifact_metadata(path: &Path) -> Option<fs::Metadata> {
    fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.file_type().is_file())
}

fn metadata_modified_ms(metadata: &fs::Metadata) -> Option<u64> {
    let millis = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    u64::try_from(millis).ok()
}

fn dashboard_help() -> Vec<DashboardHelpCommand> {
    [
        (
            "Run doctor",
            "Audit lifecycle, ownership, engines, and the current workspace.",
            "hzr doctor --workspace .",
        ),
        (
            "Service status",
            "Check the production user service without changing it.",
            "hzr daemon service status",
        ),
        (
            "Engine pins",
            "Inspect the exact managed engine versions.",
            "hzr engines status",
        ),
        (
            "Usage stats",
            "Print observed usage and estimated efficiency separately.",
            "hzr stats",
        ),
        (
            "Command help",
            "Open the complete CLI command reference.",
            "hzr --help",
        ),
    ]
    .into_iter()
    .map(|(label, description, command)| DashboardHelpCommand {
        label: label.into(),
        description: description.into(),
        command: command.into(),
    })
    .collect()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn signed_percentage(part: i64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        part as f64 * 100.0 / total as f64
    }
}

fn now_ms() -> Result<u64, ApiError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ApiError::internal(format!("system clock precedes UNIX epoch: {error}")))?
        .as_millis();
    u64::try_from(millis)
        .map_err(|error| ApiError::internal(format!("system clock is out of range: {error}")))
}

pub async fn engines() -> Result<Json<hzr_core::EngineManifest>, ApiError> {
    locked_engines()
        .map(Json)
        .map_err(|error| ApiError::internal(format!("engine lock is invalid: {error}")))
}

pub async fn search(
    State(state): State<AppState>,
    Json(request): Json<SearchApiRequest>,
) -> Result<Json<SearchApiResponse>, ApiError> {
    validate_search(&request)?;
    let workspace = canonical_workspace(&request.workspace)?;
    state
        .context
        .search(SearchRequest {
            workspace,
            query: request.query,
            path: request.path.map(PathBuf::from),
            limit: request.limit,
            mode: request.mode,
            include_content: request.include_content,
        })
        .await
        .map(Json)
        .map_err(context_error)
}

pub async fn context_plan(
    State(state): State<AppState>,
    Json(request): Json<ContextPlanApiRequest>,
) -> Result<Json<ContextPlanApiResponse>, ApiError> {
    let workspace = canonical_workspace(&request.workspace)?;
    state
        .context
        .plan(PlanRequest {
            workspace,
            intent: request.intent,
            path: request.path.map(PathBuf::from),
            topic: request.topic,
            search_limit: request.search_limit,
            memory_limit: request.memory_limit,
        })
        .await
        .map(Json)
        .map_err(context_error)
}

pub async fn memory_recall(
    State(state): State<AppState>,
    Json(request): Json<MemoryRecallApiRequest>,
) -> Result<Json<Vec<hzr_memory::MemoryRecord>>, ApiError> {
    if request.query.trim().is_empty() {
        return Err(ApiError::bad_request("memory query must not be empty"));
    }
    validate_limit(request.limit)?;
    let project = memory_project(&state, &request.workspace).await?;
    if let Some(kind) = request.topic.as_deref() {
        validate_memory_kind(kind).map_err(|error| ApiError::bad_request(error.to_string()))?;
    }
    let project_topic = request
        .topic
        .as_deref()
        .map(|kind| namespaced_topic(kind, &project))
        .transpose()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let global_topic = request
        .topic
        .as_deref()
        .map(global_topic)
        .transpose()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let candidate_limit = recall_candidate_limit(request.limit);
    let mut base = RecallRequest::new(request.query);
    base.limit = candidate_limit;
    base.keyword = request.keyword;

    let mut project_recall = base.clone();
    project_recall.project = Some(project.clone());
    project_recall.topic.clone_from(&project_topic);
    let mut global_recall = base;
    global_recall.project = Some(GLOBAL_SCOPE_TOKEN.into());
    global_recall.topic.clone_from(&global_topic);
    let client = state.memory.client();
    let unavailable = |error: hzr_memory::MemoryError| {
        ApiError::service("memory_unavailable", error.to_string(), true)
    };

    let records = match request.scope {
        MemoryScopeSelector::Project => isolate_memories(
            client.recall(&project_recall).await.map_err(unavailable)?,
            &project,
            MemoryNamespace::Project,
            project_topic.as_deref(),
            request.limit,
        ),
        MemoryScopeSelector::Global => isolate_memories(
            client.recall(&global_recall).await.map_err(unavailable)?,
            &project,
            MemoryNamespace::Global,
            global_topic.as_deref(),
            request.limit,
        ),
        MemoryScopeSelector::ProjectAndGlobal => {
            let (project_records, global_records) = tokio::try_join!(
                client.recall(&project_recall),
                client.recall(&global_recall)
            )
            .map_err(unavailable)?;
            let project_records = isolate_memories(
                project_records,
                &project,
                MemoryNamespace::Project,
                project_topic.as_deref(),
                candidate_limit,
            );
            let global_records = isolate_memories(
                global_records,
                &project,
                MemoryNamespace::Global,
                global_topic.as_deref(),
                candidate_limit,
            );
            merge_memories(project_records, global_records, request.limit)
        }
    };
    Ok(Json(records))
}

pub async fn memory_store(
    State(state): State<AppState>,
    Json(request): Json<MemoryStoreApiRequest>,
) -> Result<Json<hzr_memory::StoreReceipt>, ApiError> {
    if request.topic.trim().is_empty() {
        return Err(ApiError::bad_request("memory topic must not be empty"));
    }
    if request.content.trim().is_empty() {
        return Err(ApiError::bad_request("memory content must not be empty"));
    }
    if request.keywords.len() > 32 {
        return Err(ApiError::bad_request(
            "memory keywords must contain at most 32 entries",
        ));
    }
    let project = memory_project(&state, &request.workspace).await?;
    // One write targets exactly one namespace, so a global preference is stored once and
    // is reachable from every repository instead of being duplicated per project.
    let topic = match request.scope {
        MemoryWriteScope::Project => namespaced_topic(&request.topic, &project),
        MemoryWriteScope::Global => global_topic(&request.topic),
    }
    .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let mut store = StoreRequest::new(topic, request.content);
    store.importance = match request.importance {
        MemoryImportance::Critical => Importance::Critical,
        MemoryImportance::High => Importance::High,
        MemoryImportance::Medium => Importance::Medium,
        MemoryImportance::Low => Importance::Low,
    };
    store.keywords = request.keywords;
    store.raw = request.raw;
    state
        .memory
        .client()
        .store(&store)
        .await
        .map(Json)
        .map_err(|error| ApiError::service("memory_unavailable", error.to_string(), true))
}

pub async fn exec_run(
    State(state): State<AppState>,
    Json(request): Json<ExecApiRequest>,
) -> Result<Json<ExecutionOutcome>, ApiError> {
    if request.command.trim().is_empty() {
        return Err(ApiError::bad_request("command must not be empty"));
    }
    validate_exec_timeout(request.timeout_ms)?;
    let budget = ManagedExecutionBudget::new(&state, request.timeout_ms)?;
    let cwd = canonical_workspace(&request.cwd)?;
    let command = CanonicalCommand::shell(request.command);
    let decision = match state.rtk.decide_in(&command, Some(&cwd)).await {
        RewriteDecision::Ask { proposed, reason } => {
            let timeout_ms = Some(budget.limit_ms());
            let decision_id = if let Some(proposed_command) = proposed.clone() {
                Some(
                    state
                        .approvals
                        .insert(PendingApproval {
                            requested: command.clone(),
                            proposed: proposed_command,
                            cwd,
                            timeout_ms,
                        })
                        .await,
                )
            } else {
                None
            };
            return Ok(Json(ExecutionOutcome::NotStarted {
                disposition: NotStarted::ApprovalRequired {
                    decision_id,
                    requested: command,
                    proposed,
                    reason,
                },
            }));
        }
        decision => decision,
    };
    let mut envelope = ExecutionEnvelope::allow_raw(command);
    envelope.decision = decision;
    envelope.cwd = Some(cwd);
    envelope.timeout_ms = Some(budget.remaining_ms()?);
    state
        .executor
        .execute(envelope)
        .await
        .map(Json)
        .map_err(|error| ApiError::service("execution_failed", error.to_string(), true))
}

pub async fn exec_approval(
    State(state): State<AppState>,
    Json(request): Json<ExecApprovalApiRequest>,
) -> Result<Json<ExecutionOutcome>, ApiError> {
    if request.decision_id.trim().is_empty() || request.decision_id.len() > 128 {
        return Err(ApiError::bad_request("invalid approval decision id"));
    }
    let pending = state
        .approvals
        .take(&request.decision_id)
        .await
        .ok_or_else(|| ApiError::bad_request("approval is unknown, expired, or already used"))?;
    if !request.approved {
        return Ok(Json(ExecutionOutcome::NotStarted {
            disposition: NotStarted::Denied {
                requested: pending.requested,
                reason: "user denied the pending fork-core command".into(),
            },
        }));
    }
    let mut envelope = ExecutionEnvelope::allow_raw(pending.requested);
    envelope.decision = RewriteDecision::AllowRewrite {
        command: pending.proposed,
        source: RewriteSource::Rtk {
            version: PINNED_RTK_VERSION.into(),
        },
        reason: "user approved the pending fork-core command".into(),
    };
    envelope.cwd = Some(pending.cwd);
    envelope.timeout_ms = pending.timeout_ms;
    state
        .executor
        .execute(envelope)
        .await
        .map(Json)
        .map_err(|error| ApiError::service("execution_failed", error.to_string(), true))
}

pub async fn fork_run(
    State(state): State<AppState>,
    Json(request): Json<ForkRunApiRequest>,
) -> Result<Json<ForkRunApiResponse>, ApiError> {
    validate_fork_run(&request)?;
    let budget = ManagedExecutionBudget::new(&state, request.timeout_ms)?;
    let cwd = canonical_workspace(&request.cwd)?;
    validate_managed_fork_tool(&request.args, &cwd)?;
    let runner = state
        .rtk
        .runner()
        .map_err(|error| ApiError::service("fork_core_unavailable", error.to_string(), true))?;
    let mut invocation = ForkCoreInvocation::new(request.args);
    invocation.cwd = Some(cwd);
    invocation.timeout_ms = Some(budget.remaining_ms()?);
    invocation.stdin = request
        .stdin
        .map_or(StdinSpec::Null, |stdin| StdinSpec::Bytes {
            data: stdin.into_bytes(),
        });
    invocation.capture = CaptureConfig {
        memory_limit_bytes: 192 * 1024,
        max_capture_bytes: 192 * 1024,
        overflow: CaptureOverflow::Truncate,
        event_buffer: 16,
    };
    let outcome = runner
        .execute(invocation)
        .await
        .map_err(|error| ApiError::service("fork_core_failed", error.to_string(), true))?;
    let result = match outcome {
        ExecutionOutcome::Completed { result } => result,
        ExecutionOutcome::NotStarted { disposition } => {
            return Err(ApiError::internal(format!(
                "direct fork-core invocation was not started: {disposition:?}"
            )));
        }
    };
    let stdout = captured_utf8(&result.stdout, "stdout")?;
    let stderr = captured_utf8(&result.stderr, "stderr")?;
    Ok(Json(ForkRunApiResponse {
        stdout,
        stderr,
        termination: match result.termination.cause {
            TerminationCause::Exited => CommandTermination::Exited,
            TerminationCause::Signaled => CommandTermination::Signaled,
            TerminationCause::TimedOut => CommandTermination::TimedOut,
            TerminationCause::Cancelled => CommandTermination::Cancelled,
        },
        exit_code: result.termination.exit_code,
        signal: result.termination.signal,
        duration_ms: result.duration_ms,
        stdout_sha256: result.stdout.sha256.clone(),
        stderr_sha256: result.stderr.sha256.clone(),
        stdout_truncated: result.stdout.truncated,
        stderr_truncated: result.stderr.truncated,
    }))
}

pub async fn usage(
    State(state): State<AppState>,
    Json(request): Json<UsageApiRequest>,
) -> Result<Json<UsageApiResponse>, ApiError> {
    validate_usage(&request)?;
    let record = LedgerRecord {
        trace_id: TraceId::from_string(request.trace_id),
        provider: request.provider,
        model: request.model,
        usage: request.usage,
        turns: request.turns,
        retries: request.retries,
        latency_ms: request.latency_ms,
        outcome: request.outcome,
        policy_version: env!("CARGO_PKG_VERSION").into(),
        cost_microusd: request.cost_microusd,
    };
    state
        .ledger
        .record(record)
        .await
        .map_err(|error| ApiError::internal(format!("usage ledger write failed: {error}")))?;
    Ok(Json(UsageApiResponse { recorded: true }))
}

pub async fn exec_rewrite(
    State(state): State<AppState>,
    Json(request): Json<ExecApiRequest>,
) -> Result<Json<RewriteDecision>, ApiError> {
    if request.command.trim().is_empty() {
        return Err(ApiError::bad_request("command must not be empty"));
    }
    validate_exec_timeout(request.timeout_ms)?;
    let cwd = canonical_workspace(&request.cwd)?;
    let command = CanonicalCommand::shell(request.command);
    Ok(Json(state.rtk.decide_in(&command, Some(&cwd)).await))
}

pub async fn codec_compile(
    Json(request): Json<CodecApiRequest>,
) -> Result<Json<hzr_codec::Transform>, ApiError> {
    hzr_codec::transform(&request.content, request.fidelity, request.profile)
        .map(Json)
        .map_err(|error| ApiError::bad_request(error.to_string()))
}

fn validate_search(request: &SearchApiRequest) -> Result<(), ApiError> {
    if request.workspace.trim().is_empty() {
        return Err(ApiError::bad_request("workspace must not be empty"));
    }
    if request.query.trim().is_empty() {
        return Err(ApiError::bad_request("query must not be empty"));
    }
    validate_limit(request.limit)
}

fn validate_limit(limit: usize) -> Result<(), ApiError> {
    if !(1..=100).contains(&limit) {
        return Err(ApiError::bad_request("limit must be between 1 and 100"));
    }
    Ok(())
}

fn validate_exec_timeout(timeout_ms: Option<u64>) -> Result<(), ApiError> {
    if timeout_ms.is_some_and(|value| !(1..=1_800_000).contains(&value)) {
        return Err(ApiError::bad_request(
            "execution timeout must be between 1 and 1800000 milliseconds",
        ));
    }
    Ok(())
}

struct ManagedExecutionBudget {
    started: Instant,
    limit: Duration,
}

impl ManagedExecutionBudget {
    fn new(state: &AppState, requested: Option<u64>) -> Result<Self, ApiError> {
        let limit_ms = managed_timeout_ms(state, requested)?;
        Ok(Self {
            started: Instant::now(),
            limit: Duration::from_millis(limit_ms),
        })
    }

    fn limit_ms(&self) -> u64 {
        self.limit.as_millis().min(u128::from(u64::MAX)) as u64
    }

    fn remaining_ms(&self) -> Result<u64, ApiError> {
        let remaining = self.limit.saturating_sub(self.started.elapsed());
        let remaining_ms = remaining.as_millis().min(u128::from(u64::MAX)) as u64;
        if remaining_ms == 0 {
            return Err(ApiError::service(
                "execution_timed_out",
                "execution deadline elapsed before the child process started",
                true,
            ));
        }
        Ok(remaining_ms)
    }
}

fn managed_timeout_ms(state: &AppState, requested: Option<u64>) -> Result<u64, ApiError> {
    let available = state
        .config
        .daemon
        .request_timeout_ms
        .saturating_sub(500)
        .max(1);
    if requested.is_some_and(|timeout| timeout > available) {
        return Err(ApiError::bad_request(format!(
            "execution timeout exceeds the daemon's {available} millisecond managed limit"
        )));
    }
    Ok(requested.unwrap_or(available))
}

fn validate_fork_run(request: &ForkRunApiRequest) -> Result<(), ApiError> {
    validate_exec_timeout(request.timeout_ms)?;
    if request.args.is_empty() {
        return Err(ApiError::bad_request("fork-core args must not be empty"));
    }
    if request.args.len() > 256 {
        return Err(ApiError::bad_request(
            "fork-core args must contain at most 256 entries",
        ));
    }
    if request
        .args
        .iter()
        .any(|argument| argument.as_bytes().contains(&0))
        || request
            .stdin
            .as_deref()
            .is_some_and(|stdin| stdin.as_bytes().contains(&0))
    {
        return Err(ApiError::bad_request(
            "fork-core input must not contain NUL bytes",
        ));
    }
    Ok(())
}

fn validate_managed_fork_tool(args: &[String], workspace: &Path) -> Result<(), ApiError> {
    let path = match args.first().map(String::as_str) {
        Some("read") => validate_managed_read_args(args)?,
        Some("write") => validate_managed_write_args(args)?,
        _ => {
            return Err(ApiError::bad_request(
                "managed fork API permits only read and atomic patch/create operations",
            ));
        }
    };
    let relative = Path::new(path);
    if relative
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(ApiError::bad_request(
            "managed fork path must not contain parent traversal",
        ));
    }
    let requested = if relative.is_absolute() {
        relative.to_path_buf()
    } else {
        workspace.join(relative)
    };
    let confined = canonicalize_future_path(&requested)?;
    if !confined.starts_with(workspace) {
        return Err(ApiError::bad_request(format!(
            "managed fork path {} escapes workspace {}",
            confined.display(),
            workspace.display()
        )));
    }
    Ok(())
}

fn validate_managed_read_args(args: &[String]) -> Result<&String, ApiError> {
    let path = args
        .get(1)
        .ok_or_else(|| ApiError::bad_request("managed read is missing its path"))?;
    let mut position = 2;
    let mut mode = false;
    while position < args.len() {
        match args[position].as_str() {
            "--from" | "--to" | "--max-lines" => {
                if args
                    .get(position + 1)
                    .and_then(|value| value.parse::<usize>().ok())
                    .filter(|value| *value > 0)
                    .is_none()
                {
                    return Err(ApiError::bad_request("managed read has an invalid bound"));
                }
                position += 2;
            }
            "--line-numbers" => position += 1,
            "--outline" | "--symbols" | "--changed" if !mode => {
                mode = true;
                position += 1;
            }
            _ => {
                return Err(ApiError::bad_request(
                    "managed read contains an unsupported argument",
                ));
            }
        }
    }
    Ok(path)
}

fn validate_managed_write_args(args: &[String]) -> Result<&String, ApiError> {
    let valid_patch = matches!(
        args,
        [write, output, json, patch, _, old, _, new, _, cas, retry, two]
            if write == "write"
                && output == "--output"
                && json == "json"
                && patch == "patch"
                && old == "--old"
                && new == "--new"
                && cas == "--cas"
                && retry == "--retry"
                && two == "2"
    ) || matches!(
        args,
        [write, output, json, patch, _, old, _, new, _, cas, retry, two, all]
            if write == "write"
                && output == "--output"
                && json == "json"
                && patch == "patch"
                && old == "--old"
                && new == "--new"
                && cas == "--cas"
                && retry == "--retry"
                && two == "2"
                && all == "--all"
    );
    if valid_patch {
        return args
            .get(4)
            .ok_or_else(|| ApiError::bad_request("managed patch is missing its path"));
    }
    let valid_create = matches!(
        args,
        [write, output, json, create, _, content, stdin]
            if write == "write"
                && output == "--output"
                && json == "json"
                && create == "create"
                && content == "--content"
                && stdin == "@-"
    ) || matches!(
        args,
        [write, output, json, create, _, content, stdin, force]
            if write == "write"
                && output == "--output"
                && json == "json"
                && create == "create"
                && content == "--content"
                && stdin == "@-"
                && force == "--force"
    );
    if valid_create {
        return args
            .get(4)
            .ok_or_else(|| ApiError::bad_request("managed create is missing its path"));
    }
    Err(ApiError::bad_request(
        "managed write must use HZR's exact atomic patch/create contract",
    ))
}

fn canonicalize_future_path(path: &Path) -> Result<PathBuf, ApiError> {
    let mut existing = path;
    let mut suffix = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .ok_or_else(|| ApiError::bad_request("managed fork path has no existing ancestor"))?;
        suffix.push(name.to_os_string());
        existing = existing
            .parent()
            .ok_or_else(|| ApiError::bad_request("managed fork path has no existing ancestor"))?;
    }
    let mut canonical = std::fs::canonicalize(existing)
        .map_err(|error| ApiError::bad_request(format!("invalid managed fork path: {error}")))?;
    canonical.extend(suffix.into_iter().rev());
    Ok(canonical)
}

fn validate_usage(request: &UsageApiRequest) -> Result<(), ApiError> {
    if request.trace_id.trim().is_empty()
        || request.trace_id.len() > 128
        || !request.trace_id.is_ascii()
    {
        return Err(ApiError::bad_request("invalid usage trace id"));
    }
    if !matches!(
        request.outcome.as_str(),
        "completed" | "accepted" | "invalid_response" | "failed" | "cancelled"
    ) {
        return Err(ApiError::bad_request("invalid usage outcome"));
    }
    Ok(())
}

fn captured_utf8(
    stream: &hzr_exec::CapturedStream,
    name: &'static str,
) -> Result<String, ApiError> {
    let CapturedContent::Inline { bytes } = &stream.content else {
        return Err(ApiError::internal(format!(
            "fork-core {name} unexpectedly spilled outside the bounded API response"
        )));
    };
    match String::from_utf8(bytes.clone()) {
        Ok(content) => Ok(content),
        Err(error) if stream.truncated => {
            Ok(String::from_utf8_lossy(error.as_bytes()).into_owned())
        }
        Err(error) => Err(ApiError::service(
            "fork_core_invalid_output",
            format!("fork-core {name} was not valid UTF-8: {error}"),
            false,
        )),
    }
}

fn canonical_workspace(value: &str) -> Result<PathBuf, ApiError> {
    if value.trim().is_empty() {
        return Err(ApiError::bad_request("workspace path must not be empty"));
    }
    let path = Path::new(value);
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| ApiError::bad_request(format!("invalid workspace path: {error}")))?;
    if !canonical.is_dir() {
        return Err(ApiError::bad_request("workspace path must be a directory"));
    }
    Ok(canonical)
}

async fn memory_project(state: &AppState, value: &str) -> Result<String, ApiError> {
    let root = canonical_workspace(value)?;
    let workspace = Workspace::discover_managed_fast(
        &root,
        &state.config.engines.binary("git"),
        &state.config.data_dir,
        Deadlines::default().version,
    )
    .await
    .map_err(|error| {
        ApiError::service(
            "memory_workspace_unavailable",
            format!("failed to derive memory project identity: {error}"),
            true,
        )
    })?;
    Ok(workspace.identity.repository_id)
}

fn context_error(error: ContextError) -> ApiError {
    match error {
        error @ ContextError::InvalidRequest { .. } => ApiError::bad_request(error.to_string()),
        ContextError::Index(source) => ApiError::service(
            "context_unavailable",
            source.to_string(),
            source.recoverable(),
        ),
        ContextError::ForkUnavailable(message) => {
            ApiError::service("fork_core_unavailable", message, true)
        }
        // Recoverable by construction: the index keeps warming, so a retry succeeds.
        // Reaching this arm means the caller asked for semantic-only work; ordinary
        // search and planning degrade internally and never surface it.
        ContextError::IndexNotReady(message) => ApiError::service("index_not_ready", message, true),
        error @ (ContextError::Fork(_)
        | ContextError::ForkCommand { .. }
        | ContextError::InvalidForkOutput { .. }) => {
            ApiError::service("fork_core_failed", error.to_string(), true)
        }
        ContextError::Invariant(message) => {
            ApiError::internal(format!("context planning invariant failed: {message}"))
        }
    }
}

async fn memory_engine_health(state: &AppState) -> (EngineHealth, DashboardMemoryRetrieval) {
    let start = state.memory_start.read().await;
    let immediate = match &*start {
        MemoryStartState::Starting => Some((EngineState::Rebuilding, "ICM is starting".into())),
        MemoryStartState::Degraded(reason) => Some((EngineState::Degraded, reason.clone())),
        MemoryStartState::Disabled => Some((EngineState::Stopped, "auto start disabled".into())),
        MemoryStartState::Ready(_) => None,
    };
    drop(start);
    let ((engine_state, detail), retrieval) = match immediate {
        Some(result) => (result, DashboardMemoryRetrieval::Unavailable),
        None => match state.memory.status().await {
            ServiceStatus::Running { health, .. } | ServiceStatus::Attached { health } => {
                let retrieval = if health.has_embedder {
                    DashboardMemoryRetrieval::Hybrid
                } else {
                    DashboardMemoryRetrieval::Fts5
                };
                (memory_ready_state(health.has_embedder), retrieval)
            }
            other => (
                (EngineState::Degraded, format!("{other:?}")),
                DashboardMemoryRetrieval::Unavailable,
            ),
        },
    };
    (
        EngineHealth {
            name: "icm".into(),
            version: Some(hzr_memory::ICM_VERSION.into()),
            state: engine_state,
            detail: Some(detail),
        },
        retrieval,
    )
}

fn memory_ready_state(has_embedder: bool) -> (EngineState, String) {
    if has_embedder {
        (
            EngineState::Ready,
            "ICM singleton is ready with hybrid semantic and FTS5 retrieval".into(),
        )
    } else {
        (
            EngineState::Ready,
            "ICM singleton is ready with FTS5 retrieval; embeddings are disabled".into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, Instant};

    use super::{ManagedExecutionBudget, memory_ready_state, validate_managed_fork_tool};

    #[test]
    fn fts_only_icm_is_ready_with_an_explicit_retrieval_capability() {
        let (state, detail) = memory_ready_state(false);

        assert_eq!(state, hzr_protocol::EngineState::Ready);
        assert!(detail.contains("FTS5"));
        assert!(detail.contains("embeddings are disabled"));
    }

    #[test]
    fn managed_execution_budget_is_absolute_across_pre_spawn_work() {
        let budget = ManagedExecutionBudget {
            started: Instant::now() - Duration::from_millis(20),
            limit: Duration::from_millis(10),
        };

        assert!(budget.remaining_ms().is_err());
    }

    #[test]
    fn test_managed_fork_api_confines_read_and_write_paths() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let workspace = fs::canonicalize(directory.path()).expect("canonical workspace");
        fs::write(workspace.join("inside.rs"), "fn inside() {}\n").expect("write fixture");

        assert!(
            validate_managed_fork_tool(&["read".into(), "inside.rs".into()], &workspace).is_ok()
        );
        assert!(
            validate_managed_fork_tool(
                &[
                    "write".into(),
                    "--output".into(),
                    "json".into(),
                    "create".into(),
                    "new.rs".into(),
                    "--content".into(),
                    "@-".into(),
                ],
                &workspace,
            )
            .is_ok()
        );
        assert!(
            validate_managed_fork_tool(&["read".into(), "../outside".into()], &workspace).is_err()
        );
        assert!(validate_managed_fork_tool(&["config".into()], &workspace).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_managed_fork_api_rejects_symlink_escape() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let outside = tempfile::tempdir().expect("outside directory");
        let workspace = fs::canonicalize(directory.path()).expect("canonical workspace");
        std::os::unix::fs::symlink(outside.path(), workspace.join("escape"))
            .expect("create symlink fixture");

        assert!(
            validate_managed_fork_tool(&["read".into(), "escape/secret".into()], &workspace)
                .is_err()
        );
    }
}
