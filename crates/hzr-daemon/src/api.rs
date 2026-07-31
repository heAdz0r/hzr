use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::State;
use hzr_context::{ContextError, PlanRequest, SearchRequest};
use hzr_core::{Ledger, LedgerRecord, locked_engines};
use hzr_exec::{
    CanonicalCommand, CaptureConfig, CaptureOverflow, CapturedContent, ExecutionEnvelope,
    ExecutionOutcome, ForkCoreInvocation, NotStarted, PINNED_RTK_VERSION, RewriteDecision,
    RewriteSource, StdinSpec, TerminationCause,
};
use hzr_index::{Deadlines, Workspace};
use hzr_memory::{
    Importance, RecallRequest, ServiceStatus, StoreRequest, isolate_project_memories,
    namespaced_topic, recall_candidate_limit,
};
use hzr_protocol::{
    CodecApiRequest, CommandTermination, ContextPlanApiRequest, ContextPlanApiResponse,
    EngineHealth, EngineState, ExecApiRequest, ExecApprovalApiRequest, ForkRunApiRequest,
    ForkRunApiResponse, HealthResponse, MemoryImportance, MemoryRecallApiRequest,
    MemoryStoreApiRequest, PROTOCOL_VERSION, SearchApiRequest, SearchApiResponse, TraceId,
    UsageApiRequest, UsageApiResponse,
};

use crate::approval::PendingApproval;
use crate::error::ApiError;
use crate::state::{AppState, MemoryStartState};

pub async fn health(State(state): State<AppState>) -> Result<Json<HealthResponse>, ApiError> {
    let memory_health = memory_engine_health(&state).await;
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

    Ok(Json(HealthResponse {
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
    }))
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
    let exact_topic = request
        .topic
        .as_deref()
        .map(|kind| namespaced_topic(kind, &project))
        .transpose()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let mut recall = RecallRequest::new(request.query);
    recall.topic.clone_from(&exact_topic);
    recall.limit = recall_candidate_limit(request.limit);
    recall.keyword = request.keyword;
    recall.project = Some(project.clone());
    let records = state
        .memory
        .client()
        .recall(&recall)
        .await
        .map_err(|error| ApiError::service("memory_unavailable", error.to_string(), true))?;
    Ok(Json(isolate_project_memories(
        records,
        &project,
        exact_topic.as_deref(),
        request.limit,
    )))
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
    let topic = namespaced_topic(&request.topic, &project)
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
    let cwd = canonical_workspace(&request.cwd)?;
    let timeout_ms = Some(managed_timeout_ms(&state, request.timeout_ms)?);
    let command = CanonicalCommand::shell(request.command);
    let decision = match state.rtk.decide_in(&command, Some(&cwd)).await {
        RewriteDecision::Ask { proposed, reason } => {
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
    envelope.timeout_ms = timeout_ms;
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
    let cwd = canonical_workspace(&request.cwd)?;
    validate_managed_fork_tool(&request.args, &cwd)?;
    let runner = state
        .rtk
        .runner()
        .map_err(|error| ApiError::service("fork_core_unavailable", error.to_string(), true))?;
    let mut invocation = ForkCoreInvocation::new(request.args);
    invocation.cwd = Some(cwd);
    invocation.timeout_ms = Some(managed_timeout_ms(&state, request.timeout_ms)?);
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
    let ledger_path = state.config.data_dir.join("ledger/hzr.sqlite");
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
    tokio::task::spawn_blocking(move || -> Result<(), hzr_core::LedgerError> {
        Ledger::open(&ledger_path)?.record(&record)
    })
    .await
    .map_err(|error| ApiError::internal(format!("usage ledger task failed: {error}")))?
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
    let workspace = Workspace::discover_managed(
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

async fn memory_engine_health(state: &AppState) -> EngineHealth {
    let start = state.memory_start.read().await;
    let immediate = match &*start {
        MemoryStartState::Starting => Some((EngineState::Rebuilding, "ICM is starting".into())),
        MemoryStartState::Degraded(reason) => Some((EngineState::Degraded, reason.clone())),
        MemoryStartState::Disabled => Some((EngineState::Stopped, "auto start disabled".into())),
        MemoryStartState::Ready(_) => None,
    };
    drop(start);
    let (engine_state, detail) = match immediate {
        Some(result) => result,
        None => match state.memory.status().await {
            ServiceStatus::Running { health, .. } | ServiceStatus::Attached { health }
                if health.has_embedder =>
            {
                (EngineState::Ready, "ICM singleton is ready".into())
            }
            ServiceStatus::Running { .. } | ServiceStatus::Attached { .. } => (
                EngineState::Degraded,
                "ICM singleton is ready in FTS-only mode; embeddings are disabled".into(),
            ),
            other => (EngineState::Degraded, format!("{other:?}")),
        },
    };
    EngineHealth {
        name: "icm".into(),
        version: Some(hzr_memory::ICM_VERSION.into()),
        state: engine_state,
        detail: Some(detail),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::validate_managed_fork_tool;

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
