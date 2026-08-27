use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Path as AxumPath, Query, State};
use hzr_context::{ContextError, PlanRequest, SearchRequest};
use hzr_core::{
    FidelityAllowance, FidelityBudget, FidelityPreflight, Ledger, LedgerRecord,
    ProjectOperationRoute, ProjectOperationSummary, ProviderEconomicReceipt,
    ProviderReceiptRecordResult, RawFidelityReason, RawFidelityRequest, RawPublicEstimateRequest,
    efficient_route_replacement, first_class_replacement, load_pricing_catalog, locked_engines,
    price_avoided_input_tokens, raw_fidelity_request, validate_receipt,
};
use hzr_exec::{
    AccountingIncomplete, CanonicalCommand, CaptureConfig, CaptureOverflow, CapturedContent,
    ExecutionEnvelope, ExecutionOutcome, ForkCoreInvocation, HOST_GRANT_APPLIED_ENV, NotStarted,
    PINNED_RTK_VERSION, PinnedRtkAdapter, RewriteDecision, RewriteSource, RtkRewriteOutcome,
    StdinSpec, TerminationCause, host_grant_applied, reconcile_host_grant,
};
use hzr_index::{
    Deadlines, IndexCoordinatorSnapshot, IndexWatcherState, Workspace, WorkspaceRegistration,
    registered_workspaces,
};
use hzr_memory::{
    GLOBAL_SCOPE_TOKEN, Importance, MemoryNamespace, MemoryRecord, ProjectMemoryDetail,
    ProjectMemorySnapshot, RecallRequest, ServiceStatus, StoreRequest, global_topic,
    isolate_memories, merge_memories, namespaced_topic, read_project_snapshot,
    read_project_topic_details, recall_candidate_limit, validate_memory_kind,
};
use hzr_protocol::{
    CodecApiRequest, CommandTermination, ContextPlanApiRequest, ContextPlanApiResponse,
    DashboardEconomicAmount, DashboardEstimatedEfficiency, DashboardHelpCommand,
    DashboardIndexArtifacts, DashboardIndexObservatory, DashboardIndexWatcher,
    DashboardLocalActivity, DashboardLocalOperation, DashboardMemoryDetail, DashboardMemoryEdge,
    DashboardMemoryObservatory, DashboardMemoryRetrieval, DashboardMemoryTopic,
    DashboardMemoryTopicDetails, DashboardObservedUsage, DashboardOperationRoute, DashboardProject,
    DashboardProjectArtifacts, DashboardProjectPage, DashboardProjectState,
    DashboardProviderReceiptState, DashboardProviderReceipts, DashboardRawPublicEstimate,
    DashboardResponse, DashboardSearchActivity, DashboardService, DashboardSessionCommand,
    DashboardSessionRoi, DashboardState, DashboardTraceStage, DashboardTraceState, EnforcementTier,
    EngineHealth, EngineState, EvasionAttribution, EvasionClass, EvasionPathForm, ExecApiRequest,
    ExecApprovalApiRequest, FidelityReason, FidelityReconcileApiRequest, FidelityReconcileReceipt,
    FidelityValidation, ForkManagedWrite, ForkRunApiRequest, ForkRunApiResponse, HealthResponse,
    MemoryForgetApiRequest, MemoryImportance, MemoryMutationApiResponse, MemoryPruneApiRequest,
    MemoryRecallApiRequest, MemoryScopeSelector, MemoryStoreApiRequest, MemoryUpdateApiRequest,
    MemoryWriteScope, PROTOCOL_VERSION, PolicyDecision, SearchApiRequest, SearchApiResponse,
    TraceId, UsageApiRequest, UsageApiResponse,
};

use crate::approval::PendingApproval;
use crate::error::ApiError;
use crate::observability::TraceSpanInput;
use crate::state::{AppState, MemoryStartState};

#[cfg(test)]
#[path = "../../../fork-core/rtk/tests/fixtures/anti_evasion_fixture.rs"]
mod anti_evasion_fixture;

const MAX_CALLER_PATH_BYTES: usize = 32 * 1024;
const DEFAULT_PROJECT_PAGE_LIMIT: usize = 100;
const MAX_PROJECT_PAGE_LIMIT: usize = 200;

pub async fn health(State(state): State<AppState>) -> Result<Json<HealthResponse>, ApiError> {
    let ((memory_health, _), index_health) =
        tokio::join!(memory_engine_health(&state), grepai_engine_health(&state));
    Ok(Json(health_response(&state, memory_health, index_health)))
}

fn health_response(
    state: &AppState,
    memory_health: EngineHealth,
    index_health: EngineHealth,
) -> HealthResponse {
    let rtk_capabilities = state.rtk.capabilities();
    let rtk_ready = matches!(
        &rtk_capabilities.rewrite,
        hzr_exec::RtkRewriteInterface::ForkCli
    );
    let engines = vec![
        index_health,
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
        caveman_engine_health(&caveman_layout(&state.config)),
    ];
    let overall = overall_engine_state(&engines);

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

async fn grepai_engine_health(state: &AppState) -> EngineHealth {
    match state.context.index_registry_snapshot().await {
        Ok(snapshot) => {
            let failed = snapshot
                .watchers
                .iter()
                .filter(|watcher| watcher.state == IndexWatcherState::Failed)
                .count();
            let engine_state = if failed > 0 {
                EngineState::Degraded
            } else if snapshot.active_watchers > 0 {
                EngineState::Ready
            } else {
                EngineState::Stopped
            };
            EngineHealth {
                name: "grepai".into(),
                version: Some(hzr_index::SUPPORTED_GREPAI_VERSION.into()),
                state: engine_state,
                detail: Some(format!(
                    "{} active watcher(s), {} failed, limit {}, idle TTL {}ms",
                    snapshot.active_watchers,
                    failed,
                    snapshot.watcher_limit,
                    snapshot.watcher_idle_ttl_ms
                )),
            }
        }
        Err(error) => EngineHealth {
            name: "grepai".into(),
            version: Some(hzr_index::SUPPORTED_GREPAI_VERSION.into()),
            state: EngineState::Degraded,
            detail: Some(format!("watcher registry unavailable: {error}")),
        },
    }
}

/// The overall verdict must ignore engines that are resting by design.
///
/// `grepai` and `caveman-code` start on demand; reporting them as `Stopped` is correct and
/// says nothing about health. Folding `Stopped` into the verdict — as the previous
/// `any(Degraded)` over a list containing two permanently-stopped engines effectively did
/// once anything else slipped — trains an operator to ignore a yellow control plane.
fn overall_engine_state(engines: &[EngineHealth]) -> EngineState {
    if engines
        .iter()
        .any(|engine| engine.state == EngineState::Degraded)
    {
        return EngineState::Degraded;
    }
    if engines
        .iter()
        .any(|engine| engine.state == EngineState::Rebuilding)
    {
        return EngineState::Rebuilding;
    }
    EngineState::Ready
}

fn caveman_layout(config: &hzr_core::Config) -> hzr_agent::IntegrationLayout {
    if let Some(root) = std::env::var_os("HZR_CAVEMAN_CODE_DIR") {
        return hzr_agent::IntegrationLayout::new(PathBuf::from(root));
    }
    match &config.engines.directory {
        Some(directory) => hzr_agent::IntegrationLayout::new(directory.join("caveman-code")),
        None => hzr_agent::IntegrationLayout::new(config.data_dir.join("engines/caveman-code")),
    }
}

/// Report the managed agent runtime from what is actually on disk.
///
/// The version used to be a string literal in this file and the state was unconditionally
/// `Stopped`, so an installation missing the runtime entirely reported exactly the same
/// health as a working one. An absent bridge or package will never start on demand — that
/// is a degradation, and the detail says how to repair it.
fn caveman_engine_health(layout: &hzr_agent::IntegrationLayout) -> EngineHealth {
    let version = locked_engines()
        .ok()
        .and_then(|manifest| {
            manifest
                .engine
                .into_iter()
                .find(|engine| engine.name == "caveman-code")
        })
        .map(|engine| engine.version);
    let missing = [
        ("bridge.mjs", layout.bridge()),
        (
            "node_modules/@juliusbrussee/caveman-code",
            layout.installed_package(),
        ),
    ]
    .into_iter()
    .filter(|(_, path)| !path.exists())
    .map(|(name, _)| name)
    .collect::<Vec<_>>();
    if missing.is_empty() {
        return EngineHealth {
            name: "caveman-code".into(),
            version,
            state: EngineState::Stopped,
            detail: Some("managed agent runtime is launched by hzr agent".into()),
        };
    }
    EngineHealth {
        name: "caveman-code".into(),
        version,
        state: EngineState::Degraded,
        detail: Some(format!(
            "managed agent runtime is not installed ({}); run `hzr install --force`",
            missing.join(", ")
        )),
    }
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct DashboardQuery {
    project: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct ProjectPageQuery {
    offset: Option<usize>,
    limit: Option<usize>,
}

pub async fn dashboard_projects(
    Query(query): Query<ProjectPageQuery>,
    State(state): State<AppState>,
) -> Result<Json<DashboardProjectPage>, ApiError> {
    let offset = query.offset.unwrap_or_default();
    let limit = query.limit.unwrap_or(DEFAULT_PROJECT_PAGE_LIMIT);
    validate_project_page(limit)?;
    let registry = registered_workspaces(&state.config.data_dir);
    let total = registry.registrations.len();
    let projects = registry
        .registrations
        .iter()
        .skip(offset)
        .take(limit)
        .map(|registration| dashboard_project(registration, &state.observability))
        .collect::<Result<Vec<_>, ApiError>>()?;
    let consumed = offset.saturating_add(projects.len());
    Ok(Json(DashboardProjectPage {
        projects,
        total,
        offset,
        limit,
        next_offset: (consumed < total).then_some(consumed),
    }))
}

fn validate_project_page(limit: usize) -> Result<(), ApiError> {
    if limit == 0 || limit > MAX_PROJECT_PAGE_LIMIT {
        return Err(ApiError::bad_request(
            "project page limit must be between 1 and 200",
        ));
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
pub struct ObservabilityQuery {
    project: String,
    after: Option<u64>,
    limit: Option<usize>,
}

pub async fn dashboard_observability(
    Query(query): Query<ObservabilityQuery>,
    State(state): State<AppState>,
) -> Result<Json<hzr_protocol::DashboardObservability>, ApiError> {
    let registrations = registered_workspaces(&state.config.data_dir).registrations;
    let registration = dashboard_registration(&registrations, Some(&query.project))?
        .expect("an explicitly requested dashboard project must resolve");
    let limit = query
        .limit
        .unwrap_or(crate::observability::DEFAULT_OBSERVABILITY_LIMIT);
    if limit == 0 || limit > crate::observability::MAX_OBSERVABILITY_LIMIT {
        return Err(ApiError::bad_request(
            "observability limit must be between 1 and 100",
        ));
    }
    let project_hash = state
        .observability
        .project_hash(&registration.root.to_string_lossy());
    Ok(Json(state.observability.snapshot(
        Some(&project_hash),
        query.after,
        limit,
    )))
}

pub async fn dashboard(
    Query(query): Query<DashboardQuery>,
    State(state): State<AppState>,
) -> Result<Json<DashboardResponse>, ApiError> {
    let ((memory_health, memory_retrieval), index_health) =
        tokio::join!(memory_engine_health(&state), grepai_engine_health(&state));
    let health = health_response(&state, memory_health.clone(), index_health);
    let registry = registered_workspaces(&state.config.data_dir);
    let registry_warnings = registry.warnings.len();
    let selected = dashboard_registration(&registry.registrations, query.project.as_deref())?;
    let all_projects = registry
        .registrations
        .iter()
        .map(|registration| dashboard_project(registration, &state.observability))
        .collect::<Result<Vec<_>, ApiError>>()?;
    let projects_total = all_projects.len();
    let projects = all_projects
        .iter()
        .take(DEFAULT_PROJECT_PAGE_LIMIT)
        .cloned()
        .collect::<Vec<_>>();
    let projects_next_offset = (projects.len() < projects_total).then_some(projects.len());
    let ledger_path = state.config.data_dir.join("ledger/hzr.sqlite");
    let selected_path = selected
        .as_ref()
        .and_then(|registration| registration.root.to_str())
        .map(str::to_owned);
    let selected_project_hash = selected.as_ref().map(|registration| {
        state
            .observability
            .project_hash(&registration.root.to_string_lossy())
    });
    let ledger_project_path = selected_path.clone();
    let ledger = tokio::task::spawn_blocking(move || {
        let summaries = Ledger::summaries_read_only(&ledger_path)?;
        let activity = ledger_project_path.as_deref().map_or_else(
            || Ok(hzr_core::ProjectActivitySummary::default()),
            |project| Ledger::project_activity_read_only(&ledger_path, project),
        )?;
        let session = activity
            .recent_operations
            .iter()
            .find_map(|operation| operation.session_hash.as_deref())
            .filter(|hash| hash.starts_with("hmac-sha256:"))
            .map(|session_hash| {
                Ledger::session_roi_read_only(
                    &ledger_path,
                    ledger_project_path.as_deref().unwrap_or_default(),
                    session_hash,
                )
                .map(|summary| (session_hash.to_owned(), summary))
            })
            .transpose()?;
        Ok::<_, hzr_core::LedgerError>((summaries.0, summaries.1, activity, session))
    })
    .await
    .map_err(|error| ApiError::internal(format!("dashboard ledger task failed: {error}")))?;
    let (observed, estimated, activity, session, ledger_error) = match ledger {
        Ok((observed, estimated, activity, session)) => {
            (observed, estimated, activity, session, None)
        }
        Err(error) => (
            hzr_core::LedgerSummary::default(),
            hzr_core::EfficiencySummary::default(),
            hzr_core::ProjectActivitySummary::default(),
            None,
            Some(error.to_string()),
        ),
    };

    let search_activity = dashboard_search_activity(&activity.recent_operations);
    let (memory_observatory, index_observatory) = tokio::join!(
        dashboard_memory_observatory(
            &state,
            selected.as_ref(),
            selected_project_hash.as_deref(),
            &memory_health,
            memory_retrieval,
        ),
        dashboard_index_observatory(
            &state,
            selected.as_ref(),
            selected_project_hash.as_deref(),
            search_activity,
        ),
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
        grepai.detail = match index_observatory.state {
            DashboardState::Ready => "Managed index artifacts and watcher are ready".into(),
            DashboardState::Rebuilding => "Managed index artifacts are warming".into(),
            DashboardState::Degraded => index_observatory.watcher.detail.clone(),
            _ => "Managed index is waiting for its watcher".into(),
        };
    }
    let selected_project = selected_path
        .as_deref()
        .and_then(|root| all_projects.iter().find(|project| project.root == root));
    let overall_state = dashboard_overall_state(&services, selected_project);
    let reduction_pct = signed_percentage(
        estimated.net_avoided_tokens_estimated,
        estimated.baseline_tokens_estimated,
    );
    let observability = state.observability.latest_snapshot(
        selected_project_hash.as_deref(),
        crate::observability::DEFAULT_OBSERVABILITY_LIMIT,
    );
    let mut notes = vec![
        "Provider-observed usage and UTF-8-byte estimates are displayed separately.".into(),
        "grepai traffic is shown only when a real HZR-routed search exists in the project ledger."
            .into(),
        "The visualizer is local-only and exposes no engine lifecycle mutations.".into(),
    ];
    if ledger_error.is_some() {
        notes.push(
            "Usage ledger totals are unavailable in this snapshot; no fallback totals were used."
                .into(),
        );
    }
    if estimated.excluded_legacy_operations > 0 || activity.excluded_legacy_operations > 0 {
        notes.push(format!(
            "Current efficiency uses accounting policy {}; {} global and {} selected-project legacy operation(s) are excluded from current claims.",
            hzr_core::CURRENT_ACCOUNTING_POLICY_VERSION,
            estimated.excluded_legacy_operations,
            activity.excluded_legacy_operations,
        ));
    }
    let session_roi = dashboard_session_roi(&state.config, session.as_ref());

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
        projects_total,
        projects_next_offset,
        selected_worktree_id: selected
            .as_ref()
            .map(|registration| registration.worktree_id.clone()),
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
            accounting_policy_version: hzr_core::CURRENT_ACCOUNTING_POLICY_VERSION.into(),
            excluded_legacy_operations: estimated.excluded_legacy_operations,
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
            project: selected_project_hash.clone(),
            accounting_policy_version: hzr_core::CURRENT_ACCOUNTING_POLICY_VERSION.into(),
            excluded_legacy_operations: activity.excluded_legacy_operations,
            operations: activity.operations,
            optimized_operations: activity.optimized_operations,
            raw_operations: activity.raw_operations,
            native_unaccounted_operations: activity.native_unaccounted_operations,
            unmeasured_bypass_operations: activity.unmeasured_bypass_operations,
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
                    ledger_id: operation.ledger_id,
                    timestamp: operation.timestamp,
                    operation: operation.operation,
                    route: match operation.route {
                        hzr_core::ProjectOperationRoute::Optimized => {
                            DashboardOperationRoute::Optimized
                        }
                        hzr_core::ProjectOperationRoute::Raw => DashboardOperationRoute::Raw,
                        hzr_core::ProjectOperationRoute::NativeUnaccounted => {
                            DashboardOperationRoute::NativeUnaccounted
                        }
                    },
                    command_hash: state.observability.command_hash(&operation.command_hash),
                    project_hash: selected_project_hash.clone().unwrap_or_default(),
                    agent: operation.agent,
                    session_hash: operation
                        .session_hash
                        .filter(|hash| hash.starts_with("hmac-sha256:")),
                    producer_version: operation.producer_version,
                    policy_version: operation.policy_version,
                    baseline_tokens_estimated: operation.baseline_tokens_estimated,
                    delivered_tokens_estimated: operation.delivered_tokens_estimated,
                    net_avoided_tokens_estimated: operation.net_avoided_tokens_estimated,
                    execution_ms: operation.execution_ms,
                    replacement: operation.replacement,
                    rationale: operation.rationale,
                })
                .collect(),
        },
        observability,
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
        session_roi,
        help: dashboard_help(),
        notes,
    }))
}

