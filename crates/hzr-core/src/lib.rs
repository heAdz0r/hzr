mod budget;
mod config;
mod engines;
mod ledger;
mod operation;

pub use budget::{BudgetPlanner, FusionInput};
pub use config::{
    ActivationConfig, ActivationMode, Config, ConfigError, ConfigPaths, DaemonConfig,
    EnabledWorkspace, EngineConfig, PrivacyConfig,
};
pub use engines::{EngineManifest, EnginePin, locked_engines};
pub use ledger::{
    BypassSummary, BypassTool, BypassWindow, CURRENT_ACCOUNTING_POLICY_VERSION,
    CURRENT_PRODUCER_VERSION, DEFAULT_FIDELITY_OPERATION_ALLOWANCE,
    DEFAULT_FIDELITY_TOKEN_ALLOWANCE, DetailedOperationAttribution, EfficiencyCommandSummary,
    EfficiencySummary, EvasionClassSummary, EvasionSummary, FidelityAllowance,
    FidelitySessionUsage, Ledger, LedgerError, LedgerRecord, LedgerSummary,
    LegacyEfficiencyMigration, LegacyEfficiencySource, OperationAttribution, OperationContext,
    OperationFamilySummary, OperationModeSummary, PolicyEvent, PolicyEventSummary, PriceTable,
    PrivacyPseudonymizer, PrivacySafeFidelityOperation, ProjectActivitySummary,
    ProjectOperationRoute, ProjectOperationSummary, ReadPipelineSummary, SessionEvasionSummary,
    StatsCollection, StatsQuery, StatsSnapshot, discover_legacy_rtk_history,
    inspect_legacy_efficiency, privacy_identity_hash, privacy_keyed_identity_hash,
};
pub use operation::{
    FidelityBudget, FidelityPreflight, OperationChannel, OperationClassification,
    OperationMeasurement, OperationRoute, OperationSubsystem, RawFidelityReason,
    RawFidelityRequest, RawReplacement, ReplacementCapability, classify_operation,
    efficient_route_replacement, explicit_raw_fidelity, fidelity_preflight,
    fidelity_preflight_required, first_class_replacement, managed_raw_payload,
    raw_fidelity_request, raw_route_sql_predicate,
};
