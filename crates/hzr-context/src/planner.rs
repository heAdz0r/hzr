use std::collections::{BTreeMap, btree_map::Entry};
use std::path::{Path, PathBuf};
use std::time::Duration;

use hzr_core::{BudgetPlanner, Config, FusionInput};
use hzr_exec::{
    CaptureConfig, CaptureOverflow, CapturedContent, ExecutionOutcome, ForkCoreInvocation,
    ForkCoreRunner, TerminationCause,
};
use hzr_index::{Deadlines, IndexCoordinator, IndexGeneration, PreparedIndex, Workspace};
use hzr_memory::{
    IcmClient, RecallRequest, isolate_project_memories, namespaced_topic, recall_candidate_limit,
    validate_memory_kind,
};
use hzr_protocol::{
    ContextPlanApiResponse, ContextWarning, ContextWarningCode, ForkPlannerMetadata,
    SearchApiResponse, SearchHit, SearchLine, SearchMode, SearchSnippet, SearchStrategy,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::sync::OnceCell;

use crate::candidate::{
    ForkPlanCandidate, NormalizedSource, RetrievedCandidate, normalize_memory, normalize_plan,
    normalize_search,
};
use crate::error::{ContextError, Result};

const MAX_INTENT_BYTES: usize = 64 * 1024;
const MAX_RETRIEVAL_RESULTS: usize = 100;
const MAX_WARNINGS: usize = 8;
const MAX_WARNING_BYTES: usize = 512;
const SEARCH_CAPTURE_BYTES: usize = 2 * 1024 * 1024;
const PLAN_CAPTURE_BYTES: usize = 8 * 1024 * 1024;
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

pub struct ContextPlanner {
    indexes: IndexCoordinator,
    fork: Option<ForkCoreRunner>,
    fork_unavailable: Option<String>,
    memory: IcmClient,
    hard_token_limit: u64,
    fork_timeout_ms: u64,
    fork_search_config: OnceCell<std::result::Result<(), String>>,
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
            indexes: IndexCoordinator::new(
                config.data_dir.clone(),
                config.engines.binary("git"),
                config.engines.binary("grepai"),
                Deadlines::default(),
                config.engines.auto_index,
            ),
            fork,
            fork_unavailable,
            memory,
            hard_token_limit: config.policy.context_token_limit,
            fork_timeout_ms: config
                .daemon
                .request_timeout_ms
                .saturating_sub(FORK_TIMEOUT_MARGIN_MS)
                .max(1),
            fork_search_config: OnceCell::new(),
        }
    }

    pub async fn search(&self, mut request: SearchRequest) -> Result<SearchApiResponse> {
        request.validate()?;
        let workspace = self.indexes.workspace(&request.workspace).await?;
        let initial_generation = IndexGeneration::read(&workspace)?;
        let (workspace, generation, fallback_reason) = if request.mode == SearchMode::Exact {
            (workspace, initial_generation, None)
        } else {
            match self.prepare_within_request_budget(&workspace).await {
                Ok(prepared) => (prepared.workspace, prepared.generation, None),
                Err(error) => {
                    request.mode = SearchMode::Exact;
                    (
                        workspace,
                        initial_generation,
                        Some(format!(
                            "canonical grepai lifecycle is unavailable; fork rgai used its builtin fallback: {error}"
                        )),
                    )
                }
            }
        };
        let mut response = self.search_in(&workspace, &generation, &request).await?;
        response.fallback_reason = fallback_reason;
        Ok(response)
    }

    pub async fn plan(&self, request: PlanRequest) -> Result<ContextPlanApiResponse> {
        request.validate()?;
        let workspace = self.indexes.workspace(&request.workspace).await?;
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
        let mut sources = Vec::with_capacity(2);
        let mut planner = None;
        match code_result {
            Ok(result) => {
                planner = result.planner;
                result
                    .warnings
                    .into_iter()
                    .for_each(|warning| push_warning(&mut warnings, warning));
                if let Some(source) = result.source {
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
            return Ok(CodePlanResult {
                source: Some(normalize_plan(
                    plan.selected
                        .into_iter()
                        .take(request.search_limit)
                        .collect(),
                    workspace,
                    &generation.generation,
                    plan.pipeline_version.as_deref(),
                )?),
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
            .search_in(workspace, &generation, &search_request)
            .await
        {
            Ok(response) => Some(normalize_search(response, &generation.generation)),
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
            source,
            warnings,
            planner,
        })
    }

    async fn run_memory_plan(
        &self,
        workspace: &Workspace,
        request: &PlanRequest,
    ) -> Result<ForkPlanOutput> {
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
                ".".into(),
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
        )
        .await
    }

    async fn search_in(
        &self,
        workspace: &Workspace,
        generation: &IndexGeneration,
        request: &SearchRequest,
    ) -> Result<SearchApiResponse> {
        let filter = workspace.normalize_filter(request.path.as_deref())?;
        let path = filter.as_deref().unwrap_or_else(|| Path::new("."));
        let path_text = path.to_str().ok_or_else(|| ContextError::InvalidRequest {
            field: "path",
            reason: "path must be valid UTF-8".into(),
        })?;
        let mut args = vec![
            "rgai".into(),
            request.query.clone(),
            "--path".into(),
            path_text.into(),
            "--max".into(),
            request.limit.to_string(),
            "--json".into(),
        ];
        let strategy = if request.mode == SearchMode::Exact {
            args.push("--builtin".into());
            SearchStrategy::ForkRgaiBuiltin
        } else {
            self.ensure_managed_fork_search_config(workspace).await?;
            SearchStrategy::ForkRgaiAdaptive
        };
        if !request.include_content {
            args.push("--compact".into());
        }
        let raw: ForkSearchOutput = self
            .run_fork_json(
                args,
                &workspace.identity.root,
                SEARCH_CAPTURE_BYTES,
                "rgai search",
            )
            .await?;
        if let Some(parse_error) = raw.parse_error {
            return Err(ContextError::InvalidForkOutput {
                operation: "rgai search",
                detail: parse_error,
            });
        }
        let hits = raw
            .hits
            .into_iter()
            .take(request.limit)
            .map(|hit| normalize_search_hit(workspace, hit))
            .collect::<Result<Vec<_>>>()?;
        Ok(SearchApiResponse {
            query: raw.query,
            path: path_text.to_owned(),
            total_hits: raw.total_hits,
            shown_hits: hits.len(),
            scanned_files: raw.scanned_files,
            skipped_large: raw.skipped_large,
            skipped_binary: raw.skipped_binary,
            hits,
            strategy,
            index_generation: Some(generation.generation.clone()),
            fallback_reason: None,
        })
    }

    async fn run_fork_json<T: DeserializeOwned>(
        &self,
        args: Vec<String>,
        cwd: &Path,
        max_capture_bytes: usize,
        operation: &'static str,
    ) -> Result<T> {
        let stdout = self
            .run_fork_output(args, cwd, max_capture_bytes, operation)
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
    ) -> Result<Vec<u8>> {
        let runner = self.fork.as_ref().ok_or_else(|| {
            ContextError::ForkUnavailable(
                self.fork_unavailable
                    .clone()
                    .unwrap_or_else(|| "runner was not configured".into()),
            )
        })?;
        let mut invocation = ForkCoreInvocation::new(args);
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

    async fn ensure_managed_fork_search_config(&self, workspace: &Workspace) -> Result<()> {
        let validation = self
            .fork_search_config
            .get_or_init(|| async {
                let output = self
                    .run_fork_output(
                        vec!["config".into()],
                        &workspace.identity.root,
                        256 * 1024,
                        "config inspection",
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                let output = std::str::from_utf8(&output)
                    .map_err(|error| format!("fork config output is not UTF-8: {error}"))?;
                validate_fork_search_config(output, self.indexes.grepai_binary())
            })
            .await;
        validation.clone().map_err(ContextError::ForkUnavailable)
    }
}

struct CodePlanResult {
    source: Option<NormalizedSource>,
    warnings: Vec<ContextWarning>,
    planner: Option<ForkPlannerMetadata>,
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

fn normalize_search_hit(workspace: &Workspace, hit: ForkSearchHit) -> Result<SearchHit> {
    let path = workspace.normalize_result(Path::new(&hit.path))?;
    let path = path
        .to_str()
        .ok_or_else(|| ContextError::InvalidForkOutput {
            operation: "rgai search",
            detail: "result path is not valid UTF-8".into(),
        })?
        .to_owned();
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
                    Ok(SearchLine {
                        line: line_number,
                        text: line.text,
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

fn validate_fork_search_config(
    output: &str,
    managed_grepai: &Path,
) -> std::result::Result<(), String> {
    let config = output
        .lines()
        .skip_while(|line| !line.trim_start().starts_with('['))
        .collect::<Vec<_>>()
        .join("\n");
    if config.is_empty() {
        return Err("fork config output did not contain TOML".into());
    }
    let config: toml::Value = toml::from_str(&config)
        .map_err(|error| format!("fork config output contains invalid TOML: {error}"))?;
    let Some(grepai) = config.get("grepai") else {
        return Ok(());
    };
    if grepai
        .get("enabled")
        .and_then(toml::Value::as_bool)
        .is_some_and(|enabled| !enabled)
    {
        return Err("fork grepai delegation is disabled by the user RTK config".into());
    }
    let Some(custom) = grepai.get("binary_path").and_then(toml::Value::as_str) else {
        return Ok(());
    };
    let custom = canonical_executable(Path::new(custom))?;
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
    use std::path::PathBuf;

    use hzr_protocol::{
        CandidateSource, ContextCandidate, ContextWarning, ContextWarningCode, Provenance,
        TokenCount, TokenCountSource,
    };

    use super::{
        MAX_WARNING_BYTES, MAX_WARNINGS, PlanRequest, fuse, push_warning,
        validate_fork_search_config,
    };
    use crate::candidate::RetrievedCandidate;

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
        let result = validate_fork_search_config(
            "Config: /tmp/config.toml\n\n[grepai]\nenabled = false\n",
            PathBuf::from("grepai").as_path(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_fork_search_config_rejects_foreign_binary_override() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let managed = directory.path().join("managed-grepai");
        let foreign = directory.path().join("foreign-grepai");
        fs::write(&managed, b"managed").expect("managed fixture");
        fs::write(&foreign, b"foreign").expect("foreign fixture");
        let output = format!(
            "Config: /tmp/config.toml\n\n[grepai]\nenabled = true\nbinary_path = {:?}\n",
            foreign.to_string_lossy()
        );
        let result = validate_fork_search_config(&output, &managed);
        assert!(result.is_err());
    }
}