fn dashboard_session_roi(
    config: &hzr_core::Config,
    session: Option<&(
        String,
        (
            hzr_core::SessionEfficiencySummary,
            hzr_core::SessionEconomicSummary,
        ),
    )>,
) -> DashboardSessionRoi {
    let mut response = DashboardSessionRoi {
        selected_harness: config.billing.harness.clone(),
        selected_provider: config.billing.provider.clone(),
        selected_model: config.billing.model.clone(),
        selected_method: config.billing.method.clone(),
        selected_request_input_tokens: config.billing.request_input_tokens,
        selected_pricing_basis: config.billing.effective_pricing_basis().into(),
        detail: "No HMAC-attributed session is available for the selected project.".into(),
        ..DashboardSessionRoi::default()
    };
    let Some((session_hash, (efficiency, economics))) = session else {
        response.raw_public_estimate_unavailable_reason =
            Some("no attributed session is available for the selected project".into());
        return response;
    };
    response.session_hash = Some(session_hash.clone());
    response.operations = efficiency.operations;
    response.baseline_tokens_estimated = efficiency.baseline_tokens_estimated;
    response.delivered_tokens_estimated = efficiency.delivered_tokens_estimated;
    response.net_avoided_tokens_estimated = efficiency.net_avoided_tokens_estimated;
    response.top_commands = efficiency
        .top_commands
        .iter()
        .map(|command| DashboardSessionCommand {
            command_family: command.command.clone(),
            executions: command.executions,
            net_avoided_tokens_estimated: command.net_avoided_tokens_estimated,
        })
        .collect();
    response.imported_claim_records = economics.paired_receipts;
    response.reported_actual =
        economics
            .reported_actual
            .as_ref()
            .map(|amount| DashboardEconomicAmount {
                currency: amount.currency.clone(),
                baseline_microunits: amount.baseline_microunits,
                delivered_microunits: amount.delivered_microunits,
                savings_microunits: amount.savings_microunits,
            });
    response.receipt_provenance = economics
        .provenance
        .map(|provenance| provenance.as_str().to_owned());
    response.receipt_externally_verified = economics.externally_verified;
    response.detail = if economics.paired_receipts == 0 {
        "Latest HMAC-attributed session; no imported economic claim is attached.".into()
    } else {
        format!(
            "Latest HMAC-attributed session; {} user-supplied economic claim record(s), externally verified={}. {}",
            economics.paired_receipts,
            economics.externally_verified,
            economics.unavailable_reasons.join(" ")
        )
    };

    let catalog = load_pricing_catalog(config.billing.pricing_file.as_deref());
    if let Ok(catalog) = &catalog {
        response.catalog_identity = Some(catalog.identity.clone());
    }
    if !config.billing.public_estimate_enabled {
        response.raw_public_estimate_unavailable_reason =
            Some("public pricing estimate is opt-in and currently disabled".into());
        return response;
    }
    let catalog = match catalog {
        Ok(catalog) => catalog,
        Err(error) => {
            response.raw_public_estimate_unavailable_reason = Some(error.to_string());
            return response;
        }
    };
    match price_avoided_input_tokens(
        &catalog,
        RawPublicEstimateRequest {
            harness: &config.billing.harness,
            provider: &config.billing.provider,
            model: &config.billing.model,
            method: &config.billing.method,
            request_input_tokens: config.billing.request_input_tokens,
            basis: config.billing.effective_pricing_basis(),
            avoided_tokens: efficiency
                .net_avoided_tokens_estimated
                .max(0)
                .unsigned_abs(),
        },
    ) {
        Ok(estimate) => {
            response.raw_public_estimate = Some(DashboardRawPublicEstimate {
                currency: estimate.currency,
                savings_microunits: estimate.savings_microunits,
                avoided_input_tokens_estimated: estimate.avoided_input_tokens_estimated,
                pricing_basis: estimate.pricing_basis,
                catalog_identity: estimate.price_table_identity,
                entry_version: estimate.entry_version,
                preliminary: estimate.preliminary,
                disclaimer: estimate.disclaimer,
            });
        }
        Err(error) => response.raw_public_estimate_unavailable_reason = Some(error.to_string()),
    }
    response
}

pub async fn dashboard_memory_topic(
    State(state): State<AppState>,
    AxumPath(topic_id): AxumPath<String>,
    Query(query): Query<DashboardQuery>,
) -> Result<Json<DashboardMemoryTopicDetails>, ApiError> {
    let project = query.project.ok_or_else(|| {
        ApiError::bad_request("dashboard memory topic requires a stable project identifier")
    })?;
    memory_topic_response(&state, topic_id, true, Some(&project))
        .await
        .map(Json)
}

pub async fn memory_topic_details(
    State(state): State<AppState>,
    AxumPath(topic_id): AxumPath<String>,
    Query(query): Query<DashboardQuery>,
) -> Result<Json<DashboardMemoryTopicDetails>, ApiError> {
    let project = query.project.ok_or_else(|| {
        ApiError::bad_request("memory topic requires a stable project identifier")
    })?;
    memory_topic_response(&state, topic_id, false, Some(&project))
        .await
        .map(Json)
}

async fn memory_topic_response(
    state: &AppState,
    topic_id: String,
    redact_content: bool,
    worktree_id: Option<&str>,
) -> Result<DashboardMemoryTopicDetails, ApiError> {
    let public_topic_id = topic_id.strip_prefix("hmac-sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    });
    let internal_topic_id =
        topic_id.len() == 64 && topic_id.bytes().all(|byte| byte.is_ascii_hexdigit());
    if (redact_content && !public_topic_id) || (!redact_content && !internal_topic_id) {
        return Err(ApiError::bad_request(
            "memory topic identifier does not match the endpoint identity scheme",
        ));
    }
    let registrations = registered_workspaces(&state.config.data_dir).registrations;
    let registration = match worktree_id {
        Some(worktree_id) => dashboard_registration(&registrations, Some(worktree_id))?
            .expect("a requested dashboard project must resolve"),
        None => registrations.into_iter().next().ok_or_else(|| {
            ApiError::not_found("project_not_found", "no registered project selected")
        })?,
    };
    // Topic details are a read-only view of the canonical store. Keep them available
    // during an ICM restart; a missing/corrupt store still returns an explicit 503 below.
    let database = state.memory.layout().database.clone();
    let repository_id = registration.repository_id;
    let requested_id = topic_id.clone();
    let privacy = state.ledger.privacy_pseudonymizer();
    let details = tokio::task::spawn_blocking(move || {
        let source_id = if redact_content {
            read_project_snapshot(&database, &repository_id)?
                .topics
                .into_iter()
                .find(|topic| privacy.hash("topic", &topic.id) == requested_id)
                .map(|topic| topic.id)
        } else {
            Some(requested_id)
        };
        source_id
            .map(|source_id| read_project_topic_details(&database, &repository_id, &source_id))
            .transpose()
            .map(Option::flatten)
    })
    .await
    .map_err(|error| ApiError::internal(format!("memory topic task failed: {error}")))?
    .map_err(|error| {
        ApiError::service(
            "memory_topic_unavailable",
            format!("failed to read the project memory topic: {error}"),
            true,
        )
    })?
    .ok_or_else(|| {
        ApiError::not_found(
            "memory_topic_not_found",
            "memory topic does not exist in the selected project",
        )
    })?;

    let public_privacy = state.ledger.privacy_pseudonymizer();
    Ok(DashboardMemoryTopicDetails {
        id: if redact_content {
            public_privacy.hash("topic", &details.id)
        } else {
            details.id
        },
        label: if redact_content {
            "Memory topic".into()
        } else {
            details.label
        },
        memory_count: details.memory_count,
        visible_memory_count: details.visible_memory_count,
        hidden_memory_count: details.hidden_memory_count,
        truncated: details.truncated,
        memories: details
            .memories
            .into_iter()
            .map(|memory| dashboard_memory_detail(memory, redact_content, &public_privacy))
            .collect(),
    })
}

fn dashboard_memory_detail(
    memory: ProjectMemoryDetail,
    redact_content: bool,
    privacy: &hzr_core::PrivacyPseudonymizer,
) -> DashboardMemoryDetail {
    DashboardMemoryDetail {
        id: if redact_content {
            privacy.hash("memory", &memory.id)
        } else {
            memory.id
        },
        created_at: memory.created_at,
        updated_at: memory.updated_at,
        last_accessed: memory.last_accessed,
        access_count: memory.access_count,
        weight: memory.weight,
        summary: if redact_content {
            "Memory content is redacted on the public dashboard endpoint.".into()
        } else {
            memory.summary
        },
        raw_excerpt: if redact_content {
            None
        } else {
            memory.raw_excerpt
        },
        keywords: if redact_content {
            Vec::new()
        } else {
            memory.keywords
        },
        importance: memory.importance,
        source_type: memory.source_type,
        source_data: if redact_content {
            None
        } else {
            memory.source_data
        },
        related_ids: if redact_content {
            memory
                .related_ids
                .into_iter()
                .map(|id| privacy.hash("memory", &id))
                .collect()
        } else {
            memory.related_ids
        },
    }
}

async fn dashboard_memory_observatory(
    state: &AppState,
    selected: Option<&WorkspaceRegistration>,
    project_identity: Option<&str>,
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
            project_identity,
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
            state,
            project_identity,
            retrieval,
            observed_at_ms,
            elapsed_ms(started_at),
            runtime_detail,
            snapshot,
        ),
        Ok(Err(error)) => empty_memory_observatory(
            project_identity,
            DashboardState::Degraded,
            retrieval,
            observed_at_ms,
            elapsed_ms(started_at),
            format!("ICM is ready, but its read-only project snapshot failed: {error}"),
        ),
        Err(error) => empty_memory_observatory(
            project_identity,
            DashboardState::Degraded,
            retrieval,
            observed_at_ms,
            elapsed_ms(started_at),
            format!("ICM snapshot task failed: {error}"),
        ),
    }
}

fn memory_snapshot_observatory(
    state: &AppState,
    project_identity: Option<&str>,
    retrieval: DashboardMemoryRetrieval,
    observed_at_ms: u64,
    latency_ms: u64,
    runtime_detail: String,
    snapshot: ProjectMemorySnapshot,
) -> DashboardMemoryObservatory {
    DashboardMemoryObservatory {
        state: DashboardState::Ready,
        project: project_identity.map(str::to_owned),
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
            .enumerate()
            .map(|(index, topic)| DashboardMemoryTopic {
                id: state.observability.topic_hash(&topic.id),
                label: format!("Memory topic {}", index + 1),
                memory_count: topic.memory_count,
                average_weight: topic.average_weight,
                newest_at: topic.newest_at,
            })
            .collect(),
        edges: snapshot
            .edges
            .into_iter()
            .map(|edge| DashboardMemoryEdge {
                source: state.observability.topic_hash(&edge.source),
                target: state.observability.topic_hash(&edge.target),
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
    project_identity: Option<&str>,
    state: DashboardState,
    retrieval: DashboardMemoryRetrieval,
    observed_at_ms: u64,
    latency_ms: u64,
    detail: String,
) -> DashboardMemoryObservatory {
    DashboardMemoryObservatory {
        state,
        project: project_identity.map(str::to_owned),
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
    project_identity: Option<&str>,
    search_activity: DashboardSearchActivity,
) -> DashboardIndexObservatory {
    let observed_at_ms = now_ms().unwrap_or_default();
    let Some(registration) = selected else {
        return empty_index_observatory(
            None,
            DashboardState::Standby,
            observed_at_ms,
            "No registered project is selected".into(),
            search_activity,
        );
    };
    let initial = match state.context.index_status(&registration.root).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return empty_index_observatory(
                project_identity.map(str::to_owned),
                DashboardState::Degraded,
                observed_at_ms,
                format!("grepai index status failed: {error}"),
                search_activity,
            );
        }
    };
    index_snapshot_observatory(project_identity, observed_at_ms, initial, search_activity)
}

fn index_snapshot_observatory(
    project_identity: Option<&str>,
    observed_at_ms: u64,
    snapshot: IndexCoordinatorSnapshot,
    search_activity: DashboardSearchActivity,
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
    let state = dashboard_index_state(artifact_ready, snapshot.watcher.state);
    DashboardIndexObservatory {
        state,
        project: project_identity.map(str::to_owned),
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
        search_activity,
        diagnostic_command: "hzr index status --workspace .".into(),
    }
}

fn dashboard_index_state(artifact_ready: bool, watcher_state: IndexWatcherState) -> DashboardState {
    match watcher_state {
        IndexWatcherState::Failed => DashboardState::Degraded,
        IndexWatcherState::Live if artifact_ready => DashboardState::Ready,
        IndexWatcherState::Live => DashboardState::Rebuilding,
        IndexWatcherState::Standby => DashboardState::Standby,
    }
}

fn empty_index_observatory(
    project: Option<String>,
    state: DashboardState,
    observed_at_ms: u64,
    detail: String,
    search_activity: DashboardSearchActivity,
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
        search_activity,
        diagnostic_command: "hzr index status --workspace .".into(),
    }
}

fn dashboard_search_activity(operations: &[ProjectOperationSummary]) -> DashboardSearchActivity {
    let Some(operation) = operations.iter().find(|operation| {
        operation.route == ProjectOperationRoute::Optimized
            && matches!(operation.operation.as_str(), "rgai" | "search")
    }) else {
        return DashboardSearchActivity {
            state: DashboardState::Standby,
            ledger_id: None,
            observed_at: None,
            operation: None,
            command_hash: None,
            project_hash: None,
            agent: None,
            session_hash: None,
            route: None,
            execution_ms: None,
            detail: "No routed HZR search is present in the recent project ledger window".into(),
        };
    };

    DashboardSearchActivity {
        state: DashboardState::Ready,
        ledger_id: Some(operation.ledger_id),
        observed_at: Some(operation.timestamp.clone()),
        operation: Some(operation.operation.clone()),
        command_hash: Some(operation.command_hash.clone()),
        project_hash: Some(operation.project_hash.clone()),
        agent: operation.agent.clone(),
        session_hash: operation.session_hash.clone(),
        route: Some(DashboardOperationRoute::Optimized),
        execution_ms: Some(operation.execution_ms),
        detail: "Observed from a real HZR-routed search recorded in the project ledger".into(),
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

fn dashboard_registration(
    registrations: &[WorkspaceRegistration],
    worktree_id: Option<&str>,
) -> Result<Option<WorkspaceRegistration>, ApiError> {
    worktree_id
        .map(|worktree_id| {
            if worktree_id.len() != 64 || !worktree_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(ApiError::bad_request(
                    "dashboard project identifier must be 64 hexadecimal characters",
                ));
            }
            registrations
                .iter()
                .find(|registration| registration.worktree_id == worktree_id)
                .cloned()
                .ok_or_else(|| {
                    ApiError::not_found(
                        "dashboard_project_not_found",
                        "dashboard project is no longer registered",
                    )
                })
        })
        .transpose()
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

/// Posture describes this control plane and the project in view, not the whole fleet.
///
/// Scoring every registered workspace here made the posture permanently `Rebuilding`: any
/// project that had never been indexed counted as work in progress, so a ready daemon with
/// no project selected still reported a rebuild. Fleet progress has its own reading in
/// `projects_index_ready` / `projects_total` and does not belong in the posture.
fn dashboard_overall_state(
    services: &[DashboardService],
    selected: Option<&DashboardProject>,
) -> DashboardState {
    let service_state = |states: &[DashboardState]| {
        services
            .iter()
            .any(|service| states.contains(&service.state))
    };
    let selected_state = |states: &[DashboardProjectState]| {
        selected.is_some_and(|project| states.contains(&project.state))
    };
    if service_state(&[
        DashboardState::Degraded,
        DashboardState::Stopped,
        DashboardState::Unknown,
    ]) || selected_state(&[
        DashboardProjectState::Degraded,
        DashboardProjectState::Unavailable,
    ]) {
        DashboardState::Degraded
    } else if service_state(&[DashboardState::Rebuilding])
        || selected_state(&[
            DashboardProjectState::Warming,
            DashboardProjectState::Registered,
        ])
    {
        DashboardState::Rebuilding
    } else if selected.is_none() {
        // Idle, not healthy-and-working: nothing is selected, so there is nothing to be
        // ready about. The engines report their own readiness beside this chip.
        DashboardState::Standby
    } else {
        DashboardState::Ready
    }
}

fn dashboard_project(
    registration: &WorkspaceRegistration,
    observability: &crate::observability::ObservabilityStore,
) -> Result<DashboardProject, ApiError> {
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
    let identity = observability.project_hash(root);
    let short_identity = identity
        .strip_prefix("hmac-sha256:")
        .unwrap_or(&identity)
        .chars()
        .take(8)
        .collect::<String>();
    Ok(DashboardProject {
        name: format!("Project {short_identity}"),
        root: identity,
        repository_id: observability.repository_hash(&registration.repository_id),
        worktree_id: registration.worktree_id.clone(),
        git_backed: registration.git_backed,
        linked_worktree: registration.linked_worktree,
        state,
        registered_at_ms: registration.registered_at_ms,
        last_seen_at_ms: registration.last_seen_at_ms,
        artifacts,
        command: "hzr index status --workspace <workspace>".into(),
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
    let started = Instant::now();
    let trace = state
        .observability
        .begin_trace(&workspace.to_string_lossy(), None);
    let outcome = state
        .context
        .search(SearchRequest {
            workspace: workspace.clone(),
            query: request.query,
            path: request.path.map(PathBuf::from),
            limit: request.limit,
            mode: request.mode,
            include_content: request.include_content,
        })
        .await;
    let trace_state = if outcome.is_ok() {
        DashboardTraceState::Completed
    } else {
        DashboardTraceState::Failed
    };
    let generation = outcome
        .as_ref()
        .ok()
        .and_then(|response| response.index_generation.as_deref());
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Engine,
            state: trace_state,
            engine: "grepai",
            duration_ms: elapsed_ms(started),
            route: Some("search"),
            error_code: outcome.as_ref().err().map(|_| "search_failed"),
            generation,
        },
    );
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Request,
            state: trace_state,
            engine: "hzrd",
            duration_ms: elapsed_ms(started),
            route: Some("search"),
            error_code: outcome.as_ref().err().map(|_| "search_failed"),
            generation,
        },
    );
    outcome.map(Json).map_err(context_error)
}

pub async fn semantic_readiness(
    State(state): State<AppState>,
    Json(request): Json<hzr_protocol::SemanticReadinessApiRequest>,
) -> Result<Json<hzr_protocol::SemanticReadinessApiResponse>, ApiError> {
    let workspace = canonical_workspace(&request.workspace)?;
    state
        .context
        .semantic_readiness(&workspace)
        .await
        .map_err(context_error)?;
    Ok(Json(hzr_protocol::SemanticReadinessApiResponse {
        ready: true,
        detail: "managed fork configuration is compatible with semantic search".into(),
    }))
}

pub async fn context_plan(
    State(state): State<AppState>,
    Json(request): Json<ContextPlanApiRequest>,
) -> Result<Json<ContextPlanApiResponse>, ApiError> {
    let workspace = canonical_workspace(&request.workspace)?;
    let started = Instant::now();
    let trace = state
        .observability
        .begin_trace(&workspace.to_string_lossy(), None);
    let outcome = state
        .context
        .plan(PlanRequest {
            workspace: workspace.clone(),
            intent: request.intent,
            path: request.path.map(PathBuf::from),
            topic: request.topic,
            search_limit: request.search_limit,
            memory_limit: request.memory_limit,
        })
        .await;
    let trace_state = if outcome.is_ok() {
        DashboardTraceState::Completed
    } else {
        DashboardTraceState::Failed
    };
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Engine,
            state: trace_state,
            engine: "context_planner",
            duration_ms: elapsed_ms(started),
            route: Some("context_plan"),
            error_code: outcome.as_ref().err().map(|_| "context_plan_failed"),
            generation: None,
        },
    );
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Request,
            state: trace_state,
            engine: "hzrd",
            duration_ms: elapsed_ms(started),
            route: Some("context_plan"),
            error_code: outcome.as_ref().err().map(|_| "context_plan_failed"),
            generation: None,
        },
    );
    outcome.map(Json).map_err(context_error)
}

