use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

mod api;

pub use api::{
    AccountingAttribution, AccountingChannel, AccountingFilterLevel, AccountingMeasurement,
    AccountingOperationKind, AccountingOperationMode, AccountingRoute, AccountingSearchStrategy,
    AccountingStage, COMPLETENESS_CONTRACTS, CodecApiRequest, CommandTermination,
    CompletenessContract, ContextPlanApiRequest, ContextPlanApiResponse, ContextWarning,
    ContextWarningCode, DashboardEconomicAmount, DashboardEstimatedEfficiency,
    DashboardHelpCommand, DashboardIndexArtifacts, DashboardIndexObservatory,
    DashboardIndexWatcher, DashboardLifecycleEvent, DashboardLifecycleKind, DashboardLocalActivity,
    DashboardLocalOperation, DashboardMemoryDetail, DashboardMemoryEdge,
    DashboardMemoryObservatory, DashboardMemoryRetrieval, DashboardMemoryTopic,
    DashboardMemoryTopicDetails, DashboardObservability, DashboardObservedUsage,
    DashboardOperationRoute, DashboardProject, DashboardProjectArtifacts, DashboardProjectPage,
    DashboardProjectState, DashboardProviderReceiptState, DashboardProviderReceipts,
    DashboardRawPublicEstimate, DashboardResponse, DashboardSearchActivity, DashboardService,
    DashboardSessionCommand, DashboardSessionRoi, DashboardState, DashboardTraceSpan,
    DashboardTraceStage, DashboardTraceState, EnforcementTier, EvasionAttribution, EvasionClass,
    EvasionInterpreter, EvasionPathForm, ExecApiRequest, ExecApprovalApiRequest, FidelityReason,
    FidelityReconcileApiRequest, FidelityReconcileReceipt, FidelityUnknownResolution,
    FidelityValidation, FilterPlacement, ForkManagedWrite, ForkPlannerMetadata, ForkRunApiRequest,
    ForkRunApiResponse, HOST_EXECUTION_GRANT_ENV, HOST_EXECUTION_GRANT_MAX_AGE_MS,
    HOST_EXECUTION_GRANT_MAX_FUTURE_SKEW_MS, HostExecutionGrant, HostGrantRejection,
    HostPermissionMode, MemoryForgetApiRequest, MemoryImportance, MemoryMutationApiResponse,
    MemoryPruneApiRequest, MemoryRecallApiRequest, MemoryScopeSelector, MemoryStoreApiRequest,
    MemoryUpdateApiRequest, MemoryWriteScope, MustKeep, OperationApiRequest, OperationApiResponse,
    PolicyDecision, SearchApiRequest, SearchApiResponse, SearchFallbackCode, SearchHit, SearchLine,
    SearchMode, SearchSnippet, SearchStrategy, SemanticReadinessApiRequest,
    SemanticReadinessApiResponse, UsageApiRequest, UsageApiResponse, completeness_contract,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateSource {
    Exact,
    Index,
    Context,
    Memory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolUnavailableReason {
    WholeFileCandidate,
    OutlineUnavailable,
    NoEnclosingSymbol,
    NotApplicable,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextCandidate {
    pub id: String,
    pub source: CandidateSource,
    pub content_ref: String,
    pub path: Option<String>,
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_unavailable_reason: Option<SymbolUnavailableReason>,
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
    use super::{
        ActualUsage, EnforcementTier, EstimatedUsage, EvasionAttribution, EvasionClass,
        EvasionInterpreter, EvasionPathForm, FidelityReason, FidelityValidation, TokenCount, Usage,
    };

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
    fn test_token_count_tracks_measurement_source() {
        assert!(TokenCount::provider(5).is_actual());
        assert!(!TokenCount::estimate(5).is_actual());
    }

    #[test]
    fn evasion_wire_contract_is_closed_and_payload_free_for_every_class_and_reason() {
        let sentinel = "SENTINEL /private/path SELECT * FROM secrets";
        let classes = [
            EvasionClass::E1QuotedCoveredCommand,
            EvasionClass::E2ShellWrapper,
            EvasionClass::E3InterpreterRead,
            EvasionClass::E4ExecutablePath,
            EvasionClass::E5PipelineOrRedirect,
            EvasionClass::E6NestedUnboundedReader,
            EvasionClass::E7FidelityHatch,
            EvasionClass::E8NativeTool,
            EvasionClass::E9DiagnosticBypass,
            EvasionClass::E10CapabilityGap,
            EvasionClass::E11PrivilegedPrefix,
        ];
        let reasons = [
            FidelityReason::Binary,
            FidelityReason::Checksum,
            FidelityReason::MachineProtocol,
            FidelityReason::CompleteLog,
            FidelityReason::FullPatch,
            FidelityReason::VerbatimSource,
        ];
        for (index, class) in classes.into_iter().enumerate() {
            let attribution = EvasionAttribution {
                class,
                wrapper_depth: u8::try_from(index).expect("small index"),
                interpreter: Some(EvasionInterpreter::Shell),
                path_form: EvasionPathForm::AbsoluteSystem,
                stage_count: 2,
                hatch_marker: class == EvasionClass::E7FidelityHatch,
                avoidable: !matches!(
                    class,
                    EvasionClass::E10CapabilityGap | EvasionClass::E11PrivilegedPrefix
                ),
                tier: EnforcementTier::T1NamedCorrection,
                fidelity_reason: Some(reasons[index % reasons.len()]),
                fidelity_validation: FidelityValidation::Valid,
            };
            let encoded = serde_json::to_string(&attribution).expect("serialize attribution");
            let decoded: EvasionAttribution =
                serde_json::from_str(&encoded).expect("deserialize attribution");
            assert_eq!(decoded, attribution);
            assert!(!encoded.contains(sentinel));
            for forbidden_key in [
                "original_command",
                "recorded_command",
                "query",
                "content",
                "environment",
                "sql",
                "heredoc",
            ] {
                assert!(!encoded.contains(forbidden_key));
            }
        }
    }
}
