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
    #[serde(default)]
    pub importance: MemoryImportance,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
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
        ContextPlanApiRequest, MemoryImportance, MemoryRecallApiRequest, MemoryStoreApiRequest,
        SearchApiRequest, SearchMode,
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
}