pub async fn memory_recall(
    State(state): State<AppState>,
    Json(request): Json<MemoryRecallApiRequest>,
) -> Result<Json<hzr_memory::MemoryRecallResponse>, ApiError> {
    if request.query.trim().is_empty() {
        return Err(ApiError::bad_request("memory query must not be empty"));
    }
    validate_limit(request.limit)?;
    let project = memory_project(&state, &request.workspace).await?;
    let request_started = Instant::now();
    let trace = state.observability.begin_trace(&project, None);
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

    let records_result = async {
        Ok::<Vec<MemoryRecord>, ApiError>(match request.scope {
            MemoryScopeSelector::Project => isolate_memories(
                client.recall(&project_recall).await.map_err(unavailable)?,
                &project,
                MemoryNamespace::Project,
                project_topic.as_deref(),
                candidate_limit,
            ),
            MemoryScopeSelector::Global => isolate_memories(
                client.recall(&global_recall).await.map_err(unavailable)?,
                &project,
                MemoryNamespace::Global,
                global_topic.as_deref(),
                candidate_limit,
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
                merge_memories(project_records, global_records, candidate_limit)
            }
        })
    }
    .await;
    let records = match records_result {
        Ok(records) => records,
        Err(error) => {
            state.observability.record_span(
                &trace,
                TraceSpanInput {
                    stage: DashboardTraceStage::Engine,
                    state: DashboardTraceState::Failed,
                    engine: "icm",
                    duration_ms: elapsed_ms(request_started),
                    route: Some("memory_recall"),
                    error_code: Some("memory_unavailable"),
                    generation: None,
                },
            );
            state.observability.record_span(
                &trace,
                TraceSpanInput {
                    stage: DashboardTraceStage::Request,
                    state: DashboardTraceState::Failed,
                    engine: "hzrd",
                    duration_ms: elapsed_ms(request_started),
                    route: Some("memory_recall"),
                    error_code: Some("memory_unavailable"),
                    generation: None,
                },
            );
            return Err(error);
        }
    };
    let total_matches = records.len();
    let memories = records.into_iter().take(request.limit).collect::<Vec<_>>();
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Engine,
            state: DashboardTraceState::Completed,
            engine: "icm",
            duration_ms: elapsed_ms(request_started),
            route: Some("memory_recall"),
            error_code: None,
            generation: None,
        },
    );
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Request,
            state: DashboardTraceState::Completed,
            engine: "hzrd",
            duration_ms: elapsed_ms(request_started),
            route: Some("memory_recall"),
            error_code: None,
            generation: None,
        },
    );
    Ok(Json(hzr_memory::MemoryRecallResponse {
        count: memories.len(),
        total_matches,
        memories,
    }))
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
    let started = Instant::now();
    let trace = state.observability.begin_trace(&project, None);
    // One write targets exactly one namespace, so a global preference is stored once and
    // is reachable from every repository instead of being duplicated per project.
    let topic = match request.scope {
        MemoryWriteScope::Project => namespaced_topic(&request.topic, &project),
        MemoryWriteScope::Global => global_topic(&request.topic),
    }
    .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let mut store = StoreRequest::new(topic, request.content);
    store.importance = memory_importance(request.importance);
    store.keywords = request.keywords;
    store.raw = request.raw;
    let outcome = state.memory.client().store(&store).await;
    let trace_state = if outcome.is_ok() {
        DashboardTraceState::Completed
    } else {
        DashboardTraceState::Failed
    };
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Engine,
            state: trace_state,
            engine: "icm",
            duration_ms: elapsed_ms(started),
            route: Some("memory_store"),
            error_code: outcome.as_ref().err().map(|_| "memory_unavailable"),
            generation: None,
        },
    );
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Request,
            state: trace_state,
            engine: "hzrd",
            duration_ms: elapsed_ms(started),
            route: Some("memory_store"),
            error_code: outcome.as_ref().err().map(|_| "memory_unavailable"),
            generation: None,
        },
    );
    outcome
        .map(Json)
        .map_err(|error| ApiError::service("memory_unavailable", error.to_string(), true))
}

pub async fn memory_forget(
    State(state): State<AppState>,
    Json(request): Json<MemoryForgetApiRequest>,
) -> Result<Json<MemoryMutationApiResponse>, ApiError> {
    validate_memory_id(&request.id)?;
    let records = scoped_memory_records(&state, &request.workspace, request.scope).await?;
    if !records.iter().any(|record| record.id == request.id) {
        return Err(ApiError::not_found(
            "memory_not_found",
            "memory does not exist in the requested namespace",
        ));
    }
    state
        .memory
        .client()
        .forget(&request.id)
        .await
        .map_err(memory_maintenance_error)?;
    Ok(Json(MemoryMutationApiResponse {
        affected_ids: vec![request.id],
        dry_run: false,
    }))
}

pub async fn memory_update(
    State(state): State<AppState>,
    Json(request): Json<MemoryUpdateApiRequest>,
) -> Result<Json<MemoryMutationApiResponse>, ApiError> {
    validate_memory_id(&request.id)?;
    if request.content.trim().is_empty() {
        return Err(ApiError::bad_request("memory content must not be empty"));
    }
    if request
        .keywords
        .as_ref()
        .is_some_and(|keywords| keywords.len() > 32)
    {
        return Err(ApiError::bad_request(
            "memory keywords must contain at most 32 entries",
        ));
    }
    let records = scoped_memory_records(&state, &request.workspace, request.scope).await?;
    if !records.iter().any(|record| record.id == request.id) {
        return Err(ApiError::not_found(
            "memory_not_found",
            "memory does not exist in the requested namespace",
        ));
    }
    state
        .memory
        .client()
        .update(
            &request.id,
            &request.content,
            request.importance.map(memory_importance),
            request.keywords.as_deref(),
        )
        .await
        .map_err(memory_maintenance_error)?;
    Ok(Json(MemoryMutationApiResponse {
        affected_ids: vec![request.id],
        dry_run: false,
    }))
}

pub async fn memory_prune(
    State(state): State<AppState>,
    Json(request): Json<MemoryPruneApiRequest>,
) -> Result<Json<MemoryMutationApiResponse>, ApiError> {
    if !request.threshold.is_finite() || !(0.0..=1.0).contains(&request.threshold) {
        return Err(ApiError::bad_request(
            "memory prune threshold must be finite and between 0 and 1",
        ));
    }
    let records = scoped_memory_records(&state, &request.workspace, request.scope).await?;
    let targets = records
        .into_iter()
        .filter(|record| memory_prunable(record, request.threshold))
        .collect::<Vec<_>>();
    let affected_ids = targets
        .iter()
        .map(|record| record.id.clone())
        .collect::<Vec<_>>();
    if !request.dry_run {
        for (index, id) in affected_ids.iter().enumerate() {
            if let Err(error) = state.memory.client().forget(id).await {
                return Err(ApiError::service(
                    "memory_prune_partial",
                    format!(
                        "memory prune stopped after deleting {index} of {} selected records; \
                         inspect the namespace before retrying: {error}",
                        affected_ids.len()
                    ),
                    true,
                ));
            }
        }
    }
    Ok(Json(MemoryMutationApiResponse {
        affected_ids,
        dry_run: request.dry_run,
    }))
}

fn memory_importance(importance: MemoryImportance) -> Importance {
    match importance {
        MemoryImportance::Critical => Importance::Critical,
        MemoryImportance::High => Importance::High,
        MemoryImportance::Medium => Importance::Medium,
        MemoryImportance::Low => Importance::Low,
    }
}

fn validate_memory_id(id: &str) -> Result<(), ApiError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(ApiError::bad_request(
            "memory id must contain 1..128 ASCII letters, digits, hyphens, or underscores",
        ));
    }
    Ok(())
}

async fn scoped_memory_records(
    state: &AppState,
    workspace: &str,
    scope: MemoryWriteScope,
) -> Result<Vec<MemoryRecord>, ApiError> {
    let project = memory_project(state, workspace).await?;
    let records = state
        .memory
        .client()
        .list_all()
        .await
        .map_err(memory_maintenance_error)?;
    Ok(memory_mutation_targets(records, &project, scope, None))
}

fn memory_mutation_targets(
    records: Vec<MemoryRecord>,
    project: &str,
    scope: MemoryWriteScope,
    threshold: Option<f32>,
) -> Vec<MemoryRecord> {
    let namespace = match scope {
        MemoryWriteScope::Project => MemoryNamespace::Project,
        MemoryWriteScope::Global => MemoryNamespace::Global,
    };
    isolate_memories(records, project, namespace, None, usize::MAX)
        .into_iter()
        .filter(|record| threshold.is_none_or(|value| memory_prunable(record, value)))
        .collect()
}

fn memory_prunable(record: &MemoryRecord, threshold: f32) -> bool {
    record.weight < threshold && matches!(record.importance, Importance::Medium | Importance::Low)
}

fn memory_maintenance_error(error: hzr_memory::MemoryError) -> ApiError {
    ApiError::service("memory_unavailable", error.to_string(), true)
}

