use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

mod api;

pub use api::{
    CodecApiRequest, CommandTermination, ContextPlanApiRequest, ContextPlanApiResponse,
    ContextWarning, ContextWarningCode, DashboardEstimatedEfficiency, DashboardHelpCommand,
    DashboardIndexArtifacts, DashboardIndexObservatory, DashboardIndexWatcher,
    DashboardLocalActivity, DashboardLocalOperation, DashboardMemoryDetail, DashboardMemoryEdge,
    DashboardMemoryObservatory, DashboardMemoryRetrieval, DashboardMemoryTopic,
    DashboardMemoryTopicDetails, DashboardObservedUsage, DashboardOperationRoute, DashboardProject,
    DashboardProjectArtifacts, DashboardProjectState, DashboardProviderReceiptState,
    DashboardProviderReceipts, DashboardResponse, DashboardSearchActivity, DashboardService,
    DashboardState, ExecApiRequest, ExecApprovalApiRequest, ForkPlannerMetadata, ForkRunApiRequest,
    ForkRunApiResponse, MemoryForgetApiRequest, MemoryImportance, MemoryMutationApiResponse,
    MemoryPruneApiRequest, MemoryRecallApiRequest, MemoryScopeSelector, MemoryStoreApiRequest,
    MemoryUpdateApiRequest, MemoryWriteScope, SearchApiRequest, SearchApiResponse, SearchHit,
    SearchLine, SearchMode, SearchSnippet, SearchStrategy, UsageApiRequest, UsageApiResponse,
};

pub const PROTOCOL_VERSION: u16 = 1;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7().to_string())
            }

            pub fn from_string(value: String) -> Self {
                Self(value)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

id_type!(RequestId);
id_type!(TraceId);
id_type!(SessionId);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FidelityClass {
    Exact,
    LosslessStructural,
    #[default]
    Semantic,
    Summary,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyClass {
    Public,
    Internal,
    #[default]
    Confidential,
    Secret,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    #[default]
    Low,
    Medium,
    High,
    Irreversible,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodecProfile {
    Off,
    Safe,
    #[default]
    Adaptive,
    Compact,
    Shadow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenCountSource {
    Provider,
    ModelTokenizer,
    Estimate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TokenCount {
    pub value: u64,
    pub source: TokenCountSource,
}

impl TokenCount {
    pub const fn provider(value: u64) -> Self {
        Self {
            value,
            source: TokenCountSource::Provider,
        }
    }

    pub const fn tokenizer(value: u64) -> Self {
        Self {
            value,
            source: TokenCountSource::ModelTokenizer,
        }
    }

    pub const fn estimate(value: u64) -> Self {
        Self {
            value,
            source: TokenCountSource::Estimate,
        }
    }

    pub const fn is_actual(self) -> bool {
        matches!(
            self.source,
            TokenCountSource::Provider | TokenCountSource::ModelTokenizer
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TokenBudget {
    pub input_limit: u64,
    pub output_reserve: u64,
    pub safety_margin: u64,
}

impl TokenBudget {
    pub fn available_for_dynamic_input(&self, fixed_input: u64) -> u64 {
        self.input_limit
            .saturating_sub(self.output_reserve)
            .saturating_sub(self.safety_margin)
            .saturating_sub(fixed_input)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub source: String,
    pub content_hash: String,
    pub generation: Option<String>,
    pub canonical_ref: Option<String>,
    pub derived_by: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProtectedSpan {
    pub start: usize,
    pub end: usize,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub protocol_version: u16,
    pub request_id: RequestId,
    pub trace_id: TraceId,
    pub session_id: SessionId,
    pub workspace_id: String,
    pub worktree_id: String,
    pub deadline_ms: u64,
    pub model_id: Option<String>,
    pub tokenizer_id: Option<String>,
    pub privacy: PrivacyClass,
    pub fidelity: FidelityClass,
    pub risk: RiskClass,
    pub budget: TokenBudget,
    pub provenance: Vec<Provenance>,
    pub payload: T,
}

impl<T> Envelope<T> {
    pub fn new(workspace_id: String, worktree_id: String, budget: TokenBudget, payload: T) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id: RequestId::new(),
            trace_id: TraceId::new(),
            session_id: SessionId::new(),
            workspace_id,
            worktree_id,
            deadline_ms: 30_000,
            model_id: None,
            tokenizer_id: None,
            privacy: PrivacyClass::default(),
            fidelity: FidelityClass::default(),
            risk: RiskClass::default(),
            budget,
            provenance: Vec::new(),
            payload,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Intent {
    pub raw: String,
    pub normalized: Option<String>,
    pub retrieval_variant: Option<String>,
    pub explicit_paths: Vec<String>,
    pub symbols: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateSource {
    Exact,
    Index,
    Context,
    Memory,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextCandidate {
    pub id: String,
    pub source: CandidateSource,
    pub content_ref: String,
    pub path: Option<String>,
    pub symbol: Option<String>,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
    pub source_rank: u32,
    pub relevance: f32,
    pub tokens: TokenCount,
    pub freshness: String,
    pub trust: String,
    pub provenance: Provenance,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextPack {
    pub selected: Vec<ContextCandidate>,
    pub rejected: Vec<CandidateDecision>,
    pub used: TokenCount,
    pub hard_limit: u64,
    pub coverage: f32,
    pub confidence: f32,
    pub budget_exceeded: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CandidateDecision {
    pub candidate_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActualUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct EstimatedUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub method: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub actual: ActualUsage,
    pub estimated: EstimatedUsage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineState {
    Ready,
    Degraded,
    Rebuilding,
    Stopped,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EngineHealth {
    pub name: String,
    pub version: Option<String>,
    pub state: EngineState,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub protocol_version: u16,
    pub hzr_version: String,
    pub state: EngineState,
    pub workspace_root: Option<String>,
    pub engines: Vec<EngineHealth>,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub trace_id: Option<TraceId>,
    pub code: String,
    pub message: String,
    pub recoverable: bool,
    pub details: Value,
}

#[cfg(test)]
mod tests {
    use super::{ActualUsage, Envelope, EstimatedUsage, Intent, TokenBudget, TokenCount, Usage};

    #[test]
    fn test_budget_never_underflows() {
        let budget = TokenBudget {
            input_limit: 1_000,
            output_reserve: 400,
            safety_margin: 200,
        };

        assert_eq!(budget.available_for_dynamic_input(600), 0);
    }

    #[test]
    fn test_usage_keeps_actual_and_estimated_separate() {
        let usage = Usage {
            actual: ActualUsage {
                input_tokens: Some(100),
                ..ActualUsage::default()
            },
            estimated: EstimatedUsage {
                input_tokens: Some(130),
                method: Some("chars_div_four".into()),
                ..EstimatedUsage::default()
            },
        };

        assert_eq!(usage.actual.input_tokens, Some(100));
        assert_eq!(usage.estimated.input_tokens, Some(130));
    }

    #[test]
    fn test_envelope_serialization_preserves_protocol_version() {
        let envelope = Envelope::new(
            "workspace".into(),
            "worktree".into(),
            TokenBudget {
                input_limit: 8_000,
                output_reserve: 1_000,
                safety_margin: 500,
            },
            Intent {
                raw: "find authentication".into(),
                normalized: None,
                retrieval_variant: None,
                explicit_paths: Vec::new(),
                symbols: Vec::new(),
            },
        );

        let json = serde_json::to_string(&envelope).expect("envelope must serialize");
        assert!(json.contains("\"protocol_version\":1"));
        assert!(TokenCount::provider(5).is_actual());
        assert!(!TokenCount::estimate(5).is_actual());
    }
}
