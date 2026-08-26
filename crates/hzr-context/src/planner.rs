use std::collections::{BTreeMap, btree_map::Entry};
use std::path::{Path, PathBuf};
use std::time::Duration;

use hzr_core::{BudgetPlanner, Config, FusionInput};
use hzr_exec::{
    CaptureConfig, CaptureOverflow, CapturedContent, ExecutionOutcome, ForkCoreInvocation,
    ForkCoreRunner, TerminationCause,
};
use hzr_index::{
    Deadlines, IndexCoordinator, IndexCoordinatorSnapshot, IndexError, IndexGeneration,
    PreparedIndex, Workspace,
};
use hzr_memory::{
    IcmClient, RecallRequest, isolate_project_memories, namespaced_topic, recall_candidate_limit,
    validate_memory_kind,
};
use hzr_protocol::{
    ContextPlanApiResponse, ContextWarning, ContextWarningCode, ForkPlannerMetadata,
    SearchApiResponse, SearchFallbackCode, SearchHit, SearchLine, SearchMode, SearchSnippet,
    SearchStrategy,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Semaphore};

use crate::candidate::{
    ForkPlanCandidate, ForkSymbolIndex, NormalizedSource, RetrievedCandidate, normalize_memory,
    normalize_plan, normalize_search,
};
use crate::error::{ContextError, Result};

const MAX_INTENT_BYTES: usize = 64 * 1024;
const MAX_RETRIEVAL_RESULTS: usize = 100;
const MAX_WARNINGS: usize = 8;
const MAX_WARNING_BYTES: usize = 512;
const SEARCH_CAPTURE_BYTES: usize = 2 * 1024 * 1024;
const PLAN_CAPTURE_BYTES: usize = 8 * 1024 * 1024;
/// A symbol index is a list of names and line spans, so it is small even for a large file.
const OUTLINE_CAPTURE_BYTES: usize = 512 * 1024;
const FORK_TIMEOUT_MARGIN_MS: u64 = 500;

/// Share of the request budget a cold semantic index may consume before the request
/// degrades to exact search. A quarter leaves ample time for the exact pass that follows.
const SEMANTIC_READY_BUDGET_DIVISOR: u64 = 4;
const SEMANTIC_READY_BUDGET_MIN_MS: u64 = 1_000;
const SEMANTIC_READY_BUDGET_MAX_MS: u64 = 8_000;
const PLAN_WEIGHT: f32 = 1.0;
const MEMORY_WEIGHT: f32 = 0.8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchRequest {
    pub workspace: PathBuf,
    pub query: String,
    pub path: Option<PathBuf>,
    pub limit: usize,
    pub mode: SearchMode,
    pub include_content: bool,
}