pub async fn exec_run(
    State(state): State<AppState>,
    Json(request): Json<ExecApiRequest>,
) -> Result<Json<ExecutionOutcome>, ApiError> {
    if request.command.trim().is_empty() {
        return Err(ApiError::bad_request("command must not be empty"));
    }
    validate_exec_timeout(request.timeout_ms)?;
    validate_caller_path(request.caller_path.as_deref())?;
    let budget = ManagedExecutionBudget::new(&state, request.timeout_ms)?;
    let cwd = canonical_workspace(&request.cwd)?;
    let request_started = Instant::now();
    let trace = state
        .observability
        .begin_trace(&cwd.to_string_lossy(), request.session_id.as_deref());
    let command = CanonicalCommand::shell(request.command.clone());
    let policy_started = Instant::now();
    let (preflight, fidelity_reservation) =
        daemon_fidelity_preflight_for_run(&state.ledger, &request, &cwd).await;
    let mut plan = match &preflight {
        FidelityPreflight::Ask { evasion, reason } => RtkRewriteOutcome {
            decision: RewriteDecision::Ask {
                proposed: Some(command.clone()),
                reason: reason.clone(),
            },
            evasion: Some(*evasion),
        },
        FidelityPreflight::NotRequested | FidelityPreflight::Allow { .. } => {
            if request.fidelity_requested {
                let command = CanonicalCommand::shell(request.command.clone());
                state
                    .rtk
                    .decide_byte_fidelity_with_plan_in(&command, Some(&cwd))
                    .await
            } else {
                fork_outcome_with_managed_unwrap(&state.rtk, &request.command, &cwd).await
            }
        }
    };
    if let FidelityPreflight::Allow { evasion, .. } = preflight {
        plan.evasion = Some(evasion);
    }
    let decision = apply_host_grant(
        &request,
        enforce_first_class(&request.command, plan.decision.clone()),
    );
    let grant_applied = host_grant_applied(&decision);
    let route = match &decision {
        RewriteDecision::AllowRewrite { .. } => "optimized",
        RewriteDecision::AllowRaw { .. } => "raw",
        RewriteDecision::Ask { .. } => "ask",
        RewriteDecision::Deny { .. } => "deny",
    };
    let decision_state = match &decision {
        RewriteDecision::Ask { .. } => DashboardTraceState::ApprovalRequired,
        RewriteDecision::Deny { .. } => DashboardTraceState::Denied,
        _ => DashboardTraceState::Completed,
    };
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Policy,
            state: decision_state,
            engine: "rtk",
            duration_ms: elapsed_ms(policy_started),
            route: Some(route),
            error_code: None,
            generation: None,
        },
    );
    let ledger_started = Instant::now();
    let policy_event_expected =
        plan.evasion.is_some() && !matches!(&decision, RewriteDecision::AllowRaw { .. });
    if let Err(error) =
        record_exec_policy_event(&state, &request, &cwd, plan.evasion.as_ref(), &decision).await
    {
        state.observability.record_span(
            &trace,
            TraceSpanInput {
                stage: DashboardTraceStage::Ledger,
                state: DashboardTraceState::Failed,
                engine: "ledger",
                duration_ms: elapsed_ms(ledger_started),
                route: Some("policy_event"),
                error_code: Some("policy_event_write_failed"),
                generation: None,
            },
        );
        state.observability.record_span(
            &trace,
            TraceSpanInput {
                stage: DashboardTraceStage::Request,
                state: DashboardTraceState::Failed,
                engine: "hzrd",
                duration_ms: elapsed_ms(request_started),
                route: Some(route),
                error_code: Some("ledger_unavailable"),
                generation: None,
            },
        );
        if let Some(reservation) = fidelity_reservation {
            let _ = state.ledger.cancel_fidelity(reservation).await;
        }
        return Err(error);
    }
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Ledger,
            state: if policy_event_expected {
                DashboardTraceState::Completed
            } else {
                DashboardTraceState::Skipped
            },
            engine: "ledger",
            duration_ms: elapsed_ms(ledger_started),
            route: Some("policy_event"),
            error_code: None,
            generation: None,
        },
    );
    let decision = match decision {
        RewriteDecision::Ask { proposed, reason } => {
            if let Some(reservation) = fidelity_reservation {
                let _ = state.ledger.cancel_fidelity(reservation).await;
            }
            let timeout_ms = Some(budget.limit_ms());
            let decision_id = if let Some(proposed_command) = proposed.clone() {
                Some(
                    state
                        .approvals
                        .insert(PendingApproval {
                            requested: command.clone(),
                            approved_decision: approved_execution_decision(
                                proposed_command,
                                plan.evasion.as_ref(),
                            ),
                            evasion: plan.evasion,
                            cwd,
                            timeout_ms,
                            caller_path: request.caller_path,
                            agent: request.agent,
                            session_id: request.session_id,
                            trace_hash: trace.hash.clone(),
                        })
                        .await,
                )
            } else {
                None
            };
            state.observability.record_span(
                &trace,
                TraceSpanInput {
                    stage: DashboardTraceStage::Request,
                    state: DashboardTraceState::ApprovalRequired,
                    engine: "hzrd",
                    duration_ms: elapsed_ms(request_started),
                    route: Some(route),
                    error_code: None,
                    generation: None,
                },
            );
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
    envelope.cwd = Some(cwd.clone());
    envelope.timeout_ms = Some(budget.remaining_ms()?);
    RtkRewriteOutcome {
        decision: envelope.decision.clone(),
        evasion: plan.evasion,
    }
    .apply_evasion_environment(&mut envelope.environment)
    .map_err(|error| ApiError::internal(format!("evasion plan encoding failed: {error}")))?;
    if grant_applied {
        envelope
            .environment
            .set
            .insert(HOST_GRANT_APPLIED_ENV.into(), "1".into());
    }
    apply_caller_path(&mut envelope, request.caller_path.clone());
    if let (Some(reservation), Some(evasion)) = (fidelity_reservation.as_ref(), plan.evasion) {
        if let Err(error) = state
            .ledger
            .begin_fidelity(
                reservation,
                fidelity_pending_record_for_context(
                    &cwd,
                    request.agent.clone(),
                    request.session_id.clone(),
                    evasion,
                    grant_applied,
                ),
            )
            .await
        {
            let cancellation = state.ledger.cancel_fidelity(reservation.clone()).await;
            return Err(ApiError::service(
                "fidelity_execution_boundary_failed",
                format!(
                    "command was not executed because the durable execution boundary failed: {error}; cancellation={cancellation:?}; rerun the original command"
                ),
                false,
            ));
        }
    }
    let engine_started = Instant::now();
    let (outcome, process_started) = match state.executor.start(envelope) {
        Ok(handle) => (handle.wait().await, true),
        Err(error) => {
            if let Some(reservation) = fidelity_reservation.as_ref() {
                if let Err(recovery) = state
                    .ledger
                    .recover_fidelity_pre_spawn(reservation.clone())
                    .await
                {
                    return Err(ApiError::service(
                        "fidelity_pre_spawn_recovery_failed",
                        format!(
                            "process was not spawned ({error}), but its durable reservation could not be released: {recovery}; do not replay until `hzr doctor` is clean"
                        ),
                        false,
                    ));
                }
            }
            (Err(error), false)
        }
    };
    let fidelity_execution_unknown =
        fidelity_reservation.is_some() && process_started && outcome.is_err();
    let fidelity_accounting_error = match fidelity_reservation {
        Some(reservation) => match &outcome {
            Ok(execution) => match plan.evasion.and_then(|evasion| {
                fidelity_operation_record(&request, &cwd, evasion, execution, grant_applied)
            }) {
                Some(record) => state
                    .ledger
                    .complete_fidelity(reservation, record)
                    .await
                    .err(),
                None => Some(crate::ledger_writer::LedgerWriterError::execution_unknown(
                    "executor returned no recordable completion after the execution boundary; durable state retained",
                )),
            },
            Err(_) => None,
        },
        None => None,
    };
    let execution_state = if outcome.is_ok() {
        DashboardTraceState::Completed
    } else {
        DashboardTraceState::Failed
    };
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Engine,
            state: execution_state,
            engine: "rtk",
            duration_ms: elapsed_ms(engine_started),
            route: Some(route),
            error_code: outcome.as_ref().err().map(|_| "execution_failed"),
            generation: None,
        },
    );
    if fidelity_accounting_error.is_some() {
        state.observability.record_span(
            &trace,
            TraceSpanInput {
                stage: DashboardTraceStage::Ledger,
                state: DashboardTraceState::Failed,
                engine: "ledger",
                duration_ms: 0,
                route: Some("fidelity_completion"),
                error_code: Some("fidelity_accounting_failed"),
                generation: None,
            },
        );
    }
    let request_state = if outcome.is_ok() && fidelity_accounting_error.is_none() {
        DashboardTraceState::Completed
    } else {
        DashboardTraceState::Failed
    };
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Request,
            state: request_state,
            engine: "hzrd",
            duration_ms: elapsed_ms(request_started),
            route: Some(route),
            error_code: fidelity_accounting_error
                .as_ref()
                .map(|_| "fidelity_accounting_failed")
                .or_else(|| outcome.as_ref().err().map(|_| "execution_failed")),
            generation: None,
        },
    );
    let outcome = outcome.map_err(|error| {
        if fidelity_execution_unknown {
            ApiError::service(
                "fidelity_execution_state_unknown",
                format!(
                    "execution may have started before the executor failed: {error}; do not replay; inspect `hzr doctor`"
                ),
                false,
            )
        } else {
            ApiError::service("execution_failed", error.to_string(), true)
        }
    })?;
    Ok(Json(mark_accounting_incomplete(
        outcome,
        fidelity_accounting_error.as_ref(),
    )))
}

fn fidelity_operation_record(
    request: &ExecApiRequest,
    cwd: &Path,
    evasion: EvasionAttribution,
    outcome: &ExecutionOutcome,
    host_grant_applied: bool,
) -> Option<crate::ledger_writer::OperationRecord> {
    fidelity_operation_record_for_context(
        cwd,
        request.agent.clone(),
        request.session_id.clone(),
        evasion,
        outcome,
        host_grant_applied,
    )
}

fn fidelity_operation_record_for_context(
    cwd: &Path,
    agent: Option<String>,
    session_id: Option<String>,
    evasion: EvasionAttribution,
    outcome: &ExecutionOutcome,
    host_grant_applied: bool,
) -> Option<crate::ledger_writer::OperationRecord> {
    let ExecutionOutcome::Completed { result } = outcome else {
        return None;
    };
    let delivered_bytes = result
        .stdout
        .total_bytes
        .saturating_add(result.stderr.total_bytes);
    let delivered_tokens = delivered_bytes.div_ceil(4);
    Some(crate::ledger_writer::OperationRecord {
        original_command: "hzr exec fidelity".into(),
        recorded_command: "hzr exec fidelity".into(),
        input_tokens: delivered_tokens,
        output_tokens: delivered_tokens,
        execution_ms: result.duration_ms,
        project_path: cwd.to_string_lossy().into_owned(),
        channel: hzr_core::OperationChannel::HookCli,
        measurement: hzr_core::OperationMeasurement::Estimated,
        route: hzr_core::OperationRoute::Bypassed,
        agent,
        session_id,
        attribution: None,
        evasion: Some(evasion),
        host_grant_applied,
    })
}

fn fidelity_pending_record_for_context(
    cwd: &Path,
    agent: Option<String>,
    session_id: Option<String>,
    evasion: EvasionAttribution,
    host_grant_applied: bool,
) -> crate::ledger_writer::OperationRecord {
    crate::ledger_writer::OperationRecord {
        original_command: "hzr exec fidelity".into(),
        recorded_command: "hzr exec fidelity".into(),
        input_tokens: 0,
        output_tokens: 0,
        execution_ms: 0,
        project_path: cwd.to_string_lossy().into_owned(),
        channel: hzr_core::OperationChannel::HookCli,
        measurement: hzr_core::OperationMeasurement::Unmeasured,
        route: hzr_core::OperationRoute::Bypassed,
        agent,
        session_id,
        attribution: None,
        evasion: Some(evasion),
        host_grant_applied,
    }
}

fn approved_execution_decision(
    proposed: CanonicalCommand,
    evasion: Option<&EvasionAttribution>,
) -> RewriteDecision {
    if evasion.is_some_and(|evasion| evasion.class == EvasionClass::E7FidelityHatch) {
        RewriteDecision::allow_raw("user approved the pending T4 fidelity command")
    } else {
        RewriteDecision::AllowRewrite {
            command: proposed,
            source: RewriteSource::Rtk {
                version: PINNED_RTK_VERSION.into(),
                route: hzr_exec::RtkRewriteRoute::Optimized,
            },
            reason: "user approved the pending fork-core command".into(),
        }
    }
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
    let request_started = Instant::now();
    let trace = state.observability.begin_continuation(
        &pending.cwd.to_string_lossy(),
        pending.session_id.as_deref(),
        pending.trace_hash.clone(),
    );
    if !request.approved {
        state.observability.record_span(
            &trace,
            TraceSpanInput {
                stage: DashboardTraceStage::Policy,
                state: DashboardTraceState::Denied,
                engine: "approval",
                duration_ms: elapsed_ms(request_started),
                route: Some("approval_denied"),
                error_code: None,
                generation: None,
            },
        );
        state.observability.record_span(
            &trace,
            TraceSpanInput {
                stage: DashboardTraceStage::Request,
                state: DashboardTraceState::Denied,
                engine: "hzrd",
                duration_ms: elapsed_ms(request_started),
                route: Some("approval_continuation"),
                error_code: None,
                generation: None,
            },
        );
        return Ok(Json(ExecutionOutcome::NotStarted {
            disposition: NotStarted::Denied {
                requested: pending.requested,
                reason: "user denied the pending fork-core command".into(),
            },
        }));
    }
    let fidelity_context = pending
        .evasion
        .as_ref()
        .filter(|evasion| evasion.class == EvasionClass::E7FidelityHatch)
        .copied()
        .map(|evasion| {
            (
                pending.cwd.clone(),
                pending.agent.clone(),
                pending.session_id.clone(),
                evasion,
            )
        });
    let fidelity_reservation = if let Some((_, _, session_id, _)) = &fidelity_context {
        let session_id = session_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ApiError::bad_request(
                    "approved fidelity execution requires a non-empty session identity",
                )
            })?;
        let allowance = FidelityAllowance::default();
        Some(
            state
                .ledger
                .reserve_fidelity_override(
                    session_id.to_owned(),
                    allowance,
                    allowance.max_delivered_tokens,
                )
                .await
                .map_err(|error| {
                    ApiError::service(
                        "fidelity_reservation_failed",
                        format!(
                            "approved command was not executed because its durable fidelity reservation failed: {error}; rerun the original command to request a new approval"
                        ),
                        false,
                    )
                })?,
        )
    } else {
        None
    };
    let mut envelope = ExecutionEnvelope::allow_raw(pending.requested);
    envelope.decision = pending.approved_decision;
    envelope.cwd = Some(pending.cwd);
    envelope.timeout_ms = pending.timeout_ms;
    RtkRewriteOutcome {
        decision: envelope.decision.clone(),
        evasion: pending.evasion,
    }
    .apply_evasion_environment(&mut envelope.environment)
    .map_err(|error| ApiError::internal(format!("evasion plan encoding failed: {error}")))?;
    apply_caller_path(&mut envelope, pending.caller_path);
    if let (Some(reservation), Some((cwd, agent, session_id, evasion))) =
        (fidelity_reservation.as_ref(), fidelity_context.as_ref())
    {
        if let Err(error) = state
            .ledger
            .begin_fidelity(
                reservation,
                fidelity_pending_record_for_context(
                    cwd,
                    agent.clone(),
                    session_id.clone(),
                    *evasion,
                    false,
                ),
            )
            .await
        {
            let cancellation = state.ledger.cancel_fidelity(reservation.clone()).await;
            return Err(ApiError::service(
                "fidelity_execution_boundary_failed",
                format!(
                    "approved command was not executed because the durable execution boundary failed: {error}; cancellation={cancellation:?}; rerun the original command to request a new approval"
                ),
                false,
            ));
        }
    }
    let engine_started = Instant::now();
    let (outcome, process_started) = match state.executor.start(envelope) {
        Ok(handle) => (handle.wait().await, true),
        Err(error) => {
            if let Some(reservation) = fidelity_reservation.as_ref() {
                if let Err(recovery) = state
                    .ledger
                    .recover_fidelity_pre_spawn(reservation.clone())
                    .await
                {
                    return Err(ApiError::service(
                        "fidelity_pre_spawn_recovery_failed",
                        format!(
                            "approved process was not spawned ({error}), but its durable reservation could not be released: {recovery}; do not replay until `hzr doctor` is clean"
                        ),
                        false,
                    ));
                }
            }
            (Err(error), false)
        }
    };
    let fidelity_execution_unknown =
        fidelity_reservation.is_some() && process_started && outcome.is_err();
    let mut fidelity_accounting_error = None;
    if let Some(reservation) = fidelity_reservation {
        let completion = match (outcome.as_ref(), fidelity_context) {
            (Ok(execution), Some((cwd, agent, session_id, evasion))) => {
                fidelity_operation_record_for_context(
                    &cwd, agent, session_id, evasion, execution, false,
                )
            }
            _ => None,
        };
        if let Some(record) = completion {
            if let Err(error) = state.ledger.complete_fidelity(reservation, record).await {
                fidelity_accounting_error = Some(error);
            }
        } else if outcome.is_ok() {
            fidelity_accounting_error = Some(
                crate::ledger_writer::LedgerWriterError::execution_unknown(
                    "approved executor returned no recordable completion after the execution boundary; durable state retained",
                ),
            );
        }
    }
    let engine_state = if outcome.is_ok() {
        DashboardTraceState::Completed
    } else {
        DashboardTraceState::Failed
    };
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Engine,
            state: engine_state,
            engine: "rtk",
            duration_ms: elapsed_ms(engine_started),
            route: Some("approved_execution"),
            error_code: outcome.as_ref().err().map(|_| "execution_failed"),
            generation: None,
        },
    );
    if fidelity_accounting_error.is_some() {
        state.observability.record_span(
            &trace,
            TraceSpanInput {
                stage: DashboardTraceStage::Ledger,
                state: DashboardTraceState::Failed,
                engine: "ledger",
                duration_ms: 0,
                route: Some("approved_fidelity_completion"),
                error_code: Some("fidelity_accounting_failed"),
                generation: None,
            },
        );
    }
    let request_state = if outcome.is_ok() && fidelity_accounting_error.is_none() {
        DashboardTraceState::Completed
    } else {
        DashboardTraceState::Failed
    };
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Request,
            state: request_state,
            engine: "hzrd",
            duration_ms: elapsed_ms(request_started),
            route: Some("approval_continuation"),
            error_code: fidelity_accounting_error
                .as_ref()
                .map(|_| "fidelity_accounting_failed")
                .or_else(|| outcome.as_ref().err().map(|_| "execution_failed")),
            generation: None,
        },
    );
    let outcome = outcome.map_err(|error| {
        if fidelity_execution_unknown {
            ApiError::service(
                "fidelity_execution_state_unknown",
                format!(
                    "approved execution may have started before the executor failed: {error}; do not replay; inspect `hzr doctor`"
                ),
                false,
            )
        } else {
            ApiError::service("execution_failed", error.to_string(), true)
        }
    })?;
    Ok(Json(mark_accounting_incomplete(
        outcome,
        fidelity_accounting_error.as_ref(),
    )))
}

