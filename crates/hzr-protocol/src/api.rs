use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{CodecProfile, ContextPack, FidelityClass, RiskClass, Usage};

fn default_search_limit() -> usize {
    10
}

fn default_memory_limit() -> usize {
    5
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    Exact,
    Semantic,
    #[default]
    Auto,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchStrategy {
    ForkRgaiAdaptive,
    ForkRgaiBuiltin,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchLine {
    pub line: u32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchSnippet {
    pub lines: Vec<SearchLine>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_terms: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub path: String,
    pub score: f64,
    pub matched_lines: usize,
    pub snippets: Vec<SearchSnippet>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchApiResponse {
    pub query: String,
    pub path: String,
    pub total_hits: usize,
    pub shown_hits: usize,
    pub scanned_files: usize,
    pub skipped_large: usize,
    pub skipped_binary: usize,
    pub hits: Vec<SearchHit>,
    pub strategy: SearchStrategy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_generation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchApiRequest {
    pub workspace: String,
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
    #[serde(default)]
    pub mode: SearchMode,
    #[serde(default)]
    pub include_content: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContextPlanApiRequest {
    pub workspace: String,
    pub intent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default = "default_search_limit")]
    pub search_limit: usize,
    #[serde(default = "default_memory_limit")]
    pub memory_limit: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextWarningCode {
    PlannerFallback,
    PlannerUnavailable,
    SearchDegraded,
    SearchUnavailable,
    MemoryUnavailable,
    ContentUnavailable,
    WarningsTruncated,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForkPlannerMetadata {
    pub pipeline_version: Option<String>,
    pub semantic_backend_used: Option<String>,
    pub graph_candidate_count: Option<usize>,
    pub semantic_hit_count: Option<usize>,
    pub candidates_total: usize,
    pub candidates_selected: usize,
    pub estimated_tokens_used: u64,
    pub token_budget: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContextWarning {
    pub code: ContextWarningCode,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextPlanApiResponse {
    pub pack: ContextPack,
    pub contents: BTreeMap<String, String>,
    pub warnings: Vec<ContextWarning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner: Option<ForkPlannerMetadata>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecallApiRequest {
    pub workspace: String,
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
    /// Which namespaces the recall may reach. Defaults to project plus global so an
    /// agent sees standing preferences alongside this repository's history.
    #[serde(default)]
    pub scope: MemoryScopeSelector,
}

/// Wire form of the memory namespace. Kept in the protocol rather than inferred, so a
/// client's scope choice is explicit and auditable in the request.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScopeSelector {
    /// Only the current repository.
    Project,
    /// Only user-global records.
    Global,
    #[default]
    ProjectAndGlobal,
}

/// A write targets exactly one namespace — `project_and_global` is meaningless for a
/// store, and allowing it would duplicate the same fact into two namespaces.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryWriteScope {
    #[default]
    Project,
    Global,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryImportance {
    Critical,
    High,
    #[default]
    Medium,
    Low,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryStoreApiRequest {
    pub workspace: String,
    pub topic: String,
    pub content: String,
    /// Where the record belongs. Project by default: a fact is repository-scoped unless
    /// the caller deliberately states it is a user-wide preference or rule.
    #[serde(default)]
    pub scope: MemoryWriteScope,
    #[serde(default)]
    pub importance: MemoryImportance,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryForgetApiRequest {
    pub workspace: String,
    pub id: String,
    #[serde(default)]
    pub scope: MemoryWriteScope,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryUpdateApiRequest {
    pub workspace: String,
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub scope: MemoryWriteScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importance: Option<MemoryImportance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryPruneApiRequest {
    pub workspace: String,
    pub threshold: f32,
    #[serde(default = "default_true")]
    pub dry_run: bool,
    #[serde(default)]
    pub scope: MemoryWriteScope,
}

const fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryMutationApiResponse {
    pub affected_ids: Vec<String>,
    pub dry_run: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecApiRequest {
    pub cwd: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecApprovalApiRequest {
    pub decision_id: String,
    pub approved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForkRunApiRequest {
    pub cwd: String,
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandTermination {
    Exited,
    Signaled,
    TimedOut,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForkRunApiResponse {
    pub stdout: String,
    pub stderr: String,
    pub termination: CommandTermination,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub duration_ms: u64,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UsageApiRequest {
    pub trace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub usage: Usage,
    pub turns: u32,
    #[serde(default)]
    pub retries: u32,
    pub latency_ms: u64,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_microusd: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UsageApiResponse {
    pub recorded: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardState {
    Ready,
    Degraded,
    Rebuilding,
    Standby,
    Stopped,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardService {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub state: DashboardState,
    pub detail: String,
    pub command: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardProjectState {
    Ready,
    Warming,
    Registered,
    Unavailable,
    Degraded,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardProjectArtifacts {
    pub config_present: bool,
    pub vectors_present: bool,
    pub symbols_present: bool,
    pub repository_graph_present: bool,
    pub size_bytes: u64,
    pub modified_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardProject {
    pub name: String,
    pub root: String,
    pub repository_id: String,
    pub worktree_id: String,
    pub git_backed: bool,
    pub linked_worktree: bool,
    pub state: DashboardProjectState,
    pub registered_at_ms: u64,
    pub last_seen_at_ms: u64,
    pub artifacts: DashboardProjectArtifacts,
    pub command: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardObservedUsage {
    pub tasks: u64,
    pub accepted: u64,
    pub actual_input_tokens: u64,
    pub actual_output_tokens: u64,
    pub estimated_input_tokens: u64,
    pub cost_microusd: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DashboardEstimatedEfficiency {
    pub operations: u64,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub gross_avoided_tokens_estimated: u64,
    pub regression_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub reduction_pct: f64,
    pub total_execution_ms: u64,
    pub measurement: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardMemoryRetrieval {
    Hybrid,
    Fts5,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DashboardMemoryTopic {
    pub id: String,
    pub label: String,
    pub memory_count: usize,
    pub average_weight: f64,
    pub newest_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardMemoryEdge {
    pub source: String,
    pub target: String,
    pub relationship_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DashboardMemoryObservatory {
    pub state: DashboardState,
    pub project: Option<String>,
    pub retrieval: DashboardMemoryRetrieval,
    pub observed_at_ms: u64,
    pub latency_ms: u64,
    pub transport: String,
    pub source: String,
    pub memory_count: usize,
    pub visible_memory_count: usize,
    pub hidden_memory_count: usize,
    pub topics: Vec<DashboardMemoryTopic>,
    pub edges: Vec<DashboardMemoryEdge>,
    pub truncated: bool,
    pub diagnostic_command: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DashboardMemoryTopicDetails {
    pub id: String,
    pub label: String,
    pub memory_count: usize,
    pub visible_memory_count: usize,
    pub hidden_memory_count: usize,
    pub truncated: bool,
    pub memories: Vec<DashboardMemoryDetail>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DashboardMemoryDetail {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_accessed: Option<String>,
    pub access_count: u64,
    pub weight: f64,
    pub summary: String,
    pub raw_excerpt: Option<String>,
    pub keywords: Vec<String>,
    pub importance: String,
    pub source_type: Option<String>,
    pub source_data: Option<String>,
    pub related_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardIndexArtifacts {
    pub initialized: bool,
    pub vectors_present: bool,
    pub symbols_present: bool,
    pub repository_graph_present: bool,
    pub size_bytes: u64,
    pub modified_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardIndexWatcher {
    pub state: DashboardState,
    pub pid: Option<u32>,
    pub uptime_ms: Option<u64>,
    pub owned_by_hzr: bool,
    pub ready_marker_observed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardSemanticCanary {
    pub state: DashboardState,
    pub checked_at_ms: Option<u64>,
    pub latency_ms: u64,
    pub query: String,
    pub total_hits: usize,
    pub shown_hits: usize,
    pub scanned_files: usize,
    pub strategy: Option<String>,
    pub backend: Option<String>,
    pub generation: Option<String>,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardIndexObservatory {
    pub state: DashboardState,
    pub project: Option<String>,
    pub observed_at_ms: u64,
    pub generation: Option<String>,
    pub config_fingerprint: Option<String>,
    pub artifacts: DashboardIndexArtifacts,
    pub watcher: DashboardIndexWatcher,
    pub semantic: DashboardSemanticCanary,
    pub diagnostic_command: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardLocalActivity {
    pub project: Option<String>,
    pub operations: u64,
    pub optimized_operations: u64,
    pub raw_operations: u64,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub gross_avoided_tokens_estimated: u64,
    pub regression_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub total_execution_ms: u64,
    pub first_record_at: Option<String>,
    pub last_record_at: Option<String>,
    pub unscoped_operations: u64,
    pub measurement: String,
    pub recent_operations: Vec<DashboardLocalOperation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardOperationRoute {
    Optimized,
    Raw,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardLocalOperation {
    pub ledger_id: u64,
    pub timestamp: String,
    pub operation: String,
    pub route: DashboardOperationRoute,
    pub original_command: String,
    pub recorded_command: String,
    pub working_directory: String,
    pub agent: Option<String>,
    pub session_id: Option<String>,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub execution_ms: u64,
    pub replacement: Option<String>,
    pub rationale: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardProviderReceiptState {
    Available,
    NoReceipts,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardProviderReceipts {
    pub state: DashboardProviderReceiptState,
    pub records: u64,
    pub accepted: u64,
    pub actual_input_tokens: u64,
    pub actual_output_tokens: u64,
    pub cost_microusd: u64,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardHelpCommand {
    pub label: String,
    pub description: String,
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DashboardResponse {
    pub protocol_version: u16,
    pub hzr_version: String,
    pub visualizer_version: String,
    pub generated_at_ms: u64,
    pub uptime_ms: u64,
    pub daemon_endpoint: String,
    pub overall_state: DashboardState,
    pub services: Vec<DashboardService>,
    pub projects: Vec<DashboardProject>,
    pub registry_warnings: usize,
    pub observed_usage: DashboardObservedUsage,
    pub estimated_efficiency: DashboardEstimatedEfficiency,
    pub memory_observatory: DashboardMemoryObservatory,
    pub index_observatory: DashboardIndexObservatory,
    pub local_activity: DashboardLocalActivity,
    pub provider_receipts: DashboardProviderReceipts,
    pub help: Vec<DashboardHelpCommand>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodecApiRequest {
    pub content: String,
    #[serde(default)]
    pub fidelity: FidelityClass,
    #[serde(default)]
    pub risk: RiskClass,
    #[serde(default)]
    pub profile: CodecProfile,
}

#[cfg(test)]
mod tests {
    use super::{
        ContextPlanApiRequest, MemoryImportance, MemoryPruneApiRequest, MemoryRecallApiRequest,
        MemoryStoreApiRequest, SearchApiRequest, SearchMode,
    };

    #[test]
    fn test_search_defaults_are_bounded_and_auto() {
        let request: SearchApiRequest =
            serde_json::from_str(r#"{"workspace":"/repo","query":"authentication flow"}"#)
                .expect("search request parses");

        assert_eq!(request.limit, 10);
        assert_eq!(request.mode, SearchMode::Auto);
        assert!(!request.include_content);
        assert_eq!(MemoryImportance::default(), MemoryImportance::Medium);
    }

    #[test]
    fn test_context_plan_defaults_bound_both_retrieval_sources() {
        let request: ContextPlanApiRequest =
            serde_json::from_str(r#"{"workspace":"/repo","intent":"authentication flow"}"#)
                .expect("context plan request parses");

        assert_eq!(request.search_limit, 10);
        assert_eq!(request.memory_limit, 5);
        assert!(request.path.is_none());
        assert!(request.topic.is_none());
    }

    #[test]
    fn test_memory_requests_require_workspace_and_reject_project_override() {
        assert!(serde_json::from_str::<MemoryRecallApiRequest>(r#"{"query":"decision"}"#).is_err());
        assert!(
            serde_json::from_str::<MemoryStoreApiRequest>(
                r#"{"topic":"context","content":"decision"}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<MemoryRecallApiRequest>(
                r#"{"workspace":"/repo","query":"decision","project":"foreign"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn test_memory_prune_defaults_to_preview() {
        let request: MemoryPruneApiRequest =
            serde_json::from_str(r#"{"workspace":"/repo","threshold":0.1}"#)
                .expect("memory prune request parses");

        assert!(request.dry_run);
    }
}
