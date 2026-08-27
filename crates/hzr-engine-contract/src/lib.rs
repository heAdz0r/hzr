use serde::{Deserialize, Serialize};

pub const ENGINE_CONTRACT_VERSION: u16 = 1;
pub const ACCOUNTING_POLICY_VERSION: &str = "privacy_typed_v2";
pub const INTERNAL_EVASION_ENV: &str = "HZR_INTERNAL_EVASION_JSON";
pub const HOST_GRANT_APPLIED_ENV: &str = "HZR_INTERNAL_HOST_GRANT_APPLIED";
pub const BYTE_FIDELITY_ENV: &str = "HZR_INTERNAL_BYTE_FIDELITY";
pub const RAW_FIDELITY_ENV: &str = "HZR_RAW_FIDELITY";
pub const RAW_FIDELITY_REASON_ENV: &str = "HZR_RAW_FIDELITY_REASON";
pub const ACCOUNTING_RECEIPT_JOURNAL_ENV: &str = "HZR_INTERNAL_ACCOUNTING_RECEIPT_JOURNAL";
pub const ACCOUNTING_FAILURE_JOURNAL_ENV: &str = "HZR_INTERNAL_ACCOUNTING_FAILURE_JOURNAL";
pub const ACCOUNTING_CORRELATION_ENV: &str = "HZR_INTERNAL_ACCOUNTING_CORRELATION";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingFailureKind {
    TrackerOpen,
    OperationRecord,
    ReceiptAppend,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum AccountingWriteStatus {
    Disabled,
    Recorded,
    Failed { kind: AccountingFailureKind },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountingFailureEvent {
    pub contract_version: u16,
    pub engine: EngineContractIdentity,
    pub correlation_id: String,
    pub occurred_at_unix_ms: i64,
    pub kind: AccountingFailureKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingChannel {
    HookCli,
    Mcp,
    NativeHost,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingMeasurement {
    Estimated,
    Unmeasured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingRoute {
    Optimized,
    Bypassed,
    NativeUnaccounted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingOperationKind {
    Search,
    Read,
    Write,
    Context,
    Memory,
    Codec,
    Exec,
    Observability,
    Doctor,
}

impl AccountingOperationKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Read => "read",
            Self::Write => "write",
            Self::Context => "context",
            Self::Memory => "memory",
            Self::Codec => "codec",
            Self::Exec => "exec",
            Self::Observability => "observability",
            Self::Doctor => "doctor",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingOperationMode {
    SearchAuto,
    SearchSemantic,
    SearchExact,
    SearchBuiltin,
    ReadFull,
    ReadFiltered,
    ReadRange,
    ReadHead,
    ReadTail,
    ReadOutline,
    ReadSymbols,
    ReadChanged,
    ReadSince,
    Write,
    ContextPlan,
    MemoryRecall,
    MemoryStore,
    MemoryForget,
    MemoryUpdate,
    MemoryPrune,
    CodecCompile,
    ExecRun,
    ObservabilitySnapshot,
    DoctorCheck,
}

impl AccountingOperationMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SearchAuto => "search_auto",
            Self::SearchSemantic => "search_semantic",
            Self::SearchExact => "search_exact",
            Self::SearchBuiltin => "search_builtin",
            Self::ReadFull => "read_full",
            Self::ReadFiltered => "read_filtered",
            Self::ReadRange => "read_range",
            Self::ReadHead => "read_head",
            Self::ReadTail => "read_tail",
            Self::ReadOutline => "read_outline",
            Self::ReadSymbols => "read_symbols",
            Self::ReadChanged => "read_changed",
            Self::ReadSince => "read_since",
            Self::Write => "write",
            Self::ContextPlan => "context_plan",
            Self::MemoryRecall => "memory_recall",
            Self::MemoryStore => "memory_store",
            Self::MemoryForget => "memory_forget",
            Self::MemoryUpdate => "memory_update",
            Self::MemoryPrune => "memory_prune",
            Self::CodecCompile => "codec_compile",
            Self::ExecRun => "exec_run",
            Self::ObservabilitySnapshot => "observability_snapshot",
            Self::DoctorCheck => "doctor_check",
        }
    }

    #[must_use]
    pub const fn operation(self) -> AccountingOperationKind {
        match self {
            Self::SearchAuto | Self::SearchSemantic | Self::SearchExact | Self::SearchBuiltin => {
                AccountingOperationKind::Search
            }
            Self::ReadFull
            | Self::ReadFiltered
            | Self::ReadRange
            | Self::ReadHead
            | Self::ReadTail
            | Self::ReadOutline
            | Self::ReadSymbols
            | Self::ReadChanged
            | Self::ReadSince => AccountingOperationKind::Read,
            Self::Write => AccountingOperationKind::Write,
            Self::ContextPlan => AccountingOperationKind::Context,
            Self::MemoryRecall
            | Self::MemoryStore
            | Self::MemoryForget
            | Self::MemoryUpdate
            | Self::MemoryPrune => AccountingOperationKind::Memory,
            Self::CodecCompile => AccountingOperationKind::Codec,
            Self::ExecRun => AccountingOperationKind::Exec,
            Self::ObservabilitySnapshot => AccountingOperationKind::Observability,
            Self::DoctorCheck => AccountingOperationKind::Doctor,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingStage {
    InternalTransport,
    FinalDelivery,
    StandaloneDelivery,
    ControlPlane,
}

impl AccountingStage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InternalTransport => "internal_transport",
            Self::FinalDelivery => "final_delivery",
            Self::StandaloneDelivery => "standalone_delivery",
            Self::ControlPlane => "control_plane",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingFilterLevel {
    None,
    Minimal,
    Aggressive,
}

impl AccountingFilterLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Aggressive => "aggressive",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingSearchStrategy {
    ForkRgaiAdaptive,
    ForkRgaiBuiltin,
    ForkRgaiGrepai,
    ForkRgaiRipgrep,
    ForkRgaiFiles,
}

impl AccountingSearchStrategy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ForkRgaiAdaptive => "fork_rgai_adaptive",
            Self::ForkRgaiBuiltin => "fork_rgai_builtin",
            Self::ForkRgaiGrepai => "fork_rgai_grepai",
            Self::ForkRgaiRipgrep => "fork_rgai_ripgrep",
            Self::ForkRgaiFiles => "fork_rgai_files",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchFallbackCode {
    LegacyIndexRequiresMigration,
    SemanticIndexUnavailable,
    GrepaiUnavailable,
    RipgrepUnavailable,
}

impl SearchFallbackCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LegacyIndexRequiresMigration => "legacy_index_requires_migration",
            Self::SemanticIndexUnavailable => "semantic_index_unavailable",
            Self::GrepaiUnavailable => "grepai_unavailable",
            Self::RipgrepUnavailable => "ripgrep_unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccountingAttribution {
    pub operation: AccountingOperationKind,
    pub mode: AccountingOperationMode,
    pub stage: AccountingStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_mode: Option<AccountingOperationMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_mode: Option<AccountingOperationMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_strategy: Option<AccountingSearchStrategy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_fallback_code: Option<SearchFallbackCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_content: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_scope_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_level: Option<AccountingFilterLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evasion: Option<EvasionAttribution>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvasionClass {
    E1QuotedCoveredCommand,
    E2ShellWrapper,
    E3InterpreterRead,
    E4ExecutablePath,
    E5PipelineOrRedirect,
    E6NestedUnboundedReader,
    E7FidelityHatch,
    E8NativeTool,
    E9DiagnosticBypass,
    E10CapabilityGap,
    E11PrivilegedPrefix,
}

impl EvasionClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::E1QuotedCoveredCommand => "e1",
            Self::E2ShellWrapper => "e2",
            Self::E3InterpreterRead => "e3",
            Self::E4ExecutablePath => "e4",
            Self::E5PipelineOrRedirect => "e5",
            Self::E6NestedUnboundedReader => "e6",
            Self::E7FidelityHatch => "e7",
            Self::E8NativeTool => "e8",
            Self::E9DiagnosticBypass => "e9",
            Self::E10CapabilityGap => "e10",
            Self::E11PrivilegedPrefix => "e11",
        }
    }

    #[must_use]
    pub const fn ledger_str(self) -> &'static str {
        match self {
            Self::E1QuotedCoveredCommand => "e1_quoted_covered_command",
            Self::E2ShellWrapper => "e2_shell_wrapper",
            Self::E3InterpreterRead => "e3_interpreter_read",
            Self::E4ExecutablePath => "e4_executable_path",
            Self::E5PipelineOrRedirect => "e5_pipeline_or_redirect",
            Self::E6NestedUnboundedReader => "e6_nested_unbounded_reader",
            Self::E7FidelityHatch => "e7_fidelity_hatch",
            Self::E8NativeTool => "e8_native_tool",
            Self::E9DiagnosticBypass => "e9_diagnostic_bypass",
            Self::E10CapabilityGap => "e10_capability_gap",
            Self::E11PrivilegedPrefix => "e11_privileged_prefix",
        }
    }

    #[must_use]
    pub const fn construct(self) -> &'static str {
        match self {
            Self::E1QuotedCoveredCommand => "quoted covered command",
            Self::E2ShellWrapper => "shell wrapper",
            Self::E3InterpreterRead => "interpreter file read",
            Self::E4ExecutablePath => "executable path form",
            Self::E5PipelineOrRedirect => "pipeline or redirect",
            Self::E6NestedUnboundedReader => "nested unbounded reader",
            Self::E7FidelityHatch => "raw fidelity request",
            Self::E8NativeTool => "host-native file tool",
            Self::E9DiagnosticBypass => "direct HZR diagnostic access",
            Self::E10CapabilityGap => "no first-class route",
            Self::E11PrivilegedPrefix => "privilege elevation prefix",
        }
    }

    #[must_use]
    pub const fn prescription(self) -> &'static str {
        match self {
            Self::E1QuotedCoveredCommand | Self::E2ShellWrapper | Self::E4ExecutablePath => {
                "re-issue the inner command directly so HZR can route it"
            }
            Self::E3InterpreterRead => {
                "read files with `hzr read <file>` and scan them with `hzr search '<pattern>'`; keep the interpreter for computation that has no managed route"
            }
            Self::E5PipelineOrRedirect => {
                "run each stage through its own managed route, for example `hzr read <file> --max-lines N` instead of piping a full read into a limiter"
            }
            Self::E6NestedUnboundedReader => {
                "read the matched files with `hzr read <file>` instead of embedding an unbounded reader in the command"
            }
            Self::E7FidelityHatch => {
                "supply a fidelity reason that matches the command, or use the bounded managed route when exact bytes are not required"
            }
            Self::E8NativeTool => "use the equivalent `hzr` file operation",
            Self::E9DiagnosticBypass => {
                "use `hzr stats` (add `--since` or `--workspace`) instead of reading HZR state directly; this is an HZR policy decision surfaced through the harness approval channel, not a harness permission"
            }
            Self::E10CapabilityGap => {
                "no managed route covers this command; approving runs it as tracked raw"
            }
            Self::E11PrivilegedPrefix => {
                "elevation was granted to one binary; HZR stays out of it and runs the command verbatim — drop the elevation to get a managed route"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvasionInterpreter {
    Shell,
    Python,
    Javascript,
    Ruby,
    Perl,
    Awk,
    Sed,
}

impl EvasionInterpreter {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Python => "python",
            Self::Javascript => "javascript",
            Self::Ruby => "ruby",
            Self::Perl => "perl",
            Self::Awk => "awk",
            Self::Sed => "sed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvasionPathForm {
    Bare,
    AbsoluteSystem,
    Relative,
    ResolvedAlias,
}

impl EvasionPathForm {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bare => "bare",
            Self::AbsoluteSystem => "absolute_system",
            Self::Relative => "relative",
            Self::ResolvedAlias => "resolved_alias",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementTier {
    T0TransparentRewrite,
    T1NamedCorrection,
    T2DenyWithPrescription,
    T3BudgetExhaustion,
    T4HatchQuarantine,
}

impl EnforcementTier {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::T0TransparentRewrite => "t0",
            Self::T1NamedCorrection => "t1",
            Self::T2DenyWithPrescription => "t2",
            Self::T3BudgetExhaustion => "t3",
            Self::T4HatchQuarantine => "t4",
        }
    }

    #[must_use]
    pub const fn ledger_str(self) -> &'static str {
        match self {
            Self::T0TransparentRewrite => "t0_transparent_rewrite",
            Self::T1NamedCorrection => "t1_named_correction",
            Self::T2DenyWithPrescription => "t2_deny_with_prescription",
            Self::T3BudgetExhaustion => "t3_budget_exhaustion",
            Self::T4HatchQuarantine => "t4_hatch_quarantine",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FidelityReason {
    Binary,
    Checksum,
    MachineProtocol,
    CompleteLog,
    FullPatch,
    VerbatimSource,
}

impl FidelityReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::Checksum => "checksum",
            Self::MachineProtocol => "machine_protocol",
            Self::CompleteLog => "complete_log",
            Self::FullPatch => "full_patch",
            Self::VerbatimSource => "verbatim_source",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "binary" => Some(Self::Binary),
            "checksum" => Some(Self::Checksum),
            "machine_protocol" => Some(Self::MachineProtocol),
            "complete_log" => Some(Self::CompleteLog),
            "full_patch" => Some(Self::FullPatch),
            "verbatim_source" => Some(Self::VerbatimSource),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FidelityValidation {
    NotRequested,
    Valid,
    MissingReason,
    InvalidReason,
    Contradicted,
    ProvenEquivalent,
    BudgetExhausted,
}

impl FidelityValidation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotRequested => "not_requested",
            Self::Valid => "valid",
            Self::MissingReason => "missing_reason",
            Self::InvalidReason => "invalid_reason",
            Self::Contradicted => "contradicted",
            Self::ProvenEquivalent => "proven_equivalent",
            Self::BudgetExhausted => "budget_exhausted",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    Ask,
    Deny,
    Correction,
}

impl PolicyDecision {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Deny => "deny",
            Self::Correction => "correction",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvasionAttribution {
    pub class: EvasionClass,
    pub wrapper_depth: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpreter: Option<EvasionInterpreter>,
    pub path_form: EvasionPathForm,
    pub stage_count: u16,
    pub hatch_marker: bool,
    pub avoidable: bool,
    pub tier: EnforcementTier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fidelity_reason: Option<FidelityReason>,
    pub fidelity_validation: FidelityValidation,
}

impl EvasionAttribution {
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.wrapper_depth <= 3
            && self.stage_count > 0
            && (self.hatch_marker || self.fidelity_reason.is_none())
            && (self.hatch_marker
                || matches!(self.fidelity_validation, FidelityValidation::NotRequested))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewritePlanDecision {
    Rewrite,
    Proxy,
    Ask,
    Deny,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewritePlanReason {
    PermissionPolicy,
    CanonicalPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RewritePlan {
    pub decision: RewritePlanDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<EvasionAttribution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<RewritePlanReason>,
}

impl RewritePlan {
    #[must_use]
    pub const fn is_consistent(&self) -> bool {
        let metadata_matches = match self.decision {
            RewritePlanDecision::Rewrite => self.proposed.is_some() && self.reason.is_none(),
            RewritePlanDecision::Proxy => self.proposed.is_none() && self.reason.is_none(),
            RewritePlanDecision::Ask => matches!(
                self.reason,
                Some(RewritePlanReason::PermissionPolicy | RewritePlanReason::CanonicalPolicy)
            ),
            RewritePlanDecision::Deny => {
                self.proposed.is_none()
                    && matches!(self.reason, Some(RewritePlanReason::PermissionPolicy))
            }
        };
        metadata_matches
            && match self.attribution {
                Some(attribution) => attribution.is_valid(),
                None => true,
            }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineContractIdentity {
    pub contract_version: u16,
    pub engine_version: String,
    pub manifest_sha256: String,
    pub content_manifest_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineAccountingReceipt {
    pub contract_version: u16,
    pub engine: EngineContractIdentity,
    pub correlation_id: String,
    pub sequence: u64,
    pub occurred_at_unix_ms: i64,
    pub baseline_tokens: u64,
    pub delivered_tokens: u64,
    pub execution_ms: u64,
    pub measurement: AccountingMeasurement,
    pub route: AccountingRoute,
    pub attribution: AccountingAttribution,
    pub host_grant_applied: bool,
}

impl EngineAccountingReceipt {
    #[must_use]
    pub fn is_valid_for(&self, expected: &EngineContractIdentity) -> bool {
        self.contract_version == ENGINE_CONTRACT_VERSION
            && &self.engine == expected
            && valid_correlation_id(&self.correlation_id)
            && self.attribution.operation == self.attribution.mode.operation()
            && match self.measurement {
                AccountingMeasurement::Estimated => true,
                AccountingMeasurement::Unmeasured => {
                    self.baseline_tokens == 0 && self.delivered_tokens == 0
                }
            }
            && match self.route {
                AccountingRoute::Optimized => true,
                AccountingRoute::Bypassed | AccountingRoute::NativeUnaccounted => {
                    self.baseline_tokens == self.delivered_tokens
                }
            }
            && self
                .attribution
                .evasion
                .is_none_or(|attribution| attribution.is_valid())
    }
}

#[must_use]
pub fn valid_correlation_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::{
        AccountingAttribution, AccountingFilterLevel, AccountingMeasurement,
        AccountingOperationKind, AccountingOperationMode, AccountingRoute, AccountingStage,
        ENGINE_CONTRACT_VERSION, EnforcementTier, EngineAccountingReceipt, EngineContractIdentity,
        EvasionAttribution, EvasionClass, EvasionInterpreter, EvasionPathForm, FidelityReason,
        FidelityValidation, RewritePlan, RewritePlanDecision, RewritePlanReason,
    };

    #[test]
    fn every_evasion_class_round_trips_without_payload_fields() {
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
        for class in classes {
            let attribution = EvasionAttribution {
                class,
                wrapper_depth: 1,
                interpreter: Some(EvasionInterpreter::Shell),
                path_form: EvasionPathForm::Bare,
                stage_count: 1,
                hatch_marker: true,
                avoidable: !matches!(
                    class,
                    EvasionClass::E10CapabilityGap | EvasionClass::E11PrivilegedPrefix
                ),
                tier: EnforcementTier::T1NamedCorrection,
                fidelity_reason: Some(FidelityReason::CompleteLog),
                fidelity_validation: FidelityValidation::Valid,
            };
            let encoded = serde_json::to_string(&attribution).expect("serialize attribution");
            let decoded = serde_json::from_str(&encoded).expect("deserialize attribution");
            assert_eq!(attribution, decoded);
            let object = serde_json::from_str::<serde_json::Value>(&encoded)
                .expect("valid attribution object");
            let object = object.as_object().expect("attribution is an object");
            for forbidden in ["command", "session", "project", "query", "content"] {
                assert!(!object.contains_key(forbidden));
            }
        }
    }

    #[test]
    fn rewrite_plan_rejects_inconsistent_policy_metadata() {
        let plan = RewritePlan {
            decision: RewritePlanDecision::Deny,
            proposed: Some("hzr read secret".to_owned()),
            attribution: None,
            reason: Some(RewritePlanReason::PermissionPolicy),
        };
        assert!(!plan.is_consistent());
    }

    #[test]
    fn accounting_receipt_is_typed_and_payload_free() {
        let engine = EngineContractIdentity {
            contract_version: ENGINE_CONTRACT_VERSION,
            engine_version: "test-engine".to_owned(),
            manifest_sha256: "a".repeat(64),
            content_manifest_sha256: "b".repeat(64),
        };
        let receipt = EngineAccountingReceipt {
            contract_version: ENGINE_CONTRACT_VERSION,
            engine: engine.clone(),
            correlation_id: "0123456789abcdef0123456789abcdef".to_owned(),
            sequence: 0,
            occurred_at_unix_ms: 1,
            baseline_tokens: 20,
            delivered_tokens: 5,
            execution_ms: 3,
            measurement: AccountingMeasurement::Estimated,
            route: AccountingRoute::Optimized,
            attribution: AccountingAttribution {
                operation: AccountingOperationKind::Read,
                mode: AccountingOperationMode::ReadFiltered,
                stage: AccountingStage::InternalTransport,
                requested_mode: None,
                effective_mode: None,
                search_strategy: None,
                search_fallback_code: None,
                include_content: None,
                limit: None,
                path_scope_count: None,
                filter_level: Some(AccountingFilterLevel::Minimal),
                from_line: None,
                to_line: None,
                source_bytes: None,
                evasion: None,
            },
            host_grant_applied: false,
        };

        assert!(receipt.is_valid_for(&engine));
        let encoded = serde_json::to_string(&receipt).expect("serialize receipt");
        let object =
            serde_json::from_str::<serde_json::Value>(&encoded).expect("deserialize receipt JSON");
        let object = object.as_object().expect("receipt JSON object");
        for forbidden in [
            "command", "query", "path", "project", "session", "content", "original",
        ] {
            assert!(!object.contains_key(forbidden), "leaked key: {forbidden}");
        }
        assert_eq!(
            serde_json::from_str::<EngineAccountingReceipt>(&encoded).expect("deserialize receipt"),
            receipt
        );
    }
}