pub async fn fidelity_reconcile(
    State(state): State<AppState>,
    Json(request): Json<FidelityReconcileApiRequest>,
) -> Result<Json<FidelityReconcileReceipt>, ApiError> {
    let reservation_id = request.reservation_id.trim();
    if reservation_id.is_empty()
        || reservation_id.len() > 128
        || !reservation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ApiError::bad_request("invalid fidelity reservation id"));
    }
    state
        .ledger
        .reconcile_fidelity(reservation_id.to_owned(), request.resolution)
        .await
        .map(Json)
        .map_err(|error| ApiError::service("fidelity_reconcile_failed", error.to_string(), false))
}

fn mark_accounting_incomplete(
    outcome: ExecutionOutcome,
    error: Option<&crate::ledger_writer::LedgerWriterError>,
) -> ExecutionOutcome {
    let Some(error) = error else {
        return outcome;
    };
    match outcome {
        ExecutionOutcome::Completed { result } => ExecutionOutcome::ExecutedAccountingIncomplete {
            result,
            accounting: AccountingIncomplete {
                code: "fidelity_accounting_incomplete".into(),
                retryable: false,
                incident_persisted: error.incident_persisted(),
            },
        },
        other => other,
    }
}

pub async fn fork_run(
    State(state): State<AppState>,
    Json(request): Json<ForkRunApiRequest>,
) -> Result<Json<ForkRunApiResponse>, ApiError> {
    validate_fork_run(&request)?;
    let budget = ManagedExecutionBudget::new(&state, request.timeout_ms)?;
    let cwd = canonical_workspace(&request.cwd)?;
    let (args, stdin, _managed_write_files): (
        Vec<String>,
        Option<String>,
        Option<tempfile::TempDir>,
    ) = match request.managed_write {
        Some(write) => materialize_managed_write(write)?,
        None => (request.args, request.stdin, None),
    };
    validate_managed_fork_tool(&args, &cwd)?;
    let runner = state
        .rtk
        .runner()
        .map_err(|error| ApiError::service("fork_core_unavailable", error.to_string(), true))?;
    let mut invocation = ForkCoreInvocation::new(args);
    invocation.cwd = Some(cwd);
    invocation.timeout_ms = Some(budget.remaining_ms()?);
    invocation.stdin = stdin.map_or(StdinSpec::Null, |stdin| StdinSpec::Bytes {
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
        ExecutionOutcome::ExecutedAccountingIncomplete { accounting, .. } => {
            return Err(ApiError::internal(format!(
                "fork-core executed but accounting is incomplete; do not retry: {accounting:?}"
            )));
        }
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

/// Fork argv, optional stdin payload, and the staging directory that must outlive the spawn.
type ManagedWriteInvocation = (Vec<String>, Option<String>, Option<tempfile::TempDir>);

fn materialize_managed_write(write: ForkManagedWrite) -> Result<ManagedWriteInvocation, ApiError> {
    match write {
        ForkManagedWrite::Patch { path, old, new } => {
            if old.len() > 65_536 || new.len() > 65_536 {
                return Err(ApiError::bad_request(
                    "managed patch blocks must contain at most 65536 UTF-8 bytes",
                ));
            }
            let directory = tempfile::tempdir()
                .map_err(|error| ApiError::internal(format!("stage managed patch: {error}")))?;
            let old_path = directory.path().join("old");
            let new_path = directory.path().join("new");
            fs::write(&old_path, old)
                .and_then(|_| fs::write(&new_path, new))
                .map_err(|error| ApiError::internal(format!("stage managed patch: {error}")))?;
            Ok((
                vec![
                    "write".to_owned(),
                    "--output".to_owned(),
                    "json".to_owned(),
                    "patch".to_owned(),
                    path,
                    "--old".to_owned(),
                    format!("@{}", old_path.display()),
                    "--new".to_owned(),
                    format!("@{}", new_path.display()),
                    "--cas".to_owned(),
                    "--retry".to_owned(),
                    "2".to_owned(),
                ],
                None,
                Some(directory),
            ))
        }
        ForkManagedWrite::Create { path, content } => {
            if content.len() > 192 * 1024 {
                return Err(ApiError::bad_request(
                    "managed create content must contain at most 196608 UTF-8 bytes",
                ));
            }
            Ok((
                vec![
                    "write".to_owned(),
                    "--output".to_owned(),
                    "json".to_owned(),
                    "create".to_owned(),
                    path,
                    "--content".to_owned(),
                    "@-".to_owned(),
                ],
                Some(content),
                None,
            ))
        }
    }
}

pub async fn usage(
    State(state): State<AppState>,
    Json(request): Json<UsageApiRequest>,
) -> Result<Json<UsageApiResponse>, ApiError> {
    validate_usage(&request)?;
    let project_path = normalize_usage_project_path(request.project_path.as_deref());
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
        project_path,
    };
    state
        .ledger
        .record(record)
        .await
        .map_err(|error| ApiError::internal(format!("usage ledger write failed: {error}")))?;
    Ok(Json(UsageApiResponse { recorded: true }))
}

pub async fn provider_receipt(
    State(state): State<AppState>,
    Json(mut receipt): Json<ProviderEconomicReceipt>,
) -> Result<Json<ProviderReceiptRecordResult>, ApiError> {
    validate_receipt(&receipt).map_err(|error| ApiError::bad_request(error.to_string()))?;
    receipt.project_path = receipt_workspace(&state, &receipt.project_path)?;
    receipt.source = "user_supplied".into();
    let (catalog, pricing_unavailable_reason) = if receipt.enable_public_estimate {
        match load_pricing_catalog(state.config.billing.pricing_file.as_deref()) {
            Ok(catalog) => (Some(catalog), None),
            Err(error) => (None, Some(error.to_string())),
        }
    } else {
        (None, None)
    };
    let result = state
        .ledger
        .record_provider_receipt(receipt, catalog, pricing_unavailable_reason)
        .await
        .map_err(|error| match &error {
            crate::ledger_writer::LedgerWriterError::Ledger(hzr_core::LedgerError::Billing(
                hzr_core::BillingError::InvalidReceipt(_),
            )) => ApiError::bad_request(error.to_string()),
            _ => ApiError::internal(format!("provider receipt write failed: {error}")),
        })?;
    Ok(Json(result))
}

fn receipt_workspace(state: &AppState, requested: &str) -> Result<String, ApiError> {
    let requested = canonical_workspace(requested)?;
    let registry = registered_workspaces(&state.config.data_dir);
    let registration = registry
        .registrations
        .iter()
        .filter(|registration| requested.starts_with(&registration.root))
        .max_by_key(|registration| registration.root.components().count())
        .ok_or_else(|| {
            ApiError::bad_request(
                "provider receipt workspace must be inside a registered HZR workspace",
            )
        })?;
    registration
        .root
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| ApiError::bad_request("registered workspace path is not valid UTF-8"))
}

pub async fn operation(
    State(state): State<AppState>,
    Json(request): Json<hzr_protocol::OperationApiRequest>,
) -> Result<Json<hzr_protocol::OperationApiResponse>, ApiError> {
    let request_started = Instant::now();
    let trace = state
        .observability
        .begin_trace(&request.project_path, request.session_id.as_deref());
    let channel = match request.channel {
        hzr_protocol::AccountingChannel::HookCli => hzr_core::OperationChannel::HookCli,
        hzr_protocol::AccountingChannel::Mcp => hzr_core::OperationChannel::Mcp,
        hzr_protocol::AccountingChannel::NativeHost => hzr_core::OperationChannel::NativeHost,
    };
    let measurement = match request.measurement {
        hzr_protocol::AccountingMeasurement::Estimated => hzr_core::OperationMeasurement::Estimated,
        hzr_protocol::AccountingMeasurement::Unmeasured => {
            hzr_core::OperationMeasurement::Unmeasured
        }
    };
    let route = match request.route {
        hzr_protocol::AccountingRoute::Optimized => hzr_core::OperationRoute::Optimized,
        hzr_protocol::AccountingRoute::Bypassed => hzr_core::OperationRoute::Bypassed,
        hzr_protocol::AccountingRoute::NativeUnaccounted => {
            hzr_core::OperationRoute::NativeUnaccounted
        }
    };
    let ledger_started = Instant::now();
    let outcome = state
        .ledger
        .record_operation(crate::ledger_writer::OperationRecord {
            original_command: request.original_command,
            recorded_command: request.recorded_command,
            input_tokens: request.baseline_tokens_estimated,
            output_tokens: request.delivered_tokens_estimated,
            execution_ms: request.execution_ms,
            project_path: request.project_path,
            channel,
            measurement,
            route,
            agent: request.agent,
            session_id: request.session_id,
            attribution: request.attribution,
            evasion: None,
            host_grant_applied: false,
        })
        .await;
    let trace_state = if outcome.is_ok() {
        DashboardTraceState::Completed
    } else {
        DashboardTraceState::Failed
    };
    let route_name = match route {
        hzr_core::OperationRoute::Optimized => "optimized_operation",
        hzr_core::OperationRoute::Bypassed => "bypassed_operation",
        hzr_core::OperationRoute::NativeUnaccounted => "native_operation",
    };
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Ledger,
            state: trace_state,
            engine: "ledger",
            duration_ms: elapsed_ms(ledger_started),
            route: Some(route_name),
            error_code: outcome
                .as_ref()
                .err()
                .map(|_| "operation_ledger_write_failed"),
            generation: None,
        },
    );
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Request,
            state: trace_state,
            engine: "hzrd",
            duration_ms: elapsed_ms(request_started),
            route: Some("operation_accounting"),
            error_code: outcome
                .as_ref()
                .err()
                .map(|_| "operation_ledger_write_failed"),
            generation: None,
        },
    );
    outcome
        .map_err(|error| ApiError::internal(format!("operation ledger write failed: {error}")))?;
    Ok(Json(hzr_protocol::OperationApiResponse { recorded: true }))
}

pub async fn exec_rewrite(
    State(state): State<AppState>,
    Json(request): Json<ExecApiRequest>,
) -> Result<Json<RtkRewriteOutcome>, ApiError> {
    if request.command.trim().is_empty() {
        return Err(ApiError::bad_request("command must not be empty"));
    }
    validate_exec_timeout(request.timeout_ms)?;
    let cwd = canonical_workspace(&request.cwd)?;
    let preflight = daemon_fidelity_preflight(&state.ledger, &request, &cwd).await;
    let mut outcome = match &preflight {
        FidelityPreflight::Ask { evasion, reason } => RtkRewriteOutcome {
            decision: RewriteDecision::Ask {
                proposed: Some(CanonicalCommand::shell(request.command.clone())),
                reason: reason.clone(),
            },
            evasion: Some(*evasion),
        },
        FidelityPreflight::NotRequested | FidelityPreflight::Allow { .. } => {
            fork_outcome_with_managed_unwrap(&state.rtk, &request.command, &cwd).await
        }
    };
    if let FidelityPreflight::Allow { evasion, .. } = preflight {
        outcome.evasion = Some(evasion);
    }
    let decision = apply_host_grant(
        &request,
        enforce_first_class(&request.command, outcome.decision),
    );
    record_exec_policy_event(&state, &request, &cwd, outcome.evasion.as_ref(), &decision).await?;
    // The attribution travels with the decision: the hook forwards it to the process that will
    // execute and record the command, which has no other way to learn how it was classified.
    Ok(Json(RtkRewriteOutcome {
        decision,
        evasion: outcome.evasion,
    }))
}

async fn daemon_fidelity_preflight(
    ledger: &crate::ledger_writer::LedgerWriter,
    request: &ExecApiRequest,
    cwd: &Path,
) -> FidelityPreflight {
    if !request.fidelity_requested {
        return FidelityPreflight::NotRequested;
    }
    // The public route transports fidelity intent as typed request fields. Build the legacy
    // classifier shape only in-memory for the compatibility parser; never execute this wrapper.
    let classifier_command = match request.fidelity_reason.as_deref() {
        Some(reason) => format!(
            "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON={reason} hzr rtk -- raw {}",
            request.command
        ),
        None => format!("HZR_RAW_FIDELITY=1 hzr rtk -- raw {}", request.command),
    };
    let budget = match request
        .session_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(session_id) => ledger
            .fidelity_session_usage(session_id.to_owned(), FidelityAllowance::default())
            .await
            .ok()
            .map(|usage| FidelityBudget {
                remaining_operations: usage.remaining_operations,
                remaining_tokens: usage.remaining_tokens,
                exhausted: usage.exhausted,
            }),
        None => None,
    };
    hzr_core::fidelity_preflight(&classifier_command, cwd, budget)
}

async fn daemon_fidelity_preflight_for_run(
    ledger: &crate::ledger_writer::LedgerWriter,
    request: &ExecApiRequest,
    cwd: &Path,
) -> (
    FidelityPreflight,
    Option<crate::ledger_writer::FidelityReservation>,
) {
    let preflight = daemon_fidelity_preflight(ledger, request, cwd).await;
    let FidelityPreflight::Allow {
        evasion,
        output_tokens_upper_bound,
    } = preflight
    else {
        return (preflight, None);
    };
    let Some(session_id) = request.session_id.as_ref() else {
        return (
            FidelityPreflight::Ask {
                evasion,
                reason: "T4 fidelity preflight cannot reserve an anonymous session allowance"
                    .into(),
            },
            None,
        );
    };
    match ledger
        .reserve_fidelity(
            session_id.clone(),
            FidelityAllowance::default(),
            output_tokens_upper_bound,
        )
        .await
    {
        Ok(Some(reservation)) => (
            FidelityPreflight::Allow {
                evasion,
                output_tokens_upper_bound,
            },
            Some(reservation),
        ),
        Ok(None) => {
            let mut evasion = evasion;
            evasion.avoidable = true;
            evasion.tier = EnforcementTier::T3BudgetExhaustion;
            evasion.fidelity_validation = FidelityValidation::BudgetExhausted;
            (
                FidelityPreflight::Ask {
                    evasion,
                    reason: "T4 fidelity session allowance is exhausted or concurrently reserved"
                        .into(),
                },
                None,
            )
        }
        Err(error) => (
            FidelityPreflight::Ask {
                evasion,
                reason: format!(
                    "T4 fidelity allowance reservation is unavailable; execution remains blocked: {error}"
                ),
            },
            None,
        ),
    }
}

async fn record_exec_policy_event(
    state: &AppState,
    request: &ExecApiRequest,
    cwd: &Path,
    evasion: Option<&EvasionAttribution>,
    decision: &RewriteDecision,
) -> Result<(), ApiError> {
    let Some(evasion) = evasion.copied() else {
        return Ok(());
    };
    let decision = match decision {
        RewriteDecision::Ask { .. } => PolicyDecision::Ask,
        RewriteDecision::Deny { .. } => PolicyDecision::Deny,
        RewriteDecision::AllowRewrite { .. } => PolicyDecision::Correction,
        RewriteDecision::AllowRaw { .. } => return Ok(()),
    };
    state
        .ledger
        .record_policy_event(crate::ledger_writer::PolicyEventRecord {
            project_path: cwd.to_string_lossy().into_owned(),
            agent: request.agent.clone(),
            session_id: request.session_id.clone(),
            evasion,
            decision,
            replacement_family: None,
            command_identity: Some(request.command.clone()),
        })
        .await
        .map_err(|error| ApiError::internal(format!("policy event ledger write failed: {error}")))
}

async fn fork_outcome_with_managed_unwrap(
    rtk: &PinnedRtkAdapter,
    raw: &str,
    cwd: &Path,
) -> RtkRewriteOutcome {
    let fidelity = raw_fidelity_request(raw);
    let mut policy_evasion = raw_policy_evasion(raw, fidelity);
    let command = match fidelity {
        RawFidelityRequest::NotRequested => hzr_core::managed_raw_payload(raw).unwrap_or(raw),
        RawFidelityRequest::MissingReason => {
            return RtkRewriteOutcome {
                decision: RewriteDecision::Ask {
                    proposed: None,
                    reason: "HZR_RAW_FIDELITY=1 requires a closed HZR_RAW_FIDELITY_REASON".into(),
                },
                evasion: policy_evasion,
            };
        }
        RawFidelityRequest::InvalidReason => {
            return RtkRewriteOutcome {
                decision: RewriteDecision::Ask {
                    proposed: None,
                    reason: "HZR_RAW_FIDELITY_REASON is not an allowed fidelity reason".into(),
                },
                evasion: policy_evasion,
            };
        }
        RawFidelityRequest::Authorized { payload, .. } => {
            if let Some(replacement) = first_class_replacement(raw) {
                if let Some(evasion) = policy_evasion.as_mut() {
                    evasion.avoidable = true;
                    evasion.fidelity_validation = FidelityValidation::ProvenEquivalent;
                }
                return RtkRewriteOutcome {
                    decision: hzr_policy_rewrite(replacement),
                    evasion: policy_evasion,
                };
            }
            payload
        }
    };
    let authorized = matches!(fidelity, RawFidelityRequest::Authorized { .. });
    let canonical = CanonicalCommand::shell(command);
    let mut outcome = if authorized {
        rtk.decide_byte_fidelity_with_plan_in(&canonical, Some(cwd))
            .await
    } else {
        rtk.decide_with_plan_in(&canonical, Some(cwd)).await
    };
    if authorized
        && matches!(
            &outcome.decision,
            RewriteDecision::AllowRewrite {
                source: RewriteSource::Rtk {
                    route: hzr_exec::RtkRewriteRoute::Proxy,
                    ..
                },
                ..
            }
        )
    {
        outcome.decision = RewriteDecision::allow_raw(
            "authorized raw fidelity request has no byte-faithful managed equivalent",
        );
    }
    if policy_evasion.is_some() {
        outcome.evasion = policy_evasion;
    }
    outcome
}