impl SearchRequest {
    fn validate(&self) -> Result<()> {
        validate_text("query", &self.query, MAX_INTENT_BYTES)?;
        validate_limit("limit", self.limit)?;
        validate_optional_path(self.path.as_deref())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanRequest {
    pub workspace: PathBuf,
    pub intent: String,
    pub path: Option<PathBuf>,
    pub topic: Option<String>,
    pub search_limit: usize,
    pub memory_limit: usize,
}

impl PlanRequest {
    fn validate(&self) -> Result<()> {
        validate_text("intent", &self.intent, MAX_INTENT_BYTES)?;
        validate_limit("search_limit", self.search_limit)?;
        validate_limit("memory_limit", self.memory_limit)?;
        validate_optional_path(self.path.as_deref())?;
        if let Some(topic) = &self.topic {
            validate_memory_kind(topic).map_err(|error| ContextError::InvalidRequest {
                field: "topic",
                reason: error.to_string(),
            })?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ForkConfigFingerprint(Option<[u8; 32]>);

#[derive(Clone, Debug)]
struct ForkSearchConfigCache {
    path: PathBuf,
    fingerprint: ForkConfigFingerprint,
    validation: std::result::Result<(), String>,
}

pub struct ContextPlanner {
    indexes: IndexCoordinator,
    fork: Option<ForkCoreRunner>,
    fork_unavailable: Option<String>,
    memory: IcmClient,
    hard_token_limit: u64,
    fork_timeout_ms: u64,
    fork_search_config: Mutex<Option<ForkSearchConfigCache>>,
    fork_search_config_refresh: Semaphore,
}

impl ContextPlanner {
    pub fn from_config(
        config: &Config,
        memory: IcmClient,
        fork: std::result::Result<ForkCoreRunner, hzr_exec::ExecError>,
    ) -> Self {
        let (fork, fork_unavailable) = match fork {
            Ok(runner) => (Some(runner), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Self {
            indexes: IndexCoordinator::with_watcher_limits(
                config.data_dir.clone(),
                config.engines.binary("git"),
                config.engines.binary("grepai"),
                Deadlines::default(),
                config.engines.auto_index,
                config.engines.grepai_watcher_limit,
                Duration::from_secs(config.engines.grepai_watcher_idle_ttl_seconds),
            ),
            fork,
            fork_unavailable,
            memory,
            hard_token_limit: config.policy.input_token_budget(),
            fork_timeout_ms: config
                .daemon
                .request_timeout_ms
                .saturating_sub(FORK_TIMEOUT_MARGIN_MS)
                .max(1),
            fork_search_config: Mutex::new(None),
            fork_search_config_refresh: Semaphore::new(1),
        }
    }

    pub async fn search(&self, request: SearchRequest) -> Result<SearchApiResponse> {
        self.search_with_accounting(request, true).await
    }

    pub async fn index_registry_snapshot(
        &self,
    ) -> Result<hzr_index::IndexCoordinatorRegistrySnapshot> {
        self.indexes
            .registry_snapshot()
            .await
            .map_err(ContextError::Index)
    }

    pub async fn reap_idle_indexes(&self) -> Result<usize> {
        self.indexes
            .reap_idle_watchers()
            .await
            .map_err(ContextError::Index)
    }

    pub async fn search_unaccounted(&self, request: SearchRequest) -> Result<SearchApiResponse> {
        self.search_with_accounting(request, false).await
    }

    async fn workspace_for_read(
        &self,
        start: &Path,
    ) -> Result<(Workspace, Option<(SearchFallbackCode, String)>)> {
        match self.indexes.workspace(start).await {
            Ok(workspace) => Ok((workspace, None)),
            Err(error @ IndexError::LegacyIndexRequiresMigration { .. }) => {
                let workspace = self.indexes.workspace_for_builtin_search(start).await?;
                Ok((
                    workspace,
                    Some((
                        SearchFallbackCode::LegacyIndexRequiresMigration,
                        format!(
                            "canonical grepai index requires explicit migration; fork rgai used its builtin fallback without activating or modifying the legacy index: {error}"
                        ),
                    )),
                ))
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn search_with_accounting(
        &self,
        mut request: SearchRequest,
        account_usage: bool,
    ) -> Result<SearchApiResponse> {
        request.validate()?;
        let (workspace, migration_fallback) = self.workspace_for_read(&request.workspace).await?;
        let requested_mode = request.mode;
        if migration_fallback.is_some() {
            request.mode = SearchMode::Exact;
        }
        let initial_generation = IndexGeneration::read(&workspace)?;
        let (workspace, generation, fallback_reason) = if request.mode == SearchMode::Exact {
            (workspace, initial_generation, migration_fallback)
        } else {
            match self.prepare_within_request_budget(&workspace).await {
                Ok(prepared) => (prepared.workspace, prepared.generation, None),
                Err(error) => {
                    request.mode = SearchMode::Exact;
                    (
                        workspace,
                        initial_generation,
                        Some((
                            SearchFallbackCode::SemanticIndexUnavailable,
                            format!(
                                "canonical grepai lifecycle is unavailable; fork rgai used its builtin fallback: {error}"
                            ),
                        )),
                    )
                }
            }
        };
        if requested_mode == SearchMode::Auto && request.mode == SearchMode::Auto {
            request.mode = SearchMode::Semantic;
        }
        let mut response = self
            .search_in(&workspace, &generation, &request, account_usage)
            .await?;
        if let Some((code, reason)) = fallback_reason {
            response.fallback_code = Some(code);
            response.fallback_reason = Some(reason);
        }
        Ok(response)
    }

    pub async fn index_status(&self, workspace: &Path) -> Result<IndexCoordinatorSnapshot> {
        self.indexes
            .status(workspace)
            .await
            .map_err(ContextError::from)
    }

    pub async fn plan(&self, request: PlanRequest) -> Result<ContextPlanApiResponse> {
        request.validate()?;
        let (workspace, _) = self.workspace_for_read(&request.workspace).await?;
        workspace.normalize_filter(request.path.as_deref())?;
        let initial_generation = IndexGeneration::read(&workspace)?;
        let project = workspace.identity.repository_id.clone();
        let exact_topic = request
            .topic
            .as_deref()
            .map(|kind| namespaced_topic(kind, &project))
            .transpose()
            .map_err(|error| ContextError::InvalidRequest {
                field: "topic",
                reason: error.to_string(),
            })?;
        let mut recall_request = RecallRequest::new(request.intent.clone());
        recall_request.topic.clone_from(&exact_topic);
        recall_request.limit = recall_candidate_limit(request.memory_limit);
        recall_request.project = Some(project.clone());

        let code_plan = self.code_plan(&workspace, initial_generation, &request);
        let (code_result, memory_result) =
            tokio::join!(code_plan, self.memory.recall(&recall_request));

        let mut warnings = Vec::new();
        let mut sources = Vec::with_capacity(3);
        let mut planner = None;
        match code_result {
            Ok(result) => {
                planner = result.planner;
                result
                    .warnings
                    .into_iter()
                    .for_each(|warning| push_warning(&mut warnings, warning));
                for source in result.sources {
                    add_source(&mut sources, &mut warnings, PLAN_WEIGHT, source);
                }
            }
            Err(error) => push_warning(
                &mut warnings,
                ContextWarning {
                    code: ContextWarningCode::PlannerUnavailable,
                    message: error.to_string(),
                },
            ),
        }
        match memory_result {
            Ok(records) => {
                let records = isolate_project_memories(
                    records,
                    &project,
                    exact_topic.as_deref(),
                    request.memory_limit,
                );
                add_source(
                    &mut sources,
                    &mut warnings,
                    MEMORY_WEIGHT,
                    normalize_memory(records),
                );
            }
            Err(error) => push_warning(
                &mut warnings,
                ContextWarning {
                    code: ContextWarningCode::MemoryUnavailable,
                    message: error.to_string(),
                },
            ),
        }
        fuse(self.hard_token_limit, sources, warnings, planner)
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.indexes.shutdown().await?;
        Ok(())
    }

    /// Wait for the canonical semantic index only as long as a request may afford.
    ///
    /// `IndexCoordinator` legitimately allows a cold watcher up to `watch_start`
    /// (120 s) to finish its first scan — on a large tree that is normal. But a request
    /// deadline is shorter than that, so awaiting readiness directly made `/v1/search`
    /// and `/v1/context/plan` die at the transport with an opaque "error sending
    /// request" instead of degrading. PRD §10 requires the opposite: a missing or
    /// not-yet-ready index degrades to exact search with visible `degraded` status.
    ///
    /// Two properties matter here:
    ///
    /// * the wait is bounded by a fraction of the request budget, leaving time for the
    ///   exact search that follows a timeout;
    /// * the preparation is **not cancelled** when the wait elapses. Dropping the future
    ///   would abort the warming watcher, so every request would restart the initial scan
    ///   and the index could never become ready — which also leaked one runtime directory
    ///   per attempt. Running it detached lets this request degrade while the next one
    ///   finds a warm index.
    async fn prepare_within_request_budget(&self, workspace: &Workspace) -> Result<PreparedIndex> {
        let budget = Duration::from_millis(
            (self.fork_timeout_ms / SEMANTIC_READY_BUDGET_DIVISOR)
                .clamp(SEMANTIC_READY_BUDGET_MIN_MS, SEMANTIC_READY_BUDGET_MAX_MS),
        );
        let indexes = self.indexes.clone();
        let target = workspace.clone();
        // Detached so an elapsed wait never aborts the warm-up in flight.
        let preparation = tokio::spawn(async move { indexes.prepare_workspace(target).await });
        match tokio::time::timeout(budget, preparation).await {
            Ok(Ok(result)) => result.map_err(ContextError::from),
            // The preparation task itself failed (panic or cancellation): treat as not
            // ready rather than propagating, so search still degrades instead of failing.
            Ok(Err(error)) => Err(ContextError::IndexNotReady(format!(
                "index preparation task did not complete: {error}"
            ))),
            Err(_) => Err(ContextError::IndexNotReady(format!(
                "semantic index was not ready within {} ms; it keeps warming in the background",
                budget.as_millis()
            ))),
        }
    }

    async fn code_plan(
        &self,
        workspace: &Workspace,
        initial_generation: IndexGeneration,
        request: &PlanRequest,
    ) -> Result<CodePlanResult> {
        let mut warnings = Vec::new();
        let prepared = self.prepare_within_request_budget(workspace).await;
        let (generation, adaptive_search_ready) = match prepared {
            Ok(prepared) => (prepared.generation, true),
            Err(error) => {
                push_warning(
                    &mut warnings,
                    ContextWarning {
                        code: ContextWarningCode::SearchDegraded,
                        message: format!(
                            "canonical grepai lifecycle is unavailable; fork planner continues with structural evidence: {error}"
                        ),
                    },
                );
                (initial_generation, false)
            }
        };
        let plan = self.run_memory_plan(workspace, request).await?;
        let planner = Some(plan.metadata());
        if !plan.selected.is_empty() {
            let selected: Vec<_> = plan
                .selected
                .into_iter()
                .take(request.search_limit)
                .collect();
            let selected_count = selected.len();
            let (outlines, missing_outlines) = self.candidate_outlines(workspace, &selected).await;
            let mut source = normalize_plan(
                selected,
                workspace,
                &generation.generation,
                plan.pipeline_version.as_deref(),
                &outlines,
            )?;
            if missing_outlines > 0 {
                push_warning(
                    &mut source.warnings,
                    ContextWarning {
                        code: ContextWarningCode::OutlineUnavailable,
                        message: format!(
                            "symbol outline unavailable for {missing_outlines} of {selected_count} candidates"
                        ),
                    },
                );
            }
            let mut exact_source = None;
            if let Some(identifier) = exact_identifier(&request.intent) {
                let exact_request = SearchRequest {
                    workspace: workspace.identity.root.clone(),
                    query: identifier.to_owned(),
                    path: request.path.clone(),
                    limit: request.search_limit.min(5),
                    mode: SearchMode::Exact,
                    include_content: true,
                };
                match self
                    .search_in(workspace, &generation, &exact_request, true)
                    .await
                {
                    Ok(response) => {
                        let hit_count = response.hits.len();
                        let (outlines, missing_outlines) =
                            self.search_outlines(workspace, &response).await;
                        let mut exact =
                            normalize_search(response, &generation.generation, &outlines);
                        if missing_outlines > 0 {
                            push_warning(
                                &mut exact.warnings,
                                ContextWarning {
                                    code: ContextWarningCode::OutlineUnavailable,
                                    message: format!(
                                        "symbol outline unavailable for {missing_outlines} of {hit_count} search candidates"
                                    ),
                                },
                            );
                        }
                        exact_source = Some(exact);
                    }
                    Err(error) => push_warning(
                        &mut warnings,
                        ContextWarning {
                            code: ContextWarningCode::SearchUnavailable,
                            message: format!(
                                "exact identifier search for {identifier} was unavailable: {error}"
                            ),
                        },
                    ),
                }
            }
            return Ok(CodePlanResult {
                sources: separate_code_sources(source, exact_source),
                warnings,
                planner,
            });
        }

        push_warning(
            &mut warnings,
            ContextWarning {
                code: ContextWarningCode::PlannerFallback,
                message: "fork memory planner returned no candidates; using one fork rgai fallback"
                    .into(),
            },
        );
        let search_request = SearchRequest {
            workspace: workspace.identity.root.clone(),
            query: request.intent.clone(),
            path: request.path.clone(),
            limit: request.search_limit,
            mode: if adaptive_search_ready {
                SearchMode::Auto
            } else {
                SearchMode::Exact
            },
            include_content: true,
        };
        let source = match self
            .search_in(workspace, &generation, &search_request, true)
            .await
        {
            Ok(response) => {
                let hit_count = response.hits.len();
                let (outlines, missing_outlines) = self.search_outlines(workspace, &response).await;
                let mut source = normalize_search(response, &generation.generation, &outlines);
                if missing_outlines > 0 {
                    push_warning(
                        &mut source.warnings,
                        ContextWarning {
                            code: ContextWarningCode::OutlineUnavailable,
                            message: format!(
                                "symbol outline unavailable for {missing_outlines} of {hit_count} search candidates"
                            ),
                        },
                    );
                }
                Some(source)
            }
            Err(error) => {
                push_warning(
                    &mut warnings,
                    ContextWarning {
                        code: ContextWarningCode::SearchUnavailable,
                        message: error.to_string(),
                    },
                );
                None
            }
        };
        Ok(CodePlanResult {
            sources: source.into_iter().collect(),
            warnings,
            planner,
        })
    }

    /// Fetch the symbol outline for each plan candidate.
    ///
    /// Without this a candidate is a bare path, so an agent has to open every file just to find
    /// out whether it is relevant — the work the plan exists to save. The fork already has the
    /// extractor (`rtk read <file> --symbols`), so this delegates to it rather than growing a
    /// second, weaker one inside HZR.
    ///
    /// Outlines are best-effort by design. A file that is unreadable, generated, binary or in an
    /// unsupported language contributes no outline and its candidate degrades to exactly the
    /// path it was before; a plan must not fail because one lead could not be summarised.
    async fn candidate_outlines(
        &self,
        workspace: &Workspace,
        selected: &[ForkPlanCandidate],
    ) -> (BTreeMap<String, ForkSymbolIndex>, usize) {
        let paths = selected
            .iter()
            .map(|candidate| candidate.rel_path.clone())
            .collect::<Vec<_>>();
        self.candidate_outlines_for_paths(workspace, &paths).await
    }

    async fn search_outlines(
        &self,
        workspace: &Workspace,
        response: &SearchApiResponse,
    ) -> (BTreeMap<String, ForkSymbolIndex>, usize) {
        let mut paths = response
            .hits
            .iter()
            .map(|hit| hit.path.clone())
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        self.candidate_outlines_for_paths(workspace, &paths).await
    }

    async fn candidate_outlines_for_paths(
        &self,
        workspace: &Workspace,
        paths: &[String],
    ) -> (BTreeMap<String, ForkSymbolIndex>, usize) {
        let mut outlines = BTreeMap::new();
        let mut unavailable = 0;
        for requested_path in paths {
            let Ok(path) = workspace.normalize_result(Path::new(requested_path)) else {
                unavailable += 1;
                continue;
            };
            let Some(path) = path.to_str() else {
                unavailable += 1;
                continue;
            };
            let index: std::result::Result<ForkSymbolIndex, _> = self
                .run_fork_json(
                    vec!["read".into(), path.to_owned(), "--symbols".into()],
                    &workspace.identity.root,
                    OUTLINE_CAPTURE_BYTES,
                    "read symbols",
                    // Outline assembly happens inside one plan the ledger already accounts
                    // for; charging it again would double-count that plan.
                    false,
                )
                .await;
            if let Ok(index) = index {
                outlines.insert(requested_path.clone(), index.clone());
                outlines.insert(path.to_owned(), index);
            } else {
                unavailable += 1;
            }
        }
        (outlines, unavailable)
    }

    async fn run_memory_plan(
        &self,
        workspace: &Workspace,
        request: &PlanRequest,
    ) -> Result<ForkPlanOutput> {
        let filter = workspace.normalize_filter(request.path.as_deref())?;
        let path = filter.as_deref().unwrap_or_else(|| Path::new("."));
        let path_text = path.to_str().ok_or_else(|| ContextError::InvalidRequest {
            field: "path",
            reason: "path must be valid UTF-8".into(),
        })?;
        let token_budget = self
            .hard_token_limit
            .saturating_mul(3)
            .saturating_div(4)
            .clamp(1, u64::from(u32::MAX));
        self.run_fork_json(
            vec![
                "memory".into(),
                "plan".into(),
                request.intent.clone(),
                path_text.into(),
                "--token-budget".into(),
                token_budget.to_string(),
                "--format".into(),
                "json".into(),
                "--top".into(),
                request.search_limit.to_string(),
            ],
            &workspace.identity.root,
            PLAN_CAPTURE_BYTES,
            "memory plan",
            true,
        )
        .await
    }

    async fn search_in(
        &self,
        workspace: &Workspace,
        generation: &IndexGeneration,
        request: &SearchRequest,
        account_usage: bool,
    ) -> Result<SearchApiResponse> {
        let filter = workspace.normalize_filter(request.path.as_deref())?;
        let path = filter.as_deref().unwrap_or_else(|| Path::new("."));
        let path_text = path.to_str().ok_or_else(|| ContextError::InvalidRequest {
            field: "path",
            reason: "path must be valid UTF-8".into(),
        })?;
        let root_text =
            workspace
                .identity
                .root
                .to_str()
                .ok_or_else(|| ContextError::InvalidRequest {
                    field: "workspace",
                    reason: "workspace root must be valid UTF-8".into(),
                })?;
        let planned_strategy = if request.mode == SearchMode::Exact {
            SearchStrategy::ForkRgaiBuiltin
        } else {
            self.ensure_managed_fork_search_config(workspace, account_usage)
                .await?;
            SearchStrategy::ForkRgaiAdaptive
        };
        let args = fork_search_args(
            &request.query,
            Path::new(path_text),
            Path::new(root_text),
            request.mode,
            request.limit,
            request.include_content,
        );
        let raw: ForkSearchOutput = self
            .run_fork_json(
                args,
                &workspace.identity.root,
                SEARCH_CAPTURE_BYTES,
                "rgai search",
                account_usage,
            )
            .await?;
        if let Some(parse_error) = raw.parse_error {
            return Err(ContextError::InvalidForkOutput {
                operation: "rgai search",
                detail: parse_error,
            });
        }
        let strategy = raw
            .backend
            .map(ForkSearchBackend::strategy)
            .unwrap_or(planned_strategy);
        let fallback_code = fork_backend_fallback_code(request.mode, strategy);
        let hits = raw
            .hits
            .into_iter()
            .take(request.limit)
            .map(|hit| normalize_search_hit(workspace, Path::new(path_text), hit))
            .collect::<Result<Vec<_>>>()?;
        let next_step = (raw.total_hits > hits.len()).then(|| {
            format!(
                "{} additional matches were bounded; narrow the query/path or rerun `hzr --json search '{}' --limit {}`",
                raw.total_hits.saturating_sub(hits.len()),
                raw.query.replace('\'', "'\\''"),
                raw.total_hits.min(50)
            )
        });
        Ok(SearchApiResponse {
            query: raw.query,
            path: path_text.to_owned(),
            total_hits: raw.total_hits,
            shown_hits: hits.len(),
            scanned_files: raw.scanned_files,
            skipped_large: raw.skipped_large,
            skipped_binary: raw.skipped_binary,
            hits,
            effective_mode: request.mode,
            strategy,
            fallback_code,
            index_generation: Some(generation.generation.clone()),
            fallback_reason: None,
            next_step,
        })
    }

    async fn run_fork_json<T: DeserializeOwned>(
        &self,
        args: Vec<String>,
        cwd: &Path,
        max_capture_bytes: usize,
        operation: &'static str,
        account_usage: bool,
    ) -> Result<T> {
        let stdout = self
            .run_fork_output(args, cwd, max_capture_bytes, operation, account_usage)
            .await?;
        serde_json::from_slice(&stdout).map_err(|error| ContextError::InvalidForkOutput {
            operation,
            detail: error.to_string(),
        })
    }

    async fn run_fork_output(
        &self,
        args: Vec<String>,
        cwd: &Path,
        max_capture_bytes: usize,
        operation: &'static str,
        account_usage: bool,
    ) -> Result<Vec<u8>> {
        let runner = self.fork.as_ref().ok_or_else(|| {
            ContextError::ForkUnavailable(
                self.fork_unavailable
                    .clone()
                    .unwrap_or_else(|| "runner was not configured".into()),
            )
        })?;
        let mut invocation = ForkCoreInvocation::new(args);
        if !account_usage {
            invocation = invocation.without_accounting();
        }
        invocation.cwd = Some(cwd.to_owned());
        invocation.timeout_ms = Some(self.fork_timeout_ms);
        invocation.capture = CaptureConfig {
            memory_limit_bytes: max_capture_bytes,
            max_capture_bytes: max_capture_bytes as u64,
            overflow: CaptureOverflow::Truncate,
            event_buffer: 16,
        };
        let outcome = runner.execute(invocation).await?;
        let result = match outcome {
            ExecutionOutcome::Completed { result } => result,
            ExecutionOutcome::ExecutedAccountingIncomplete { accounting, .. } => {
                return Err(ContextError::InvalidForkOutput {
                    operation,
                    detail: format!(
                        "managed invocation executed but accounting is incomplete (code={}, retryable={})",
                        accounting.code, accounting.retryable
                    ),
                });
            }
            ExecutionOutcome::NotStarted { disposition } => {
                return Err(ContextError::InvalidForkOutput {
                    operation,
                    detail: format!("managed invocation was not started: {disposition:?}"),
                });
            }
        };
        let stderr = captured_text(&result.stderr, operation)?;
        if result.termination.cause != TerminationCause::Exited
            || result.termination.exit_code != Some(0)
        {
            return Err(ContextError::ForkCommand {
                operation,
                exit_code: result.termination.exit_code,
                stderr,
            });
        }
        let stdout = captured_bytes(&result.stdout, operation)?;
        Ok(stdout.to_vec())
    }

    async fn ensure_managed_fork_search_config(
        &self,
        workspace: &Workspace,
        account_usage: bool,
    ) -> Result<()> {
        if let Some(validation) = self.cached_fork_search_validation().await? {
            return validation.map_err(ContextError::ForkUnavailable);
        }

        let _refresh = self
            .fork_search_config_refresh
            .acquire()
            .await
            .map_err(|_| ContextError::ForkUnavailable("fork config refresh closed".into()))?;
        if let Some(validation) = self.cached_fork_search_validation().await? {
            return validation.map_err(ContextError::ForkUnavailable);
        }
        let mut stable_snapshot = None;
        for _ in 0..2 {
            let output = self
                .run_fork_output(
                    vec!["config".into(), "--format".into(), "json".into()],
                    &workspace.identity.root,
                    256 * 1024,
                    "config inspection",
                    account_usage,
                )
                .await?;
            let output = std::str::from_utf8(&output).map_err(|error| {
                ContextError::ForkUnavailable(format!("fork config output is not UTF-8: {error}"))
            })?;
            let snapshot = parse_fork_search_config(output, self.indexes.grepai_binary())
                .map_err(ContextError::ForkUnavailable)?;
            let observed =
                fork_config_fingerprint(&snapshot.path).map_err(ContextError::ForkUnavailable)?;
            if observed == snapshot.fingerprint {
                stable_snapshot = Some(snapshot);
                break;
            }
        }
        let snapshot = stable_snapshot.ok_or_else(|| {
            ContextError::ForkUnavailable(
                "fork config changed during two consecutive typed inspections".into(),
            )
        })?;
        let validation = snapshot.validation;
        let mut cache = self.fork_search_config.lock().await;
        *cache = Some(ForkSearchConfigCache {
            path: snapshot.path,
            fingerprint: snapshot.fingerprint,
            validation: validation.clone(),
        });
        validation.map_err(ContextError::ForkUnavailable)
    }

    async fn cached_fork_search_validation(
        &self,
    ) -> Result<Option<std::result::Result<(), String>>> {
        let cache = self.fork_search_config.lock().await;
        let Some(cached) = cache.as_ref() else {
            return Ok(None);
        };
        let fingerprint =
            fork_config_fingerprint(&cached.path).map_err(ContextError::ForkUnavailable)?;
        Ok((fingerprint == cached.fingerprint).then(|| cached.validation.clone()))
    }
}

struct CodePlanResult {
    sources: Vec<NormalizedSource>,
    warnings: Vec<ContextWarning>,
    planner: Option<ForkPlannerMetadata>,
}

/// Keep retrieval families in separate calibration domains. Fork memory-plan scores and
/// exact-search scores are not numerically comparable; `BudgetPlanner` normalizes each input
/// source before applying cross-source utility, so merging them here would let a high BM25-like
/// exact score suppress otherwise relevant graph evidence.
fn separate_code_sources(
    plan: NormalizedSource,
    exact: Option<NormalizedSource>,
) -> Vec<NormalizedSource> {
    let mut sources = Vec::with_capacity(1 + usize::from(exact.is_some()));
    sources.push(plan);
    sources.extend(exact);
    sources
}

#[derive(Deserialize)]
struct ForkSearchOutput {
    query: String,
    total_hits: usize,
    #[serde(default)]
    scanned_files: usize,
    #[serde(default)]
    skipped_large: usize,
    #[serde(default)]
    skipped_binary: usize,
    #[serde(default)]
    hits: Vec<ForkSearchHit>,
    #[serde(default)]
    parse_error: Option<String>,
    #[serde(default)]
    backend: Option<ForkSearchBackend>,
}

#[derive(Clone, Copy, Deserialize)]
enum ForkSearchBackend {
    #[serde(rename = "grepai")]
    Grepai,
    #[serde(rename = "rg")]
    Ripgrep,
    #[serde(rename = "rg-files")]
    Files,
    #[serde(rename = "builtin")]
    Builtin,
}

impl ForkSearchBackend {
    const fn strategy(self) -> SearchStrategy {
        match self {
            Self::Grepai => SearchStrategy::ForkRgaiGrepai,
            Self::Ripgrep => SearchStrategy::ForkRgaiRipgrep,
            Self::Files => SearchStrategy::ForkRgaiFiles,
            Self::Builtin => SearchStrategy::ForkRgaiBuiltin,
        }
    }
}

const fn fork_backend_fallback_code(
    requested_mode: SearchMode,
    strategy: SearchStrategy,
) -> Option<SearchFallbackCode> {
    match (requested_mode, strategy) {
        (SearchMode::Exact, _) | (_, SearchStrategy::ForkRgaiGrepai) => None,
        (_, SearchStrategy::ForkRgaiRipgrep) => Some(SearchFallbackCode::GrepaiUnavailable),
        (_, SearchStrategy::ForkRgaiBuiltin) => Some(SearchFallbackCode::RipgrepUnavailable),
        _ => None,
    }
}

#[derive(Deserialize)]
struct ForkSearchHit {
    path: String,
    score: f64,
    matched_lines: usize,
    #[serde(default)]
    snippets: Vec<ForkSearchSnippet>,
}

#[derive(Deserialize)]
struct ForkSearchSnippet {
    #[serde(default)]
    lines: Vec<ForkSearchLine>,
    #[serde(default)]
    matched_terms: Vec<String>,
}

#[derive(Deserialize)]
struct ForkSearchLine {
    line: usize,
    text: String,
}

#[derive(Deserialize)]
struct ForkPlanOutput {
    #[serde(default)]
    selected: Vec<ForkPlanCandidate>,
    budget_report: ForkBudgetReport,
    #[serde(default)]
    pipeline_version: Option<String>,
    #[serde(default)]
    semantic_backend_used: Option<String>,
    #[serde(default)]
    graph_candidate_count: Option<usize>,
    #[serde(default)]
    semantic_hit_count: Option<usize>,
}

impl ForkPlanOutput {
    fn metadata(&self) -> ForkPlannerMetadata {
        ForkPlannerMetadata {
            pipeline_version: self.pipeline_version.clone(),
            semantic_backend_used: self.semantic_backend_used.clone(),
            graph_candidate_count: self.graph_candidate_count,
            semantic_hit_count: self.semantic_hit_count,
            candidates_total: self.budget_report.candidates_total,
            candidates_selected: self.budget_report.candidates_selected,
            estimated_tokens_used: u64::from(self.budget_report.estimated_used),
            token_budget: u64::from(self.budget_report.token_budget),
        }
    }
}

#[derive(Deserialize)]
struct ForkBudgetReport {
    token_budget: u32,
    estimated_used: u32,
    candidates_total: usize,
    candidates_selected: usize,
}

/// Largest file HZR will re-read to complete a chunk-boundary fragment. Snippet lines are
/// few and bounded, but the file they came from need not be, and a search must not turn into
/// an unbounded read.
const MAX_SNIPPET_REPAIR_BYTES: u64 = 4 * 1024 * 1024;

/// Rebase a fork hit path onto the project root.
///
/// The fork reports hit paths relative to `--path`, not to the project root, so any scoped
/// search corrupted the one field an agent must act on: scoping to `src` reported `lib.rs`,
/// which does not exist at the root, and scoping to a single file reported the empty string.
/// Joining is uniform because an unscoped search is sent as `--path .`, and joining onto `.`
/// changes nothing.
fn hit_relative_path(search_path: &Path, hit_path: &str) -> PathBuf {
    if hit_path.is_empty() {
        return search_path.to_path_buf();
    }
    if search_path == Path::new(".") {
        return PathBuf::from(hit_path);
    }
    search_path.join(hit_path)
}

/// Complete a snippet line the semantic engine returned mid-token.
///
/// grepai chunks are byte windows, so the first line of a chunk can begin mid-identifier.
/// Passing that through breaks HZR's own protocol — `SearchLine::text` means "the text of
/// this line" — and hands the model source that looks real and does not parse.
///
/// Repair is deliberately provable rather than best-effort: the fragment is only completed
/// when it genuinely occurs in the line the engine pointed at. An index older than the file
/// therefore keeps the engine's text instead of having an unrelated line substituted for it,
/// which would be a worse failure than the truncation.
fn repair_snippet_line(fragment: &str, actual: Option<&str>) -> String {
    let Some(actual) = actual else {
        return fragment.to_owned();
    };
    let complete = actual.trim();
    // An empty fragment matches every line, so it proves nothing about belonging to this one.
    if fragment.is_empty() || fragment == complete || !complete.contains(fragment) {
        return fragment.to_owned();
    }
    complete.to_owned()
}

/// Read a file's lines for snippet repair. A missing, oversized or non-UTF-8 file simply
/// yields no repair material; search results must still be returned.
fn repair_source(workspace: &Workspace, path: &Path) -> Option<Vec<String>> {
    let absolute = workspace.identity.root.join(path);
    let metadata = std::fs::metadata(&absolute).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_SNIPPET_REPAIR_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(&absolute).ok()?;
    Some(text.lines().map(str::to_owned).collect())
}

fn normalize_search_hit(
    workspace: &Workspace,
    search_path: &Path,
    hit: ForkSearchHit,
) -> Result<SearchHit> {
    let path = workspace.normalize_result(&hit_relative_path(search_path, &hit.path))?;
    let path = path
        .to_str()
        .ok_or_else(|| ContextError::InvalidForkOutput {
            operation: "rgai search",
            detail: "result path is not valid UTF-8".into(),
        })?
        .to_owned();
    // Read once per hit, not once per line: a snippet has at most a handful of lines and they
    // all come from the same file.
    let source = repair_source(workspace, Path::new(&path));
    let snippets = hit
        .snippets
        .into_iter()
        .map(|snippet| {
            let lines = snippet
                .lines
                .into_iter()
                .map(|line| {
                    let line_number =
                        u32::try_from(line.line).map_err(|_| ContextError::InvalidForkOutput {
                            operation: "rgai search",
                            detail: "line number exceeds the protocol range".into(),
                        })?;
                    // Line numbers are 1-based in every engine that produces them.
                    let actual = source
                        .as_ref()
                        .and_then(|lines| {
                            line.line.checked_sub(1).and_then(|index| lines.get(index))
                        })
                        .map(String::as_str);
                    Ok(SearchLine {
                        line: line_number,
                        text: repair_snippet_line(&line.text, actual),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(SearchSnippet {
                lines,
                matched_terms: snippet.matched_terms,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(SearchHit {
        path,
        score: hit.score,
        matched_lines: hit.matched_lines,
        snippets,
    })
}

fn captured_bytes<'a>(
    stream: &'a hzr_exec::CapturedStream,
    operation: &'static str,
) -> Result<&'a [u8]> {
    if !stream.is_exact() {
        return Err(ContextError::InvalidForkOutput {
            operation,
            detail: format!(
                "captured output exceeded the {} byte bound",
                stream.stored_bytes
            ),
        });
    }
    match &stream.content {
        CapturedContent::Inline { bytes } => Ok(bytes),
        CapturedContent::Spilled { .. } => Err(ContextError::InvalidForkOutput {
            operation,
            detail: "bounded fork output unexpectedly spilled to disk".into(),
        }),
    }
}

fn captured_text(stream: &hzr_exec::CapturedStream, operation: &'static str) -> Result<String> {
    Ok(String::from_utf8_lossy(captured_bytes(stream, operation)?).into_owned())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForkConfigOutput {
    schema_version: u32,
    config_path: PathBuf,
    config_exists: bool,
    config_sha256: Option<String>,
    config: ForkRuntimeConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForkRuntimeConfig {
    grepai: ForkGrepaiConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForkGrepaiConfig {
    enabled: bool,
    #[serde(rename = "auto_init")]
    _auto_init: bool,
    binary_path: Option<PathBuf>,
}

#[derive(Debug)]
struct ForkSearchConfigSnapshot {
    path: PathBuf,
    fingerprint: ForkConfigFingerprint,
    validation: std::result::Result<(), String>,
}

fn parse_fork_search_config(
    output: &str,
    managed_grepai: &Path,
) -> std::result::Result<ForkSearchConfigSnapshot, String> {
    let config: ForkConfigOutput = serde_json::from_str(output)
        .map_err(|error| format!("fork config output contains invalid JSON: {error}"))?;
    if config.schema_version != 2 {
        return Err(format!(
            "unsupported fork config schema version {}",
            config.schema_version
        ));
    }
    if !config.config_path.is_absolute() {
        return Err("fork config path must be absolute".into());
    }
    let fingerprint = match (config.config_exists, config.config_sha256) {
        (false, None) => ForkConfigFingerprint(None),
        (true, Some(digest)) => {
            let bytes = hex::decode(&digest)
                .map_err(|error| format!("fork config SHA-256 is invalid: {error}"))?;
            let digest: [u8; 32] = bytes
                .try_into()
                .map_err(|_| "fork config SHA-256 must contain exactly 32 bytes".to_owned())?;
            ForkConfigFingerprint(Some(digest))
        }
        _ => return Err("fork config existence and SHA-256 state are inconsistent".into()),
    };
    let validation = validate_fork_grepai_config(&config.config.grepai, managed_grepai);
    Ok(ForkSearchConfigSnapshot {
        path: config.config_path,
        fingerprint,
        validation,
    })
}

fn validate_fork_grepai_config(
    grepai: &ForkGrepaiConfig,
    managed_grepai: &Path,
) -> std::result::Result<(), String> {
    if !grepai.enabled {
        return Err("fork grepai delegation is disabled by the user RTK config".into());
    }
    let Some(custom) = grepai.binary_path.as_deref() else {
        return Ok(());
    };
    let custom = canonical_executable(custom)?;
    let managed = canonical_executable(managed_grepai)?;
    if custom != managed {
        return Err(format!(
            "fork grepai binary override {} differs from managed {}",
            custom.display(),
            managed.display()
        ));
    }
    Ok(())
}

fn fork_config_fingerprint(path: &Path) -> std::result::Result<ForkConfigFingerprint, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(ForkConfigFingerprint(Some(Sha256::digest(bytes).into()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(ForkConfigFingerprint(None))
        }
        Err(error) => Err(format!(
            "cannot fingerprint fork config {}: {error}",
            path.display()
        )),
    }
}

fn canonical_executable(path: &Path) -> std::result::Result<PathBuf, String> {
    let candidate = if path.components().count() > 1 || path.is_absolute() {
        path.to_owned()
    } else {
        std::env::var_os("PATH")
            .and_then(|value| {
                std::env::split_paths(&value)
                    .map(|directory| directory.join(path))
                    .find(|candidate| candidate.is_file())
            })
            .ok_or_else(|| format!("cannot resolve managed executable {}", path.display()))?
    };
    std::fs::canonicalize(&candidate)
        .map_err(|error| format!("cannot canonicalize {}: {error}", candidate.display()))
}

fn add_source(
    sources: &mut Vec<(f32, Vec<RetrievedCandidate>)>,
    warnings: &mut Vec<ContextWarning>,
    weight: f32,
    normalized: NormalizedSource,
) {
    normalized
        .warnings
        .into_iter()
        .for_each(|warning| push_warning(warnings, warning));
    if !normalized.candidates.is_empty() {
        sources.push((weight, normalized.candidates));
    }
}

/// Build the fork-core `rgai` argument list.
///
/// `--project-root` is passed for every mode, not only the ranked ones. The fork treats
/// `--path` as the project root when no root is given, so an exact search scoped to a
/// single file failed with "project root is not a directory" — surfaced to the agent as
/// an opaque HTTP 503 — while the identical semantic search worked. Scope and project
/// identity are different things and are now always sent as different arguments.
fn exact_identifier(intent: &str) -> Option<&str> {
    intent
        .split_whitespace()
        .map(|token| {
            token.trim_matches(|character: char| {
                !character.is_ascii_alphanumeric() && character != '_' && character != ':'
            })
        })
        .filter(|token| (3..=256).contains(&token.len()))
        .filter(|token| {
            token.contains('_')
                || token.contains("::")
                || token
                    .as_bytes()
                    .windows(2)
                    .any(|pair| pair[0].is_ascii_lowercase() && pair[1].is_ascii_uppercase())
        })
        .max_by_key(|token| token.len())
}

fn fork_search_args(
    query: &str,
    path: &Path,
    workspace_root: &Path,
    mode: SearchMode,
    limit: usize,
    include_content: bool,
) -> Vec<String> {
    let mut args = vec![
        "rgai".into(),
        "--path".into(),
        path.to_string_lossy().into_owned(),
        "--project-root".into(),
        workspace_root.to_string_lossy().into_owned(),
        "--max".into(),
        limit.to_string(),
        "--json".into(),
    ];
    if mode == SearchMode::Exact {
        // `--literal` matches the query verbatim and case-sensitively, and implies
        // `--builtin`. Sending only `--builtin` handed the query to the ranked term
        // model, which lowercases it, splits it on non-alphanumerics, drops stop words
        // and stems the rest — so "exact" returned every file containing any one of the
        // surviving tokens, and agents learned to distrust the mode entirely.
        args.push("--literal".into());
    }
    if !include_content {
        args.push("--compact".into());
    }
    // The query goes last, behind `--`, so a pattern that begins with a dash is matched
    // rather than parsed as a flag.
    args.push("--".into());
    args.push(query.to_owned());
    args
}

fn fuse(
    hard_token_limit: u64,
    sources: Vec<(f32, Vec<RetrievedCandidate>)>,
    warnings: Vec<ContextWarning>,
    planner: Option<ForkPlannerMetadata>,
) -> Result<ContextPlanApiResponse> {
    let mut contents = BTreeMap::<String, String>::new();
    let mut fusion_inputs = Vec::with_capacity(sources.len());
    for (source_weight, retrieved) in sources {
        let mut candidates = Vec::with_capacity(retrieved.len());
        for retrieved_candidate in retrieved {
            let content_ref = retrieved_candidate.candidate.content_ref.clone();
            match contents.entry(content_ref) {
                Entry::Vacant(entry) => {
                    entry.insert(retrieved_candidate.content);
                }
                Entry::Occupied(entry) if entry.get() != &retrieved_candidate.content => {
                    return Err(ContextError::Invariant(
                        "one content reference resolved to different content".into(),
                    ));
                }
                Entry::Occupied(_) => {}
            }
            candidates.push(retrieved_candidate.candidate);
        }
        fusion_inputs.push(FusionInput {
            source_weight,
            candidates,
        });
    }
    let pack = BudgetPlanner::new(hard_token_limit).plan(fusion_inputs);
    let mut selected_contents = BTreeMap::new();
    for candidate in &pack.selected {
        let content = contents.remove(&candidate.content_ref).ok_or_else(|| {
            ContextError::Invariant(format!(
                "selected content {} is unavailable",
                candidate.content_ref
            ))
        })?;
        selected_contents.insert(candidate.content_ref.clone(), content);
    }
    Ok(ContextPlanApiResponse {
        pack,
        contents: selected_contents,
        warnings,
        planner,
    })
}

fn validate_text(field: &'static str, text: &str, max_bytes: usize) -> Result<()> {
    if text.trim().is_empty() {
        return Err(ContextError::InvalidRequest {
            field,
            reason: format!("{field} must not be empty"),
        });
    }
    if text.len() > max_bytes {
        return Err(ContextError::InvalidRequest {
            field,
            reason: format!("{field} must not exceed {max_bytes} bytes"),
        });
    }
    Ok(())
}

fn validate_limit(field: &'static str, limit: usize) -> Result<()> {
    if !(1..=MAX_RETRIEVAL_RESULTS).contains(&limit) {
        return Err(ContextError::InvalidRequest {
            field,
            reason: format!("{field} must be between 1 and {MAX_RETRIEVAL_RESULTS}"),
        });
    }
    Ok(())
}

fn validate_optional_path(path: Option<&Path>) -> Result<()> {
    if path.is_some_and(|path| path.as_os_str().is_empty()) {
        return Err(ContextError::InvalidRequest {
            field: "path",
            reason: "path must not be empty".into(),
        });
    }
    Ok(())
}

fn push_warning(warnings: &mut Vec<ContextWarning>, mut warning: ContextWarning) {
    warning.message = bounded_message(warning.message);
    if warnings.len() < MAX_WARNINGS {
        warnings.push(warning);
    } else if warnings
        .last()
        .is_none_or(|warning| warning.code != ContextWarningCode::WarningsTruncated)
    {
        warnings[MAX_WARNINGS - 1] = ContextWarning {
            code: ContextWarningCode::WarningsTruncated,
            message: "additional context warnings were omitted".into(),
        };
    }
}

fn bounded_message(message: String) -> String {
    if message.len() <= MAX_WARNING_BYTES {
        return message;
    }
    const SUFFIX: &str = "...[truncated]";
    let mut end = MAX_WARNING_BYTES - SUFFIX.len();
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{SUFFIX}", &message[..end])
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    #[cfg(unix)]
    use std::sync::Arc;
    #[cfg(unix)]
    use std::time::Duration;

    #[cfg(unix)]
    use hzr_core::Config;
    #[cfg(unix)]
    use hzr_exec::{ForkCoreConfig, ForkRuntimePaths, PinnedRtkAdapter};
    #[cfg(unix)]
    use hzr_memory::{IcmClient, IcmConfig, IcmTransport};
    use hzr_protocol::{
        CandidateSource, ContextCandidate, ContextWarning, ContextWarningCode, Provenance,
        SearchFallbackCode, SearchMode, SearchStrategy, TokenCount, TokenCountSource,
    };
    #[cfg(unix)]
    use sha2::{Digest, Sha256};

    use super::{
        ContextPlanner, ForkSearchOutput, MAX_WARNING_BYTES, MAX_WARNINGS, PlanRequest,
        exact_identifier, fork_backend_fallback_code, fork_config_fingerprint, fork_search_args,
        fuse, hit_relative_path, parse_fork_search_config, push_warning, repair_snippet_line,
        separate_code_sources,
    };
    use crate::candidate::RetrievedCandidate;

    #[test]
    fn typed_fork_config_rejects_unknown_schema_and_accepts_version_two() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let managed = directory.path().join("grepai");
        fs::write(&managed, "stub").expect("managed executable");
        let config_path = directory.path().join("config.toml");
        let output = serde_json::json!({
            "schema_version": 2,
            "config_path": config_path,
            "config_exists": false,
            "config_sha256": null,
            "config": {"grepai": {"enabled": true, "auto_init": true, "binary_path": null}}
        });
        let parsed = parse_fork_search_config(&output.to_string(), &managed).expect("typed config");
        assert_eq!(parsed.path, directory.path().join("config.toml"));
        assert_eq!(parsed.validation, Ok(()));

        let mut missing_enabled = output.clone();
        missing_enabled["config"]["grepai"]
            .as_object_mut()
            .expect("grepai object")
            .remove("enabled");
        assert!(
            parse_fork_search_config(&missing_enabled.to_string(), &managed)
                .expect_err("missing enabled must fail")
                .contains("missing field `enabled`")
        );

        let mut unknown_field = output.clone();
        unknown_field["unexpected"] = serde_json::json!(true);
        assert!(
            parse_fork_search_config(&unknown_field.to_string(), &managed)
                .expect_err("unknown envelope field must fail")
                .contains("unknown field `unexpected`")
        );

        let mut unsupported = output;
        unsupported["schema_version"] = serde_json::json!(3);
        assert!(
            parse_fork_search_config(&unsupported.to_string(), &managed)
                .expect_err("unknown schema must fail")
                .contains("unsupported fork config schema version 3")
        );
    }

    #[test]
    fn fork_config_fingerprint_changes_on_same_size_rewrite_and_creation() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("config.toml");
        assert_eq!(
            fork_config_fingerprint(&path).expect("missing fingerprint"),
            super::ForkConfigFingerprint(None)
        );
        fs::write(&path, "aa").expect("first config");
        let first = fork_config_fingerprint(&path).expect("first fingerprint");
        fs::write(&path, "bb").expect("second config");
        let second = fork_config_fingerprint(&path).expect("second fingerprint");
        assert_ne!(first, second);
        fs::remove_file(&path).expect("remove config");
        assert_eq!(
            fork_config_fingerprint(&path).expect("removed fingerprint"),
            super::ForkConfigFingerprint(None)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn planner_config_refresh_is_live_and_singleflight() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let workspace_root = directory.path().join("workspace");
        fs::create_dir(&workspace_root).expect("workspace");
        let config_path = directory.path().join("rtk-config.toml");
        let counter_path = directory.path().join("config-invocations");
        let binary = directory.path().join("rtk");
        write_config_probe(&binary, &config_path, &counter_path);

        let mut config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        config.engines.directory = Some(directory.path().to_path_buf());
        config.ensure_layout().expect("HZR data layout");
        let adapter = PinnedRtkAdapter::detect(ForkCoreConfig {
            binary,
            runtime_paths: Some(ForkRuntimePaths::from_data_root(&config.data_dir)),
            probe_timeout_ms: 20_000,
            ..ForkCoreConfig::default()
        })
        .await;
        let planner = Arc::new(ContextPlanner::from_config(
            &config,
            unavailable_memory(directory.path()),
            adapter.runner(),
        ));
        let workspace = hzr_index::Workspace::discover_managed(
            &workspace_root,
            Path::new("git"),
            &config.data_dir,
            Duration::from_secs(3),
        )
        .await
        .expect("managed workspace");

        planner
            .ensure_managed_fork_search_config(&workspace, false)
            .await
            .expect("missing config");
        assert_eq!(config_invocations(&counter_path), 1);
        planner
            .ensure_managed_fork_search_config(&workspace, false)
            .await
            .expect("unchanged config");
        assert_eq!(config_invocations(&counter_path), 1);

        fs::write(&config_path, b"alpha").expect("create config");
        let created = planner
            .ensure_managed_fork_search_config(&workspace, false)
            .await;
        assert!(created.is_ok(), "created config: {created:?}");
        assert_eq!(config_invocations(&counter_path), 2);
        fs::write(&config_path, b"bravo").expect("same-size config change");
        planner
            .ensure_managed_fork_search_config(&workspace, false)
            .await
            .expect("same-size changed config");
        assert_eq!(config_invocations(&counter_path), 3);
        fs::remove_file(&config_path).expect("remove config");
        planner
            .ensure_managed_fork_search_config(&workspace, false)
            .await
            .expect("removed config");
        assert_eq!(config_invocations(&counter_path), 4);

        fs::write(&config_path, b"charlie").expect("concurrent config change");
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let planner = Arc::clone(&planner);
            let workspace = workspace.clone();
            tasks.spawn(async move {
                planner
                    .ensure_managed_fork_search_config(&workspace, false)
                    .await
            });
        }
        while let Some(result) = tasks.join_next().await {
            result.expect("config task").expect("concurrent config");
        }
        assert_eq!(
            config_invocations(&counter_path),
            5,
            "one subprocess must refresh all concurrent callers"
        );
        planner.shutdown().await.expect("index shutdown");
    }

    #[cfg(unix)]
    fn unavailable_memory(root: &Path) -> IcmClient {
        let mut config = IcmConfig::from_data_root(root.join("missing-icm"), root.join("icm"));
        config.bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 65_534);
        config.request_timeout = Duration::from_millis(50);
        config.cli_fallback = false;
        config.transport = IcmTransport::Http;
        IcmClient::from_config(config).expect("ICM client fixture")
    }

    #[cfg(unix)]
    fn config_invocations(path: &Path) -> u64 {
        fs::read_to_string(path)
            .expect("config invocation counter")
            .parse()
            .expect("numeric config invocation counter")
    }

    #[cfg(unix)]
    fn write_config_probe(path: &Path, config_path: &Path, counter_path: &Path) {
        let digest = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
        let config_json = serde_json::to_string(config_path).expect("JSON config path");
        let config_shell = config_path.to_string_lossy();
        let counter_shell = counter_path.to_string_lossy();
        assert!(!config_shell.contains('\''));
        assert!(!counter_shell.contains('\''));
        let script = r#"#!/bin/sh
case "$1" in
  --version)
    printf '%s\n' 'rtk 0.44.1-fork.1'
    ;;
  rewrite)
    [ "$2" = "--help" ] && printf '%s\n' 'rtk rewrite - Raw command to rewrite' || exit 64
    ;;
  proxy)
    [ "$2" = "--help" ] && printf '%s\n' 'rtk proxy - execute without filtering' || exit 64
    ;;
  config)
    [ "$2" = "--format" ] && [ "$3" = "json" ] || exit 69
    count=0
    [ -f '__COUNTER_SHELL__' ] && count=$(cat '__COUNTER_SHELL__')
    count=$((count + 1))
    printf '%s' "$count" > '__COUNTER_SHELL__'
    if [ ! -f '__CONFIG_SHELL__' ]; then
      printf '%s\n' '{"schema_version":2,"config_path":__CONFIG_JSON__,"config_exists":false,"config_sha256":null,"config":{"grepai":{"enabled":true,"auto_init":true,"binary_path":null}}}'
      exit 0
    fi
    content=$(cat '__CONFIG_SHELL__')
    case "$content" in
      alpha) digest='__ALPHA_DIGEST__' ;;
      bravo) digest='__BRAVO_DIGEST__' ;;
      charlie) digest='__CHARLIE_DIGEST__' ;;
      *) exit 70 ;;
    esac
    printf '{"schema_version":2,"config_path":%s,"config_exists":true,"config_sha256":"%s","config":{"grepai":{"enabled":true,"auto_init":true,"binary_path":null}}}\n' '__CONFIG_JSON__' "$digest"
    ;;
  *)
    exit 67
    ;;
esac
"#
        .replace("__CONFIG_JSON__", &config_json)
        .replace("__CONFIG_SHELL__", &config_shell)
        .replace("__COUNTER_SHELL__", &counter_shell)
        .replace("__ALPHA_DIGEST__", &digest(b"alpha"))
        .replace("__BRAVO_DIGEST__", &digest(b"bravo"))
        .replace("__CHARLIE_DIGEST__", &digest(b"charlie"));
        fs::write(path, script).expect("fake rtk script");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("fake rtk permissions");
    }

    #[test]
    fn test_exact_identifier_routes_symbol_shaped_intent() {
        assert_eq!(
            exact_identifier("inspect classify_workspace_binding before editing"),
            Some("classify_workspace_binding")
        );
        assert_eq!(
            exact_identifier("explain how workspace binding works"),
            None
        );
    }

    #[test]
    fn acceptance_gate_fork_backend_metadata_is_closed_and_drives_typed_fallback() {
        let output: ForkSearchOutput = serde_json::from_value(serde_json::json!({
            "query": "payload omitted",
            "total_hits": 0,
            "backend": "rg",
            "hits": []
        }))
        .expect("typed fork search output");
        let strategy = output.backend.expect("backend").strategy();

        assert_eq!(strategy, SearchStrategy::ForkRgaiRipgrep);
        assert_eq!(
            fork_backend_fallback_code(SearchMode::Semantic, strategy),
            Some(SearchFallbackCode::GrepaiUnavailable)
        );
        assert!(
            serde_json::from_value::<ForkSearchOutput>(serde_json::json!({
                "query": "payload omitted",
                "total_hits": 0,
                "backend": "arbitrary-shell-backend",
                "hits": []
            }))
            .is_err(),
            "backend attribution must reject free-form values"
        );
    }

    /// `--path` is how an agent narrows a search, and a single file is the most natural way
    /// to narrow one — it is what the native Grep tool accepts. Exact mode passed the path
    /// through as the fork's *project root*, so any file path failed with
    /// "project root is not a directory" surfaced as an opaque HTTP 503, while the same
    /// query in semantic mode worked because only that branch pinned the root.
    #[test]
    fn test_exact_search_scoped_to_a_single_file_pins_the_project_root() {
        for mode in [SearchMode::Exact, SearchMode::Semantic, SearchMode::Auto] {
            let args = fork_search_args(
                "needle",
                std::path::Path::new("crates/hzr-cli/src/mcp.rs"),
                std::path::Path::new("/repo"),
                mode,
                10,
                false,
            );

            let root = args
                .iter()
                .position(|argument| argument == "--project-root")
                .map(|index| args[index + 1].as_str());
            assert_eq!(
                root,
                Some("/repo"),
                "{mode:?} must pin the workspace root so --path can be a file"
            );
            let path = args
                .iter()
                .position(|argument| argument == "--path")
                .map(|index| args[index + 1].as_str());
            assert_eq!(
                path,
                Some("crates/hzr-cli/src/mcp.rs"),
                "{mode:?} must keep the requested scope as the search path"
            );
        }
    }

    /// Semantic search returned source truncated mid-token. grepai chunks are byte windows,
    /// so the first line of a chunk can begin mid-identifier, and that fragment was emitted
    /// as a `SearchLine` — a field whose whole meaning is "the text of this line". Observed
    /// live: line 194 of `hook_runner.rs` came back as `en(Value::as_str) else {`, the tail
    /// of `.and_then(Value::as_str) else {`. Code that looks real and does not parse is worse
    /// for a model than no code at all.
    #[test]
    fn test_a_chunk_boundary_fragment_is_completed_from_the_recorded_line() {
        let actual = "    let Some(prompt) = input.pointer(\"/tool_input/prompt\").and_then(Value::as_str) else {";

        assert_eq!(
            repair_snippet_line("en(Value::as_str) else {", Some(actual)),
            actual.trim(),
            "a fragment of the recorded line must be completed to the whole line"
        );
    }

    /// The fork reports hit paths relative to `--path`, not to the project root, so scoping a
    /// search silently corrupted the one field an agent has to act on: scoping to `src`
    /// reported `lib.rs`, which does not exist at the root, and scoping to a single file
    /// reported the empty string, which normalized to `.`. A hit an agent cannot open is not
    /// a hit.
    #[test]
    fn test_a_scoped_hit_is_reported_relative_to_the_project_root() {
        use std::path::Path;

        assert_eq!(
            hit_relative_path(Path::new("."), "crates/hzr-cli/src/mcp.rs"),
            Path::new("crates/hzr-cli/src/mcp.rs"),
            "an unscoped search already reports root-relative paths"
        );
        assert_eq!(
            hit_relative_path(Path::new("src"), "lib.rs"),
            Path::new("src/lib.rs"),
            "a directory scope must keep its prefix"
        );
        assert_eq!(
            hit_relative_path(Path::new("src/lib.rs"), ""),
            Path::new("src/lib.rs"),
            "a file scope reports no sub-path, and the file itself is the hit"
        );
    }

    /// Repair must be provably safe. The index can be older than the file, so a fragment is
    /// only completed when it really is part of the line the engine pointed at; otherwise the
    /// engine's text is preserved rather than replaced with an unrelated line.
    #[test]
    fn test_repair_never_invents_a_line_it_cannot_verify() {
        let actual = "    let total = compute(input);";

        assert_eq!(
            repair_snippet_line("fn something_else() {", Some(actual)),
            "fn something_else() {",
            "a fragment that is not part of the recorded line must be left untouched"
        );
        assert_eq!(
            repair_snippet_line("let total = compute(input);", Some(actual)),
            "let total = compute(input);",
            "an already-complete line must not change"
        );
        assert_eq!(
            repair_snippet_line("let total = compute(input);", None),
            "let total = compute(input);",
            "an unreadable file must not lose the engine's text"
        );
        assert_eq!(
            repair_snippet_line("", Some(actual)),
            "",
            "an empty fragment carries no proof of belonging to this line"
        );
    }

    /// The exact/ranked distinction must survive the fix: `--literal` is what makes exact
    /// mode exact, and it must not leak into the ranked modes.
    #[test]
    fn test_only_exact_mode_asks_the_fork_for_a_literal_match() {
        let exact = fork_search_args(
            "fn handle_request",
            std::path::Path::new("."),
            std::path::Path::new("/repo"),
            SearchMode::Exact,
            10,
            true,
        );
        assert!(exact.iter().any(|argument| argument == "--literal"));
        assert!(
            !exact.iter().any(|argument| argument == "--compact"),
            "include_content must suppress --compact"
        );

        let ranked = fork_search_args(
            "handle a request",
            std::path::Path::new("."),
            std::path::Path::new("/repo"),
            SearchMode::Auto,
            10,
            false,
        );
        assert!(!ranked.iter().any(|argument| argument == "--literal"));
        assert!(ranked.iter().any(|argument| argument == "--compact"));
    }

    fn retrieved(
        id: &str,
        source: CandidateSource,
        content: &str,
        tokens: u64,
    ) -> RetrievedCandidate {
        let content_ref = format!("sha256:{id}");
        RetrievedCandidate {
            candidate: ContextCandidate {
                id: id.into(),
                source,
                content_ref: content_ref.clone(),
                path: Some(format!("{id}.rs")),
                symbol: None,
                symbol_unavailable_reason: Some(
                    hzr_protocol::SymbolUnavailableReason::OutlineUnavailable,
                ),
                line_start: Some(1),
                line_end: Some(1),
                source_rank: 1,
                relevance: 0.0,
                tokens: TokenCount::estimate(tokens),
                freshness: "fresh".into(),
                trust: "workspace".into(),
                provenance: Provenance {
                    source: "test".into(),
                    content_hash: id.into(),
                    generation: Some("generation".into()),
                    canonical_ref: None,
                    derived_by: None,
                },
            },
            content: content.into(),
        }
    }

    #[test]
    fn test_fuse_returns_only_content_selected_within_hard_limit() {
        let response = fuse(
            100,
            vec![(
                1.0,
                vec![
                    retrieved("first", CandidateSource::Context, "first content", 70),
                    retrieved("second", CandidateSource::Context, "second content", 60),
                ],
            )],
            Vec::new(),
            None,
        )
        .expect("fusion succeeds");
        assert!(response.pack.used.value <= 100);
        assert_eq!(response.pack.used.source, TokenCountSource::Estimate);
        assert_eq!(response.pack.selected.len(), 1);
        assert_eq!(response.contents.len(), 1);
    }

    #[test]
    fn regression_exact_and_graph_scores_keep_independent_calibration_domains() {
        let plan = crate::candidate::NormalizedSource {
            candidates: vec![retrieved(
                "graph",
                CandidateSource::Context,
                "graph evidence",
                20,
            )],
            warnings: Vec::new(),
        };
        let exact = crate::candidate::NormalizedSource {
            candidates: vec![retrieved(
                "exact",
                CandidateSource::Exact,
                "exact evidence",
                20,
            )],
            warnings: Vec::new(),
        };
        let sources = separate_code_sources(plan, Some(exact));
        let response = fuse(
            100,
            sources
                .into_iter()
                .map(|source| (1.0, source.candidates))
                .collect(),
            Vec::new(),
            None,
        )
        .expect("fusion succeeds");

        assert!(
            response
                .pack
                .selected
                .iter()
                .any(|candidate| candidate.source == CandidateSource::Context),
            "graph evidence must not be suppressed by exact-search score scale"
        );
        assert!(
            response
                .pack
                .selected
                .iter()
                .any(|candidate| candidate.source == CandidateSource::Exact),
            "exact evidence remains independently calibrated"
        );
    }

    #[test]
    fn test_warning_payload_is_bounded_and_reports_truncation() {
        let mut warnings = Vec::new();
        for index in 0..=MAX_WARNINGS {
            push_warning(
                &mut warnings,
                ContextWarning {
                    code: ContextWarningCode::ContentUnavailable,
                    message: format!("{index}:{}", "x".repeat(MAX_WARNING_BYTES * 2)),
                },
            );
        }
        assert_eq!(warnings.len(), MAX_WARNINGS);
        assert!(
            warnings
                .iter()
                .all(|warning| warning.message.len() <= MAX_WARNING_BYTES)
        );
        assert_eq!(
            warnings[MAX_WARNINGS - 1].code,
            ContextWarningCode::WarningsTruncated
        );
    }

    #[test]
    fn test_plan_request_rejects_unbounded_retrieval() {
        let request = PlanRequest {
            workspace: PathBuf::from("/workspace"),
            intent: "find authentication".into(),
            path: None,
            topic: None,
            search_limit: 101,
            memory_limit: 5,
        };
        assert!(request.validate().is_err());
    }

    #[test]
    fn test_plan_request_rejects_non_canonical_memory_kind() {
        for topic in ["Architecture", "project_notes", "foo--bar"] {
            let request = PlanRequest {
                workspace: PathBuf::from("/workspace"),
                intent: "find authentication".into(),
                path: None,
                topic: Some(topic.into()),
                search_limit: 5,
                memory_limit: 5,
            };

            assert!(request.validate().is_err(), "accepted {topic:?}");
        }
    }

    #[test]
    fn test_fork_search_config_rejects_disabled_delegation() {
        let output = serde_json::json!({
            "schema_version": 2,
            "config_path": "/tmp/config.toml",
            "config_exists": true,
            "config_sha256": "00".repeat(32),
            "config": {"grepai": {"enabled": false, "auto_init": true, "binary_path": null}}
        });
        let parsed =
            parse_fork_search_config(&output.to_string(), PathBuf::from("grepai").as_path())
                .expect("typed config");
        assert!(parsed.validation.is_err());
    }

    #[test]
    fn test_fork_search_config_rejects_foreign_binary_override() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let managed = directory.path().join("managed-grepai");
        let foreign = directory.path().join("foreign-grepai");
        fs::write(&managed, b"managed").expect("managed fixture");
        fs::write(&foreign, b"foreign").expect("foreign fixture");
        let output = serde_json::json!({
            "schema_version": 2,
            "config_path": "/tmp/config.toml",
            "config_exists": true,
            "config_sha256": "00".repeat(32),
            "config": {
                "grepai": {"enabled": true, "auto_init": true, "binary_path": foreign}
            }
        });
        let parsed = parse_fork_search_config(&output.to_string(), &managed).expect("typed config");
        assert!(parsed.validation.is_err());
    }
}