fn raw_policy_evasion(raw: &str, fidelity: RawFidelityRequest<'_>) -> Option<EvasionAttribution> {
    let (reason, validation, hatch_marker, avoidable, tier) = match fidelity {
        RawFidelityRequest::Authorized { reason, .. } => (
            Some(protocol_fidelity_reason(reason)),
            FidelityValidation::Valid,
            true,
            false,
            EnforcementTier::T4HatchQuarantine,
        ),
        RawFidelityRequest::MissingReason => (
            None,
            FidelityValidation::MissingReason,
            true,
            true,
            EnforcementTier::T4HatchQuarantine,
        ),
        RawFidelityRequest::InvalidReason => (
            None,
            FidelityValidation::InvalidReason,
            true,
            true,
            EnforcementTier::T4HatchQuarantine,
        ),
        RawFidelityRequest::NotRequested if hzr_core::managed_raw_payload(raw).is_some() => (
            None,
            FidelityValidation::NotRequested,
            false,
            true,
            EnforcementTier::T1NamedCorrection,
        ),
        RawFidelityRequest::NotRequested => return None,
    };
    Some(EvasionAttribution {
        class: EvasionClass::E7FidelityHatch,
        wrapper_depth: 1,
        interpreter: None,
        path_form: EvasionPathForm::Bare,
        stage_count: 1,
        hatch_marker,
        avoidable,
        tier,
        fidelity_reason: reason,
        fidelity_validation: validation,
    })
}

fn protocol_fidelity_reason(reason: RawFidelityReason) -> FidelityReason {
    match reason {
        RawFidelityReason::Binary => FidelityReason::Binary,
        RawFidelityReason::Checksum => FidelityReason::Checksum,
        RawFidelityReason::MachineProtocol => FidelityReason::MachineProtocol,
        RawFidelityReason::CompleteLog => FidelityReason::CompleteLog,
        RawFidelityReason::FullPatch => FidelityReason::FullPatch,
        RawFidelityReason::VerbatimSource => FidelityReason::VerbatimSource,
    }
}

/// Apply the caller's host execution grant to a verdict the daemon has just derived.
///
/// The daemon is the last place the desync could survive: `hzr exec run` forwards the host's
/// answer, and if the daemon ignored it the CLI would still refuse what the hook approved. The
/// grant is re-validated here against the request's own session rather than trusted as sent —
/// a caller may not assert an approval for a session it is not running in.
fn apply_host_grant(request: &ExecApiRequest, decision: RewriteDecision) -> RewriteDecision {
    let granted = request.host_execution_grant.as_ref().is_some_and(|grant| {
        let digest = request
            .session_id
            .as_deref()
            .map(str::trim)
            .filter(|session| !session.is_empty())
            .map(|session| hzr_core::privacy_identity_hash("session", session));
        grant
            .authorize(digest.as_deref(), unix_millis_now())
            .is_ok()
    });
    reconcile_host_grant(decision, granted)
}

fn unix_millis_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn enforce_first_class(raw: &str, decision: RewriteDecision) -> RewriteDecision {
    if matches!(
        decision,
        RewriteDecision::AllowRaw { .. } | RewriteDecision::AllowRewrite { .. }
    ) {
        if let Some(replacement) = efficient_route_replacement(raw) {
            return hzr_policy_rewrite(replacement);
        }
    }
    if !matches!(
        decision,
        RewriteDecision::AllowRaw { .. }
            | RewriteDecision::AllowRewrite {
                source: RewriteSource::Rtk {
                    route: hzr_exec::RtkRewriteRoute::Proxy,
                    ..
                },
                ..
            }
    ) {
        return decision;
    }
    let Some(replacement) = first_class_replacement(raw) else {
        return decision;
    };
    hzr_policy_rewrite(replacement)
}

fn hzr_policy_rewrite(replacement: hzr_core::RawReplacement) -> RewriteDecision {
    RewriteDecision::AllowRewrite {
        command: CanonicalCommand::shell(replacement.suggestion.clone()),
        source: RewriteSource::HzrPolicy,
        reason: format!(
            "`{}` selected a higher-output route. {}. HZR automatically selected the \
             lower-output first-class route.",
            replacement.tool, replacement.rationale
        ),
    }
}

pub async fn codec_compile(
    State(state): State<AppState>,
    Json(request): Json<CodecApiRequest>,
) -> Result<Json<hzr_codec::Transform>, ApiError> {
    let started = Instant::now();
    let trace = state.observability.begin_trace("", None);
    let transform = hzr_codec::transform_for_risk(
        &request.content,
        request.fidelity,
        request.profile,
        request.risk,
    )
    .map_err(|error| ApiError::bad_request(error.to_string()))?;
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Engine,
            state: DashboardTraceState::Completed,
            engine: "caveman",
            duration_ms: elapsed_ms(started),
            route: Some("codec_compile"),
            error_code: None,
            generation: None,
        },
    );
    let ledger_started = Instant::now();
    let recorded = record_codec_operation(&state, &request, &transform, started.elapsed()).await;
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Ledger,
            state: if recorded {
                DashboardTraceState::Completed
            } else {
                DashboardTraceState::Failed
            },
            engine: "ledger",
            duration_ms: elapsed_ms(ledger_started),
            route: Some("codec_compile"),
            error_code: (!recorded).then_some("codec_accounting_failed"),
            generation: None,
        },
    );
    state.observability.record_span(
        &trace,
        TraceSpanInput {
            stage: DashboardTraceStage::Request,
            state: DashboardTraceState::Completed,
            engine: "hzrd",
            duration_ms: elapsed_ms(started),
            route: Some("codec_compile"),
            error_code: None,
            generation: None,
        },
    );
    Ok(Json(transform))
}

/// Credit the codec in the same ledger the pinned engine writes to.
///
/// Without this the codec is unmeasurable: it saves output tokens that nothing counts, so
/// the `codec` subsystem never appears in `hzr stats` and the capability is indistinguishable
/// from one that is never called. A failed write is not surfaced to the caller — the
/// transform itself succeeded, and losing a measurement must never fail a working request.
async fn record_codec_operation(
    state: &AppState,
    request: &CodecApiRequest,
    transform: &hzr_codec::Transform,
    elapsed: Duration,
) -> bool {
    // The returned tool/CLI payload is observable delivery and may earn estimated codec-token
    // credit. That does not prove the host replaced a later assistant response, and it never
    // becomes provider-billed credit without a matching provider receipt.
    let delivered = match &transform.counterfactual {
        Some(_) => request.content.len(),
        None => transform.content.len(),
    };
    state
        .ledger
        .record_operation(crate::ledger_writer::OperationRecord {
            original_command: "hzr codec compile".into(),
            recorded_command: format!("hzr codec {}", codec_profile_name(request.profile)),
            input_tokens: estimated_tokens(request.content.len()),
            output_tokens: estimated_tokens(delivered),
            execution_ms: elapsed.as_millis() as u64,
            project_path: request.project_path.clone(),
            channel: match request.channel {
                Some(hzr_protocol::AccountingChannel::Mcp) => hzr_core::OperationChannel::Mcp,
                Some(hzr_protocol::AccountingChannel::NativeHost) => {
                    hzr_core::OperationChannel::NativeHost
                }
                Some(hzr_protocol::AccountingChannel::HookCli) | None => {
                    hzr_core::OperationChannel::HookCli
                }
            },
            measurement: hzr_core::OperationMeasurement::Estimated,
            route: hzr_core::OperationRoute::Optimized,
            agent: matches!(request.channel, Some(hzr_protocol::AccountingChannel::Mcp))
                .then(|| "mcp".to_owned()),
            session_id: None,
            attribution: Some(hzr_protocol::AccountingAttribution {
                operation: hzr_protocol::AccountingOperationKind::Codec,
                mode: hzr_protocol::AccountingOperationMode::CodecCompile,
                stage: hzr_protocol::AccountingStage::InternalTransport,
                requested_mode: None,
                effective_mode: Some(hzr_protocol::AccountingOperationMode::CodecCompile),
                search_strategy: None,
                search_fallback_code: None,
                include_content: None,
                limit: None,
                path_scope_count: None,
                filter_level: None,
                from_line: None,
                to_line: None,
                source_bytes: Some(u64::try_from(request.content.len()).unwrap_or(u64::MAX)),
                evasion: None,
            }),
            evasion: None,
            host_grant_applied: false,
        })
        .await
        .is_ok()
}

/// The same `bytes / 4` heuristic the rest of the estimated ledger uses. Sharing it is the
/// point: a codec figure that used a different estimator could not be summed with the rest.
fn estimated_tokens(bytes: usize) -> u64 {
    (bytes / 4) as u64
}

fn codec_profile_name(profile: hzr_protocol::CodecProfile) -> &'static str {
    match profile {
        hzr_protocol::CodecProfile::Off => "off",
        hzr_protocol::CodecProfile::Safe => "safe",
        hzr_protocol::CodecProfile::Adaptive => "adaptive",
        hzr_protocol::CodecProfile::Compact => "compact",
        hzr_protocol::CodecProfile::Shadow => "shadow",
    }
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

fn validate_caller_path(caller_path: Option<&str>) -> Result<(), ApiError> {
    if caller_path.is_some_and(|path| path.len() > MAX_CALLER_PATH_BYTES || path.contains('\0')) {
        return Err(ApiError::bad_request(
            "caller PATH must not exceed 32768 bytes or contain NUL",
        ));
    }
    Ok(())
}

fn apply_caller_path(envelope: &mut ExecutionEnvelope, caller_path: Option<String>) {
    if let Some(path) = caller_path {
        envelope.environment.set.insert("PATH".into(), path);
    }
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
    if request.args.is_empty() && request.managed_write.is_none() {
        return Err(ApiError::bad_request("fork-core args must not be empty"));
    }
    if request.managed_write.is_some() && (!request.args.is_empty() || request.stdin.is_some()) {
        return Err(ApiError::bad_request(
            "typed managed write cannot be combined with raw fork args or stdin",
        ));
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
    if let Some(path) = request.project_path.as_deref() {
        let trimmed = path.trim();
        if trimmed.len() > 4096 || trimmed.contains('\0') {
            return Err(ApiError::bad_request("invalid usage project path"));
        }
    }
    Ok(())
}

/// Сохраняет канонический путь, если он существует; иначе — trimmed строку (не роняем чек).
fn normalize_usage_project_path(value: Option<&str>) -> String {
    let Some(raw) = value.map(str::trim).filter(|path| !path.is_empty()) else {
        return String::new();
    };
    std::fs::canonicalize(raw)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| raw.to_owned())
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
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    use super::anti_evasion_fixture::{ProbeDecision, ProbeLayer, ProbeSurface};
    use super::{
        ManagedExecutionBudget, apply_caller_path, apply_host_grant, approved_execution_decision,
        caveman_engine_health, daemon_fidelity_preflight, dashboard_index_state,
        dashboard_memory_detail, dashboard_overall_state, dashboard_registration,
        dashboard_search_activity, dashboard_session_roi, enforce_first_class,
        fidelity_operation_record_for_context, fork_outcome_with_managed_unwrap,
        materialize_managed_write, memory_mutation_targets, memory_ready_state,
        overall_engine_state, raw_policy_evasion, validate_caller_path, validate_managed_fork_tool,
    };
    use crate::ledger_writer::{LedgerWriter, PolicyEventRecord};
    use hzr_core::{
        Config, DetailedOperationAttribution, EconomicAmount, FidelityAllowance, FidelityPreflight,
        Ledger, OperationAttribution, OperationChannel, OperationMeasurement, OperationRoute,
        ProjectOperationRoute, ProjectOperationSummary, ReceiptProvenance, SessionEconomicSummary,
        SessionEfficiencySummary,
    };
    use hzr_exec::{
        CanonicalCommand, CapturedContent, ExecutionEnvelope, ExecutionOutcome, ExecutionPipeline,
        ForkRuntimePaths, NotStarted, PINNED_RTK_VERSION, PinnedRtkAdapter, RewriteDecision,
        RewriteSource, RtkAdapterConfig, RtkRewriteOutcome,
    };
    use hzr_index::{IndexWatcherState, WorkspaceRegistration};
    use hzr_memory::{Importance, MemoryRecord, MemoryScope, MemorySource, ProjectMemoryDetail};
    use hzr_protocol::{
        DashboardOperationRoute, DashboardProject, DashboardProjectArtifacts,
        DashboardProjectState, DashboardService, DashboardState, EnforcementTier, EngineHealth,
        EngineState, EvasionAttribution, EvasionClass, EvasionPathForm, ExecApiRequest,
        FidelityReason, FidelityValidation, HostExecutionGrant, HostPermissionMode,
        MemoryWriteScope, PolicyDecision,
    };

    const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn acceptance_gate_exec_run_honors_a_valid_grant_without_weakening_deny() {
        let session = "exec-run-granted-session";
        let request = ExecApiRequest {
            cwd: "/tmp".into(),
            command: "hzr stats --accounting-version all".into(),
            fidelity_requested: false,
            fidelity_reason: None,
            timeout_ms: None,
            caller_path: None,
            agent: Some("claude-code".into()),
            session_id: Some(session.into()),
            host_execution_grant: Some(HostExecutionGrant {
                mode: HostPermissionMode::BypassPermissions,
                granted_for_session: hzr_core::privacy_identity_hash("session", session),
                granted_at_ms: super::unix_millis_now(),
                source: "test".into(),
            }),
        };
        let allowed = apply_host_grant(
            &request,
            RewriteDecision::Ask {
                proposed: Some(CanonicalCommand::shell(
                    "hzr stats --accounting-version all",
                )),
                reason: "policy approval".into(),
            },
        );
        assert!(matches!(allowed, RewriteDecision::AllowRewrite { .. }));

        let denied = apply_host_grant(
            &request,
            RewriteDecision::Deny {
                reason: "explicit deny".into(),
            },
        );
        assert!(matches!(denied, RewriteDecision::Deny { .. }));
    }

    #[test]
    fn dashboard_session_roi_keeps_estimate_and_imported_claim_provenance_distinct() {
        let mut config = Config::default();
        config.billing.public_estimate_enabled = true;
        config.billing.harness = "openai_compatible".into();
        config.billing.provider = "alibaba_model_studio".into();
        config.billing.model = "qwen3.5-plus".into();
        config.billing.method = "global_standard_0_128k".into();
        config.billing.request_input_tokens = Some(100_000);
        config.billing.pricing_basis = "input".into();
        let session = (
            "hmac-sha256:session".into(),
            (
                SessionEfficiencySummary {
                    operations: 2,
                    baseline_tokens_estimated: 1_200,
                    delivered_tokens_estimated: 200,
                    gross_avoided_tokens_estimated: 1_000,
                    regression_tokens_estimated: 0,
                    net_avoided_tokens_estimated: 1_000,
                    total_observed_operations: 2,
                    stage_excluded_operations: 0,
                    excluded_legacy_operations: 0,
                    top_commands: Vec::new(),
                },
                SessionEconomicSummary {
                    paired_receipts: 1,
                    reported_actual: Some(EconomicAmount {
                        currency: "CNY".into(),
                        baseline_microunits: 900,
                        delivered_microunits: 100,
                        savings_microunits: 800,
                    }),
                    provenance: Some(ReceiptProvenance::UserSupplied),
                    externally_verified: false,
                    ..SessionEconomicSummary::default()
                },
            ),
        );

        let roi = dashboard_session_roi(&config, Some(&session));

        assert_eq!(roi.selected_provider, "alibaba_model_studio");
        assert_eq!(roi.selected_model, "qwen3.5-plus");
        assert_eq!(roi.receipt_provenance.as_deref(), Some("user_supplied"));
        assert!(!roi.receipt_externally_verified);
        assert_eq!(
            roi.raw_public_estimate
                .expect("preliminary estimate")
                .currency,
            "CNY"
        );
    }

    #[tokio::test]
    async fn acceptance_gate_daemon_fidelity_budget_contradiction_and_policy_event() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let ledger_path = directory.path().join("ledger.sqlite");
        let writer = LedgerWriter::open(&ledger_path).expect("ledger writer");
        let request = ExecApiRequest {
            cwd: directory.path().to_string_lossy().into_owned(),
            command: "sha256sum artifact.bin".into(),
            fidelity_requested: true,
            fidelity_reason: Some("checksum".into()),
            timeout_ms: None,
            caller_path: None,
            agent: Some("claude-code:agent-private".into()),
            session_id: Some("session-fidelity".into()),
            host_execution_grant: None,
        };

        assert!(matches!(
            daemon_fidelity_preflight(&writer, &request, directory.path()).await,
            FidelityPreflight::Allow { .. }
        ));

        let mut contradicted = request.clone();
        contradicted.command = "cat artifact.bin".into();
        assert!(matches!(
            daemon_fidelity_preflight(&writer, &contradicted, directory.path()).await,
            FidelityPreflight::Ask {
                evasion: EvasionAttribution {
                    fidelity_validation: FidelityValidation::Contradicted,
                    ..
                },
                ..
            }
        ));

        let evasion = EvasionAttribution {
            class: EvasionClass::E7FidelityHatch,
            wrapper_depth: 1,
            interpreter: None,
            path_form: EvasionPathForm::Bare,
            stage_count: 1,
            hatch_marker: true,
            avoidable: false,
            tier: EnforcementTier::T4HatchQuarantine,
            fidelity_reason: Some(FidelityReason::Checksum),
            fidelity_validation: FidelityValidation::Valid,
        };
        let ledger = Ledger::open(&ledger_path).expect("ledger");
        for _ in 0..FidelityAllowance::default().max_operations {
            ledger
                .record_operation_attributed_with_detail(
                    "checksum",
                    "checksum",
                    1,
                    1,
                    0,
                    DetailedOperationAttribution {
                        attribution: OperationAttribution {
                            project_path: directory.path().to_str().expect("utf8 path"),
                            agent: Some("claude-code:agent-private"),
                            session_id: Some("session-fidelity"),
                            channel: OperationChannel::HookCli,
                            measurement: OperationMeasurement::Estimated,
                            route: OperationRoute::Bypassed,
                        },
                        detail: None,
                        evasion: Some(&evasion),
                        host_grant_applied: false,
                    },
                )
                .expect("fidelity operation");
        }
        assert!(matches!(
            daemon_fidelity_preflight(&writer, &request, directory.path()).await,
            FidelityPreflight::Ask {
                evasion: EvasionAttribution {
                    fidelity_validation: FidelityValidation::BudgetExhausted,
                    ..
                },
                ..
            }
        ));

        let operations_before = ledger
            .efficiency_summary()
            .expect("efficiency before policy event")
            .operations;
        writer
            .record_policy_event(PolicyEventRecord {
                project_path: directory.path().to_string_lossy().into_owned(),
                agent: request.agent.clone(),
                session_id: request.session_id.clone(),
                evasion: EvasionAttribution {
                    fidelity_validation: FidelityValidation::BudgetExhausted,
                    avoidable: true,
                    ..evasion
                },
                decision: PolicyDecision::Ask,
                replacement_family: None,
                command_identity: Some(request.command.clone()),
            })
            .await
            .expect("policy event");
        assert_eq!(
            ledger
                .efficiency_summary()
                .expect("efficiency after policy event")
                .operations,
            operations_before,
            "Ask policy event must not create a second command row"
        );
        let score = ledger
            .session_evasion_summary("session-fidelity", FidelityAllowance::default())
            .expect("session score");
        assert_eq!(score.policy_asks, 1);
    }

    #[test]
    fn acceptance_gate_approved_checksum_preserves_exact_requested_command() {
        let raw = "sha256sum artifact.bin";
        let requested = CanonicalCommand::shell(raw);
        let evasion = EvasionAttribution {
            class: EvasionClass::E7FidelityHatch,
            wrapper_depth: 1,
            interpreter: None,
            path_form: EvasionPathForm::Bare,
            stage_count: 1,
            hatch_marker: true,
            avoidable: true,
            tier: EnforcementTier::T4HatchQuarantine,
            fidelity_reason: Some(FidelityReason::Checksum),
            fidelity_validation: FidelityValidation::BudgetExhausted,
        };
        let decision = approved_execution_decision(requested.clone(), Some(&evasion));
        assert!(matches!(decision, RewriteDecision::AllowRaw { .. }));

        let mut envelope = ExecutionEnvelope::allow_raw(requested.clone());
        envelope.decision = decision.clone();
        RtkRewriteOutcome {
            decision,
            evasion: Some(evasion),
        }
        .apply_evasion_environment(&mut envelope.environment)
        .expect("closed evasion environment");
        assert_eq!(envelope.command, requested);
        assert_eq!(
            envelope
                .environment
                .set
                .get("HZR_INTERNAL_EVASION_JSON")
                .and_then(|encoded| serde_json::from_str::<EvasionAttribution>(encoded).ok()),
            Some(evasion)
        );
    }

    #[test]
    fn acceptance_gate_no_raw_for_optimizable_exec_commands() {
        for command in [
            "hzr rtk -- raw hzr stats",
            "hzr rtk -- raw hzr search \"two words\" --mode exact",
        ] {
            let decision = enforce_first_class(command, RewriteDecision::allow_raw("fork raw"));
            assert!(
                matches!(
                    decision,
                    RewriteDecision::AllowRewrite {
                        command: CanonicalCommand::Shell { .. },
                        source: RewriteSource::HzrPolicy,
                        ..
                    }
                ),
                "{command} remained raw: {decision:?}"
            );
        }

        let proxy = RewriteDecision::AllowRewrite {
            command: CanonicalCommand::shell("rtk proxy nl -ba src/main.rs"),
            source: RewriteSource::Rtk {
                version: PINNED_RTK_VERSION.into(),
                route: hzr_exec::RtkRewriteRoute::Proxy,
            },
            reason: "fork selected tracked raw proxy".into(),
        };
        let decision = enforce_first_class("hzr rtk -- raw nl -ba src/main.rs", proxy.clone());
        assert_eq!(
            decision, proxy,
            "daemon overrode the canonical Proxy decision"
        );

        let specialized = RewriteDecision::AllowRewrite {
            command: CanonicalCommand::shell("rtk rg -n RewriteDecision crates/hzr-exec"),
            source: RewriteSource::Rtk {
                version: PINNED_RTK_VERSION.into(),
                route: hzr_exec::RtkRewriteRoute::Optimized,
            },
            reason: "fork-core approved and produced the managed command".into(),
        };
        assert_eq!(
            enforce_first_class(
                "hzr rtk -- raw rg -n RewriteDecision crates/hzr-exec",
                specialized.clone(),
            ),
            specialized,
            "a specialized fork-core filter must not be replaced by indexed search"
        );
    }

    #[test]
    fn acceptance_gate_no_raw_for_top_level_hzr_file_aliases_in_exec() {
        for command in [
            "hzr read \"docs/file with spaces.md\" --outline",
            "hzr write patch \"docs/file with spaces.md\" --old 'a b' --new 'c d'",
        ] {
            let decision = enforce_first_class(
                command,
                RewriteDecision::AllowRewrite {
                    command: CanonicalCommand::shell(format!("rtk proxy {command}")),
                    source: RewriteSource::Rtk {
                        version: PINNED_RTK_VERSION.into(),
                        route: hzr_exec::RtkRewriteRoute::Proxy,
                    },
                    reason: "fork selected tracked raw proxy".into(),
                },
            );
            assert!(matches!(
                decision,
                RewriteDecision::AllowRewrite {
                    command: CanonicalCommand::Shell {
                        command: ref rewritten,
                        ..
                    },
                    source: RewriteSource::HzrPolicy,
                    ..
                } if rewritten == command
            ));
        }
    }

    #[test]
    fn acceptance_gate_no_unbounded_exact_read_in_exec() {
        let filtered = RewriteDecision::AllowRewrite {
            command: CanonicalCommand::shell("rtk read src/main.rs --level none"),
            source: RewriteSource::Rtk {
                version: PINNED_RTK_VERSION.into(),
                route: hzr_exec::RtkRewriteRoute::Optimized,
            },
            reason: "fork-core accepted the explicit read".into(),
        };
        let decision =
            enforce_first_class("hzr rtk -- read src/main.rs --level none", filtered.clone());
        assert!(matches!(
            decision,
            RewriteDecision::AllowRewrite {
                command: CanonicalCommand::Shell { ref command, .. },
                source: RewriteSource::HzrPolicy,
                ..
            } if command == "hzr rtk -- read src/main.rs"
        ));

        for command in [
            "hzr rtk -- read src/main.rs --from 40 --to 80 --level none",
            "HZR_EXACT_FIDELITY=1 hzr rtk -- read src/main.rs --level none",
        ] {
            assert_eq!(
                enforce_first_class(command, filtered.clone()),
                filtered,
                "bounded or explicit exact read was changed: {command}"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn acceptance_gate_shared_fixture_reaches_daemon_policy() {
        let probes = super::anti_evasion_fixture::load_anti_evasion_probes();
        let root_shell = probes
            .iter()
            .filter(|probe| probe.layer == ProbeLayer::Root && probe.surface == ProbeSurface::Shell)
            .collect::<Vec<_>>();
        assert_eq!(root_shell.len(), 5, "all root shell probes must execute");

        for probe in root_shell {
            let directory = tempfile::tempdir().expect("temporary directory");
            let binary = directory.path().join("rtk");
            let plan = match probe.route.as_deref() {
                Some(route) if route.starts_with("rtk ") => {
                    serde_json::json!({"decision": "rewrite", "proposed": route})
                }
                _ => serde_json::json!({"decision": "proxy"}),
            };
            let plan_path = directory.path().join("rewrite-plan.json");
            fs::write(
                &plan_path,
                serde_json::to_vec(&plan).expect("rewrite plan JSON"),
            )
            .expect("rewrite plan fixture");
            let script = format!(
                r#"#!/bin/sh
if test "${{1:-}}" = --version; then
  printf 'rtk %s\n' '{PINNED_RTK_VERSION}'
  exit 0
fi
if test "${{1:-}}" = rewrite && test "${{2:-}}" = --help; then
  printf 'Usage: rtk rewrite [ARGS]... Raw command to rewrite\n'
  exit 0
fi
if test "${{1:-}}" = proxy && test "${{2:-}}" = --help; then
  printf 'Usage: rtk proxy [ARGS]... Execute command without filtering\n'
  exit 0
fi
if test "${{1:-}}" = rewrite-plan; then
  /bin/cat '{}'
  exit 0
fi
exit 64
"#,
                plan_path.display()
            );
            fs::write(&binary, script).expect("fake fork-core");
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))
                .expect("fake fork-core permissions");
            let adapter = PinnedRtkAdapter::detect(RtkAdapterConfig {
                binary,
                runtime_paths: Some(ForkRuntimePaths::from_data_root(
                    &directory.path().join("data"),
                )),
                ..RtkAdapterConfig::default()
            })
            .await;
            let command = probe.command.as_deref().expect("root shell command");
            let outcome =
                fork_outcome_with_managed_unwrap(&adapter, command, directory.path()).await;
            let decision = enforce_first_class(command, outcome.decision);
            match probe.decision {
                ProbeDecision::Rewrite => {
                    let RewriteDecision::AllowRewrite {
                        command: CanonicalCommand::Shell { command, .. },
                        ..
                    } = decision
                    else {
                        assert!(
                            matches!(
                                &decision,
                                RewriteDecision::AllowRewrite {
                                    command: CanonicalCommand::Shell { .. },
                                    ..
                                }
                            ),
                            "root probe {} was not rewritten: {decision:?}",
                            probe.id
                        );
                        continue;
                    };
                    assert!(
                        command.ends_with(probe.route.as_deref().expect("rewrite route")),
                        "root probe {} selected {command}",
                        probe.id
                    );
                }
                ProbeDecision::Ask => assert!(
                    matches!(decision, RewriteDecision::Ask { .. }),
                    "root probe {} was not Ask: {decision:?}",
                    probe.id
                ),
                ProbeDecision::Raw => assert!(
                    matches!(decision, RewriteDecision::AllowRaw { .. }),
                    "root probe {} did not preserve fidelity: {decision:?}",
                    probe.id
                ),
                ProbeDecision::Proxy | ProbeDecision::Allow | ProbeDecision::Deny => {
                    assert!(
                        matches!(
                            probe.decision,
                            ProbeDecision::Rewrite | ProbeDecision::Ask | ProbeDecision::Raw
                        ),
                        "invalid root decision for {}",
                        probe.id
                    )
                }
            }
        }
    }

    #[test]
    fn first_class_gate_keeps_raw_when_no_safe_reconstructed_route_exists() {
        let decision = enforce_first_class(
            "hzr rtk -- raw sh -c 'printf complete-output'",
            RewriteDecision::allow_raw("fork raw"),
        );

        assert!(matches!(decision, RewriteDecision::AllowRaw { .. }));
    }

    #[test]
    fn first_class_gate_does_not_reconstruct_quoted_or_shell_grammar() {
        for command in [
            "hzr rtk -- raw nl -ba \"src/file with spaces.rs\"",
            "hzr rtk -- raw rg -n \"two words\" src",
            "hzr rtk -- raw rg -n needle src | head -n 20",
        ] {
            let decision = enforce_first_class(command, RewriteDecision::allow_raw("fork raw"));
            assert!(
                matches!(decision, RewriteDecision::AllowRaw { .. }),
                "{command} was reconstructed: {decision:?}"
            );
        }
    }

    #[test]
    fn first_class_gate_preserves_an_ambiguous_shell_ask() {
        let ask = RewriteDecision::Ask {
            proposed: None,
            reason: "fork-core could not safely decompose an opaque shell wrapper".into(),
        };

        assert_eq!(enforce_first_class("sh -c 'git status", ask.clone()), ask);
    }

    #[test]
    fn exec_caller_path_is_validated_and_applied_to_the_envelope() {
        assert!(validate_caller_path(Some("/toolchain/bin:/usr/bin")).is_ok());
        assert!(validate_caller_path(Some(&"x".repeat(32 * 1024 + 1))).is_err());
        assert!(validate_caller_path(Some("/bin\0/usr/bin")).is_err());

        let mut envelope = ExecutionEnvelope::allow_raw(CanonicalCommand::shell("cargo test"));
        apply_caller_path(&mut envelope, Some("/toolchain/bin:/usr/bin".to_owned()));
        assert_eq!(
            envelope.environment.set.get("PATH").map(String::as_str),
            Some("/toolchain/bin:/usr/bin")
        );
    }

    #[test]
    fn acceptance_gate_exec_plan_carries_only_closed_evasion_metadata() {
        let mut envelope = ExecutionEnvelope::allow_raw(CanonicalCommand::shell("cargo test"));
        let evasion = EvasionAttribution {
            class: EvasionClass::E2ShellWrapper,
            wrapper_depth: 1,
            interpreter: None,
            path_form: EvasionPathForm::Bare,
            stage_count: 1,
            hatch_marker: false,
            avoidable: true,
            tier: EnforcementTier::T1NamedCorrection,
            fidelity_reason: None,
            fidelity_validation: FidelityValidation::NotRequested,
        };
        RtkRewriteOutcome {
            decision: envelope.decision.clone(),
            evasion: Some(evasion),
        }
        .apply_evasion_environment(&mut envelope.environment)
        .expect("closed evasion environment");
        let encoded = envelope
            .environment
            .set
            .get("HZR_INTERNAL_EVASION_JSON")
            .expect("evasion environment");
        assert_eq!(
            serde_json::from_str::<EvasionAttribution>(encoded).expect("closed attribution"),
            evasion
        );
        assert!(!encoded.contains("cargo test"));
    }

    #[test]
    fn acceptance_gate_raw_fidelity_is_typed_before_wrapper_removal() {
        let cases = [
            (
                "hzr rtk -- raw cat file.txt",
                FidelityValidation::NotRequested,
                false,
                true,
                None,
            ),
            (
                "HZR_RAW_FIDELITY=1 hzr rtk -- raw cat file.txt",
                FidelityValidation::MissingReason,
                true,
                true,
                None,
            ),
            (
                "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=private-value hzr rtk -- raw cat file.txt",
                FidelityValidation::InvalidReason,
                true,
                true,
                None,
            ),
            (
                "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=checksum hzr rtk -- raw sha256sum file.txt",
                FidelityValidation::Valid,
                true,
                false,
                Some(FidelityReason::Checksum),
            ),
        ];
        for (command, validation, hatch_marker, avoidable, reason) in cases {
            let attribution = raw_policy_evasion(command, hzr_core::raw_fidelity_request(command))
                .expect("raw policy attribution");
            assert_eq!(attribution.class, EvasionClass::E7FidelityHatch);
            assert_eq!(attribution.fidelity_validation, validation);
            assert_eq!(attribution.hatch_marker, hatch_marker);
            assert_eq!(attribution.avoidable, avoidable);
            assert_eq!(attribution.fidelity_reason, reason);
            assert!(
                !serde_json::to_string(&attribution)
                    .expect("closed attribution")
                    .contains("private-value")
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_caller_path_resolves_a_caller_only_executable() -> Result<(), std::io::Error> {
        let directory = tempfile::tempdir().expect("temporary directory");
        let binary = directory.path().join("caller-only");
        fs::write(&binary, "#!/bin/sh\nprintf caller-path\n").expect("caller executable");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))
            .expect("caller executable permissions");

        let mut envelope = ExecutionEnvelope::allow_raw(CanonicalCommand::shell("caller-only"));
        apply_caller_path(
            &mut envelope,
            Some(directory.path().to_string_lossy().into_owned()),
        );
        let outcome = ExecutionPipeline
            .execute(envelope)
            .await
            .expect("managed execution");
        let result = match outcome {
            ExecutionOutcome::Completed { result } => result,
            ExecutionOutcome::ExecutedAccountingIncomplete { accounting, .. } => {
                return Err(std::io::Error::other(format!(
                    "caller executable accounting is incomplete: {accounting:?}"
                )));
            }
            ExecutionOutcome::NotStarted { disposition } => {
                return Err(std::io::Error::other(format!(
                    "caller executable did not start: {disposition:?}"
                )));
            }
        };
        let bytes = match result.stdout.content {
            CapturedContent::Inline { bytes } => bytes,
            CapturedContent::Spilled { path } => fs::read(path)?,
        };
        assert_eq!(bytes, b"caller-path");
        Ok(())
    }

    #[tokio::test]
    async fn approved_fidelity_completion_is_recorded_once_with_delivered_bytes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let ledger_path = directory.path().join("ledger.sqlite");
        let writer = LedgerWriter::open(&ledger_path).expect("ledger writer");
        let outcome = ExecutionPipeline
            .execute(ExecutionEnvelope::allow_raw(CanonicalCommand::shell(
                "printf exact-output",
            )))
            .await
            .expect("managed execution");
        let evasion = EvasionAttribution {
            class: EvasionClass::E7FidelityHatch,
            wrapper_depth: 0,
            interpreter: None,
            path_form: EvasionPathForm::Bare,
            stage_count: 1,
            hatch_marker: true,
            avoidable: false,
            tier: EnforcementTier::T4HatchQuarantine,
            fidelity_reason: Some(FidelityReason::MachineProtocol),
            fidelity_validation: FidelityValidation::Valid,
        };
        let record = fidelity_operation_record_for_context(
            directory.path(),
            Some("test".into()),
            Some("approved-session".into()),
            evasion,
            &outcome,
            false,
        )
        .expect("completed execution record");
        assert_eq!(record.output_tokens, 3);
        writer
            .record_operation(record)
            .await
            .expect("approved fidelity ledger write");

        let summary = Ledger::open(&ledger_path)
            .expect("ledger reader")
            .evasion_summary(hzr_core::StatsQuery::default())
            .expect("evasion summary");
        assert_eq!(summary.fidelity_operations, 1);
        assert_eq!(summary.fidelity_delivered_tokens, 3);
        assert!(
            fidelity_operation_record_for_context(
                directory.path(),
                None,
                Some("failed-session".into()),
                evasion,
                &ExecutionOutcome::NotStarted {
                    disposition: NotStarted::Denied {
                        requested: CanonicalCommand::shell("printf denied"),
                        reason: "denied".into(),
                    },
                },
                false,
            )
            .is_none()
        );
    }

    fn memory(id: &str, topic: &str, weight: f32) -> MemoryRecord {
        MemoryRecord {
            score: None,
            id: id.into(),
            created_at: "2026-08-02T00:00:00Z".into(),
            updated_at: "2026-08-02T00:00:00Z".into(),
            last_accessed: "2026-08-02T00:00:00Z".into(),
            access_count: 0,
            weight,
            topic: topic.into(),
            summary: "fixture".into(),
            raw_excerpt: None,
            keywords: Vec::new(),
            importance: Importance::Medium,
            source: MemorySource::Manual,
            related_ids: Vec::new(),
            scope: MemoryScope::Project,
        }
    }

    #[test]
    fn memory_maintenance_never_targets_foreign_or_above_threshold_records() {
        let records = vec![
            memory("low", &format!("decision-{PROJECT}"), 0.05),
            memory("high", &format!("decision-{PROJECT}"), 0.9),
            MemoryRecord {
                importance: Importance::Critical,
                ..memory("critical-low-weight", &format!("decision-{PROJECT}"), 0.01)
            },
            MemoryRecord {
                importance: Importance::High,
                ..memory("important-low-weight", &format!("decision-{PROJECT}"), 0.01)
            },
            memory(
                "foreign",
                "decision-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                0.01,
            ),
        ];

        let targets =
            memory_mutation_targets(records, PROJECT, MemoryWriteScope::Project, Some(0.1));

        assert_eq!(
            targets
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            ["low"]
        );
    }

    #[test]
    fn public_memory_detail_redacts_every_content_bearing_field() {
        let detail = ProjectMemoryDetail {
            id: "memory-1".into(),
            created_at: "2026-08-02T00:00:00Z".into(),
            updated_at: "2026-08-02T00:00:00Z".into(),
            last_accessed: None,
            access_count: 0,
            weight: 1.0,
            summary: "secret summary".into(),
            raw_excerpt: Some("secret raw".into()),
            keywords: vec!["secret-keyword".into()],
            importance: "high".into(),
            source_type: Some("manual".into()),
            source_data: Some("secret source".into()),
            related_ids: vec!["related".into()],
        };

        let privacy = hzr_core::PrivacyPseudonymizer::from_key("11".repeat(32))
            .expect("valid test privacy key");
        let public = dashboard_memory_detail(detail.clone(), true, &privacy);
        assert!(!public.summary.contains("secret"));
        assert!(public.raw_excerpt.is_none());
        assert!(public.keywords.is_empty());
        assert!(public.source_data.is_none());

        let authenticated = dashboard_memory_detail(detail, false, &privacy);
        assert_eq!(authenticated.summary, "secret summary");
        assert_eq!(authenticated.raw_excerpt.as_deref(), Some("secret raw"));
        assert_eq!(authenticated.keywords, ["secret-keyword"]);
        assert_eq!(authenticated.source_data.as_deref(), Some("secret source"));
    }

    fn engine(name: &str, state: EngineState) -> EngineHealth {
        EngineHealth {
            name: name.into(),
            version: None,
            state,
            detail: None,
        }
    }

    /// `Stopped` is the designed resting state for engines that start on demand. Folding
    /// it into the overall verdict is what painted the whole control plane yellow while
    /// every part of it was working.
    #[test]
    fn an_on_demand_engine_at_rest_does_not_degrade_the_control_plane() {
        let engines = [
            engine("rtk", EngineState::Ready),
            engine("icm", EngineState::Ready),
            engine("grepai", EngineState::Stopped),
            engine("caveman-code", EngineState::Stopped),
        ];

        assert_eq!(overall_engine_state(&engines), EngineState::Ready);
    }

    #[test]
    fn index_state_distinguishes_standby_from_active_rebuild() {
        let standby = dashboard_index_state(true, IndexWatcherState::Standby);
        assert_eq!(
            dashboard_overall_state(
                &[DashboardService {
                    id: "grepai".into(),
                    name: "grepai index".into(),
                    version: None,
                    state: standby,
                    detail: "on demand".into(),
                    command: None,
                }],
                None
            ),
            // Idle, not rebuilding: an on-demand index at rest with nothing selected.
            DashboardState::Standby
        );
        assert_eq!(standby, DashboardState::Standby);
        assert_eq!(
            dashboard_index_state(false, IndexWatcherState::Live),
            DashboardState::Rebuilding
        );
        assert_eq!(
            dashboard_index_state(true, IndexWatcherState::Live),
            DashboardState::Ready
        );
        assert_eq!(
            dashboard_index_state(true, IndexWatcherState::Failed),
            DashboardState::Degraded
        );
    }

    #[test]
    fn dashboard_requires_explicit_stable_project_selection() {
        let worktree_id = "b".repeat(64);
        let registration = WorkspaceRegistration {
            schema_version: 1,
            root: "/workspace/one".into(),
            repository_id: "repository-one".into(),
            worktree_id: worktree_id.clone(),
            git_backed: true,
            linked_worktree: false,
            index_directory: "/index/one".into(),
            registered_at_ms: 1,
            last_seen_at_ms: 2,
        };

        assert!(matches!(
            dashboard_registration(std::slice::from_ref(&registration), None),
            Ok(None)
        ));
        let selected = dashboard_registration(&[registration], Some(&worktree_id))
            .ok()
            .flatten()
            .expect("registration");
        assert_eq!(selected.worktree_id, worktree_id);
        assert!(dashboard_registration(&[], Some(&"c".repeat(64))).is_err());
        assert!(dashboard_registration(&[], Some("malformed")).is_err());
    }

    #[test]
    fn a_warming_selected_project_prevents_global_ready_state() {
        let project = DashboardProject {
            name: "project".into(),
            root: "project".into(),
            repository_id: "repository".into(),
            worktree_id: "worktree".into(),
            git_backed: true,
            linked_worktree: false,
            state: DashboardProjectState::Warming,
            registered_at_ms: 1,
            last_seen_at_ms: 2,
            artifacts: DashboardProjectArtifacts {
                config_present: true,
                vectors_present: false,
                symbols_present: false,
                repository_graph_present: false,
                size_bytes: 0,
                modified_at_ms: None,
            },
            command: "hzr index status --workspace <workspace>".into(),
        };

        assert_eq!(
            dashboard_overall_state(&[], Some(&project)),
            DashboardState::Rebuilding
        );
        // The same project unselected is fleet backlog, not a rebuild in progress.
        assert_eq!(dashboard_overall_state(&[], None), DashboardState::Standby);
    }

    /// A fleet of never-indexed workspaces used to pin the posture to `Rebuilding` forever,
    /// so the one chip a user reads first was wrong on every healthy idle daemon.
    #[test]
    fn an_unselected_fleet_backlog_does_not_report_a_rebuild() {
        let ready = |id: &str| DashboardService {
            id: id.into(),
            name: id.into(),
            version: None,
            state: DashboardState::Ready,
            detail: "ready".into(),
            command: None,
        };
        let standby_index = DashboardService {
            state: DashboardState::Standby,
            ..ready("grepai")
        };

        assert_eq!(
            dashboard_overall_state(
                &[ready("hzrd"), ready("rtk"), ready("icm"), standby_index],
                None
            ),
            DashboardState::Standby,
            "no project is selected, so there is nothing to be ready or rebuilding about"
        );
    }

    #[test]
    fn a_degraded_engine_still_degrades_the_control_plane() {
        let engines = [
            engine("rtk", EngineState::Ready),
            engine("icm", EngineState::Degraded),
        ];

        assert_eq!(overall_engine_state(&engines), EngineState::Degraded);
    }

    /// The caveman version used to be the string literal "0.65.2" in this file, which
    /// stays convincing long after the pin moves. It must come from the lock.
    #[test]
    fn the_caveman_version_comes_from_the_engine_lock() {
        let directory = tempfile::tempdir().expect("temp directory");
        let health = caveman_engine_health(&hzr_agent::IntegrationLayout::new(
            directory.path().join("caveman-code"),
        ));

        let pinned = hzr_core::locked_engines()
            .expect("engine lock parses")
            .engine
            .into_iter()
            .find(|engine| engine.name == "caveman-code")
            .expect("caveman pin")
            .version;
        assert_eq!(health.version.as_deref(), Some(pinned.as_str()));
    }

    /// An absent runtime is not "stopped, will start on demand" — it will never start.
    #[test]
    fn an_uninstalled_caveman_runtime_is_reported_as_degraded_not_stopped() {
        let directory = tempfile::tempdir().expect("temp directory");

        let health = caveman_engine_health(&hzr_agent::IntegrationLayout::new(
            directory.path().join("caveman-code"),
        ));

        assert_eq!(health.state, EngineState::Degraded);
        assert!(
            health
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("hzr install")),
            "the detail must say how to fix it, got {:?}",
            health.detail
        );
    }

    #[test]
    fn an_installed_caveman_runtime_is_stopped_until_hzr_agent_launches_it() {
        let directory = tempfile::tempdir().expect("temp directory");
        let root = directory.path().join("caveman-code");
        let package = root.join("node_modules/@juliusbrussee/caveman-code");
        fs::create_dir_all(&package).expect("package directory");
        fs::write(root.join("bridge.mjs"), "// bridge").expect("bridge");
        fs::write(package.join("package.json"), "{}").expect("package manifest");

        let health = caveman_engine_health(&hzr_agent::IntegrationLayout::new(root));

        assert_eq!(health.state, EngineState::Stopped);
    }

    #[test]
    fn fts_only_icm_is_ready_with_an_explicit_retrieval_capability() {
        let (state, detail) = memory_ready_state(false);

        assert_eq!(state, hzr_protocol::EngineState::Ready);
        assert!(detail.contains("FTS5"));
        assert!(detail.contains("embeddings are disabled"));
    }

    #[test]
    fn index_observatory_reports_only_a_real_recorded_hzr_search() {
        let operations = vec![
            ProjectOperationSummary {
                ledger_id: 42,
                timestamp: "2026-08-02T16:00:00Z".into(),
                operation: "read".into(),
                route: ProjectOperationRoute::Optimized,
                command_hash: "sha256:read".into(),
                project_hash: "sha256:project".into(),
                agent: Some("codex".into()),
                session_hash: Some("sha256:session-42".into()),
                producer_version: Some("hzr-core/test".into()),
                policy_version: Some("privacy_typed_v1".into()),
                baseline_tokens_estimated: 10,
                delivered_tokens_estimated: 5,
                net_avoided_tokens_estimated: 5,
                execution_ms: 3,
                replacement: None,
                rationale: None,
            },
            ProjectOperationSummary {
                ledger_id: 41,
                timestamp: "2026-08-02T15:59:00Z".into(),
                operation: "rgai".into(),
                route: ProjectOperationRoute::Optimized,
                command_hash: "sha256:search".into(),
                project_hash: "sha256:project".into(),
                agent: Some("codex".into()),
                session_hash: Some("sha256:session-41".into()),
                producer_version: Some("hzr-core/test".into()),
                policy_version: Some("privacy_typed_v1".into()),
                baseline_tokens_estimated: 120,
                delivered_tokens_estimated: 30,
                net_avoided_tokens_estimated: 90,
                execution_ms: 17,
                replacement: None,
                rationale: None,
            },
        ];

        let activity = dashboard_search_activity(&operations);

        assert_eq!(activity.state, DashboardState::Ready);
        assert_eq!(activity.ledger_id, Some(41));
        assert_eq!(activity.operation.as_deref(), Some("rgai"));
        assert_eq!(activity.command_hash.as_deref(), Some("sha256:search"));
        assert_eq!(activity.route, Some(DashboardOperationRoute::Optimized));
        assert_eq!(activity.agent.as_deref(), Some("codex"));
        assert_eq!(activity.session_hash.as_deref(), Some("sha256:session-41"));
        assert_eq!(activity.execution_ms, Some(17));
        let encoded = serde_json::to_string(&activity).expect("activity JSON");
        for sensitive in ["real routed query", "/work/hzr", "task-41"] {
            assert!(!encoded.contains(sensitive), "dashboard leaked {sensitive}");
        }
    }

    #[test]
    fn index_observatory_does_not_invent_search_traffic() {
        let activity = dashboard_search_activity(&[]);

        assert_eq!(activity.state, DashboardState::Standby);
        assert_eq!(activity.ledger_id, None);
        assert_eq!(activity.operation, None);
        assert!(activity.detail.contains("No routed HZR search"));
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

    #[test]
    fn typed_managed_patch_keeps_content_out_of_process_arguments() {
        let secret_old = "private old block";
        let secret_new = "private new block";
        let (args, stdin, files) =
            materialize_managed_write(hzr_protocol::ForkManagedWrite::Patch {
                path: "src/lib.rs".into(),
                old: secret_old.into(),
                new: secret_new.into(),
            })
            .expect("typed patch materializes");
        let command_line = args.join(" ");
        assert!(!command_line.contains(secret_old));
        assert!(!command_line.contains(secret_new));
        assert!(stdin.is_none());
        let files = files.expect("patch staging files");
        assert_eq!(
            fs::read_to_string(files.path().join("old")).expect("old"),
            secret_old
        );
        assert_eq!(
            fs::read_to_string(files.path().join("new")).expect("new"),
            secret_new
        );
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
