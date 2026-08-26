use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use directories::BaseDirs;
use hzr_protocol::{
    AccountingAttribution, AccountingOperationKind, AccountingOperationMode, AccountingStage,
    EnforcementTier, EvasionAttribution, EvasionClass, EvasionInterpreter, EvasionPathForm,
    FidelityReason, FidelityValidation, PolicyDecision, TraceId, Usage,
};

use crate::operation::{
    OperationChannel, OperationMeasurement, OperationRoute, ReplacementCapability,
    classify_operation, efficient_route_replacement, first_class_replacement,
    raw_route_sql_predicate,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::billing::{
    BillingError, EconomicAmount, PricingCatalog, ProviderEconomicReceipt,
    ProviderReceiptRecordResult, SessionEconomicSummary, price_receipt, receipt_payload_hash,
    validate_receipt,
};

pub const CURRENT_ACCOUNTING_POLICY_VERSION: &str = "privacy_typed_v2";
const LEGACY_ACCOUNTING_POLICY_VERSION_V1: &str = "privacy_typed_v1";
const IDENTITY_HMAC_UUID_V1: &str = "hmac_sha256_uuid_v1";
const IDENTITY_HMAC_KEY256_V2: &str = "hmac_sha256_key256_v2";
pub const CURRENT_PRODUCER_VERSION: &str = concat!("hzr-core/", env!("CARGO_PKG_VERSION"));

pub struct Ledger {
    connection: Connection,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LedgerRecord {
    pub trace_id: TraceId,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub usage: Usage,
    pub turns: u32,
    pub retries: u32,
    pub latency_ms: u64,
    pub outcome: String,
    pub policy_version: String,
    pub cost_microusd: Option<u64>,
    /// Канонический корень workspace; пустая строка — глобальный/исторический чек без атрибуции.
    pub project_path: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LedgerSummary {
    pub tasks: u64,
    pub accepted: u64,
    pub actual_input_tokens: u64,
    pub actual_output_tokens: u64,
    pub estimated_input_tokens: u64,
    pub cost_microusd: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EfficiencySummary {
    pub operations: u64,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub gross_avoided_tokens_estimated: u64,
    pub regression_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub total_execution_ms: u64,
    /// Rows included in the measured reduction ratio plus explicit unmeasured bypasses.
    pub accounted_operations: u64,
    /// Every row observed across measured, unmeasured, and host-native channels.
    pub total_observed_operations: u64,
    pub native_unaccounted_operations: u64,
    pub unmeasured_bypass_operations: u64,
    pub by_channel: BTreeMap<String, u64>,
    pub by_mode: Vec<OperationModeSummary>,
    pub by_command: Vec<EfficiencyCommandSummary>,
    pub read_pipeline: ReadPipelineSummary,
    pub excluded_legacy_operations: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReadPipelineSummary {
    pub operations: u64,
    pub source_tokens_estimated: u64,
    pub selected_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub selection_avoided_tokens_estimated: u64,
    pub selection_overhead_tokens_estimated: u64,
    pub transform_avoided_tokens_estimated: u64,
    pub transform_overhead_tokens_estimated: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationModeSummary {
    pub operation: AccountingOperationKind,
    pub mode: AccountingOperationMode,
    pub stage: AccountingStage,
    pub operations: u64,
    pub delivered_tokens_estimated: u64,
}

/// Privacy-safe aggregation for auditing which operation families and routes consume output.
/// No recorded command, argument, query, path, or content is retained in this type.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationFamilySummary {
    pub family: String,
    pub route: OperationRoute,
    pub operations: u64,
    pub delivered_tokens_estimated: u64,
    pub replacement_capability: ReplacementCapability,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StatsQuery<'a> {
    pub project_path: Option<&'a str>,
    /// Inclusive Unix-second cutoff shared by every section of one stats snapshot.
    pub since_unix_seconds: Option<i64>,
    /// Current policy is the only scope suitable for headline efficiency claims.
    pub include_legacy_versions: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StatsSnapshot {
    pub efficiency: EfficiencySummary,
    pub bypass: BypassSummary,
    pub provider_usage: LedgerSummary,
    pub by_family: Vec<OperationFamilySummary>,
    pub evasion: EvasionSummary,
}

pub const DEFAULT_FIDELITY_OPERATION_ALLOWANCE: u64 = 5;
pub const DEFAULT_FIDELITY_TOKEN_ALLOWANCE: u64 = 100_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FidelityAllowance {
    pub max_operations: u64,
    pub max_delivered_tokens: u64,
}

impl Default for FidelityAllowance {
    fn default() -> Self {
        Self {
            max_operations: DEFAULT_FIDELITY_OPERATION_ALLOWANCE,
            max_delivered_tokens: DEFAULT_FIDELITY_TOKEN_ALLOWANCE,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct FidelitySessionUsage {
    pub operations: u64,
    pub delivered_tokens: u64,
    pub remaining_operations: u64,
    pub remaining_tokens: u64,
    pub exhausted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionEvasionSummary {
    pub agent: Option<String>,
    pub agent_hash: Option<String>,
    pub session_hash: String,
    pub operations: u64,
    pub delivered_tokens: u64,
    pub avoidable_operations: u64,
    pub avoidable_tokens: u64,
    pub avoidable_share_pct: f64,
    pub top_class: Option<EvasionClass>,
    pub recoverable_tokens: u64,
    pub fidelity: FidelitySessionUsage,
    pub policy_attempts: u64,
    pub policy_asks: u64,
    pub policy_denials: u64,
    pub policy_corrections: u64,
}

/// The measured efficiency slice for one private session identity.
///
/// This deliberately mirrors the token arithmetic used by `hzr stats` while keeping provider
/// receipts separate. Commands are already reduced to privacy-safe families by the current
/// accounting policy before they can reach this view.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionEfficiencySummary {
    pub operations: u64,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub gross_avoided_tokens_estimated: u64,
    pub regression_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub top_commands: Vec<EfficiencyCommandSummary>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvasionClassSummary {
    pub class: EvasionClass,
    pub operations: u64,
    pub delivered_tokens: u64,
    pub avoidable_operations: u64,
    pub avoidable_tokens: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvasionSummary {
    pub by_class: Vec<EvasionClassSummary>,
    pub fidelity_operations: u64,
    pub fidelity_delivered_tokens: u64,
    pub fidelity_invalid_operations: u64,
    pub default_allowance: FidelityAllowance,
    pub policy_attempts: u64,
    pub policy_by_class: Vec<PolicyEventSummary>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PolicyEventSummary {
    pub class: EvasionClass,
    pub decision: PolicyDecision,
    pub attempts: u64,
    pub avoidable_attempts: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct PolicyEvent<'a> {
    pub project_path: &'a str,
    pub agent: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub evasion: EvasionAttribution,
    pub decision: PolicyDecision,
    pub replacement_family: Option<&'a str>,
}

#[derive(Clone, Debug)]
pub struct StatsCollection {
    pub snapshot: StatsSnapshot,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EfficiencyCommandSummary {
    pub command: String,
    pub executions: u64,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub gross_avoided_tokens_estimated: u64,
    pub regression_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub avg_time_ms: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectActivitySummary {
    pub operations: u64,
    pub optimized_operations: u64,
    pub raw_operations: u64,
    pub native_unaccounted_operations: u64,
    pub unmeasured_bypass_operations: u64,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub gross_avoided_tokens_estimated: u64,
    pub regression_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub total_execution_ms: u64,
    pub first_record_at: Option<String>,
    pub last_record_at: Option<String>,
    pub unscoped_operations: u64,
    pub excluded_legacy_operations: u64,
    pub recent_operations: Vec<ProjectOperationSummary>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectOperationRoute {
    Optimized,
    Raw,
    NativeUnaccounted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectOperationSummary {
    pub ledger_id: u64,
    pub timestamp: String,
    pub operation: String,
    pub route: ProjectOperationRoute,
    pub command_hash: String,
    pub project_hash: String,
    pub agent: Option<String>,
    pub session_hash: Option<String>,
    pub producer_version: Option<String>,
    pub policy_version: Option<String>,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub execution_ms: u64,
    pub replacement: Option<String>,
    pub rationale: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OperationContext<'a> {
    pub project_path: &'a str,
    pub agent: Option<&'a str>,
    pub session_id: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationAttribution<'a> {
    pub project_path: &'a str,
    pub agent: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub channel: OperationChannel,
    pub measurement: OperationMeasurement,
    pub route: OperationRoute,
}

#[derive(Clone, Copy, Debug)]
pub struct DetailedOperationAttribution<'a> {
    pub attribution: OperationAttribution<'a>,
    pub detail: Option<&'a AccountingAttribution>,
    pub evasion: Option<&'a EvasionAttribution>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PrivacySafeFidelityOperation {
    pub reservation_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub execution_ms: u64,
    pub measurement: OperationMeasurement,
    pub project_hash: String,
    pub project_scope_hashes: String,
    pub session_hash: Option<String>,
    pub agent: Option<String>,
    pub agent_hash: Option<String>,
    pub evasion: EvasionAttribution,
}

/// Operations that never reached the optimizer, split out of the reduction ratio they
/// would otherwise silently dilute.
///
/// A bypassed row delivers exactly as many tokens as it consumed, so it contributes
/// equally to both sides of the ratio and cancels out instead of lowering it. Reporting
/// it separately is the only way an operator sees that half the tool output skipped HZR.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BypassSummary {
    pub lifetime: BypassWindow,
    /// Bypassed tools ranked by delivered tokens — the costliest leak first.
    pub by_tool: Vec<BypassTool>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BypassWindow {
    pub operations: u64,
    pub total_operations: u64,
    pub delivered_tokens_estimated: u64,
    pub total_delivered_tokens_estimated: u64,
}

impl BypassWindow {
    pub fn operation_share_pct(&self) -> f64 {
        percentage_of(self.operations, self.total_operations)
    }

    pub fn token_share_pct(&self) -> f64 {
        percentage_of(
            self.delivered_tokens_estimated,
            self.total_delivered_tokens_estimated,
        )
    }
}

fn percentage_of(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    part as f64 * 100.0 / whole as f64
}

fn parse_operation_kind(value: &str) -> Option<AccountingOperationKind> {
    match value {
        "search" => Some(AccountingOperationKind::Search),
        "read" => Some(AccountingOperationKind::Read),
        "write" => Some(AccountingOperationKind::Write),
        "context" => Some(AccountingOperationKind::Context),
        "memory" => Some(AccountingOperationKind::Memory),
        "codec" => Some(AccountingOperationKind::Codec),
        "exec" => Some(AccountingOperationKind::Exec),
        "observability" => Some(AccountingOperationKind::Observability),
        "doctor" => Some(AccountingOperationKind::Doctor),
        _ => None,
    }
}

fn parse_operation_mode(value: &str) -> Option<AccountingOperationMode> {
    match value {
        "search_auto" => Some(AccountingOperationMode::SearchAuto),
        "search_semantic" => Some(AccountingOperationMode::SearchSemantic),
        "search_exact" => Some(AccountingOperationMode::SearchExact),
        "search_builtin" => Some(AccountingOperationMode::SearchBuiltin),
        "read_full" => Some(AccountingOperationMode::ReadFull),
        "read_filtered" => Some(AccountingOperationMode::ReadFiltered),
        "read_range" => Some(AccountingOperationMode::ReadRange),
        "read_head" => Some(AccountingOperationMode::ReadHead),
        "read_tail" => Some(AccountingOperationMode::ReadTail),
        "read_outline" => Some(AccountingOperationMode::ReadOutline),
        "read_symbols" => Some(AccountingOperationMode::ReadSymbols),
        "read_changed" => Some(AccountingOperationMode::ReadChanged),
        "read_since" => Some(AccountingOperationMode::ReadSince),
        "write" => Some(AccountingOperationMode::Write),
        "context_plan" => Some(AccountingOperationMode::ContextPlan),
        "memory_recall" => Some(AccountingOperationMode::MemoryRecall),
        "memory_store" => Some(AccountingOperationMode::MemoryStore),
        "memory_forget" => Some(AccountingOperationMode::MemoryForget),
        "memory_update" => Some(AccountingOperationMode::MemoryUpdate),
        "memory_prune" => Some(AccountingOperationMode::MemoryPrune),
        "codec_compile" => Some(AccountingOperationMode::CodecCompile),
        "exec_run" => Some(AccountingOperationMode::ExecRun),
        "observability_snapshot" => Some(AccountingOperationMode::ObservabilitySnapshot),
        "doctor_check" => Some(AccountingOperationMode::DoctorCheck),
        _ => None,
    }
}

fn parse_accounting_stage(value: &str) -> Option<AccountingStage> {
    match value {
        "internal_transport" => Some(AccountingStage::InternalTransport),
        "final_delivery" => Some(AccountingStage::FinalDelivery),
        "standalone_delivery" => Some(AccountingStage::StandaloneDelivery),
        "control_plane" => Some(AccountingStage::ControlPlane),
        _ => None,
    }
}

fn parse_replacement_capability(value: Option<&str>) -> ReplacementCapability {
    match value {
        Some("available") => ReplacementCapability::Available,
        Some("unavailable") => ReplacementCapability::Unavailable,
        _ => ReplacementCapability::Unknown,
    }
}

/// Create `path` owner-only if it does not exist yet, so a later opener
/// inherits the tight mode rather than the umask.
fn create_private_file(path: &Path) {
    if path.exists() {
        set_owner_only(path, 0o600);
        return;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if options.open(path).is_ok() {
        set_owner_only(path, 0o600);
    }
}

/// SQLite appends `-wal`/`-shm` to the whole filename, so these are siblings
/// rather than extension swaps — concatenating on `OsString` rather than
/// pushing a path component keeps them from targeting `hzr.sqlite/-wal`.
fn restrict_db_files(path: &Path) {
    set_owner_only(path, 0o600);
    for suffix in ["-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        set_owner_only(&PathBuf::from(name), 0o600);
    }
}

#[cfg(unix)]
fn set_owner_only(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path, _mode: u32) {}

fn parse_evasion_class(value: &str) -> Option<EvasionClass> {
    match value {
        "e1" => Some(EvasionClass::E1QuotedCoveredCommand),
        "e2" => Some(EvasionClass::E2ShellWrapper),
        "e3" => Some(EvasionClass::E3InterpreterRead),
        "e4" => Some(EvasionClass::E4ExecutablePath),
        "e5" => Some(EvasionClass::E5PipelineOrRedirect),
        "e6" => Some(EvasionClass::E6NestedUnboundedReader),
        "e7" => Some(EvasionClass::E7FidelityHatch),
        "e8" => Some(EvasionClass::E8NativeTool),
        "e9" => Some(EvasionClass::E9DiagnosticBypass),
        "e10" => Some(EvasionClass::E10CapabilityGap),
        "e11" => Some(EvasionClass::E11PrivilegedPrefix),
        _ => None,
    }
}

fn parse_policy_decision(value: &str) -> Option<PolicyDecision> {
    match value {
        "ask" => Some(PolicyDecision::Ask),
        "deny" => Some(PolicyDecision::Deny),
        "correction" => Some(PolicyDecision::Correction),
        _ => None,
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BypassTool {
    pub tool: String,
    pub executions: u64,
    pub delivered_tokens_estimated: u64,
    /// The costliest concrete invocation seen for this tool.
    pub example_command: String,
    /// The first-class HZR command that would have replaced it, when one exists.
    pub replacement: Option<String>,
    pub replacement_capability: ReplacementCapability,
    pub rationale: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LegacyEfficiencySource {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
    pub operations: u64,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub gross_avoided_tokens_estimated: u64,
    pub regression_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub parse_failures: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LegacyEfficiencyMigration {
    pub source: LegacyEfficiencySource,
    pub source_id: String,
    pub backup_path: PathBuf,
    pub manifest_path: PathBuf,
    pub imported_commands: usize,
    pub imported_parse_failures: usize,
    pub changed: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct PriceTable {
    pub input_microusd_per_million: u64,
    pub output_microusd_per_million: u64,
    pub cache_write_microusd_per_million: u64,
    pub cache_read_microusd_per_million: u64,
}

impl PriceTable {
    pub fn cost_microusd(self, usage: &Usage) -> Option<u64> {
        let input = usage.actual.input_tokens?;
        let output = usage.actual.output_tokens?;
        let cache_write = usage.actual.cache_write_tokens.unwrap_or_default();
        let cache_read = usage.actual.cache_read_tokens.unwrap_or_default();
        let total = [
            (input, self.input_microusd_per_million),
            (output, self.output_microusd_per_million),
            (cache_write, self.cache_write_microusd_per_million),
            (cache_read, self.cache_read_microusd_per_million),
        ]
        .into_iter()
        .try_fold(0_u128, |total, (tokens, rate)| {
            total.checked_add(u128::from(tokens).checked_mul(u128::from(rate))?)
        })?;
        u64::try_from(total.div_ceil(1_000_000)).ok()
    }
}

pub fn privacy_identity_hash(domain: &str, value: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(domain.as_bytes());
    digest.update([0]);
    digest.update(value.as_bytes());
    format!("sha256:{}", hex::encode(digest.finalize()))
}

fn keyed_identity_hash(key: &[u8], domain: &str, value: &str) -> String {
    const BLOCK: usize = 64;
    let mut normalized = [0_u8; BLOCK];
    if key.len() > BLOCK {
        normalized[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; BLOCK];
    let mut outer_pad = [0x5c_u8; BLOCK];
    for index in 0..BLOCK {
        inner_pad[index] ^= normalized[index];
        outer_pad[index] ^= normalized[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(domain.as_bytes());
    inner.update([0]);
    inner.update(value.as_bytes());
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner.finalize());
    format!("hmac-sha256:{}", hex::encode(outer.finalize()))
}

/// Build a public pseudonym with caller-owned key material.
///
/// The key must remain process- or store-private. This is exposed for sibling HZR
/// components that publish bounded observability identities without exposing raw IDs.
pub fn privacy_keyed_identity_hash(key: &[u8], domain: &str, value: &str) -> String {
    keyed_identity_hash(key, domain, value)
}

/// Store-private, restart-stable identity mapper shared by public telemetry surfaces.
///
/// The key is deliberately not exposed. Callers can join bounded identities produced by
/// independent HZR components without publishing raw workspace or session identifiers.
#[derive(Clone)]
pub struct PrivacyPseudonymizer {
    key: String,
}

impl PrivacyPseudonymizer {
    /// Construct a mapper from caller-owned private key material.
    pub fn from_key(key: impl Into<String>) -> Result<Self, LedgerError> {
        let key = key.into();
        validate_identity_hmac_key(&key, IDENTITY_HMAC_KEY256_V2)?;
        Ok(Self { key })
    }

    pub fn hash(&self, domain: &str, value: &str) -> String {
        keyed_identity_hash(self.key.as_bytes(), domain, value)
    }
}

fn ledger_identity_key(connection: &Connection) -> Result<String, LedgerError> {
    let key = connection
        .query_row(
            "SELECT value FROM ledger_privacy_meta WHERE key = 'identity_hmac_key'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map_err(LedgerError::Database)?;
    let version = connection
        .query_row(
            "SELECT value FROM ledger_privacy_meta WHERE key = 'identity_hmac_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map_err(LedgerError::Database)?;
    validate_identity_hmac_key(&key, &version)?;
    Ok(key)
}

fn validate_identity_hmac_key(key: &str, version: &str) -> Result<(), LedgerError> {
    let valid = match version {
        IDENTITY_HMAC_KEY256_V2 => {
            key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit())
        }
        IDENTITY_HMAC_UUID_V1 | "hmac_sha256_v1" => {
            key.len() == 36
                && key.bytes().enumerate().all(|(index, byte)| match index {
                    8 | 13 | 18 | 23 => byte == b'-',
                    _ => byte.is_ascii_hexdigit(),
                })
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(LedgerError::InvalidPrivacyIdentity(format!(
            "identity_hmac_key does not match declared version {version}"
        )))
    }
}

fn new_identity_hmac_key() -> String {
    let first = TraceId::new();
    let second = TraceId::new();
    let seed = format!("{}\0{}", first.as_str(), second.as_str());
    hex::encode(Sha256::digest(seed.as_bytes()))
}

fn initialize_identity_hmac(connection: &Connection) -> Result<(), LedgerError> {
    connection
        .execute(
            "INSERT OR IGNORE INTO ledger_privacy_meta (key, value)
             VALUES ('identity_hmac_key', ?1)",
            [new_identity_hmac_key()],
        )
        .map_err(LedgerError::Database)?;
    let key: String = connection
        .query_row(
            "SELECT value FROM ledger_privacy_meta WHERE key = 'identity_hmac_key'",
            [],
            |row| row.get(0),
        )
        .map_err(LedgerError::Database)?;
    let existing_version = connection
        .query_row(
            "SELECT value FROM ledger_privacy_meta WHERE key = 'identity_hmac_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(LedgerError::Database)?;
    let version = existing_version.unwrap_or_else(|| {
        if key.len() == 36 {
            IDENTITY_HMAC_UUID_V1.to_owned()
        } else {
            IDENTITY_HMAC_KEY256_V2.to_owned()
        }
    });
    validate_identity_hmac_key(&key, &version)?;
    connection
        .execute(
            "INSERT OR IGNORE INTO ledger_privacy_meta (key, value)
             VALUES ('identity_hmac_version', ?1)",
            [&version],
        )
        .map_err(LedgerError::Database)?;
    Ok(())
}

fn agent_identity_hash(connection: &Connection, value: &str) -> Result<String, LedgerError> {
    let key = ledger_identity_key(connection)?;
    Ok(keyed_identity_hash(key.as_bytes(), "agent", value))
}

fn session_identity_hash(connection: &Connection, value: &str) -> Result<String, LedgerError> {
    let key = ledger_identity_key(connection)?;
    Ok(keyed_identity_hash(key.as_bytes(), "session", value))
}

fn session_identity_hashes(
    connection: &Connection,
    value: &str,
) -> Result<(String, String), LedgerError> {
    Ok((
        session_identity_hash(connection, value)?,
        privacy_identity_hash("session", value),
    ))
}

fn accounting_policy_predicate(include_legacy_versions: bool) -> String {
    if include_legacy_versions {
        "1 = 1".into()
    } else {
        format!("accounting_policy_version = '{CURRENT_ACCOUNTING_POLICY_VERSION}'")
    }
}

fn privacy_safe_family(value: &str) -> String {
    match value {
        "read" | "search" | "rgai" | "write" | "memory" | "codec" | "build" | "cargo" | "git"
        | "rg" | "grep" | "sed" | "cat" | "find" | "fd" | "python" | "python3" | "sh" | "bash"
        | "zsh" | "ssh" | "gh" | "bun" | "npm" | "pnpm" | "yarn" | "go" | "rustc" | "rustup"
        | "curl" | "wget" | "docker" | "podman" | "jq" | "awk" | "tar" | "node" | "deno"
        | "dotnet" | "native" => value.to_owned(),
        _ => "other".to_owned(),
    }
}

fn safe_recorded_command(family: &str, route: OperationRoute) -> String {
    match route {
        OperationRoute::Optimized => format!("rtk {family}"),
        OperationRoute::Bypassed => format!("rtk raw {family}"),
        OperationRoute::NativeUnaccounted => format!("native {family}"),
    }
}

fn privacy_safe_agent(agent: Option<&str>) -> Option<String> {
    agent.map(|value| {
        let host = value.split_once(':').map_or(value, |(host, _)| host);
        match host {
            "codex" | "claude-code" | "cursor" | "mcp" | "cli" | "hook" | "test" => host.to_owned(),
            _ => "other".to_owned(),
        }
    })
}

/// One-time compatibility recovery for sensitive legacy rows before their payload is scrubbed.
/// New producers must provide canonical typed attribution; this fallback is never used for a new
/// operation and therefore cannot become a second policy authority.
fn infer_legacy_evasion(command: &str, route: OperationRoute) -> Option<EvasionAttribution> {
    let lower = command.to_ascii_lowercase();
    let hatch_marker = lower.contains("hzr_raw_fidelity=1");
    let proven_equivalent = first_class_replacement(command).is_some()
        || efficient_route_replacement(command).is_some();
    let shell_wrapper = [
        "sh -c", "sh -lc", "bash -c", "bash -lc", "zsh -c", "zsh -lc",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let interpreter = if shell_wrapper {
        Some(EvasionInterpreter::Shell)
    } else if lower.contains("python -c") || lower.contains("python3 -c") {
        Some(EvasionInterpreter::Python)
    } else if lower.contains("node -e") {
        Some(EvasionInterpreter::Javascript)
    } else if lower.contains("ruby -e") {
        Some(EvasionInterpreter::Ruby)
    } else if lower.contains("perl -e") || lower.contains("perl -ne") {
        Some(EvasionInterpreter::Perl)
    } else if lower.starts_with("awk ") {
        Some(EvasionInterpreter::Awk)
    } else if lower.starts_with("sed ") {
        Some(EvasionInterpreter::Sed)
    } else {
        None
    };
    let head = lower.split_whitespace().next().unwrap_or_default();
    // A privilege-elevation prefix is the dominant fact about a command: HZR
    // deliberately stays out of an elevation the user granted to one binary, so
    // the row is a refusal to rewrite rather than an avoidable agent choice.
    let privileged_prefix = matches!(
        head.rsplit('/').next().unwrap_or(head),
        "sudo" | "doas" | "pkexec"
    );
    let path_form = if head.starts_with("/bin/") || head.starts_with("/usr/bin/") {
        EvasionPathForm::AbsoluteSystem
    } else if head.starts_with("./") || head.starts_with("../") {
        EvasionPathForm::Relative
    } else {
        EvasionPathForm::Bare
    };
    let stage_count = lower
        .matches('|')
        .count()
        .saturating_add(lower.matches(';').count())
        + 1;
    let diagnostic = lower.contains("hzr stats") && lower.contains("--all")
        || lower.contains("ledger/hzr.sqlite")
        || lower.contains("ledger.sqlite");
    let nested_reader = (lower.contains("find ") || lower.contains("xargs "))
        && [" cat", " nl", " head", " tail"]
            .iter()
            .any(|reader| lower.contains(reader));
    let quoted_covered = (lower.contains('\'') || lower.contains('"'))
        && [" cat ", " nl ", " sed ", " rg ", " grep "]
            .iter()
            .any(|reader| format!(" {lower} ").contains(reader));
    let class = if privileged_prefix {
        EvasionClass::E11PrivilegedPrefix
    } else if hatch_marker {
        EvasionClass::E7FidelityHatch
    } else if diagnostic {
        EvasionClass::E9DiagnosticBypass
    } else if nested_reader {
        EvasionClass::E6NestedUnboundedReader
    } else if shell_wrapper {
        EvasionClass::E2ShellWrapper
    } else if interpreter.is_some() {
        EvasionClass::E3InterpreterRead
    } else if path_form != EvasionPathForm::Bare {
        EvasionClass::E4ExecutablePath
    } else if stage_count > 1 || lower.contains('>') {
        EvasionClass::E5PipelineOrRedirect
    } else if quoted_covered {
        EvasionClass::E1QuotedCoveredCommand
    } else if route == OperationRoute::Bypassed {
        EvasionClass::E10CapabilityGap
    } else {
        return None;
    };
    let avoidable = !matches!(
        class,
        EvasionClass::E10CapabilityGap | EvasionClass::E11PrivilegedPrefix
    );
    let fidelity_reason = [
        ("binary", FidelityReason::Binary),
        ("checksum", FidelityReason::Checksum),
        ("machine_protocol", FidelityReason::MachineProtocol),
        ("complete_log", FidelityReason::CompleteLog),
        ("full_patch", FidelityReason::FullPatch),
        ("verbatim_source", FidelityReason::VerbatimSource),
    ]
    .into_iter()
    .find_map(|(name, reason)| {
        lower
            .contains(&format!("hzr_raw_fidelity_reason={name}"))
            .then_some(reason)
    });
    let reason_fits = fidelity_reason.is_some_and(|reason| match reason {
        FidelityReason::Checksum => ["sha256", "sha512", "shasum", "md5sum"]
            .iter()
            .any(|needle| lower.contains(needle)),
        FidelityReason::MachineProtocol => ["--json", "--csv", "--porcelain", "-0", "--null"]
            .iter()
            .any(|needle| lower.contains(needle)),
        FidelityReason::CompleteLog => lower.contains(" log") || lower.contains("logs"),
        FidelityReason::FullPatch => lower.contains(" diff") || lower.contains(" patch"),
        FidelityReason::Binary => ["file ", "xxd ", "hexdump ", "base64 "]
            .iter()
            .any(|needle| lower.contains(needle)),
        FidelityReason::VerbatimSource => [" cat ", " read ", " sed ", "head ", "tail "]
            .iter()
            .any(|needle| format!(" {lower} ").contains(needle)),
    });
    let fidelity_validation = if !hatch_marker {
        FidelityValidation::NotRequested
    } else if proven_equivalent {
        FidelityValidation::ProvenEquivalent
    } else if fidelity_reason.is_none() {
        FidelityValidation::MissingReason
    } else if !reason_fits {
        FidelityValidation::Contradicted
    } else {
        FidelityValidation::Valid
    };
    Some(EvasionAttribution {
        class,
        wrapper_depth: u8::from(shell_wrapper),
        interpreter,
        path_form,
        stage_count: u16::try_from(stage_count).unwrap_or(u16::MAX),
        hatch_marker,
        avoidable,
        tier: if hatch_marker && fidelity_validation != FidelityValidation::Valid {
            EnforcementTier::T4HatchQuarantine
        } else if matches!(
            class,
            EvasionClass::E1QuotedCoveredCommand
                | EvasionClass::E2ShellWrapper
                | EvasionClass::E3InterpreterRead
                | EvasionClass::E4ExecutablePath
                | EvasionClass::E5PipelineOrRedirect
        ) {
            EnforcementTier::T1NamedCorrection
        } else {
            EnforcementTier::T0TransparentRewrite
        },
        fidelity_reason,
        fidelity_validation,
    })
}

fn project_hashes(project_path: &str) -> (String, String) {
    if project_path.is_empty() || project_path == "[redacted]" {
        return (String::new(), String::new());
    }
    let exact = privacy_identity_hash("project", project_path);
    let scopes = Path::new(project_path)
        .ancestors()
        .filter_map(Path::to_str)
        .filter(|value| !value.is_empty())
        .map(|value| privacy_identity_hash("project", value))
        .collect::<Vec<_>>()
        .join("|");
    (exact, scopes)
}

fn scrub_sensitive_ledger_payloads(connection: &Connection) -> Result<(), LedgerError> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(LedgerError::Database)?;
    {
        let mut statement = transaction
            .prepare(
                "SELECT id, original_cmd, rtk_cmd, project_path, session_id, agent, route,
                        operation_kind, producer_version, accounting_policy_version
                   FROM commands
                  WHERE accounting_policy_version IS NULL
                     OR original_cmd NOT LIKE '[redacted:%'
                     OR project_path NOT IN ('', '[redacted]')
                     OR session_id IS NOT NULL
                     OR COALESCE(agent, '') NOT IN ('', 'codex', 'claude-code', 'cursor',
                                                    'mcp', 'cli', 'hook', 'test', 'other')",
            )
            .map_err(LedgerError::Database)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            })
            .map_err(LedgerError::Database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LedgerError::Database)?;
        drop(statement);
        for (
            id,
            original,
            recorded,
            project,
            session,
            agent,
            stored_route,
            stored_operation,
            producer_version,
            policy_version,
        ) in rows
        {
            let classification = classify_operation(&recorded);
            let route = route_from_ledger(stored_route.as_deref(), classification.route);
            let family = privacy_safe_family(
                stored_operation
                    .as_deref()
                    .unwrap_or(&classification.operation),
            );
            let command_hash = privacy_identity_hash("command", &format!("{original}\0{recorded}"));
            let (project_hash, scope_hashes) = project_hashes(&project);
            let session_hash = session
                .as_deref()
                .map(|value| session_identity_hash(&transaction, value))
                .transpose()?;
            let agent_hash = agent
                .as_deref()
                .map(|value| agent_identity_hash(&transaction, value))
                .transpose()?;
            let evasion = infer_legacy_evasion(&recorded, route);
            transaction
                .execute(
                    "UPDATE commands
                        SET original_cmd = ?1, rtk_cmd = ?2, project_path = ?3,
                            session_id = NULL, operation_family = ?4, command_hash = ?5,
                            project_hash = ?6, project_scope_hashes = ?7, session_hash = ?8,
                            agent = ?9, producer_version = ?10, accounting_policy_version = ?11,
                            route = COALESCE(route, ?12), agent_hash = ?13,
                            evasion_class = ?14, wrapper_depth = ?15,
                            interpreter_kind = ?16, path_form = ?17, stage_count = ?18,
                            hatch_marker = ?19, avoidable = ?20, enforcement_tier = ?21,
                            fidelity_reason = ?22, fidelity_validation = ?23
                      WHERE id = ?24",
                    params![
                        format!("[redacted:{family}]"),
                        safe_recorded_command(&family, route),
                        if project.is_empty() { "" } else { "[redacted]" },
                        family,
                        command_hash,
                        project_hash,
                        scope_hashes,
                        session_hash,
                        privacy_safe_agent(agent.as_deref()),
                        producer_version.as_deref().unwrap_or("legacy"),
                        policy_version.as_deref().unwrap_or("legacy_scrubbed_v1"),
                        route.as_str(),
                        agent_hash,
                        evasion.map(|value| value.class.as_str()),
                        evasion.map(|value| value.wrapper_depth),
                        evasion.and_then(|value| value.interpreter.map(|kind| kind.as_str())),
                        evasion.map(|value| value.path_form.as_str()),
                        evasion.map(|value| value.stage_count),
                        evasion.map(|value| value.hatch_marker),
                        evasion.map(|value| value.avoidable),
                        evasion.map(|value| value.tier.as_str()),
                        evasion
                            .and_then(|value| value.fidelity_reason.map(|reason| reason.as_str())),
                        evasion.map(|value| value.fidelity_validation.as_str()),
                        id,
                    ],
                )
                .map_err(LedgerError::Database)?;
        }
    }
    {
        let mut statement = transaction
            .prepare(
                "SELECT id, raw_command, error_message, producer_version,
                        accounting_policy_version
                   FROM parse_failures
                  WHERE accounting_policy_version IS NULL
                     OR raw_command != '[redacted]'
                     OR error_message != '[redacted]'",
            )
            .map_err(LedgerError::Database)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(LedgerError::Database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LedgerError::Database)?;
        drop(statement);
        for (id, command, error, producer, policy) in rows {
            transaction
                .execute(
                    "UPDATE parse_failures
                        SET raw_command = '[redacted]', error_message = '[redacted]',
                            command_hash = ?1, error_hash = ?2,
                            producer_version = ?3, accounting_policy_version = ?4
                      WHERE id = ?5",
                    params![
                        privacy_identity_hash("parse-command", &command),
                        privacy_identity_hash("parse-error", &error),
                        producer.as_deref().unwrap_or("legacy"),
                        policy.as_deref().unwrap_or("legacy_scrubbed_v1"),
                        id,
                    ],
                )
                .map_err(LedgerError::Database)?;
        }
    }
    {
        let mut statement = transaction
            .prepare(
                "SELECT trace_id, project_path, producer_version FROM usage_records
                  WHERE project_path NOT IN ('', '[redacted]') OR producer_version IS NULL",
            )
            .map_err(LedgerError::Database)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(LedgerError::Database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LedgerError::Database)?;
        drop(statement);
        for (trace_id, project, producer) in rows {
            let (project_hash, scope_hashes) = project_hashes(&project);
            transaction
                .execute(
                    "UPDATE usage_records
                        SET project_path = ?1, project_hash = ?2, project_scope_hashes = ?3,
                            producer_version = ?4
                      WHERE trace_id = ?5",
                    params![
                        if project.is_empty() { "" } else { "[redacted]" },
                        project_hash,
                        scope_hashes,
                        producer.as_deref().unwrap_or("legacy"),
                        trace_id,
                    ],
                )
                .map_err(LedgerError::Database)?;
        }
    }
    transaction.commit().map_err(LedgerError::Database)
}

impl Ledger {
    pub fn privacy_pseudonymizer(&self) -> Result<PrivacyPseudonymizer, LedgerError> {
        Ok(PrivacyPseudonymizer {
            key: ledger_identity_key(&self.connection)?,
        })
    }

    /// Read dashboard totals without creating or migrating the ledger.
    ///
    /// The visualizer endpoint is GET-only, so a fresh installation with no ledger file
    /// returns zero totals instead of turning a read into an implicit database write.
    pub fn summaries_read_only(
        path: &Path,
    ) -> Result<(LedgerSummary, EfficiencySummary), LedgerError> {
        if !path.is_file() {
            return Ok((LedgerSummary::default(), EfficiencySummary::default()));
        }
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(LedgerError::Database)?;
        connection
            .busy_timeout(std::time::Duration::from_millis(250))
            .map_err(LedgerError::Database)?;
        let ledger = Self { connection };
        Ok((ledger.summary()?, ledger.efficiency_summary()?))
    }

    /// Collect a complete stats snapshot without schema writes or migrations.
    /// The daemon/installer owns schema reconciliation; concurrent readers must not contend on DDL.
    pub fn stats_collection_read_only(
        path: &Path,
        query: StatsQuery<'_>,
    ) -> Result<StatsCollection, LedgerError> {
        if !path.is_file() {
            return Ok(StatsCollection {
                snapshot: StatsSnapshot::default(),
            });
        }
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(LedgerError::Database)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(LedgerError::Database)?;
        Self { connection }.stats_collection(query)
    }

    /// Read exact-path local activity without creating or migrating the ledger.
    pub fn project_activity_read_only(
        path: &Path,
        project_path: &str,
    ) -> Result<ProjectActivitySummary, LedgerError> {
        if !path.is_file() {
            return Ok(ProjectActivitySummary::default());
        }
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(LedgerError::Database)?;
        connection
            .busy_timeout(std::time::Duration::from_millis(250))
            .map_err(LedgerError::Database)?;
        Self { connection }.project_activity(project_path)
    }

    pub fn open(path: &Path) -> Result<Self, LedgerError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| LedgerError::Directory {
                path: parent.to_path_buf(),
                source,
            })?;
            set_owner_only(parent, 0o700);
        }
        // Own the database file before SQLite does, so it and the -wal/-shm
        // siblings inherit owner-only mode instead of the process umask. The
        // ledger records every command an agent ran in this workspace.
        create_private_file(path);
        let connection = Connection::open(path).map_err(LedgerError::Database)?;
        restrict_db_files(path);
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA busy_timeout=5000;
                 CREATE TABLE IF NOT EXISTS usage_records (
                    trace_id TEXT PRIMARY KEY,
                    created_at_ms INTEGER NOT NULL,
                    provider TEXT,
                    model TEXT,
                    actual_input INTEGER,
                    actual_output INTEGER,
                    actual_reasoning INTEGER,
                    actual_cache_write INTEGER,
                    actual_cache_read INTEGER,
                    estimated_input INTEGER,
                    estimated_output INTEGER,
                    estimate_method TEXT,
                    turns INTEGER NOT NULL,
                    retries INTEGER NOT NULL,
                    latency_ms INTEGER NOT NULL,
                    outcome TEXT NOT NULL,
                    policy_version TEXT NOT NULL,
                    cost_microusd INTEGER,
                    project_path TEXT NOT NULL DEFAULT ''
                 );
                 CREATE INDEX IF NOT EXISTS idx_usage_created
                    ON usage_records(created_at_ms DESC);
                 CREATE TABLE IF NOT EXISTS commands (
                    id INTEGER PRIMARY KEY,
                    timestamp TEXT NOT NULL,
                    original_cmd TEXT NOT NULL,
                    rtk_cmd TEXT NOT NULL,
                    input_tokens INTEGER NOT NULL,
                    output_tokens INTEGER NOT NULL,
                    saved_tokens INTEGER NOT NULL,
                    savings_pct REAL NOT NULL,
                    exec_time_ms INTEGER DEFAULT 0,
                    project_path TEXT DEFAULT '',
                    agent TEXT,
                    session_id TEXT,
                    channel TEXT NOT NULL DEFAULT 'hook_cli',
                    measurement TEXT NOT NULL DEFAULT 'estimated',
                    route TEXT,
                    operation_kind TEXT,
                    operation_mode TEXT,
                    accounting_stage TEXT,
                    requested_mode TEXT,
                    effective_mode TEXT,
                    search_strategy TEXT,
                    search_fallback_code TEXT,
                    search_include_content INTEGER,
                    result_limit INTEGER,
                    path_scope_count INTEGER,
                    filter_level TEXT,
                    range_from INTEGER,
                    range_to INTEGER,
                    source_bytes INTEGER,
                    operation_family TEXT,
                    command_hash TEXT,
                    project_hash TEXT,
                    project_scope_hashes TEXT,
                    session_hash TEXT,
                    agent_hash TEXT,
                    producer_version TEXT,
                    accounting_policy_version TEXT,
                    evasion_class TEXT,
                    wrapper_depth INTEGER,
                    interpreter_kind TEXT,
                    path_form TEXT,
                    stage_count INTEGER,
                    hatch_marker INTEGER,
                    avoidable INTEGER,
                    enforcement_tier TEXT,
                    fidelity_reason TEXT,
                    fidelity_validation TEXT,
                    replacement_capability TEXT,
                    replacement_route TEXT,
                    replacement_reason TEXT,
                    fidelity_reservation_id TEXT
                 );
                 CREATE INDEX IF NOT EXISTS idx_timestamp ON commands(timestamp);
                 CREATE INDEX IF NOT EXISTS idx_project_path_timestamp
                    ON commands(project_path, timestamp);
                 CREATE TABLE IF NOT EXISTS parse_failures (
                    id INTEGER PRIMARY KEY,
                    timestamp TEXT NOT NULL,
                    raw_command TEXT NOT NULL,
                    error_message TEXT NOT NULL,
                    fallback_succeeded INTEGER NOT NULL DEFAULT 0,
                    command_hash TEXT,
                    error_hash TEXT,
                    producer_version TEXT,
                    accounting_policy_version TEXT
                 );
                 CREATE INDEX IF NOT EXISTS idx_pf_timestamp ON parse_failures(timestamp);
                 CREATE TABLE IF NOT EXISTS tracking_meta (
                    key TEXT PRIMARY KEY,
                    value INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS hzr_migrations (
                    key TEXT PRIMARY KEY,
                    completed_at_ms INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS ledger_privacy_meta (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS policy_events (
                    id INTEGER PRIMARY KEY,
                    timestamp TEXT NOT NULL,
                    project_hash TEXT NOT NULL DEFAULT '',
                    project_scope_hashes TEXT NOT NULL DEFAULT '',
                    session_hash TEXT,
                    agent TEXT,
                    agent_hash TEXT,
                    evasion_class TEXT NOT NULL,
                    wrapper_depth INTEGER NOT NULL,
                    interpreter_kind TEXT,
                    path_form TEXT NOT NULL,
                    stage_count INTEGER NOT NULL,
                    hatch_marker INTEGER NOT NULL,
                    avoidable INTEGER NOT NULL,
                    enforcement_tier TEXT NOT NULL,
                    fidelity_reason TEXT,
                    fidelity_validation TEXT NOT NULL,
                    decision TEXT NOT NULL,
                    replacement_family TEXT,
                    producer_version TEXT NOT NULL,
                    accounting_policy_version TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_policy_events_timestamp
                    ON policy_events(timestamp);
                 CREATE INDEX IF NOT EXISTS idx_policy_events_session
                    ON policy_events(session_hash, timestamp);
                 CREATE TABLE IF NOT EXISTS provider_economic_receipts (
                    receipt_hash TEXT PRIMARY KEY,
                    payload_hash TEXT NOT NULL,
                    created_at_ms INTEGER NOT NULL,
                    observed_at_ms INTEGER NOT NULL,
                    source TEXT NOT NULL,
                    harness TEXT NOT NULL,
                    provider TEXT NOT NULL,
                    model TEXT NOT NULL,
                    method TEXT NOT NULL,
                    currency TEXT NOT NULL,
                    session_hash TEXT NOT NULL,
                    project_hash TEXT NOT NULL DEFAULT '',
                    project_scope_hashes TEXT NOT NULL DEFAULT '',
                    baseline_usage_json TEXT NOT NULL,
                    delivered_usage_json TEXT NOT NULL,
                    invoice_baseline_microunits INTEGER,
                    invoice_delivered_microunits INTEGER,
                    public_baseline_microunits INTEGER,
                    public_delivered_microunits INTEGER,
                    price_table_identity TEXT,
                    price_entry_version TEXT,
                    public_estimate_enabled INTEGER NOT NULL,
                    producer_version TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_provider_economic_session
                    ON provider_economic_receipts(session_hash, observed_at_ms);
                 CREATE INDEX IF NOT EXISTS idx_provider_economic_project
                    ON provider_economic_receipts(project_hash, observed_at_ms);
                 CREATE TABLE IF NOT EXISTS legacy_command_imports (
                    source_id TEXT NOT NULL,
                    source_row_id INTEGER NOT NULL,
                    PRIMARY KEY (source_id, source_row_id)
                 );
                 CREATE TABLE IF NOT EXISTS legacy_parse_failure_imports (
                    source_id TEXT NOT NULL,
                    source_row_id INTEGER NOT NULL,
                    PRIMARY KEY (source_id, source_row_id)
                 );",
            )
            .map_err(LedgerError::Database)?;
        initialize_identity_hmac(&connection)?;
        let _ = connection.execute("ALTER TABLE commands ADD COLUMN agent TEXT", []);
        let _ = connection.execute("ALTER TABLE commands ADD COLUMN session_id TEXT", []);
        let _ = connection.execute(
            "ALTER TABLE commands ADD COLUMN channel TEXT NOT NULL DEFAULT 'hook_cli'",
            [],
        );
        let _ = connection.execute(
            "ALTER TABLE commands ADD COLUMN measurement TEXT NOT NULL DEFAULT 'estimated'",
            [],
        );
        let _ = connection.execute("ALTER TABLE commands ADD COLUMN route TEXT", []);
        for column in [
            "operation_kind TEXT",
            "operation_mode TEXT",
            "accounting_stage TEXT",
            "requested_mode TEXT",
            "effective_mode TEXT",
            "search_strategy TEXT",
            "search_fallback_code TEXT",
            "search_include_content INTEGER",
            "result_limit INTEGER",
            "path_scope_count INTEGER",
            "filter_level TEXT",
            "range_from INTEGER",
            "range_to INTEGER",
            "source_bytes INTEGER",
            "operation_family TEXT",
            "command_hash TEXT",
            "project_hash TEXT",
            "project_scope_hashes TEXT",
            "session_hash TEXT",
            "agent_hash TEXT",
            "producer_version TEXT",
            "accounting_policy_version TEXT",
            "evasion_class TEXT",
            "wrapper_depth INTEGER",
            "interpreter_kind TEXT",
            "path_form TEXT",
            "stage_count INTEGER",
            "hatch_marker INTEGER",
            "avoidable INTEGER",
            "enforcement_tier TEXT",
            "fidelity_reason TEXT",
            "fidelity_validation TEXT",
            "replacement_capability TEXT",
            "replacement_route TEXT",
            "replacement_reason TEXT",
            "fidelity_reservation_id TEXT",
        ] {
            let _ = connection.execute(&format!("ALTER TABLE commands ADD COLUMN {column}"), []);
        }
        connection
            .execute(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_commands_fidelity_reservation
                   ON commands(fidelity_reservation_id)
                 WHERE fidelity_reservation_id IS NOT NULL",
                [],
            )
            .map_err(LedgerError::Database)?;
        // Идемпотентно: существующие БД получают колонку; повторный ALTER безопасно игнорируется.
        let _ = connection.execute(
            "ALTER TABLE usage_records ADD COLUMN project_path TEXT NOT NULL DEFAULT ''",
            [],
        );
        let _ = connection.execute("ALTER TABLE usage_records ADD COLUMN project_hash TEXT", []);
        let _ = connection.execute(
            "ALTER TABLE usage_records ADD COLUMN project_scope_hashes TEXT",
            [],
        );
        let _ = connection.execute(
            "ALTER TABLE usage_records ADD COLUMN producer_version TEXT",
            [],
        );
        for column in [
            "command_hash TEXT",
            "error_hash TEXT",
            "producer_version TEXT",
            "accounting_policy_version TEXT",
        ] {
            let _ = connection.execute(
                &format!("ALTER TABLE parse_failures ADD COLUMN {column}"),
                [],
            );
        }
        migrate_legacy_ledgers(&connection, path)?;
        scrub_sensitive_ledger_payloads(&connection)?;
        Ok(Self { connection })
    }

    pub fn record(&self, record: &LedgerRecord) -> Result<(), LedgerError> {
        let (project_hash, project_scope_hashes) = project_hashes(&record.project_path);
        self.connection
            .execute(
                "INSERT INTO usage_records (
                    trace_id, created_at_ms, provider, model,
                    actual_input, actual_output, actual_reasoning,
                    actual_cache_write, actual_cache_read,
                    estimated_input, estimated_output, estimate_method,
                    turns, retries, latency_ms, outcome, policy_version, cost_microusd,
                    project_path, project_hash, project_scope_hashes, producer_version
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                    ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22
                 ) ON CONFLICT(trace_id) DO NOTHING",
                params![
                    record.trace_id.as_str(),
                    now_ms(),
                    record.provider.as_deref(),
                    record.model.as_deref(),
                    record.usage.actual.input_tokens,
                    record.usage.actual.output_tokens,
                    record.usage.actual.reasoning_tokens,
                    record.usage.actual.cache_write_tokens,
                    record.usage.actual.cache_read_tokens,
                    record.usage.estimated.input_tokens,
                    record.usage.estimated.output_tokens,
                    record.usage.estimated.method.as_deref(),
                    record.turns,
                    record.retries,
                    record.latency_ms,
                    record.outcome.as_str(),
                    record.policy_version.as_str(),
                    record.cost_microusd,
                    if record.project_path.is_empty() {
                        ""
                    } else {
                        "[redacted]"
                    },
                    project_hash,
                    project_scope_hashes,
                    CURRENT_PRODUCER_VERSION,
                ],
            )
            .map_err(LedgerError::Database)?;
        Ok(())
    }

    pub fn record_provider_receipt(
        &self,
        receipt: &ProviderEconomicReceipt,
        catalog: &PricingCatalog,
    ) -> Result<ProviderReceiptRecordResult, LedgerError> {
        validate_receipt(receipt).map_err(LedgerError::Billing)?;
        let key = ledger_identity_key(&self.connection)?;
        let public_payload_hash = receipt_payload_hash(receipt).map_err(LedgerError::Billing)?;
        let payload_hash = keyed_identity_hash(
            key.as_bytes(),
            "provider-receipt-payload",
            &public_payload_hash,
        );
        let receipt_hash =
            keyed_identity_hash(key.as_bytes(), "provider-receipt", &receipt.receipt_id);
        let existing = self
            .connection
            .query_row(
                "SELECT payload_hash FROM provider_economic_receipts WHERE receipt_hash = ?1",
                [&receipt_hash],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(LedgerError::Database)?;
        let invoice_actual = match (
            receipt.actual_baseline_cost_microunits,
            receipt.actual_delivered_cost_microunits,
        ) {
            (Some(baseline), Some(delivered)) => Some(EconomicAmount {
                currency: receipt.currency.clone(),
                baseline_microunits: baseline,
                delivered_microunits: delivered,
                savings_microunits: signed_u64_difference(baseline, delivered)?,
            }),
            (None, None) => None,
            _ => {
                return Err(LedgerError::Billing(BillingError::InvalidReceipt(
                    "actual provider cost requires both baseline and delivered amounts".into(),
                )));
            }
        };
        let (public_estimate, unavailable_reason) = if receipt.enable_public_estimate {
            match price_receipt(catalog, receipt) {
                Ok(estimate) => (Some(estimate), None),
                Err(BillingError::PricingUnavailable(reason)) => (None, Some(reason)),
                Err(error) => return Err(LedgerError::Billing(error)),
            }
        } else {
            (
                None,
                Some("public pricing estimate is disabled for this receipt".into()),
            )
        };
        if let Some(existing) = existing {
            if existing != payload_hash {
                return Err(LedgerError::InvalidOperation(
                    "provider receipt id was already used with different content".into(),
                ));
            }
            return Ok(ProviderReceiptRecordResult {
                recorded: false,
                idempotent_replay: true,
                receipt_hash,
                invoice_actual,
                public_estimate,
                unavailable_reason,
            });
        }
        let (project_hash, project_scope_hashes) = project_hashes(&receipt.project_path);
        let session_hash = session_identity_hash(&self.connection, &receipt.session_id)?;
        let baseline_json =
            serde_json::to_string(&receipt.baseline).map_err(LedgerError::Serialize)?;
        let delivered_json =
            serde_json::to_string(&receipt.delivered).map_err(LedgerError::Serialize)?;
        self.connection
            .execute(
                "INSERT INTO provider_economic_receipts (
                    receipt_hash, payload_hash, created_at_ms, observed_at_ms, source,
                    harness, provider, model, method, currency, session_hash,
                    project_hash, project_scope_hashes, baseline_usage_json, delivered_usage_json,
                    invoice_baseline_microunits, invoice_delivered_microunits,
                    public_baseline_microunits, public_delivered_microunits,
                    price_table_identity, price_entry_version, public_estimate_enabled,
                    producer_version
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                    ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
                params![
                    receipt_hash,
                    payload_hash,
                    now_ms(),
                    receipt.observed_at_ms,
                    receipt.source,
                    receipt.harness,
                    receipt.provider,
                    receipt.model,
                    receipt.method,
                    receipt.currency,
                    session_hash,
                    project_hash,
                    project_scope_hashes,
                    baseline_json,
                    delivered_json,
                    invoice_actual
                        .as_ref()
                        .map(|value| value.baseline_microunits),
                    invoice_actual
                        .as_ref()
                        .map(|value| value.delivered_microunits),
                    public_estimate
                        .as_ref()
                        .map(|value| value.amount.baseline_microunits),
                    public_estimate
                        .as_ref()
                        .map(|value| value.amount.delivered_microunits),
                    public_estimate
                        .as_ref()
                        .map(|value| value.price_table_identity.as_str()),
                    public_estimate
                        .as_ref()
                        .map(|value| value.entry_version.as_str()),
                    receipt.enable_public_estimate,
                    CURRENT_PRODUCER_VERSION,
                ],
            )
            .map_err(LedgerError::Database)?;
        Ok(ProviderReceiptRecordResult {
            recorded: true,
            idempotent_replay: false,
            receipt_hash,
            invoice_actual,
            public_estimate,
            unavailable_reason,
        })
    }

    pub fn session_economic_summary(
        &self,
        session_id: &str,
    ) -> Result<SessionEconomicSummary, LedgerError> {
        let (session_hash, legacy_session_hash) =
            session_identity_hashes(&self.connection, session_id)?;
        let rows = self
            .connection
            .prepare_cached(
                "SELECT currency,
                        invoice_baseline_microunits, invoice_delivered_microunits,
                        public_baseline_microunits, public_delivered_microunits,
                        price_table_identity
                   FROM provider_economic_receipts
                  WHERE session_hash IN (?1, ?2)",
            )
            .map_err(LedgerError::Database)?
            .query_map(params![session_hash, legacy_session_hash], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<u64>>(1)?,
                    row.get::<_, Option<u64>>(2)?,
                    row.get::<_, Option<u64>>(3)?,
                    row.get::<_, Option<u64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(LedgerError::Database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LedgerError::Database)?;
        aggregate_economic_rows(&rows)
    }

    pub fn find(&self, trace_id: &TraceId) -> Result<Option<LedgerRecord>, LedgerError> {
        self.connection
            .query_row(
                "SELECT provider, model, actual_input, actual_output, actual_reasoning,
                        actual_cache_write, actual_cache_read, estimated_input,
                        estimated_output, estimate_method, turns, retries, latency_ms,
                        outcome, policy_version, cost_microusd, project_path
                   FROM usage_records WHERE trace_id = ?1",
                [trace_id.as_str()],
                |row| {
                    Ok(LedgerRecord {
                        trace_id: trace_id.clone(),
                        provider: row.get(0)?,
                        model: row.get(1)?,
                        usage: Usage {
                            actual: hzr_protocol::ActualUsage {
                                input_tokens: row.get(2)?,
                                output_tokens: row.get(3)?,
                                reasoning_tokens: row.get(4)?,
                                cache_write_tokens: row.get(5)?,
                                cache_read_tokens: row.get(6)?,
                            },
                            estimated: hzr_protocol::EstimatedUsage {
                                input_tokens: row.get(7)?,
                                output_tokens: row.get(8)?,
                                method: row.get(9)?,
                            },
                        },
                        turns: row.get(10)?,
                        retries: row.get(11)?,
                        latency_ms: row.get(12)?,
                        outcome: row.get(13)?,
                        policy_version: row.get(14)?,
                        cost_microusd: row.get(15)?,
                        project_path: row.get(16)?,
                    })
                },
            )
            .optional()
            .map_err(LedgerError::Database)
    }

    pub fn summary(&self) -> Result<LedgerSummary, LedgerError> {
        self.summary_scoped(None, None)
    }

    /// Суммирует только чеки с совпадающим project_path; пустые (legacy) строки не входят.
    pub fn summary_for_project(&self, project_path: &str) -> Result<LedgerSummary, LedgerError> {
        self.summary_scoped(Some(project_path), None)
    }

    /// Collect every public stats section against one immutable scope and cutoff.
    pub fn stats_snapshot(&self, query: StatsQuery<'_>) -> Result<StatsSnapshot, LedgerError> {
        Ok(self.stats_collection(query)?.snapshot)
    }

    pub fn stats_collection(&self, query: StatsQuery<'_>) -> Result<StatsCollection, LedgerError> {
        let by_family = self.operation_family_summary(
            query.project_path,
            query.since_unix_seconds,
            query.include_legacy_versions,
        )?;
        Ok(StatsCollection {
            snapshot: StatsSnapshot {
                efficiency: self.efficiency_summary_scoped(
                    query.project_path,
                    query.since_unix_seconds,
                    query.include_legacy_versions,
                )?,
                bypass: self.bypass_summary_scoped(
                    query.project_path,
                    query.since_unix_seconds,
                    query.include_legacy_versions,
                )?,
                provider_usage: self
                    .summary_scoped(query.project_path, query.since_unix_seconds)?,
                by_family,
                evasion: self.evasion_summary(query)?,
            },
        })
    }

    /// Aggregate evasion evidence without returning commands, paths, arguments, or identities.
    pub fn evasion_summary(&self, query: StatsQuery<'_>) -> Result<EvasionSummary, LedgerError> {
        let version_predicate = accounting_policy_predicate(query.include_legacy_versions);
        let sql = format!(
            "SELECT evasion_class, COUNT(*), COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(CASE WHEN avoidable = 1 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN avoidable = 1 THEN output_tokens ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN hatch_marker = 1 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN hatch_marker = 1 THEN output_tokens ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN hatch_marker = 1 AND fidelity_validation NOT IN
                        ('valid', 'not_requested') THEN 1 ELSE 0 END), 0)
               FROM commands
              WHERE evasion_class IS NOT NULL
                AND (?1 IS NULL OR instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0)
                AND (?2 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?2)
                AND ({version_predicate})
              GROUP BY evasion_class"
        );
        let project_hash = query
            .project_path
            .map(|value| privacy_identity_hash("project", value));
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(LedgerError::Database)?;
        let rows = statement
            .query_map(params![project_hash, query.since_unix_seconds], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u64>(4)?,
                    row.get::<_, u64>(5)?,
                    row.get::<_, u64>(6)?,
                    row.get::<_, u64>(7)?,
                ))
            })
            .map_err(LedgerError::Database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LedgerError::Database)?;
        let mut summary = EvasionSummary {
            default_allowance: FidelityAllowance::default(),
            ..EvasionSummary::default()
        };
        for (
            class,
            operations,
            delivered,
            avoidable_operations,
            avoidable_tokens,
            fidelity_ops,
            fidelity_tokens,
            invalid,
        ) in rows
        {
            let Some(class) = parse_evasion_class(&class) else {
                continue;
            };
            summary.by_class.push(EvasionClassSummary {
                class,
                operations,
                delivered_tokens: delivered,
                avoidable_operations,
                avoidable_tokens,
            });
            summary.fidelity_operations = summary.fidelity_operations.saturating_add(fidelity_ops);
            summary.fidelity_delivered_tokens = summary
                .fidelity_delivered_tokens
                .saturating_add(fidelity_tokens);
            summary.fidelity_invalid_operations =
                summary.fidelity_invalid_operations.saturating_add(invalid);
        }
        summary.by_class.sort_by(|left, right| {
            right
                .delivered_tokens
                .cmp(&left.delivered_tokens)
                .then_with(|| left.class.cmp(&right.class))
        });
        let policy_sql = format!(
            "SELECT evasion_class, decision, COUNT(*),
                    COALESCE(SUM(CASE WHEN avoidable = 1 THEN 1 ELSE 0 END), 0)
               FROM policy_events
              WHERE (?1 IS NULL OR instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0)
                AND (?2 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?2)
                AND ({version_predicate})
              GROUP BY evasion_class, decision"
        );
        let mut policy_statement = self
            .connection
            .prepare(&policy_sql)
            .map_err(LedgerError::Database)?;
        let policy_rows = policy_statement
            .query_map(params![project_hash, query.since_unix_seconds], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                ))
            })
            .map_err(LedgerError::Database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LedgerError::Database)?;
        for (class, decision, attempts, avoidable_attempts) in policy_rows {
            let (Some(class), Some(decision)) = (
                parse_evasion_class(&class),
                parse_policy_decision(&decision),
            ) else {
                continue;
            };
            summary.policy_attempts = summary.policy_attempts.saturating_add(attempts);
            summary.policy_by_class.push(PolicyEventSummary {
                class,
                decision,
                attempts,
                avoidable_attempts,
            });
        }
        summary.policy_by_class.sort_by(|left, right| {
            right
                .attempts
                .cmp(&left.attempts)
                .then_with(|| left.class.cmp(&right.class))
                .then_with(|| left.decision.cmp(&right.decision))
        });
        Ok(summary)
    }

    /// Current per-session allowance state. The raw session identifier is used only to derive a
    /// domain-separated hash and is never returned or persisted by this query.
    pub fn fidelity_session_usage(
        &self,
        session_id: &str,
        allowance: FidelityAllowance,
    ) -> Result<FidelitySessionUsage, LedgerError> {
        let (session_hash, legacy_session_hash) =
            session_identity_hashes(&self.connection, session_id)?;
        let (operations, delivered_tokens) = self
            .connection
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(output_tokens), 0) FROM commands
              WHERE session_hash IN (?1, ?2) AND hatch_marker = 1
                AND accounting_policy_version IN (?3, ?4)",
                params![
                    session_hash,
                    legacy_session_hash,
                    CURRENT_ACCOUNTING_POLICY_VERSION,
                    LEGACY_ACCOUNTING_POLICY_VERSION_V1
                ],
                |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
            )
            .map_err(LedgerError::Database)?;
        Ok(FidelitySessionUsage {
            operations,
            delivered_tokens,
            remaining_operations: allowance.max_operations.saturating_sub(operations),
            remaining_tokens: allowance
                .max_delivered_tokens
                .saturating_sub(delivered_tokens),
            exhausted: operations >= allowance.max_operations
                || delivered_tokens >= allowance.max_delivered_tokens,
        })
    }

    pub fn session_efficiency_summary(
        &self,
        session_id: &str,
    ) -> Result<SessionEfficiencySummary, LedgerError> {
        let (session_hash, legacy_session_hash) =
            session_identity_hashes(&self.connection, session_id)?;
        let raw_predicate = raw_route_sql_predicate("rtk_cmd");
        let neutral_predicate = format!("({raw_predicate}) OR rtk_cmd = 'rtk write'");
        let measured_scope = "session_hash IN (?1, ?2)
               AND accounting_policy_version = ?3
               AND measurement = 'estimated'
               AND COALESCE(route, '') != 'native_unaccounted'
               AND COALESCE(accounting_stage, 'internal_transport') NOT IN ('final_delivery', 'control_plane')";
        let totals_query = format!(
            "SELECT
                COUNT(*),
                COALESCE(SUM(CASE WHEN ({neutral_predicate})
                                  THEN output_tokens ELSE input_tokens END), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(CASE WHEN NOT ({neutral_predicate}) AND input_tokens > output_tokens
                                  THEN input_tokens - output_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN NOT ({neutral_predicate}) AND output_tokens > input_tokens
                                  THEN output_tokens - input_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN ({neutral_predicate})
                                  THEN 0 ELSE input_tokens - output_tokens END), 0)
             FROM commands WHERE {measured_scope}"
        );
        let mut summary = self
            .connection
            .query_row(
                &totals_query,
                params![
                    session_hash,
                    legacy_session_hash,
                    CURRENT_ACCOUNTING_POLICY_VERSION
                ],
                |row| {
                    Ok(SessionEfficiencySummary {
                        operations: row.get(0)?,
                        baseline_tokens_estimated: row.get(1)?,
                        delivered_tokens_estimated: row.get(2)?,
                        gross_avoided_tokens_estimated: row.get(3)?,
                        regression_tokens_estimated: row.get(4)?,
                        net_avoided_tokens_estimated: row.get(5)?,
                        top_commands: Vec::new(),
                    })
                },
            )
            .map_err(LedgerError::Database)?;
        let commands_query = format!(
            "SELECT
                rtk_cmd,
                COUNT(*),
                COALESCE(SUM(CASE WHEN ({neutral_predicate})
                                  THEN output_tokens ELSE input_tokens END), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(CASE WHEN NOT ({neutral_predicate}) AND input_tokens > output_tokens
                                  THEN input_tokens - output_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN NOT ({neutral_predicate}) AND output_tokens > input_tokens
                                  THEN output_tokens - input_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN ({neutral_predicate})
                                  THEN 0 ELSE input_tokens - output_tokens END), 0),
                CAST(COALESCE(AVG(exec_time_ms), 0) AS INTEGER)
             FROM commands WHERE {measured_scope}
             GROUP BY rtk_cmd
             ORDER BY COUNT(*) DESC,
                      SUM(CASE WHEN ({neutral_predicate})
                               THEN 0 ELSE input_tokens - output_tokens END) DESC,
                      rtk_cmd
             LIMIT 3"
        );
        let mut statement = self
            .connection
            .prepare_cached(&commands_query)
            .map_err(LedgerError::Database)?;
        summary.top_commands = statement
            .query_map(
                params![
                    session_hash,
                    legacy_session_hash,
                    CURRENT_ACCOUNTING_POLICY_VERSION
                ],
                |row| {
                    Ok(EfficiencyCommandSummary {
                        command: row.get(0)?,
                        executions: row.get(1)?,
                        baseline_tokens_estimated: row.get(2)?,
                        delivered_tokens_estimated: row.get(3)?,
                        gross_avoided_tokens_estimated: row.get(4)?,
                        regression_tokens_estimated: row.get(5)?,
                        net_avoided_tokens_estimated: row.get(6)?,
                        avg_time_ms: row.get(7)?,
                    })
                },
            )
            .map_err(LedgerError::Database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LedgerError::Database)?;
        Ok(summary)
    }

    pub fn session_evasion_summary(
        &self,
        session_id: &str,
        allowance: FidelityAllowance,
    ) -> Result<SessionEvasionSummary, LedgerError> {
        let (session_hash, legacy_session_hash) =
            session_identity_hashes(&self.connection, session_id)?;
        let mut summary = self
            .connection
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(CASE WHEN avoidable = 1 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN avoidable = 1 THEN output_tokens ELSE 0 END), 0),
                    MAX(agent), MAX(agent_hash)
               FROM commands WHERE session_hash IN (?1, ?2) AND accounting_policy_version = ?3",
                params![
                    session_hash,
                    legacy_session_hash,
                    CURRENT_ACCOUNTING_POLICY_VERSION
                ],
                |row| {
                    Ok(SessionEvasionSummary {
                        agent: row.get(4)?,
                        agent_hash: row.get(5)?,
                        session_hash: session_hash.clone(),
                        operations: row.get(0)?,
                        delivered_tokens: row.get(1)?,
                        avoidable_operations: row.get(2)?,
                        avoidable_tokens: row.get(3)?,
                        ..SessionEvasionSummary::default()
                    })
                },
            )
            .map_err(LedgerError::Database)?;
        summary.avoidable_share_pct =
            percentage_of(summary.avoidable_tokens, summary.delivered_tokens);
        summary.recoverable_tokens = summary.avoidable_tokens;
        summary.fidelity = self.fidelity_session_usage(session_id, allowance)?;
        summary.top_class = self
            .connection
            .query_row(
                "SELECT evasion_class FROM commands WHERE session_hash IN (?1, ?2)
                AND evasion_class IS NOT NULL AND accounting_policy_version = ?3
              GROUP BY evasion_class ORDER BY SUM(output_tokens) DESC, evasion_class LIMIT 1",
                params![
                    session_hash,
                    legacy_session_hash,
                    CURRENT_ACCOUNTING_POLICY_VERSION
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(LedgerError::Database)?
            .as_deref()
            .and_then(parse_evasion_class);
        if summary.top_class.is_none() {
            summary.top_class = self
                .connection
                .query_row(
                    "SELECT evasion_class FROM policy_events WHERE session_hash IN (?1, ?2)
                        AND accounting_policy_version = ?3
                      GROUP BY evasion_class ORDER BY COUNT(*) DESC, evasion_class LIMIT 1",
                    params![
                        session_hash,
                        legacy_session_hash,
                        CURRENT_ACCOUNTING_POLICY_VERSION
                    ],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(LedgerError::Database)?
                .as_deref()
                .and_then(parse_evasion_class);
        }
        let (attempts, asks, denials, corrections) = self
            .connection
            .query_row(
                "SELECT COUNT(*),
                    COALESCE(SUM(CASE WHEN decision = 'ask' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN decision = 'deny' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN decision = 'correction' THEN 1 ELSE 0 END), 0)
               FROM policy_events
              WHERE session_hash IN (?1, ?2) AND accounting_policy_version = ?3",
                params![
                    session_hash,
                    legacy_session_hash,
                    CURRENT_ACCOUNTING_POLICY_VERSION
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(LedgerError::Database)?;
        summary.policy_attempts = attempts;
        summary.policy_asks = asks;
        summary.policy_denials = denials;
        summary.policy_corrections = corrections;
        Ok(summary)
    }

    fn summary_scoped(
        &self,
        project_path: Option<&str>,
        since_unix_seconds: Option<i64>,
    ) -> Result<LedgerSummary, LedgerError> {
        self.connection
            .query_row(
                "SELECT
                    COUNT(*),
                    -- COALESCE is required, not cosmetic: SUM over zero rows returns
                    -- NULL, and reading that into an integer fails with an
                    -- Invalid-column-type-Null error. Without it hzr stats failed on
                    -- every fresh install, which is exactly when it is first run.
                    COALESCE(SUM(CASE WHEN outcome = 'accepted' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(actual_input), 0),
                    COALESCE(SUM(actual_output), 0),
                    COALESCE(SUM(estimated_input), 0),
                    COALESCE(SUM(cost_microusd), 0)
                 FROM usage_records
                 WHERE (?1 IS NULL OR (
                    project_hash != ''
                    AND instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0
                    AND length(?2) = 1
                 ))
                   AND (?3 IS NULL OR created_at_ms >= ?3 * 1000)",
                params![
                    project_path.map(|value| privacy_identity_hash("project", value)),
                    std::path::MAIN_SEPARATOR.to_string(),
                    since_unix_seconds
                ],
                |row| {
                    Ok(LedgerSummary {
                        tasks: row.get(0)?,
                        accepted: row.get(1)?,
                        actual_input_tokens: row.get(2)?,
                        actual_output_tokens: row.get(3)?,
                        estimated_input_tokens: row.get(4)?,
                        cost_microusd: row.get(5)?,
                    })
                },
            )
            .map_err(LedgerError::Database)
    }

    pub fn efficiency_summary(&self) -> Result<EfficiencySummary, LedgerError> {
        self.efficiency_summary_scoped(None, None, false)
    }

    pub fn efficiency_summary_for_project(
        &self,
        project_path: &str,
    ) -> Result<EfficiencySummary, LedgerError> {
        self.efficiency_summary_scoped(Some(project_path), None, false)
    }

    fn efficiency_summary_scoped(
        &self,
        project_path: Option<&str>,
        since_unix_seconds: Option<i64>,
        include_legacy_versions: bool,
    ) -> Result<EfficiencySummary, LedgerError> {
        let raw_predicate = raw_route_sql_predicate("rtk_cmd");
        let neutral_predicate = format!("({raw_predicate}) OR rtk_cmd = 'rtk write'");
        let version_predicate = accounting_policy_predicate(include_legacy_versions);
        let totals_query = format!(
            "SELECT
                COUNT(*),
                COALESCE(SUM(CASE WHEN ({neutral_predicate})
                                  THEN output_tokens ELSE input_tokens END), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(CASE WHEN NOT ({neutral_predicate}) AND input_tokens > output_tokens
                                  THEN input_tokens - output_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN NOT ({neutral_predicate}) AND output_tokens > input_tokens
                                  THEN output_tokens - input_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN ({neutral_predicate})
                                  THEN 0 ELSE input_tokens - output_tokens END), 0),
                COALESCE(SUM(exec_time_ms), 0)
             FROM commands
             WHERE measurement = 'estimated'
               AND COALESCE(route, '') != 'native_unaccounted'
               AND COALESCE(accounting_stage, 'internal_transport') NOT IN ('final_delivery', 'control_plane')
               AND ({version_predicate})
               AND (?1 IS NULL OR instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0)
               AND length(?2) = 1
               AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)"
        );
        let mut summary = self
            .connection
            .query_row(
                &totals_query,
                params![
                    project_path.map(|value| privacy_identity_hash("project", value)),
                    std::path::MAIN_SEPARATOR.to_string(),
                    since_unix_seconds
                ],
                |row| {
                    Ok(EfficiencySummary {
                        operations: row.get(0)?,
                        baseline_tokens_estimated: row.get(1)?,
                        delivered_tokens_estimated: row.get(2)?,
                        gross_avoided_tokens_estimated: row.get(3)?,
                        regression_tokens_estimated: row.get(4)?,
                        net_avoided_tokens_estimated: row.get(5)?,
                        total_execution_ms: row.get(6)?,
                        accounted_operations: 0,
                        total_observed_operations: 0,
                        native_unaccounted_operations: 0,
                        unmeasured_bypass_operations: 0,
                        by_channel: BTreeMap::new(),
                        by_mode: Vec::new(),
                        by_command: Vec::new(),
                        read_pipeline: ReadPipelineSummary::default(),
                        excluded_legacy_operations: 0,
                    })
                },
            )
            .map_err(LedgerError::Database)?;
        let by_command_query = format!(
            "SELECT
                rtk_cmd,
                COUNT(*),
                COALESCE(SUM(CASE WHEN ({neutral_predicate})
                                  THEN output_tokens ELSE input_tokens END), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(CASE WHEN NOT ({neutral_predicate}) AND input_tokens > output_tokens
                                  THEN input_tokens - output_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN NOT ({neutral_predicate}) AND output_tokens > input_tokens
                                  THEN output_tokens - input_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN ({neutral_predicate})
                                  THEN 0 ELSE input_tokens - output_tokens END), 0),
                COALESCE(AVG(exec_time_ms), 0)
             FROM commands
             WHERE measurement = 'estimated'
               AND COALESCE(route, '') != 'native_unaccounted'
               AND COALESCE(accounting_stage, 'internal_transport') NOT IN ('final_delivery', 'control_plane')
               AND ({version_predicate})
               AND (?1 IS NULL OR instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0)
               AND length(?2) = 1
               AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)
             GROUP BY rtk_cmd
             ORDER BY SUM(CASE WHEN ({neutral_predicate})
                               THEN 0 ELSE input_tokens - output_tokens END) DESC"
        );
        let mut statement = self
            .connection
            .prepare_cached(&by_command_query)
            .map_err(LedgerError::Database)?;
        summary.by_command = statement
            .query_map(
                params![
                    project_path.map(|value| privacy_identity_hash("project", value)),
                    std::path::MAIN_SEPARATOR.to_string(),
                    since_unix_seconds
                ],
                |row| {
                    Ok(EfficiencyCommandSummary {
                        command: row.get(0)?,
                        executions: row.get(1)?,
                        baseline_tokens_estimated: row.get(2)?,
                        delivered_tokens_estimated: row.get(3)?,
                        gross_avoided_tokens_estimated: row.get(4)?,
                        regression_tokens_estimated: row.get(5)?,
                        net_avoided_tokens_estimated: row.get(6)?,
                        avg_time_ms: row.get::<_, f64>(7)? as u64,
                    })
                },
            )
            .map_err(LedgerError::Database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LedgerError::Database)?;
        let scope_separator = std::path::MAIN_SEPARATOR.to_string();
        let coverage_query = format!(
            "SELECT COUNT(*),
                    COALESCE(SUM(route = 'native_unaccounted'), 0),
                    COALESCE(SUM(measurement = 'unmeasured' AND route = 'bypassed'), 0)
               FROM commands
              WHERE (?1 IS NULL OR instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0)
                AND length(?2) = 1
                AND COALESCE(accounting_stage, 'internal_transport') NOT IN ('final_delivery', 'control_plane')
                AND ({version_predicate})
                AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)"
        );
        let (total, native, unmeasured) = self
            .connection
            .query_row(
                &coverage_query,
                params![
                    project_path.map(|value| privacy_identity_hash("project", value)),
                    scope_separator,
                    since_unix_seconds
                ],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, u64>(2)?,
                    ))
                },
            )
            .map_err(LedgerError::Database)?;
        summary.total_observed_operations = total;
        summary.native_unaccounted_operations = native;
        summary.unmeasured_bypass_operations = unmeasured;
        summary.accounted_operations = total.saturating_sub(native);
        let channels_query = format!(
            "SELECT channel, COUNT(*) FROM commands
              WHERE (?1 IS NULL OR instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0)
                AND length(?2) = 1
                AND COALESCE(accounting_stage, 'internal_transport') NOT IN ('final_delivery', 'control_plane')
                AND ({version_predicate})
                AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)
              GROUP BY channel"
        );
        let mut channels = self
            .connection
            .prepare_cached(&channels_query)
            .map_err(LedgerError::Database)?;
        summary.by_channel = channels
            .query_map(
                params![
                    project_path.map(|value| privacy_identity_hash("project", value)),
                    std::path::MAIN_SEPARATOR.to_string(),
                    since_unix_seconds
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
            )
            .map_err(LedgerError::Database)?
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(LedgerError::Database)?;
        summary.by_mode = self.operation_modes_summary(
            project_path,
            since_unix_seconds,
            include_legacy_versions,
        )?;
        let read_pipeline_query = format!(
            "SELECT COUNT(*),
                    COALESCE(SUM((source_bytes + 3) / 4), 0),
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(CASE WHEN (source_bytes + 3) / 4 > input_tokens
                                      THEN (source_bytes + 3) / 4 - input_tokens ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN input_tokens > (source_bytes + 3) / 4
                                      THEN input_tokens - (source_bytes + 3) / 4 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN input_tokens > output_tokens
                                      THEN input_tokens - output_tokens ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN output_tokens > input_tokens
                                      THEN output_tokens - input_tokens ELSE 0 END), 0)
               FROM commands
              WHERE operation_kind = 'read'
                AND source_bytes IS NOT NULL
                AND measurement = 'estimated'
                AND COALESCE(route, '') = 'optimized'
                AND COALESCE(accounting_stage, 'internal_transport') = 'internal_transport'
                AND ({version_predicate})
                AND (?1 IS NULL OR instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0)
                AND length(?2) = 1
                AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)"
        );
        summary.read_pipeline = self
            .connection
            .query_row(
                &read_pipeline_query,
                params![
                    project_path.map(|value| privacy_identity_hash("project", value)),
                    std::path::MAIN_SEPARATOR.to_string(),
                    since_unix_seconds
                ],
                |row| {
                    Ok(ReadPipelineSummary {
                        operations: row.get(0)?,
                        source_tokens_estimated: row.get(1)?,
                        selected_tokens_estimated: row.get(2)?,
                        delivered_tokens_estimated: row.get(3)?,
                        selection_avoided_tokens_estimated: row.get(4)?,
                        selection_overhead_tokens_estimated: row.get(5)?,
                        transform_avoided_tokens_estimated: row.get(6)?,
                        transform_overhead_tokens_estimated: row.get(7)?,
                    })
                },
            )
            .map_err(LedgerError::Database)?;
        if !include_legacy_versions {
            let excluded_query = format!(
                "SELECT COUNT(*) FROM commands
                  WHERE COALESCE(accounting_policy_version, '') != '{CURRENT_ACCOUNTING_POLICY_VERSION}'
                    AND COALESCE(accounting_stage, 'internal_transport') NOT IN ('final_delivery', 'control_plane')
                    AND (?1 IS NULL OR instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0)
                    AND length(?2) = 1
                    AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)"
            );
            summary.excluded_legacy_operations = self
                .connection
                .query_row(
                    &excluded_query,
                    params![
                        project_path.map(|value| privacy_identity_hash("project", value)),
                        std::path::MAIN_SEPARATOR.to_string(),
                        since_unix_seconds
                    ],
                    |row| row.get(0),
                )
                .map_err(LedgerError::Database)?;
        }
        Ok(summary)
    }

    fn operation_modes_summary(
        &self,
        project_path: Option<&str>,
        since_unix_seconds: Option<i64>,
        include_legacy_versions: bool,
    ) -> Result<Vec<OperationModeSummary>, LedgerError> {
        let version_predicate = accounting_policy_predicate(include_legacy_versions);
        let query = format!(
            "SELECT operation_kind, operation_mode, accounting_stage, COUNT(*),
                    COALESCE(SUM(output_tokens), 0)
               FROM commands
              WHERE operation_kind IS NOT NULL
                AND operation_mode IS NOT NULL
                AND accounting_stage IS NOT NULL
                AND (?1 IS NULL OR instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0)
                AND (?2 IS NULL OR ?2 IS NOT NULL)
                AND ({version_predicate})
                AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)
              GROUP BY operation_kind, operation_mode, accounting_stage
              ORDER BY operation_kind, operation_mode, accounting_stage"
        );
        let mut statement = self
            .connection
            .prepare_cached(&query)
            .map_err(LedgerError::Database)?;
        let rows = statement
            .query_map(
                params![
                    project_path.map(|value| privacy_identity_hash("project", value)),
                    std::path::MAIN_SEPARATOR.to_string(),
                    since_unix_seconds
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, u64>(3)?,
                        row.get::<_, u64>(4)?,
                    ))
                },
            )
            .map_err(LedgerError::Database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LedgerError::Database)?;
        Ok(rows
            .into_iter()
            .filter_map(|(operation, mode, stage, operations, delivered)| {
                Some(OperationModeSummary {
                    operation: parse_operation_kind(&operation)?,
                    mode: parse_operation_mode(&mode)?,
                    stage: parse_accounting_stage(&stage)?,
                    operations,
                    delivered_tokens_estimated: delivered,
                })
            })
            .collect())
    }

    /// Record one HZR-owned operation in the same table the pinned engine writes to.
    ///
    /// Everything in the efficiency ledger used to arrive from fork-core, which is why HZR's
    /// own reductions — the density codec above all — were invisible: they saved tokens that
    /// nothing counted, so the subsystem could never appear in `hzr stats` and the capability
    /// read as dead weight. The summaries derive their figures from the token columns, so a
    /// transform that grew the text stays a regression rather than being clamped to zero.
    pub fn record_operation(
        &self,
        original_command: &str,
        recorded_command: &str,
        input_tokens: u64,
        output_tokens: u64,
        execution_ms: u64,
        project_path: &str,
    ) -> Result<(), LedgerError> {
        self.record_operation_with_context(
            original_command,
            recorded_command,
            input_tokens,
            output_tokens,
            execution_ms,
            OperationContext {
                project_path,
                agent: None,
                session_id: None,
            },
        )
    }

    pub fn record_operation_with_context(
        &self,
        original_command: &str,
        recorded_command: &str,
        input_tokens: u64,
        output_tokens: u64,
        execution_ms: u64,
        context: OperationContext<'_>,
    ) -> Result<(), LedgerError> {
        let route = classify_operation(recorded_command).route;
        self.record_operation_attributed(
            original_command,
            recorded_command,
            input_tokens,
            output_tokens,
            execution_ms,
            OperationAttribution {
                project_path: context.project_path,
                agent: context.agent,
                session_id: context.session_id,
                channel: OperationChannel::HookCli,
                measurement: OperationMeasurement::Estimated,
                route,
            },
        )
    }

    pub fn record_operation_attributed(
        &self,
        original_command: &str,
        recorded_command: &str,
        input_tokens: u64,
        output_tokens: u64,
        execution_ms: u64,
        attribution: OperationAttribution<'_>,
    ) -> Result<(), LedgerError> {
        self.record_operation_attributed_with_detail(
            original_command,
            recorded_command,
            input_tokens,
            output_tokens,
            execution_ms,
            DetailedOperationAttribution {
                attribution,
                detail: None,
                evasion: None,
            },
        )
    }

    pub fn record_operation_attributed_with_detail(
        &self,
        original_command: &str,
        recorded_command: &str,
        input_tokens: u64,
        output_tokens: u64,
        execution_ms: u64,
        accounting: DetailedOperationAttribution<'_>,
    ) -> Result<(), LedgerError> {
        let attribution = accounting.attribution;
        let detail = accounting.detail;
        let evasion = accounting
            .evasion
            .or_else(|| detail.and_then(|detail| detail.evasion.as_ref()));
        if attribution.measurement == OperationMeasurement::Unmeasured
            && (input_tokens != 0 || output_tokens != 0)
        {
            return Err(LedgerError::InvalidOperation(
                "unmeasured operations cannot carry invented token counts".into(),
            ));
        }
        if detail.is_some_and(|detail| detail.mode.operation() != detail.operation) {
            return Err(LedgerError::InvalidOperation(
                "operation mode does not match its operation family".into(),
            ));
        }
        if detail.is_some_and(|detail| {
            detail
                .requested_mode
                .is_some_and(|mode| mode.operation() != detail.operation)
                || detail
                    .effective_mode
                    .is_some_and(|mode| mode.operation() != detail.operation)
        }) {
            return Err(LedgerError::InvalidOperation(
                "requested/effective mode does not match its operation family".into(),
            ));
        }
        if detail.is_some_and(|detail| {
            detail
                .effective_mode
                .is_some_and(|effective| effective != detail.mode)
        }) {
            return Err(LedgerError::InvalidOperation(
                "canonical operation mode must equal effective mode".into(),
            ));
        }
        if detail.is_some_and(|detail| {
            detail.operation != AccountingOperationKind::Search
                && (detail.search_strategy.is_some() || detail.search_fallback_code.is_some())
        }) {
            return Err(LedgerError::InvalidOperation(
                "search attribution cannot be attached to another operation family".into(),
            ));
        }
        let saved = input_tokens.saturating_sub(output_tokens);
        let savings_pct = if input_tokens == 0 {
            0.0
        } else {
            saved as f64 * 100.0 / input_tokens as f64
        };
        let classification = classify_operation(recorded_command);
        let family = privacy_safe_family(
            detail
                .map(|detail| detail.operation.as_str())
                .unwrap_or(&classification.operation),
        );
        let replacement = first_class_replacement(recorded_command)
            .or_else(|| efficient_route_replacement(recorded_command));
        let replacement_capability =
            if attribution.route == OperationRoute::Optimized || replacement.is_some() {
                ReplacementCapability::Available
            } else {
                ReplacementCapability::Unknown
            };
        let replacement_route = replacement
            .as_ref()
            .map(|_| format!("hzr exec run '<{family} command>'"));
        let command_hash = privacy_identity_hash(
            "command",
            &format!("{original_command}\0{recorded_command}"),
        );
        let (project_hash, project_scope_hashes) = project_hashes(attribution.project_path);
        let session_hash = attribution
            .session_id
            .map(|value| session_identity_hash(&self.connection, value))
            .transpose()?;
        let agent_hash = attribution
            .agent
            .map(|value| agent_identity_hash(&self.connection, value))
            .transpose()?;
        self.connection
            .execute(
                "INSERT INTO commands (
                    timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens,
                    saved_tokens, savings_pct, exec_time_ms, project_path, agent, session_id,
                    channel, measurement, route, operation_kind, operation_mode,
                    accounting_stage, requested_mode, effective_mode, search_strategy,
                    search_fallback_code, search_include_content, result_limit, path_scope_count,
                    filter_level, range_from, range_to, source_bytes, operation_family,
                    command_hash, project_hash, project_scope_hashes, session_hash,
                    agent_hash, producer_version, accounting_policy_version,
                    evasion_class, wrapper_depth, interpreter_kind, path_form, stage_count,
                    hatch_marker, avoidable, enforcement_tier, fidelity_reason,
                    fidelity_validation, replacement_capability, replacement_route,
                    replacement_reason
                 ) VALUES (
                    datetime('now'), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                    ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27,
                    ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40,
                    ?41, ?42, ?43, ?44, ?45, ?46, ?47, ?48
                 )",
                params![
                    format!("[redacted:{family}]"),
                    safe_recorded_command(&family, attribution.route),
                    input_tokens,
                    output_tokens,
                    saved,
                    savings_pct,
                    execution_ms,
                    if attribution.project_path.is_empty() {
                        ""
                    } else {
                        "[redacted]"
                    },
                    privacy_safe_agent(attribution.agent),
                    Option::<&str>::None,
                    attribution.channel.as_str(),
                    attribution.measurement.as_str(),
                    attribution.route.as_str(),
                    detail.map(|detail| detail.operation.as_str()),
                    detail.map(|detail| detail.mode.as_str()),
                    detail.map(|detail| detail.stage.as_str()),
                    detail.and_then(|detail| detail.requested_mode.map(|mode| mode.as_str())),
                    detail.and_then(|detail| detail.effective_mode.map(|mode| mode.as_str())),
                    detail.and_then(|detail| {
                        detail.search_strategy.map(|strategy| strategy.as_str())
                    }),
                    detail.and_then(|detail| {
                        detail.search_fallback_code.map(|code| code.as_str())
                    }),
                    detail.and_then(|detail| detail.include_content),
                    detail.and_then(|detail| detail.limit),
                    detail.and_then(|detail| detail.path_scope_count),
                    detail.and_then(|detail| detail.filter_level.map(|level| level.as_str())),
                    detail.and_then(|detail| detail.from_line),
                    detail.and_then(|detail| detail.to_line),
                    detail.and_then(|detail| detail.source_bytes),
                    family,
                    command_hash,
                    project_hash,
                    project_scope_hashes,
                    session_hash,
                    agent_hash,
                    CURRENT_PRODUCER_VERSION,
                    CURRENT_ACCOUNTING_POLICY_VERSION,
                    evasion.map(|value| value.class.as_str()),
                    evasion.map(|value| value.wrapper_depth),
                    evasion.and_then(|value| value.interpreter.map(|kind| kind.as_str())),
                    evasion.map(|value| value.path_form.as_str()),
                    evasion.map(|value| value.stage_count),
                    evasion.map(|value| value.hatch_marker),
                    evasion.map(|value| value.avoidable),
                    evasion.map(|value| value.tier.as_str()),
                    evasion.and_then(|value| value.fidelity_reason.map(|reason| reason.as_str())),
                    evasion.map(|value| value.fidelity_validation.as_str()),
                    replacement_capability.as_str(),
                    replacement_route,
                    replacement.as_ref().map(|value| value.rationale),
                ],
            )
            .map_err(LedgerError::Database)?;
        Ok(())
    }

    pub fn record_privacy_safe_fidelity_operation(
        &self,
        record: &PrivacySafeFidelityOperation,
    ) -> Result<(), LedgerError> {
        if record.reservation_id.is_empty() || record.evasion.class != EvasionClass::E7FidelityHatch
        {
            return Err(LedgerError::InvalidOperation(
                "durable fidelity record requires an E7 reservation identity".into(),
            ));
        }
        if record.measurement == OperationMeasurement::Unmeasured
            && (record.input_tokens != 0 || record.output_tokens != 0)
        {
            return Err(LedgerError::InvalidOperation(
                "unmeasured durable fidelity operations cannot carry token counts".into(),
            ));
        }
        let saved = record.input_tokens.saturating_sub(record.output_tokens);
        let savings_pct = if record.input_tokens == 0 {
            0.0
        } else {
            saved as f64 * 100.0 / record.input_tokens as f64
        };
        self.connection
            .execute(
                "INSERT OR IGNORE INTO commands (
                    timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens,
                    saved_tokens, savings_pct, exec_time_ms, project_path, agent, session_id,
                    channel, measurement, route, operation_family, command_hash, project_hash,
                    project_scope_hashes, session_hash, agent_hash, producer_version,
                    accounting_policy_version, evasion_class, wrapper_depth, interpreter_kind,
                    path_form, stage_count, hatch_marker, avoidable, enforcement_tier,
                    fidelity_reason, fidelity_validation, fidelity_reservation_id
                 ) VALUES (
                    datetime('now'), '[redacted:execution]', 'rtk raw execution', ?1, ?2, ?3,
                    ?4, ?5, '[redacted]', ?6, NULL, 'hook_cli', ?14, 'bypassed',
                    'execution', ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?15, ?16, ?17, ?18,
                    ?19, ?20, ?21, ?22, ?23, ?24, ?25
                 )",
                params![
                    record.input_tokens,
                    record.output_tokens,
                    saved,
                    savings_pct,
                    record.execution_ms,
                    record.agent.as_deref(),
                    privacy_identity_hash("command", &record.reservation_id),
                    record.project_hash.as_str(),
                    record.project_scope_hashes.as_str(),
                    record.session_hash.as_deref(),
                    record.agent_hash.as_deref(),
                    CURRENT_PRODUCER_VERSION,
                    CURRENT_ACCOUNTING_POLICY_VERSION,
                    record.measurement.as_str(),
                    record.evasion.class.as_str(),
                    record.evasion.wrapper_depth,
                    record.evasion.interpreter.map(|value| value.as_str()),
                    record.evasion.path_form.as_str(),
                    record.evasion.stage_count,
                    record.evasion.hatch_marker,
                    record.evasion.avoidable,
                    record.evasion.tier.as_str(),
                    record.evasion.fidelity_reason.map(|value| value.as_str()),
                    record.evasion.fidelity_validation.as_str(),
                    record.reservation_id.as_str(),
                ],
            )
            .map_err(LedgerError::Database)?;
        Ok(())
    }

    /// Record a policy attempt that did not itself deliver tool output. Policy events are kept in
    /// a separate table so Ask/Deny/Correction cannot inflate operation or savings counters.
    pub fn record_policy_event(&self, event: PolicyEvent<'_>) -> Result<(), LedgerError> {
        let (project_hash, project_scope_hashes) = project_hashes(event.project_path);
        let session_hash = event
            .session_id
            .map(|value| session_identity_hash(&self.connection, value))
            .transpose()?;
        let agent_hash = event
            .agent
            .map(|value| agent_identity_hash(&self.connection, value))
            .transpose()?;
        let replacement_family = event.replacement_family.map(privacy_safe_family);
        self.connection
            .execute(
                "INSERT INTO policy_events (
                    timestamp, project_hash, project_scope_hashes, session_hash, agent,
                    agent_hash, evasion_class, wrapper_depth, interpreter_kind, path_form,
                    stage_count, hatch_marker, avoidable, enforcement_tier, fidelity_reason,
                    fidelity_validation, decision, replacement_family, producer_version,
                    accounting_policy_version
                 ) VALUES (
                    datetime('now'), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                    ?13, ?14, ?15, ?16, ?17, ?18, ?19
                 )",
                params![
                    project_hash,
                    project_scope_hashes,
                    session_hash,
                    privacy_safe_agent(event.agent),
                    agent_hash,
                    event.evasion.class.as_str(),
                    event.evasion.wrapper_depth,
                    event.evasion.interpreter.map(|value| value.as_str()),
                    event.evasion.path_form.as_str(),
                    event.evasion.stage_count,
                    event.evasion.hatch_marker,
                    event.evasion.avoidable,
                    event.evasion.tier.as_str(),
                    event.evasion.fidelity_reason.map(|value| value.as_str()),
                    event.evasion.fidelity_validation.as_str(),
                    event.decision.as_str(),
                    replacement_family,
                    CURRENT_PRODUCER_VERSION,
                    CURRENT_ACCOUNTING_POLICY_VERSION,
                ],
            )
            .map_err(LedgerError::Database)?;
        Ok(())
    }

    /// Count the operations that reached the shell without passing through the optimizer.
    ///
    /// A bypassed row delivers exactly as many tokens as it consumed, so it cancels out of
    /// the reduction ratio instead of lowering it. Without this query a workspace can send
    /// half of its tool output straight to the model while `hzr stats` still reports a
    /// healthy percentage.
    pub fn bypass_summary(&self) -> Result<BypassSummary, LedgerError> {
        self.bypass_summary_scoped(None, None, false)
    }

    pub fn bypass_summary_for_project(
        &self,
        project_path: &str,
    ) -> Result<BypassSummary, LedgerError> {
        self.bypass_summary_scoped(Some(project_path), None, false)
    }

    fn bypass_summary_scoped(
        &self,
        project_path: Option<&str>,
        since_unix_seconds: Option<i64>,
        include_legacy_versions: bool,
    ) -> Result<BypassSummary, LedgerError> {
        let version_predicate = accounting_policy_predicate(include_legacy_versions);
        let totals_query = format!(
            "SELECT COUNT(*), COALESCE(SUM(output_tokens), 0)
               FROM commands
              WHERE (?1 IS NULL OR instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0)
                AND (?2 IS NULL OR ?2 IS NOT NULL)
                AND COALESCE(accounting_stage, 'internal_transport') NOT IN ('final_delivery', 'control_plane')
                AND ({version_predicate})
                AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)"
        );
        let (total_operations, total_delivered) = self
            .connection
            .query_row(
                &totals_query,
                params![
                    project_path.map(|value| privacy_identity_hash("project", value)),
                    std::path::MAIN_SEPARATOR.to_string(),
                    since_unix_seconds
                ],
                |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
            )
            .map_err(LedgerError::Database)?;
        let query = format!(
            "SELECT rtk_cmd, replacement_capability, replacement_route, replacement_reason,
                    COUNT(*), COALESCE(SUM(output_tokens), 0)
             FROM commands
             WHERE ({})
               AND COALESCE(accounting_stage, 'internal_transport') NOT IN ('final_delivery', 'control_plane')
               AND ({version_predicate})
               AND (?1 IS NULL OR instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0)
               AND (?2 IS NULL OR ?2 IS NOT NULL)
               AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)
             GROUP BY rtk_cmd, replacement_capability, replacement_route, replacement_reason",
            raw_route_sql_predicate("rtk_cmd")
        );
        let mut statement = self
            .connection
            .prepare(&query)
            .map_err(LedgerError::Database)?;
        let groups = statement
            .query_map(
                params![
                    project_path.map(|value| privacy_identity_hash("project", value)),
                    std::path::MAIN_SEPARATOR.to_string(),
                    since_unix_seconds
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, u64>(4)?,
                        row.get::<_, u64>(5)?,
                    ))
                },
            )
            .map_err(LedgerError::Database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LedgerError::Database)?;

        let mut by_tool: BTreeMap<(String, ReplacementCapability), BypassTool> = BTreeMap::new();
        let mut heaviest: BTreeMap<(String, ReplacementCapability), u64> = BTreeMap::new();
        let mut operations = 0;
        let mut delivered = 0;
        for (
            command,
            stored_capability,
            stored_replacement,
            stored_reason,
            executions,
            delivered_tokens,
        ) in groups
        {
            let classification = classify_operation(&command);
            let inferred_replacement = classification.replacement.as_ref();
            let stored_capability = parse_replacement_capability(stored_capability.as_deref());
            let capability = if stored_capability == ReplacementCapability::Unknown
                && inferred_replacement.is_some()
            {
                ReplacementCapability::Available
            } else {
                stored_capability
            };
            let key = (classification.operation.clone(), capability);
            operations += executions;
            delivered += delivered_tokens;
            let entry = by_tool.entry(key.clone()).or_insert_with(|| BypassTool {
                tool: classification.operation.clone(),
                executions: 0,
                delivered_tokens_estimated: 0,
                example_command: command.clone(),
                replacement: stored_replacement.clone(),
                replacement_capability: capability,
                rationale: stored_reason.clone(),
            });
            entry.executions += executions;
            entry.delivered_tokens_estimated += delivered_tokens;
            // The costliest concrete invocation becomes the worked example, so the
            // suggestion an operator reads is the one that would have saved the most.
            let previous = heaviest.entry(key).or_default();
            if delivered_tokens >= *previous {
                *previous = delivered_tokens;
                entry.example_command = command.clone();
                entry.replacement = stored_replacement.clone();
                entry.rationale = stored_reason.clone();
            }
        }
        let mut by_tool = by_tool.into_values().collect::<Vec<_>>();
        by_tool.sort_by(|left, right| {
            right
                .delivered_tokens_estimated
                .cmp(&left.delivered_tokens_estimated)
                .then_with(|| left.tool.cmp(&right.tool))
        });

        Ok(BypassSummary {
            lifetime: BypassWindow {
                operations,
                total_operations,
                delivered_tokens_estimated: delivered,
                total_delivered_tokens_estimated: total_delivered,
            },
            by_tool,
        })
    }

    fn operation_family_summary(
        &self,
        project_path: Option<&str>,
        since_unix_seconds: Option<i64>,
        include_legacy_versions: bool,
    ) -> Result<Vec<OperationFamilySummary>, LedgerError> {
        let version_predicate = accounting_policy_predicate(include_legacy_versions);
        let query = format!(
            "SELECT rtk_cmd, route, COALESCE(operation_family, operation_kind),
                    replacement_capability, COUNT(*), COALESCE(SUM(output_tokens), 0)
               FROM commands
              WHERE (?1 IS NULL OR instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0)
                AND (?2 IS NULL OR ?2 IS NOT NULL)
                AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)
                AND COALESCE(accounting_stage, 'internal_transport') NOT IN ('final_delivery', 'control_plane')
                AND ({version_predicate})
              GROUP BY rtk_cmd, route, COALESCE(operation_family, operation_kind),
                       replacement_capability"
        );
        let mut statement = self
            .connection
            .prepare_cached(&query)
            .map_err(LedgerError::Database)?;
        let rows = statement
            .query_map(
                params![
                    project_path.map(|value| privacy_identity_hash("project", value)),
                    std::path::MAIN_SEPARATOR.to_string(),
                    since_unix_seconds
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, u64>(4)?,
                        row.get::<_, u64>(5)?,
                    ))
                },
            )
            .map_err(LedgerError::Database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LedgerError::Database)?;

        let mut families =
            BTreeMap::<(String, String, ReplacementCapability), OperationFamilySummary>::new();
        for (command, stored_route, stored_operation, stored_capability, operations, delivered) in
            rows
        {
            let classification = classify_operation(&command);
            let route = route_from_ledger(stored_route.as_deref(), classification.route);
            let family = stored_operation
                .as_deref()
                .and_then(parse_operation_kind)
                .map(|operation| operation.as_str().to_owned())
                .unwrap_or(classification.operation);
            let stored_capability = parse_replacement_capability(stored_capability.as_deref());
            let capability = if stored_capability == ReplacementCapability::Unknown
                && (route == OperationRoute::Optimized
                    || first_class_replacement(&command).is_some()
                    || efficient_route_replacement(&command).is_some())
            {
                ReplacementCapability::Available
            } else {
                stored_capability
            };
            let key = (family.clone(), route.as_str().to_owned(), capability);
            let summary = families
                .entry(key)
                .or_insert_with(|| OperationFamilySummary {
                    family,
                    route,
                    operations: 0,
                    delivered_tokens_estimated: 0,
                    replacement_capability: capability,
                });
            summary.operations = summary.operations.saturating_add(operations);
            summary.delivered_tokens_estimated =
                summary.delivered_tokens_estimated.saturating_add(delivered);
        }

        let mut summaries = families.into_values().collect::<Vec<_>>();
        summaries.sort_by(|left, right| {
            right
                .delivered_tokens_estimated
                .cmp(&left.delivered_tokens_estimated)
                .then_with(|| right.operations.cmp(&left.operations))
                .then_with(|| left.family.cmp(&right.family))
                .then_with(|| left.route.as_str().cmp(right.route.as_str()))
        });
        Ok(summaries)
    }

    pub fn project_activity(
        &self,
        project_path: &str,
    ) -> Result<ProjectActivitySummary, LedgerError> {
        let project_hash = privacy_identity_hash("project", project_path);
        let raw_predicate = raw_route_sql_predicate("rtk_cmd");
        let measured_predicate =
            "measurement = 'estimated' AND COALESCE(route, '') != 'native_unaccounted'";
        let headline_stage_predicate = "COALESCE(accounting_stage, 'internal_transport') NOT IN ('final_delivery', 'control_plane')";
        let activity_query = format!(
            "SELECT
                COUNT(*),
                COALESCE(SUM(CASE WHEN ({measured_predicate}) AND ({raw_predicate})
                                  THEN output_tokens
                                  WHEN ({measured_predicate}) THEN input_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN ({measured_predicate}) THEN output_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN ({measured_predicate}) AND NOT ({raw_predicate}) AND input_tokens > output_tokens
                                  THEN input_tokens - output_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN ({measured_predicate}) AND NOT ({raw_predicate}) AND output_tokens > input_tokens
                                  THEN output_tokens - input_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN NOT ({measured_predicate}) OR ({raw_predicate})
                                  THEN 0 ELSE input_tokens - output_tokens END), 0),
                COALESCE(SUM(CASE WHEN ({measured_predicate}) THEN exec_time_ms ELSE 0 END), 0),
                MIN(timestamp),
                MAX(timestamp)
             FROM commands
             WHERE instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0
               AND accounting_policy_version = ?2
               AND command_hash IS NOT NULL
               AND {headline_stage_predicate}"
        );
        let mut summary = self
            .connection
            .query_row(
                &activity_query,
                params![project_hash, CURRENT_ACCOUNTING_POLICY_VERSION],
                |row| {
                    Ok(ProjectActivitySummary {
                        operations: row.get(0)?,
                        optimized_operations: 0,
                        raw_operations: 0,
                        native_unaccounted_operations: 0,
                        unmeasured_bypass_operations: 0,
                        baseline_tokens_estimated: row.get(1)?,
                        delivered_tokens_estimated: row.get(2)?,
                        gross_avoided_tokens_estimated: row.get(3)?,
                        regression_tokens_estimated: row.get(4)?,
                        net_avoided_tokens_estimated: row.get(5)?,
                        total_execution_ms: row.get(6)?,
                        first_record_at: row.get(7)?,
                        last_record_at: row.get(8)?,
                        unscoped_operations: 0,
                        excluded_legacy_operations: 0,
                        recent_operations: Vec::new(),
                    })
                },
            )
            .map_err(LedgerError::Database)?;
        summary.unscoped_operations = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM commands
                  WHERE project_path = ''
                    AND accounting_policy_version = ?1
                    AND command_hash IS NOT NULL
                    AND COALESCE(accounting_stage, 'internal_transport') NOT IN ('final_delivery', 'control_plane')",
                [CURRENT_ACCOUNTING_POLICY_VERSION],
                |row| row.get(0),
            )
            .map_err(LedgerError::Database)?;
        summary.raw_operations = self
            .connection
            .query_row(
                &format!(
                    "SELECT COUNT(*)
                     FROM commands
                     WHERE instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0
                       AND accounting_policy_version = ?2
                       AND measurement = 'estimated'
                       AND COALESCE(route, '') != 'native_unaccounted'
                       AND command_hash IS NOT NULL
                       AND {headline_stage_predicate}
                       AND ({})",
                    raw_route_sql_predicate("rtk_cmd")
                ),
                params![project_hash, CURRENT_ACCOUNTING_POLICY_VERSION],
                |row| row.get(0),
            )
            .map_err(LedgerError::Database)?;
        summary.optimized_operations = self
            .connection
            .query_row(
                &format!(
                    "SELECT COUNT(*)
                     FROM commands
                     WHERE instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0
                       AND accounting_policy_version = ?2
                       AND measurement = 'estimated'
                       AND COALESCE(route, '') != 'native_unaccounted'
                       AND command_hash IS NOT NULL
                       AND {headline_stage_predicate}
                       AND NOT ({})",
                    raw_route_sql_predicate("rtk_cmd")
                ),
                params![project_hash, CURRENT_ACCOUNTING_POLICY_VERSION],
                |row| row.get(0),
            )
            .map_err(LedgerError::Database)?;
        (
            summary.native_unaccounted_operations,
            summary.unmeasured_bypass_operations,
        ) = self
            .connection
            .query_row(
                "SELECT
                    COALESCE(SUM(route = 'native_unaccounted'), 0),
                    COALESCE(SUM(measurement = 'unmeasured' AND route = 'bypassed'), 0)
                 FROM commands
                 WHERE instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0
                   AND accounting_policy_version = ?2
                   AND command_hash IS NOT NULL
                   AND COALESCE(accounting_stage, 'internal_transport') NOT IN ('final_delivery', 'control_plane')",
                params![project_hash, CURRENT_ACCOUNTING_POLICY_VERSION],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(LedgerError::Database)?;
        summary.excluded_legacy_operations = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM commands
                  WHERE instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0
                    AND accounting_policy_version != ?2
                    AND command_hash IS NOT NULL
                    AND COALESCE(accounting_stage, 'internal_transport') NOT IN ('final_delivery', 'control_plane')",
                params![project_hash, CURRENT_ACCOUNTING_POLICY_VERSION],
                |row| row.get(0),
            )
            .map_err(LedgerError::Database)?;
        let mut statement = self
            .connection
            .prepare_cached(
                "SELECT id, timestamp, rtk_cmd, agent, command_hash, project_hash, session_hash,
                        input_tokens, output_tokens, input_tokens - output_tokens, exec_time_ms,
                        COALESCE(route, ''), producer_version, accounting_policy_version
                 FROM commands
                 WHERE instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0
                   AND accounting_policy_version = ?2
                   AND command_hash IS NOT NULL
                   AND COALESCE(accounting_stage, 'internal_transport') NOT IN ('final_delivery', 'control_plane')
                 ORDER BY id DESC
                 LIMIT 24",
            )
            .map_err(LedgerError::Database)?;
        summary.recent_operations = statement
            .query_map(
                params![project_hash, CURRENT_ACCOUNTING_POLICY_VERSION],
                |row| {
                    let command: String = row.get(2)?;
                    let (mut operation, classified_route, replacement, rationale) =
                        operation_identity(&command);
                    let route = if row.get::<_, String>(11)? == "native_unaccounted" {
                        operation = command
                            .strip_prefix("native ")
                            .unwrap_or(&command)
                            .to_owned();
                        ProjectOperationRoute::NativeUnaccounted
                    } else {
                        classified_route
                    };
                    let delivered_tokens_estimated = row.get(8)?;
                    let (baseline_tokens_estimated, net_avoided_tokens_estimated) = match route {
                        ProjectOperationRoute::Optimized => (row.get(7)?, row.get(9)?),
                        ProjectOperationRoute::Raw => (delivered_tokens_estimated, 0),
                        ProjectOperationRoute::NativeUnaccounted => (delivered_tokens_estimated, 0),
                    };
                    Ok(ProjectOperationSummary {
                        ledger_id: row.get(0)?,
                        timestamp: row.get(1)?,
                        operation,
                        route,
                        command_hash: row.get(4)?,
                        project_hash: row.get(5)?,
                        agent: row.get(3)?,
                        session_hash: row.get(6)?,
                        producer_version: row.get(12)?,
                        policy_version: row.get(13)?,
                        baseline_tokens_estimated,
                        delivered_tokens_estimated,
                        net_avoided_tokens_estimated,
                        execution_ms: row.get(10)?,
                        replacement,
                        rationale,
                    })
                },
            )
            .map_err(LedgerError::Database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LedgerError::Database)?;
        Ok(summary)
    }

    pub fn migrate_legacy_efficiency(
        &self,
        source_path: &Path,
        migration_root: &Path,
    ) -> Result<LegacyEfficiencyMigration, LedgerError> {
        let source_path = source_path
            .canonicalize()
            .map_err(|source| LedgerError::Io {
                operation: "resolve legacy RTK history",
                path: source_path.to_path_buf(),
                source,
            })?;
        let source_id = hex::encode(Sha256::digest(source_path.to_string_lossy().as_bytes()));
        let snapshot_directory = migration_root.join("snapshots");
        std::fs::create_dir_all(&snapshot_directory).map_err(|source| LedgerError::Io {
            operation: "create migration snapshot directory",
            path: snapshot_directory.clone(),
            source,
        })?;
        let temporary = tempfile::NamedTempFile::new_in(&snapshot_directory).map_err(|source| {
            LedgerError::Io {
                operation: "create migration snapshot",
                path: snapshot_directory.clone(),
                source,
            }
        })?;
        let source_connection = open_legacy_read_only(&source_path)?;
        let mut snapshot_connection =
            Connection::open(temporary.path()).map_err(LedgerError::Database)?;
        {
            let backup =
                rusqlite::backup::Backup::new(&source_connection, &mut snapshot_connection)
                    .map_err(LedgerError::Database)?;
            backup
                .run_to_completion(128, std::time::Duration::from_millis(1), None)
                .map_err(LedgerError::Database)?;
        }
        drop(snapshot_connection);
        drop(source_connection);
        let snapshot_bytes = std::fs::read(temporary.path()).map_err(|source| LedgerError::Io {
            operation: "read migration snapshot",
            path: temporary.path().to_path_buf(),
            source,
        })?;
        let snapshot_sha256 = hex::encode(Sha256::digest(&snapshot_bytes));
        let backup_path = snapshot_directory.join(format!("rtk-history-{snapshot_sha256}.sqlite"));
        if backup_path.exists() {
            let existing = std::fs::read(&backup_path).map_err(|source| LedgerError::Io {
                operation: "read existing migration snapshot",
                path: backup_path.clone(),
                source,
            })?;
            if hex::encode(Sha256::digest(existing)) != snapshot_sha256 {
                return Err(LedgerError::SnapshotMismatch(backup_path));
            }
        } else {
            temporary
                .persist(&backup_path)
                .map_err(|error| LedgerError::Io {
                    operation: "persist migration snapshot",
                    path: backup_path.clone(),
                    source: error.error,
                })?;
        }
        let source = inspect_legacy_efficiency(&backup_path)?;

        attach_legacy(&self.connection, &backup_path)?;
        let result = (|| {
            self.connection
                .execute_batch("BEGIN IMMEDIATE")
                .map_err(LedgerError::Database)?;
            let imported_commands = self
                .connection
                .execute(
                    "INSERT INTO commands (
                        timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens,
                        saved_tokens, savings_pct, exec_time_ms, project_path
                     ) SELECT
                        legacy.timestamp, legacy.original_cmd, legacy.rtk_cmd,
                        legacy.input_tokens, legacy.output_tokens, legacy.saved_tokens,
                        legacy.savings_pct, COALESCE(legacy.exec_time_ms, 0),
                        COALESCE(legacy.project_path, '')
                     FROM legacy_hzr.commands AS legacy
                     WHERE NOT EXISTS (
                        SELECT 1 FROM legacy_command_imports AS imported
                        WHERE imported.source_id = ?1
                          AND imported.source_row_id = legacy.id
                     )",
                    [&source_id],
                )
                .map_err(LedgerError::Database)?;
            self.connection
                .execute(
                    "INSERT OR IGNORE INTO legacy_command_imports (source_id, source_row_id)
                     SELECT ?1, id FROM legacy_hzr.commands",
                    [&source_id],
                )
                .map_err(LedgerError::Database)?;
            let imported_parse_failures = self
                .connection
                .execute(
                    "INSERT INTO parse_failures (
                        timestamp, raw_command, error_message, fallback_succeeded
                     ) SELECT
                        legacy.timestamp, legacy.raw_command, legacy.error_message,
                        legacy.fallback_succeeded
                     FROM legacy_hzr.parse_failures AS legacy
                     WHERE NOT EXISTS (
                        SELECT 1 FROM legacy_parse_failure_imports AS imported
                        WHERE imported.source_id = ?1
                          AND imported.source_row_id = legacy.id
                     )",
                    [&source_id],
                )
                .map_err(LedgerError::Database)?;
            self.connection
                .execute(
                    "INSERT OR IGNORE INTO legacy_parse_failure_imports
                        (source_id, source_row_id)
                     SELECT ?1, id FROM legacy_hzr.parse_failures",
                    [&source_id],
                )
                .map_err(LedgerError::Database)?;
            self.connection
                .execute_batch("COMMIT")
                .map_err(LedgerError::Database)?;
            Ok((imported_commands, imported_parse_failures))
        })();
        if result.is_err() {
            let _ = self.connection.execute_batch("ROLLBACK");
        }
        let detach = detach_legacy(&self.connection);
        let (imported_commands, imported_parse_failures) = result?;
        detach?;
        scrub_sensitive_ledger_payloads(&self.connection)?;

        let manifest_directory = migration_root.join("manifests");
        std::fs::create_dir_all(&manifest_directory).map_err(|source| LedgerError::Io {
            operation: "create migration manifest directory",
            path: manifest_directory.clone(),
            source,
        })?;
        let manifest_path = manifest_directory.join(format!("rtk-history-{snapshot_sha256}.json"));
        let report = LegacyEfficiencyMigration {
            source: LegacyEfficiencySource {
                path: source_path,
                size_bytes: source.size_bytes,
                sha256: snapshot_sha256,
                operations: source.operations,
                baseline_tokens_estimated: source.baseline_tokens_estimated,
                delivered_tokens_estimated: source.delivered_tokens_estimated,
                gross_avoided_tokens_estimated: source.gross_avoided_tokens_estimated,
                regression_tokens_estimated: source.regression_tokens_estimated,
                net_avoided_tokens_estimated: source.net_avoided_tokens_estimated,
                parse_failures: source.parse_failures,
            },
            source_id,
            backup_path,
            manifest_path: manifest_path.clone(),
            imported_commands,
            imported_parse_failures,
            changed: imported_commands > 0 || imported_parse_failures > 0,
        };
        let mut manifest = serde_json::to_vec_pretty(&report).map_err(LedgerError::Serialize)?;
        manifest.push(b'\n');
        atomic_write(&manifest_path, &manifest)?;
        Ok(report)
    }
}

pub fn discover_legacy_rtk_history() -> Vec<PathBuf> {
    let Some(base) = BaseDirs::new() else {
        return Vec::new();
    };
    let candidates = [
        base.data_dir().join("rtk/history.db"),
        base.home_dir()
            .join("Library/Application Support/rtk/history.db"),
        base.home_dir().join(".local/share/rtk/history.db"),
    ];
    let mut found = Vec::new();
    for candidate in candidates {
        if candidate.is_file() && !found.contains(&candidate) {
            found.push(candidate);
        }
    }
    found
}

pub fn inspect_legacy_efficiency(path: &Path) -> Result<LegacyEfficiencySource, LedgerError> {
    let connection = open_legacy_read_only(path)?;
    let (
        operations,
        baseline_tokens_estimated,
        delivered_tokens_estimated,
        gross_avoided_tokens_estimated,
        regression_tokens_estimated,
        net_avoided_tokens_estimated,
    ) = connection
        .query_row(
            "SELECT
                COUNT(*),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(CASE WHEN input_tokens > output_tokens
                                  THEN input_tokens - output_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN output_tokens > input_tokens
                                  THEN output_tokens - input_tokens ELSE 0 END), 0),
                COALESCE(SUM(input_tokens - output_tokens), 0)
             FROM commands",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .map_err(LedgerError::Database)?;
    let parse_failures = connection
        .query_row("SELECT COUNT(*) FROM parse_failures", [], |row| row.get(0))
        .map_err(LedgerError::Database)?;
    let bytes = std::fs::read(path).map_err(|source| LedgerError::Io {
        operation: "read legacy RTK history",
        path: path.to_path_buf(),
        source,
    })?;
    Ok(LegacyEfficiencySource {
        path: path.to_path_buf(),
        size_bytes: bytes.len() as u64,
        sha256: hex::encode(Sha256::digest(bytes)),
        operations,
        baseline_tokens_estimated,
        delivered_tokens_estimated,
        gross_avoided_tokens_estimated,
        regression_tokens_estimated,
        net_avoided_tokens_estimated,
        parse_failures,
    })
}

fn operation_identity(
    command: &str,
) -> (
    String,
    ProjectOperationRoute,
    Option<String>,
    Option<String>,
) {
    let classification = classify_operation(command);
    let route = match classification.route {
        OperationRoute::Optimized => ProjectOperationRoute::Optimized,
        OperationRoute::Bypassed => ProjectOperationRoute::Raw,
        OperationRoute::NativeUnaccounted => ProjectOperationRoute::NativeUnaccounted,
    };
    let replacement = classification
        .replacement
        .as_ref()
        .map(|value| value.suggestion.clone());
    let rationale = classification
        .replacement
        .map(|value| value.rationale.to_owned());
    (classification.operation, route, replacement, rationale)
}

fn route_from_ledger(stored: Option<&str>, legacy: OperationRoute) -> OperationRoute {
    match stored {
        Some("optimized") => OperationRoute::Optimized,
        Some("bypassed" | "raw") => OperationRoute::Bypassed,
        Some("native_unaccounted") => OperationRoute::NativeUnaccounted,
        _ => legacy,
    }
}

fn open_legacy_read_only(path: &Path) -> Result<Connection, LedgerError> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(LedgerError::Database)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), LedgerError> {
    let parent = path
        .parent()
        .ok_or_else(|| LedgerError::InvalidPath(path.to_path_buf()))?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| LedgerError::Io {
            operation: "create temporary migration manifest",
            path: parent.to_path_buf(),
            source,
        })?;
    use std::io::Write;
    temporary
        .write_all(bytes)
        .map_err(|source| LedgerError::Io {
            operation: "write migration manifest",
            path: path.to_path_buf(),
            source,
        })?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| LedgerError::Io {
            operation: "sync migration manifest",
            path: path.to_path_buf(),
            source,
        })?;
    temporary.persist(path).map_err(|error| LedgerError::Io {
        operation: "persist migration manifest",
        path: path.to_path_buf(),
        source: error.error,
    })?;
    Ok(())
}

fn migrate_legacy_ledgers(
    connection: &Connection,
    canonical_path: &Path,
) -> Result<(), LedgerError> {
    if canonical_path.file_name().and_then(|name| name.to_str()) != Some("hzr.sqlite") {
        return Ok(());
    }
    let Some(ledger_directory) = canonical_path.parent() else {
        return Ok(());
    };
    if ledger_directory.file_name().and_then(|name| name.to_str()) != Some("ledger") {
        return Ok(());
    }
    let Some(data_root) = ledger_directory.parent() else {
        return Ok(());
    };
    import_legacy_usage(connection, &ledger_directory.join("usage.sqlite"))?;
    import_legacy_efficiency(connection, &data_root.join("fork/history.db"))
}

fn migration_complete(connection: &Connection, key: &str) -> Result<bool, LedgerError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM hzr_migrations WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
        .map_err(LedgerError::Database)
}

fn attach_legacy(connection: &Connection, path: &Path) -> Result<(), LedgerError> {
    connection
        .execute(
            "ATTACH DATABASE ?1 AS legacy_hzr",
            [path.to_string_lossy().as_ref()],
        )
        .map(|_| ())
        .map_err(LedgerError::Database)
}

fn detach_legacy(connection: &Connection) -> Result<(), LedgerError> {
    connection
        .execute_batch("DETACH DATABASE legacy_hzr")
        .map_err(LedgerError::Database)
}

type EconomicRow = (
    String,
    Option<u64>,
    Option<u64>,
    Option<u64>,
    Option<u64>,
    Option<String>,
);

fn aggregate_economic_rows(rows: &[EconomicRow]) -> Result<SessionEconomicSummary, LedgerError> {
    let mut summary = SessionEconomicSummary {
        paired_receipts: u64::try_from(rows.len()).unwrap_or(u64::MAX),
        public_estimate_preliminary: true,
        ..SessionEconomicSummary::default()
    };
    if rows.is_empty() {
        summary
            .unavailable_reasons
            .push("no paired provider receipt is attributed to this session".into());
        return Ok(summary);
    }
    let currencies = rows
        .iter()
        .map(|row| row.0.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if currencies.len() != 1 {
        summary.unavailable_reasons.push(
            "session receipts contain multiple currencies; no hidden FX conversion was applied"
                .into(),
        );
        return Ok(summary);
    }
    let currency = currencies.first().ok_or_else(|| {
        LedgerError::Billing(BillingError::InvalidReceipt(
            "provider receipt currency is missing".into(),
        ))
    })?;
    let invoice_pairs = rows
        .iter()
        .filter_map(|row| row.1.zip(row.2))
        .collect::<Vec<_>>();
    if invoice_pairs.len() == rows.len() {
        summary.invoice_actual = Some(sum_economic_pairs(currency, &invoice_pairs)?);
    } else {
        summary.unavailable_reasons.push(
            "actual billed savings require baseline and delivered provider cost on every receipt"
                .into(),
        );
    }
    let public_pairs = rows
        .iter()
        .filter_map(|row| row.3.zip(row.4))
        .collect::<Vec<_>>();
    if public_pairs.len() == rows.len() {
        summary.public_estimate = Some(sum_economic_pairs(currency, &public_pairs)?);
        summary.price_table_identities = rows
            .iter()
            .filter_map(|row| row.5.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
    } else {
        summary.unavailable_reasons.push(
            "public estimate requires an opted-in exact pricing match for every receipt".into(),
        );
    }
    Ok(summary)
}

fn sum_economic_pairs(currency: &str, pairs: &[(u64, u64)]) -> Result<EconomicAmount, LedgerError> {
    let (baseline, delivered) = pairs
        .iter()
        .try_fold(
            (0_u64, 0_u64),
            |(baseline_total, delivered_total), (baseline, delivered)| {
                Some((
                    baseline_total.checked_add(*baseline)?,
                    delivered_total.checked_add(*delivered)?,
                ))
            },
        )
        .ok_or_else(|| LedgerError::Billing(BillingError::ArithmeticOverflow))?;
    Ok(EconomicAmount {
        currency: currency.into(),
        baseline_microunits: baseline,
        delivered_microunits: delivered,
        savings_microunits: signed_u64_difference(baseline, delivered)?,
    })
}

fn signed_u64_difference(baseline: u64, delivered: u64) -> Result<i64, LedgerError> {
    i64::try_from(i128::from(baseline) - i128::from(delivered))
        .map_err(|_| LedgerError::Billing(BillingError::ArithmeticOverflow))
}

fn import_legacy_usage(connection: &Connection, path: &Path) -> Result<(), LedgerError> {
    const KEY: &str = "usage_sqlite_v1";
    if !path.is_file() || migration_complete(connection, KEY)? {
        return Ok(());
    }
    attach_legacy(connection, path)?;
    let result = connection.execute_batch(
        "BEGIN IMMEDIATE;
         INSERT OR IGNORE INTO usage_records (
            trace_id, created_at_ms, provider, model,
            actual_input, actual_output, actual_reasoning,
            actual_cache_write, actual_cache_read,
            estimated_input, estimated_output, estimate_method,
            turns, retries, latency_ms, outcome, policy_version, cost_microusd
         ) SELECT
            trace_id, created_at_ms, provider, model,
            actual_input, actual_output, actual_reasoning,
            actual_cache_write, actual_cache_read,
            estimated_input, estimated_output, estimate_method,
            turns, retries, latency_ms, outcome, policy_version, cost_microusd
         FROM legacy_hzr.usage_records;
         INSERT INTO hzr_migrations(key, completed_at_ms)
            VALUES ('usage_sqlite_v1', CAST(unixepoch('subsec') * 1000 AS INTEGER));
         COMMIT;",
    );
    if result.is_err() {
        let _ = connection.execute_batch("ROLLBACK");
    }
    let detach = detach_legacy(connection);
    result.map_err(LedgerError::Database)?;
    detach
}

fn import_legacy_efficiency(connection: &Connection, path: &Path) -> Result<(), LedgerError> {
    const KEY: &str = "fork_history_v1";
    if !path.is_file() || migration_complete(connection, KEY)? {
        return Ok(());
    }
    attach_legacy(connection, path)?;
    let result = connection.execute_batch(
        "BEGIN IMMEDIATE;
         INSERT INTO commands (
            timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens,
            saved_tokens, savings_pct, exec_time_ms, project_path
         ) SELECT
            timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens,
            saved_tokens, savings_pct, COALESCE(exec_time_ms, 0), COALESCE(project_path, '')
         FROM legacy_hzr.commands;
         INSERT INTO parse_failures (
            timestamp, raw_command, error_message, fallback_succeeded
         ) SELECT timestamp, raw_command, error_message, fallback_succeeded
         FROM legacy_hzr.parse_failures;
         INSERT INTO hzr_migrations(key, completed_at_ms)
            VALUES ('fork_history_v1', CAST(unixepoch('subsec') * 1000 AS INTEGER));
         COMMIT;",
    );
    if result.is_err() {
        let _ = connection.execute_batch("ROLLBACK");
    }
    let detach = detach_legacy(connection);
    result.map_err(LedgerError::Database)?;
    detach
}

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("invalid operation accounting: {0}")]
    InvalidOperation(String),
    #[error("invalid ledger privacy identity: {0}")]
    InvalidPrivacyIdentity(String),
    #[error("failed to create ledger directory {path}: {source}")]
    Directory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("ledger database error: {0}")]
    Database(rusqlite::Error),
    #[error("{operation} failed for {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("migration snapshot digest does not match its content-addressed name: {0}")]
    SnapshotMismatch(PathBuf),
    #[error("migration path has no parent: {0}")]
    InvalidPath(PathBuf),
    #[error("failed to serialize migration manifest: {0}")]
    Serialize(serde_json::Error),
    #[error(transparent)]
    Billing(BillingError),
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use crate::operation::{OperationChannel, OperationMeasurement, OperationRoute};
    use hzr_protocol::{
        AccountingAttribution, AccountingOperationKind, AccountingOperationMode,
        AccountingSearchStrategy, AccountingStage, ActualUsage, EnforcementTier, EstimatedUsage,
        EvasionAttribution, EvasionClass, EvasionPathForm, FidelityReason, FidelityValidation,
        SearchFallbackCode, TraceId, Usage,
    };
    use rusqlite::{Connection, params};
    use tempfile::tempdir;

    #[test]
    fn every_evasion_class_survives_the_ledger_round_trip() {
        // A class the parser does not know is dropped from the summary by the
        // `continue` in the aggregation loop — silently, and only for the rows
        // that class covers. Growing the taxonomy must therefore fail here
        // rather than in production.
        for class in [
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
        ] {
            assert_eq!(
                super::parse_evasion_class(class.as_str()),
                Some(class),
                "{} has no parser entry",
                class.as_str()
            );
            assert!(
                !class.construct().is_empty() && !class.prescription().is_empty(),
                "{} must carry an agent-facing construct and prescription",
                class.as_str()
            );
        }
    }

    use super::{
        CURRENT_ACCOUNTING_POLICY_VERSION, CURRENT_PRODUCER_VERSION, DetailedOperationAttribution,
        Ledger, LedgerRecord, OperationAttribution, PriceTable, ProjectOperationRoute, StatsQuery,
        operation_identity,
    };
    use crate::operation::ReplacementCapability;

    fn insert_family_row(
        ledger: &Ledger,
        timestamp: i64,
        command: &str,
        delivered: u64,
        route: Option<&str>,
        operation_kind: Option<&str>,
    ) {
        ledger
            .connection
            .execute(
                "INSERT INTO commands (
                    timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens,
                    saved_tokens, savings_pct, exec_time_ms, project_path, channel,
                    measurement, route, operation_kind, producer_version,
                    accounting_policy_version
                 ) VALUES (datetime(?1, 'unixepoch'), '', ?2, ?3, ?3, 0, 0, 0, '',
                           'hook_cli', 'estimated', ?4, ?5, 'test', ?6)",
                params![
                    timestamp,
                    command,
                    delivered,
                    route,
                    operation_kind,
                    CURRENT_ACCOUNTING_POLICY_VERSION
                ],
            )
            .expect("family fixture row");
    }

    #[test]
    fn acceptance_gate_stats_cutoff_is_inclusive_and_shared_by_snapshot_sections() {
        let directory = tempdir().expect("temporary directory");
        let ledger = Ledger::open(&directory.path().join("ledger.sqlite")).expect("ledger");
        let cutoff = 2_000_000_000;
        insert_family_row(&ledger, cutoff - 1, "rtk raw rg old-value", 90, None, None);
        insert_family_row(&ledger, cutoff, "rtk raw rg boundary-value", 10, None, None);
        for (trace_id, created_at_seconds) in [("old", cutoff - 1), ("boundary", cutoff)] {
            ledger
                .connection
                .execute(
                    "INSERT INTO usage_records (
                        trace_id, created_at_ms, turns, retries, latency_ms, outcome,
                        policy_version, project_path
                     ) VALUES (?1, ?2 * 1000, 1, 0, 0, 'accepted', 'test', '')",
                    params![trace_id, created_at_seconds],
                )
                .expect("provider usage fixture row");
        }

        let snapshot = ledger
            .stats_snapshot(StatsQuery {
                project_path: None,
                since_unix_seconds: Some(cutoff),
                include_legacy_versions: true,
            })
            .expect("windowed snapshot");

        assert_eq!(snapshot.efficiency.operations, 1);
        assert_eq!(snapshot.bypass.lifetime.operations, 1);
        assert_eq!(snapshot.provider_usage.tasks, 1);
        assert_eq!(snapshot.by_family.len(), 1);
        assert_eq!(snapshot.by_family[0].delivered_tokens_estimated, 10);
    }

    #[test]
    fn acceptance_gate_family_summary_prefers_typed_route_and_classifies_legacy_rows() {
        let directory = tempdir().expect("temporary directory");
        let ledger = Ledger::open(&directory.path().join("ledger.sqlite")).expect("ledger");
        insert_family_row(
            &ledger,
            2_000_000_000,
            "rtk raw rg legacy-pattern",
            11,
            None,
            None,
        );
        insert_family_row(
            &ledger,
            2_000_000_001,
            "rtk raw rg typed-route-must-win",
            7,
            Some("optimized"),
            Some("search"),
        );

        let families = ledger
            .stats_snapshot(StatsQuery::default())
            .expect("snapshot")
            .by_family;

        assert!(families.iter().any(|family| {
            family.family == "rg"
                && family.route == OperationRoute::Bypassed
                && family.operations == 1
        }));
        assert!(families.iter().any(|family| {
            family.family == "search"
                && family.route == OperationRoute::Optimized
                && family.operations == 1
        }));
    }

    #[test]
    fn acceptance_gate_stored_capability_reconciles_family_and_tool_views() {
        let directory = tempdir().expect("temporary directory");
        let ledger = Ledger::open(&directory.path().join("ledger.sqlite")).expect("ledger");
        insert_family_row(
            &ledger,
            2_000_000_000,
            "rtk raw other",
            11,
            Some("bypassed"),
            Some("other"),
        );
        ledger
            .connection
            .execute(
                "UPDATE commands
                    SET replacement_capability = 'unavailable'
                  WHERE rtk_cmd = 'rtk raw other'",
                [],
            )
            .expect("persist capability evidence");
        let collection = ledger
            .stats_collection(StatsQuery::default())
            .expect("stats collection");
        let family = collection
            .snapshot
            .by_family
            .iter()
            .find(|family| family.family == "other")
            .expect("family");
        let tool = collection
            .snapshot
            .bypass
            .by_tool
            .iter()
            .find(|tool| tool.tool == "other")
            .expect("bypass tool");
        assert_eq!(
            family.replacement_capability,
            ReplacementCapability::Unavailable
        );
        assert_eq!(
            tool.replacement_capability,
            ReplacementCapability::Unavailable
        );
    }

    #[test]
    fn acceptance_gate_family_summary_redacts_recorded_payloads() {
        let directory = tempdir().expect("temporary directory");
        let ledger = Ledger::open(&directory.path().join("ledger.sqlite")).expect("ledger");
        let sensitive = "secret=value /private/customer/query";
        insert_family_row(
            &ledger,
            2_000_000_000,
            &format!("rtk raw rg {sensitive}"),
            5,
            None,
            None,
        );

        let families = ledger
            .stats_snapshot(StatsQuery::default())
            .expect("snapshot")
            .by_family;
        let encoded = serde_json::to_string(&families).expect("family JSON");

        assert!(!encoded.contains("secret=value"));
        assert!(!encoded.contains("/private/customer/query"));
        assert_eq!(families[0].family, "rg");
    }

    #[test]
    fn acceptance_gate_family_summary_groups_and_orders_stably() {
        let directory = tempdir().expect("temporary directory");
        let ledger = Ledger::open(&directory.path().join("ledger.sqlite")).expect("ledger");
        insert_family_row(&ledger, 2_000_000_000, "rtk raw rg one", 10, None, None);
        insert_family_row(&ledger, 2_000_000_001, "rtk raw rg two", 20, None, None);
        insert_family_row(&ledger, 2_000_000_002, "rtk raw cat file", 40, None, None);

        let families = ledger
            .stats_snapshot(StatsQuery::default())
            .expect("snapshot")
            .by_family;

        assert_eq!(families.len(), 2);
        assert_eq!(
            (families[0].family.as_str(), families[0].operations),
            ("cat", 1)
        );
        assert_eq!(
            (families[1].family.as_str(), families[1].operations),
            ("rg", 2)
        );
        assert_eq!(families[1].delivered_tokens_estimated, 30);
        assert_eq!(
            families[1].replacement_capability,
            ReplacementCapability::Unknown,
            "historical rows without evidence must remain unknown"
        );
    }

    #[test]
    fn acceptance_gate_seven_day_legacy_raw_families_report_route_capability() {
        let directory = tempdir().expect("temporary directory");
        let ledger = Ledger::open(&directory.path().join("ledger.sqlite")).expect("ledger");
        let now = 2_000_000_000;
        let cutoff = now - 7 * 24 * 60 * 60;
        for (offset, family) in ["bun", "git", "ssh", "gh", "cargo"].into_iter().enumerate() {
            insert_family_row(
                &ledger,
                cutoff + i64::try_from(offset).expect("small fixture offset"),
                &format!("rtk raw {family} sensitive-argument"),
                10,
                None,
                None,
            );
        }
        insert_family_row(
            &ledger,
            now,
            "rtk raw unknown-tool sensitive-argument",
            10,
            None,
            None,
        );
        insert_family_row(
            &ledger,
            cutoff - 1,
            "rtk raw terraform plan stale",
            10,
            None,
            None,
        );

        ledger
            .connection
            .execute(
                "UPDATE commands
                    SET replacement_capability = 'available'
                  WHERE rtk_cmd GLOB 'rtk raw bun *'
                     OR rtk_cmd GLOB 'rtk raw git *'
                     OR rtk_cmd GLOB 'rtk raw ssh *'
                     OR rtk_cmd GLOB 'rtk raw gh *'
                     OR rtk_cmd GLOB 'rtk raw cargo *'",
                [],
            )
            .expect("persist canonical registry evidence");
        let collection = ledger
            .stats_collection(StatsQuery {
                project_path: None,
                since_unix_seconds: Some(cutoff),
                include_legacy_versions: true,
            })
            .expect("seven-day snapshot");
        let families = collection.snapshot.by_family;

        for family in ["bun", "git", "ssh", "gh", "cargo"] {
            let summary = families
                .iter()
                .find(|summary| summary.family == family)
                .expect("dedicated legacy family");
            assert_eq!(
                summary.replacement_capability,
                ReplacementCapability::Available,
                "{family}"
            );
        }
        assert!(families.iter().any(|summary| {
            summary.family == "unknown-tool"
                && summary.replacement_capability == ReplacementCapability::Unknown
        }));
        assert!(!families.iter().any(|summary| summary.family == "terraform"));
        let encoded = serde_json::to_string(&families).expect("family JSON");
        assert!(!encoded.contains("sensitive-argument"));
    }

    #[test]
    fn acceptance_gate_operation_attribution_migrates_without_sensitive_payloads() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("ledger.sqlite");
        let legacy = Connection::open(&path).expect("legacy database");
        legacy
            .execute_batch(
                "CREATE TABLE commands (
                    id INTEGER PRIMARY KEY, timestamp TEXT NOT NULL, original_cmd TEXT NOT NULL,
                    rtk_cmd TEXT NOT NULL, input_tokens INTEGER NOT NULL,
                    output_tokens INTEGER NOT NULL, saved_tokens INTEGER NOT NULL,
                    savings_pct REAL NOT NULL, exec_time_ms INTEGER DEFAULT 0,
                    project_path TEXT DEFAULT '', agent TEXT, session_id TEXT,
                    channel TEXT NOT NULL DEFAULT 'hook_cli',
                    measurement TEXT NOT NULL DEFAULT 'estimated', route TEXT
                 );",
            )
            .expect("legacy schema");
        drop(legacy);

        let ledger = Ledger::open(&path).expect("migrated ledger");
        let detail = AccountingAttribution {
            operation: AccountingOperationKind::Search,
            mode: AccountingOperationMode::SearchExact,
            stage: AccountingStage::FinalDelivery,
            requested_mode: Some(AccountingOperationMode::SearchAuto),
            effective_mode: Some(AccountingOperationMode::SearchExact),
            search_strategy: Some(AccountingSearchStrategy::ForkRgaiBuiltin),
            search_fallback_code: Some(SearchFallbackCode::SemanticIndexUnavailable),
            include_content: Some(false),
            limit: Some(7),
            path_scope_count: Some(1),
            filter_level: None,
            from_line: None,
            to_line: None,
            source_bytes: None,
            evasion: None,
        };
        ledger
            .record_operation_attributed_with_detail(
                "hzr search <query omitted>",
                "hzr search",
                12,
                8,
                2,
                DetailedOperationAttribution {
                    attribution: OperationAttribution {
                        project_path: "/work",
                        agent: Some("mcp"),
                        session_id: Some("session"),
                        channel: OperationChannel::Mcp,
                        measurement: OperationMeasurement::Estimated,
                        route: OperationRoute::Optimized,
                    },
                    detail: Some(&detail),
                    evasion: None,
                },
            )
            .expect("attributed operation");

        let persisted: (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            bool,
            u64,
            u64,
        ) = ledger
            .connection
            .query_row(
                "SELECT operation_kind, operation_mode, accounting_stage, requested_mode,
                        effective_mode, search_strategy, search_fallback_code,
                        search_include_content, result_limit, path_scope_count FROM commands",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                    ))
                },
            )
            .expect("persisted dimensions");
        assert_eq!(
            persisted,
            (
                "search".into(),
                "search_exact".into(),
                "final_delivery".into(),
                "search_auto".into(),
                "search_exact".into(),
                "fork_rgai_builtin".into(),
                "semantic_index_unavailable".into(),
                false,
                7,
                1,
            )
        );
        let versions: (String, String) = ledger
            .connection
            .query_row(
                "SELECT producer_version, accounting_policy_version FROM commands",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("accounting versions");
        assert_eq!(versions.0, CURRENT_PRODUCER_VERSION);
        assert_eq!(versions.1, CURRENT_ACCOUNTING_POLICY_VERSION);
        let summary = ledger.efficiency_summary().expect("efficiency summary");
        assert_eq!(summary.by_mode.len(), 1);
        assert_eq!(
            summary.by_mode[0].mode,
            AccountingOperationMode::SearchExact
        );
        assert_eq!(summary.by_mode[0].delivered_tokens_estimated, 8);
    }

    #[test]
    fn acceptance_gate_existing_payloads_are_scrubbed_without_losing_aggregates() {
        for sentinel in [
            "secret=value",
            "/private/customer/file.rs",
            "SELECT * FROM customer_secrets",
            "python3 -c 'print(credential)'",
            "<<HEREDOC private-body HEREDOC",
        ] {
            let directory = tempdir().expect("temporary directory");
            let path = directory.path().join("ledger.sqlite");
            drop(Ledger::open(&path).expect("initial ledger"));
            let connection = Connection::open(&path).expect("fixture connection");
            connection
                .execute(
                    "INSERT INTO commands (
                        timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens,
                        saved_tokens, savings_pct, exec_time_ms, project_path, session_id
                     ) VALUES (datetime('now'), ?1, ?2, 100, 25, 75, 75, 3, ?3, ?4)",
                    params![
                        format!("run {sentinel}"),
                        format!("rtk raw python3 {sentinel}"),
                        format!("/workspace/{sentinel}"),
                        sentinel,
                    ],
                )
                .expect("legacy command");
            connection
                .execute(
                    "INSERT INTO parse_failures (
                        timestamp, raw_command, error_message, fallback_succeeded
                     ) VALUES (datetime('now'), ?1, ?1, 0)",
                    [sentinel],
                )
                .expect("legacy parse failure");
            drop(connection);

            let ledger = Ledger::open(&path).expect("scrubbed ledger");
            let serialized: String = ledger
                .connection
                .query_row(
                    "SELECT original_cmd || rtk_cmd || project_path ||
                            COALESCE(session_id, '') || COALESCE(command_hash, '')
                       FROM commands",
                    [],
                    |row| row.get(0),
                )
                .expect("scrubbed command");
            let parse_payload: String = ledger
                .connection
                .query_row(
                    "SELECT raw_command || error_message || COALESCE(command_hash, '') ||
                            COALESCE(error_hash, '') FROM parse_failures",
                    [],
                    |row| row.get(0),
                )
                .expect("scrubbed failure");
            assert!(!serialized.contains(sentinel), "command leaked {sentinel}");
            assert!(!parse_payload.contains(sentinel), "error leaked {sentinel}");
            assert!(serialized.contains("sha256:"));
            let compatibility = ledger
                .efficiency_summary_scoped(None, None, true)
                .expect("compatibility aggregates");
            assert_eq!(compatibility.operations, 1);
            assert_eq!(compatibility.baseline_tokens_estimated, 25);
            assert_eq!(compatibility.delivered_tokens_estimated, 25);
            let current = ledger.efficiency_summary().expect("current aggregates");
            assert_eq!(current.operations, 0);
            assert_eq!(current.excluded_legacy_operations, 1);
        }
    }

    #[test]
    fn acceptance_gate_read_pipeline_splits_selection_from_transform() {
        let directory = tempdir().expect("temporary directory");
        let ledger = Ledger::open(&directory.path().join("ledger.sqlite")).expect("ledger");
        let detail = AccountingAttribution {
            operation: AccountingOperationKind::Read,
            mode: AccountingOperationMode::ReadRange,
            stage: AccountingStage::InternalTransport,
            requested_mode: Some(AccountingOperationMode::ReadRange),
            effective_mode: Some(AccountingOperationMode::ReadRange),
            search_strategy: None,
            search_fallback_code: None,
            include_content: None,
            limit: None,
            path_scope_count: Some(1),
            filter_level: None,
            from_line: Some(10),
            to_line: Some(30),
            source_bytes: Some(4_000),
            evasion: None,
        };
        ledger
            .record_operation_attributed_with_detail(
                "read private.rs --from 10 --to 30",
                "rtk read",
                200,
                100,
                2,
                DetailedOperationAttribution {
                    attribution: OperationAttribution {
                        project_path: "/work",
                        agent: Some("test"),
                        session_id: Some("session"),
                        channel: OperationChannel::HookCli,
                        measurement: OperationMeasurement::Estimated,
                        route: OperationRoute::Optimized,
                    },
                    detail: Some(&detail),
                    evasion: None,
                },
            )
            .expect("read operation");

        let pipeline = ledger
            .efficiency_summary()
            .expect("efficiency")
            .read_pipeline;
        assert_eq!(pipeline.operations, 1);
        assert_eq!(pipeline.source_tokens_estimated, 1_000);
        assert_eq!(pipeline.selected_tokens_estimated, 200);
        assert_eq!(pipeline.delivered_tokens_estimated, 100);
        assert_eq!(pipeline.selection_avoided_tokens_estimated, 800);
        assert_eq!(pipeline.selection_overhead_tokens_estimated, 0);
        assert_eq!(pipeline.transform_avoided_tokens_estimated, 100);
        assert_eq!(pipeline.transform_overhead_tokens_estimated, 0);
    }

    #[test]
    fn acceptance_gate_final_delivery_is_stage_visible_but_not_double_counted() {
        let directory = tempdir().expect("temporary directory");
        let ledger = Ledger::open(&directory.path().join("ledger.sqlite")).expect("ledger");
        let internal = AccountingAttribution {
            operation: AccountingOperationKind::Search,
            mode: AccountingOperationMode::SearchSemantic,
            stage: AccountingStage::InternalTransport,
            requested_mode: Some(AccountingOperationMode::SearchAuto),
            effective_mode: Some(AccountingOperationMode::SearchSemantic),
            search_strategy: Some(AccountingSearchStrategy::ForkRgaiAdaptive),
            search_fallback_code: None,
            include_content: Some(false),
            limit: Some(10),
            path_scope_count: Some(1),
            filter_level: None,
            from_line: None,
            to_line: None,
            source_bytes: None,
            evasion: None,
        };
        let final_delivery = AccountingAttribution {
            stage: AccountingStage::FinalDelivery,
            ..internal.clone()
        };
        for (baseline, delivered, detail) in [(100, 20, &internal), (20, 20, &final_delivery)] {
            ledger
                .record_operation_attributed_with_detail(
                    "hzr search",
                    "hzr search",
                    baseline,
                    delivered,
                    1,
                    DetailedOperationAttribution {
                        attribution: OperationAttribution {
                            project_path: "/work",
                            agent: Some("cli"),
                            session_id: Some("session"),
                            channel: OperationChannel::HookCli,
                            measurement: OperationMeasurement::Estimated,
                            route: OperationRoute::Optimized,
                        },
                        detail: Some(detail),
                        evasion: None,
                    },
                )
                .expect("record stage");
        }
        let control_plane = AccountingAttribution {
            operation: AccountingOperationKind::Doctor,
            mode: AccountingOperationMode::DoctorCheck,
            stage: AccountingStage::ControlPlane,
            requested_mode: None,
            effective_mode: Some(AccountingOperationMode::DoctorCheck),
            search_strategy: None,
            search_fallback_code: None,
            include_content: None,
            limit: None,
            path_scope_count: None,
            filter_level: None,
            from_line: None,
            to_line: None,
            source_bytes: None,
            evasion: None,
        };
        ledger
            .record_operation_attributed_with_detail(
                "hzr doctor",
                "hzr doctor",
                10,
                10,
                1,
                DetailedOperationAttribution {
                    attribution: OperationAttribution {
                        project_path: "/work",
                        agent: Some("cli"),
                        session_id: Some("session"),
                        channel: OperationChannel::HookCli,
                        measurement: OperationMeasurement::Estimated,
                        route: OperationRoute::Optimized,
                    },
                    detail: Some(&control_plane),
                    evasion: None,
                },
            )
            .expect("record control plane stage");

        let summary = ledger.efficiency_summary().expect("efficiency summary");
        assert_eq!(summary.operations, 1);
        assert_eq!(summary.baseline_tokens_estimated, 100);
        assert_eq!(summary.delivered_tokens_estimated, 20);
        assert_eq!(summary.total_observed_operations, 1);
        assert_eq!(summary.by_mode.len(), 3);
        assert!(
            summary
                .by_mode
                .iter()
                .any(|mode| mode.stage == AccountingStage::FinalDelivery && mode.operations == 1)
        );
        let bypass = ledger.bypass_summary().expect("bypass summary");
        assert_eq!(bypass.lifetime.operations, 0);
        assert_eq!(bypass.lifetime.total_operations, 1);
        assert_eq!(bypass.lifetime.total_delivered_tokens_estimated, 20);
        let project = ledger.project_activity("/work").expect("project activity");
        assert_eq!(project.operations, 1);
        assert_eq!(project.recent_operations.len(), 1);
        assert_eq!(project.delivered_tokens_estimated, 20);
    }

    #[test]
    fn test_accounting_dimensions_are_migrated_and_reported_without_faking_zero_output() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("usage.sqlite");
        let legacy = Connection::open(&path).expect("legacy database");
        legacy
            .execute_batch(
                "CREATE TABLE commands (
                    id INTEGER PRIMARY KEY,
                    timestamp TEXT NOT NULL,
                    original_cmd TEXT NOT NULL,
                    rtk_cmd TEXT NOT NULL,
                    input_tokens INTEGER NOT NULL,
                    output_tokens INTEGER NOT NULL,
                    saved_tokens INTEGER NOT NULL,
                    savings_pct REAL NOT NULL,
                    exec_time_ms INTEGER DEFAULT 0,
                    project_path TEXT DEFAULT '',
                    agent TEXT,
                    session_id TEXT
                 );
                 INSERT INTO commands (
                    timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens,
                    saved_tokens, savings_pct, exec_time_ms, project_path
                 ) VALUES ('2026-08-05', 'cat old', 'rtk read old', 100, 20, 80, 80, 1, '/work');",
            )
            .expect("legacy schema");
        drop(legacy);

        let ledger = Ledger::open(&path).expect("ledger migration");
        ledger
            .record_operation_attributed(
                "native Read",
                "native Read",
                40,
                40,
                2,
                OperationAttribution {
                    project_path: "/work",
                    agent: Some("claude"),
                    session_id: Some("session"),
                    channel: OperationChannel::NativeHost,
                    measurement: OperationMeasurement::Estimated,
                    route: OperationRoute::NativeUnaccounted,
                },
            )
            .expect("native observation");
        ledger
            .record_operation_attributed(
                "npx package",
                "rtk proxy npx package",
                0,
                0,
                3,
                OperationAttribution {
                    project_path: "/work",
                    agent: None,
                    session_id: None,
                    channel: OperationChannel::HookCli,
                    measurement: OperationMeasurement::Unmeasured,
                    route: OperationRoute::Bypassed,
                },
            )
            .expect("unmeasured bypass");

        let summary = ledger.efficiency_summary().expect("efficiency summary");
        assert_eq!(
            summary.operations, 0,
            "legacy rows leave the current-policy ratio"
        );
        assert_eq!(summary.native_unaccounted_operations, 1);
        assert_eq!(summary.unmeasured_bypass_operations, 1);
        assert_eq!(summary.accounted_operations, 1);
        assert_eq!(summary.total_observed_operations, 2);
        assert_eq!(summary.by_channel.get("hook_cli"), Some(&1));
        assert_eq!(summary.by_channel.get("native_host"), Some(&1));
        assert_eq!(summary.excluded_legacy_operations, 1);
    }

    #[test]
    fn test_unobserved_write_counterfactual_is_neutral_in_summary() {
        let directory = tempdir().expect("temp directory");
        let ledger = Ledger::open(&directory.path().join("usage.sqlite")).expect("ledger");
        ledger
            .record_operation("write patch file", "rtk write", 1_000, 10, 1, "/work")
            .expect("write operation");

        let summary = ledger
            .efficiency_summary_for_project("/work")
            .expect("efficiency summary");
        assert_eq!(summary.baseline_tokens_estimated, 10);
        assert_eq!(summary.delivered_tokens_estimated, 10);
        assert_eq!(summary.gross_avoided_tokens_estimated, 0);
        assert_eq!(summary.regression_tokens_estimated, 0);
        assert_eq!(summary.net_avoided_tokens_estimated, 0);
        assert_eq!(summary.by_command.len(), 1);
        assert_eq!(summary.by_command[0].baseline_tokens_estimated, 10);
        assert_eq!(summary.by_command[0].net_avoided_tokens_estimated, 0);
    }

    #[test]
    fn project_activity_separates_current_policy_from_legacy_rows() {
        let directory = tempdir().expect("temp directory");
        let ledger = Ledger::open(&directory.path().join("usage.sqlite")).expect("ledger");
        for command in ["cargo test current", "cargo test legacy"] {
            ledger
                .record_operation_attributed(
                    command,
                    "rtk cargo test",
                    100,
                    20,
                    1,
                    OperationAttribution {
                        project_path: "/work/project",
                        agent: Some("test"),
                        session_id: Some("session"),
                        channel: OperationChannel::HookCli,
                        measurement: OperationMeasurement::Estimated,
                        route: OperationRoute::Optimized,
                    },
                )
                .expect("operation");
        }
        ledger
            .connection
            .execute(
                "UPDATE commands SET accounting_policy_version = ?1 WHERE id = (SELECT MAX(id) FROM commands)",
                [super::LEGACY_ACCOUNTING_POLICY_VERSION_V1],
            )
            .expect("legacy fixture");

        let activity = ledger
            .project_activity("/work/project")
            .expect("current project activity");
        assert_eq!(activity.operations, 1);
        assert_eq!(activity.excluded_legacy_operations, 1);
        assert_eq!(activity.recent_operations.len(), 1);
        assert!(activity.recent_operations.iter().all(|operation| {
            operation.policy_version.as_deref() == Some(CURRENT_ACCOUNTING_POLICY_VERSION)
        }));
    }

    #[test]
    fn test_proxy_ledger_rows_are_classified_as_raw() {
        assert_eq!(
            operation_identity("rtk proxy sed -n 1,20p file"),
            ("sed".into(), ProjectOperationRoute::Raw, None, None)
        );
    }

    /// Regression for the empty-ledger crash: `SUM(...)` over zero rows yields NULL, so
    /// a fresh install — the very first `hzr stats` anyone runs — failed instead of
    /// reporting zeros.
    #[test]
    fn test_summary_on_empty_database_reports_zero_totals_without_error() {
        let directory = tempdir().expect("temp directory");
        let ledger = Ledger::open(&directory.path().join("empty.sqlite")).expect("ledger open");
        let summary = ledger
            .summary()
            .expect("an empty ledger must summarize, not error");
        assert_eq!(summary.tasks, 0);
        assert_eq!(summary.accepted, 0, "NULL accepted must read as zero");
        assert_eq!(summary.actual_input_tokens, 0);
        assert_eq!(summary.actual_output_tokens, 0);
        assert_eq!(summary.estimated_input_tokens, 0);
    }

    #[test]
    fn test_read_only_dashboard_summary_does_not_create_a_fresh_ledger() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("absent.sqlite");
        let (usage, efficiency) =
            Ledger::summaries_read_only(&path).expect("absent ledger has zero dashboard totals");

        assert_eq!(usage.tasks, 0);
        assert_eq!(efficiency.operations, 0);
        assert!(
            !path.exists(),
            "a GET-style summary must not create the ledger"
        );
    }

    #[test]
    fn test_project_activity_is_exactly_scoped_and_reports_unscoped_rows() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("usage.sqlite");
        let ledger = Ledger::open(&path).expect("ledger open");
        ledger
            .connection
            .execute_batch(
                "INSERT INTO commands (
                    timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens,
                    saved_tokens, savings_pct, exec_time_ms, project_path
                 ) VALUES
                    ('2026-08-01T10:00:00Z', 'cat a', 'read', 100, 20, 80, 80.0, 5, '/work/a'),
                    ('2026-08-01T10:01:00Z', 'cat b', 'read', 70, 10, 60, 85.7, 7, '/work/b'),
                    ('2026-08-01T10:02:00Z', 'cat x', 'read', 50, 10, 40, 80.0, 3, ''),
                    ('2026-08-01T10:03:00Z', 'sed a', 'rtk proxy sed', 40, 5, 35, 87.5, 4, '/work/a'),
                    ('2026-08-01T10:04:00Z', 'read nested', 'read', 30, 10, 20, 66.7, 2, '/work/a/sub'),
                    ('2026-08-01T10:05:00Z', 'read sibling', 'read', 100, 0, 100, 100.0, 1, '/work/ab');",
            )
            .expect("activity fixture");
        drop(ledger);
        let ledger = Ledger::open(&path).expect("scrubbed ledger");
        ledger
            .connection
            .execute(
                "UPDATE commands SET accounting_policy_version = ?1",
                [CURRENT_ACCOUNTING_POLICY_VERSION],
            )
            .expect("current-policy scope fixture");

        let activity = ledger
            .project_activity("/work/a")
            .expect("project activity");

        assert_eq!(activity.operations, 3);
        assert_eq!(activity.optimized_operations, 2);
        assert_eq!(activity.raw_operations, 1);
        assert_eq!(activity.baseline_tokens_estimated, 135);
        assert_eq!(activity.delivered_tokens_estimated, 35);
        assert_eq!(activity.net_avoided_tokens_estimated, 100);
        assert_eq!(activity.total_execution_ms, 11);
        assert_eq!(activity.unscoped_operations, 1);
        assert_eq!(activity.recent_operations.len(), 3);
        assert_eq!(
            activity.recent_operations[1].route,
            ProjectOperationRoute::Raw
        );
        assert_eq!(activity.recent_operations[1].baseline_tokens_estimated, 5);
        assert_eq!(activity.recent_operations[1].delivered_tokens_estimated, 5);
        assert_eq!(
            activity.recent_operations[1].net_avoided_tokens_estimated,
            0
        );
        assert_eq!(
            activity.first_record_at.as_deref(),
            Some("2026-08-01T10:00:00Z")
        );
        assert_eq!(
            activity.last_record_at.as_deref(),
            Some("2026-08-01T10:04:00Z")
        );
    }

    #[test]
    fn test_efficiency_and_bypass_summaries_can_be_scoped_to_one_project() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("usage.sqlite");
        let ledger = Ledger::open(&path).expect("ledger open");
        ledger
            .connection
            .execute_batch(
                "INSERT INTO commands (
                    timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens,
                    saved_tokens, savings_pct, exec_time_ms, project_path
                 ) VALUES
                    ('2026-08-01T10:00:00Z', 'cat a', 'read', 100, 20, 80, 80.0, 5, '/work/a'),
                    ('2026-08-01T10:01:00Z', 'cat nested', 'read', 50, 10, 40, 80.0, 3, '/work/a/sub'),
                    ('2026-08-01T10:02:00Z', 'sed a', 'rtk proxy sed', 30, 30, 0, 0.0, 2, '/work/a'),
                    ('2026-08-01T10:03:00Z', 'cat b', 'read', 500, 5, 495, 99.0, 7, '/work/b');",
            )
            .expect("summary fixture");
        drop(ledger);
        let ledger = Ledger::open(&path).expect("scrubbed ledger");

        let gain = ledger
            .efficiency_summary_scoped(Some("/work/a"), None, true)
            .expect("project efficiency");
        let bypass = ledger
            .bypass_summary_scoped(Some("/work/a"), None, true)
            .expect("project bypass");

        assert_eq!(gain.operations, 3);
        assert_eq!(gain.baseline_tokens_estimated, 180);
        assert_eq!(gain.delivered_tokens_estimated, 60);
        assert_eq!(gain.net_avoided_tokens_estimated, 120);
        assert_eq!(bypass.lifetime.operations, 1);
        assert_eq!(bypass.lifetime.total_operations, 3);
        assert_eq!(bypass.lifetime.total_delivered_tokens_estimated, 60);
    }

    #[test]
    fn test_legacy_named_database_is_not_migrated_into_itself() {
        let directory = tempdir().expect("temp directory");
        let ledger_directory = directory.path().join("ledger");
        std::fs::create_dir_all(&ledger_directory).expect("ledger directory");
        let ledger = Ledger::open(&ledger_directory.join("usage.sqlite"))
            .expect("legacy-named database opens without self-attach");

        assert_eq!(ledger.summary().expect("summary").tasks, 0);
    }

    #[test]
    fn test_platform_history_migration_snapshots_and_imports_each_row_once() {
        let directory = tempdir().expect("temp directory");
        let source_path = directory.path().join("legacy/history.db");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("source directory");
        let source = Connection::open(&source_path).expect("legacy database");
        source
            .execute_batch(
                "CREATE TABLE commands (
                    id INTEGER PRIMARY KEY,
                    timestamp TEXT NOT NULL,
                    original_cmd TEXT NOT NULL,
                    rtk_cmd TEXT NOT NULL,
                    input_tokens INTEGER NOT NULL,
                    output_tokens INTEGER NOT NULL,
                    saved_tokens INTEGER NOT NULL,
                    savings_pct REAL NOT NULL,
                    exec_time_ms INTEGER DEFAULT 0,
                    project_path TEXT
                 );
                 CREATE TABLE parse_failures (
                    id INTEGER PRIMARY KEY,
                    timestamp TEXT NOT NULL,
                    raw_command TEXT NOT NULL,
                    error_message TEXT NOT NULL,
                    fallback_succeeded INTEGER NOT NULL DEFAULT 0
                 );
                 INSERT INTO commands VALUES
                    (1, '2026-01-01', 'cat a', 'rtk read a', 100, 20, 80, 80.0, 5, '/a'),
                    (2, '2026-01-02', 'cat b', 'rtk read b', 10, 30, -20, -200.0, 7, '/b');
                 INSERT INTO parse_failures VALUES
                    (1, '2026-01-03', 'bad', 'parse', 1);",
            )
            .expect("legacy fixture");
        drop(source);
        let source_before = std::fs::read(&source_path).expect("source bytes");

        let data_root = directory.path().join("data");
        let ledger = Ledger::open(&data_root.join("ledger/hzr.sqlite")).expect("canonical ledger");
        let first = ledger
            .migrate_legacy_efficiency(&source_path, &data_root.join("migrations"))
            .expect("first migration");
        let second = ledger
            .migrate_legacy_efficiency(&source_path, &data_root.join("migrations"))
            .expect("idempotent migration");
        let summary = ledger
            .efficiency_summary_scoped(None, None, true)
            .expect("efficiency summary");

        assert_eq!(first.imported_commands, 2);
        assert_eq!(first.imported_parse_failures, 1);
        assert!(first.changed);
        assert!(first.backup_path.is_file());
        assert!(first.manifest_path.is_file());
        assert_eq!(first.source.operations, 2);
        assert_eq!(first.source.gross_avoided_tokens_estimated, 80);
        assert_eq!(first.source.regression_tokens_estimated, 20);
        assert_eq!(first.source.net_avoided_tokens_estimated, 60);
        assert_eq!(second.imported_commands, 0);
        assert_eq!(second.imported_parse_failures, 0);
        assert!(!second.changed);
        assert_eq!(summary.operations, 2);
        assert_eq!(summary.net_avoided_tokens_estimated, 60);
        assert_eq!(
            std::fs::read(&source_path).expect("source after migration"),
            source_before,
            "migration must never mutate the legacy database"
        );
    }

    #[test]
    fn test_ledger_keeps_estimates_out_of_actual_totals() {
        let directory = tempdir().expect("temp directory");
        let ledger = Ledger::open(&directory.path().join("usage.sqlite")).expect("ledger open");
        let trace_id = TraceId::new();
        let record = LedgerRecord {
            trace_id: trace_id.clone(),
            provider: Some("test".into()),
            model: Some("model".into()),
            usage: Usage {
                actual: ActualUsage {
                    input_tokens: Some(100),
                    output_tokens: Some(20),
                    ..ActualUsage::default()
                },
                estimated: EstimatedUsage {
                    input_tokens: Some(900),
                    method: Some("estimate".into()),
                    ..EstimatedUsage::default()
                },
            },
            turns: 1,
            retries: 0,
            latency_ms: 10,
            outcome: "accepted".into(),
            policy_version: "0.4.6".into(),
            cost_microusd: Some(50),
            project_path: String::new(),
        };

        ledger.record(&record).expect("record");
        let summary = ledger.summary().expect("summary");
        let loaded = ledger
            .find(&trace_id)
            .expect("find")
            .expect("record exists");

        assert_eq!(summary.actual_input_tokens, 100);
        assert_eq!(summary.estimated_input_tokens, 900);
        assert_eq!(loaded.trace_id, trace_id);
    }

    /// Старые чеки без project_path остаются глобальными; scoped summary считает только
    /// строки с совпадающей workspace-идентичностью и не смешивает их с соседним проектом.
    #[test]
    fn test_provider_summary_scopes_to_matching_workspace_and_skips_unscoped_rows() {
        let directory = tempdir().expect("temp directory");
        let ledger = Ledger::open(&directory.path().join("usage.sqlite")).expect("ledger open");

        let scoped = |trace: &str, path: &str, input: u64| LedgerRecord {
            trace_id: TraceId::from_string(trace.to_owned()),
            provider: Some("test".into()),
            model: Some("model".into()),
            usage: Usage {
                actual: ActualUsage {
                    input_tokens: Some(input),
                    output_tokens: Some(1),
                    ..ActualUsage::default()
                },
                ..Usage::default()
            },
            turns: 1,
            retries: 0,
            latency_ms: 1,
            outcome: "completed".into(),
            policy_version: "0.4.6".into(),
            cost_microusd: Some(10),
            project_path: path.to_owned(),
        };

        ledger
            .record(&scoped("legacy-unscoped", "", 1_000))
            .expect("unscoped");
        ledger
            .record(&scoped("project-a", "/work/a", 100))
            .expect("project a");
        ledger
            .record(&scoped("project-a-child", "/work/a/pkg", 50))
            .expect("project a child");
        ledger
            .record(&scoped("project-ab-prefix", "/work/ab", 900))
            .expect("prefix sibling");

        let global = ledger.summary().expect("global");
        let scoped_a = ledger
            .summary_for_project("/work/a")
            .expect("scoped summary");
        let loaded = ledger
            .find(&TraceId::from_string("project-a".into()))
            .expect("find")
            .expect("exists");

        assert_eq!(global.actual_input_tokens, 2_050);
        assert_eq!(scoped_a.tasks, 2);
        assert_eq!(scoped_a.actual_input_tokens, 150);
        assert_eq!(scoped_a.actual_output_tokens, 2);
        assert_eq!(scoped_a.cost_microusd, 20);
        assert_eq!(loaded.project_path, "[redacted]");
    }

    #[test]
    fn test_usage_project_path_column_migrates_idempotently_on_legacy_schema() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("usage.sqlite");
        let legacy = Connection::open(&path).expect("legacy database");
        legacy
            .execute_batch(
                "CREATE TABLE usage_records (
                    trace_id TEXT PRIMARY KEY,
                    created_at_ms INTEGER NOT NULL,
                    provider TEXT,
                    model TEXT,
                    actual_input INTEGER,
                    actual_output INTEGER,
                    actual_reasoning INTEGER,
                    actual_cache_write INTEGER,
                    actual_cache_read INTEGER,
                    estimated_input INTEGER,
                    estimated_output INTEGER,
                    estimate_method TEXT,
                    turns INTEGER NOT NULL,
                    retries INTEGER NOT NULL,
                    latency_ms INTEGER NOT NULL,
                    outcome TEXT NOT NULL,
                    policy_version TEXT NOT NULL,
                    cost_microusd INTEGER
                 );
                 INSERT INTO usage_records (
                    trace_id, created_at_ms, provider, model,
                    actual_input, actual_output, actual_reasoning,
                    actual_cache_write, actual_cache_read,
                    estimated_input, estimated_output, estimate_method,
                    turns, retries, latency_ms, outcome, policy_version, cost_microusd
                 ) VALUES (
                    'legacy', 1, 'test', 'model',
                    40, 2, NULL, NULL, NULL,
                    NULL, NULL, NULL,
                    1, 0, 1, 'completed', '0.3.6', 5
                 );",
            )
            .expect("legacy usage schema");
        drop(legacy);

        let ledger = Ledger::open(&path).expect("first open migrates");
        let _ = Ledger::open(&path).expect("second open stays idempotent");
        ledger
            .record(&LedgerRecord {
                trace_id: TraceId::from_string("scoped".into()),
                provider: Some("test".into()),
                model: Some("model".into()),
                usage: Usage {
                    actual: ActualUsage {
                        input_tokens: Some(10),
                        output_tokens: Some(1),
                        ..ActualUsage::default()
                    },
                    ..Usage::default()
                },
                turns: 1,
                retries: 0,
                latency_ms: 1,
                outcome: "completed".into(),
                policy_version: "0.4.6".into(),
                cost_microusd: Some(1),
                project_path: "/work/a".into(),
            })
            .expect("scoped insert");

        assert_eq!(ledger.summary().expect("global").actual_input_tokens, 50);
        assert_eq!(
            ledger
                .summary_for_project("/work/a")
                .expect("scoped")
                .actual_input_tokens,
            10
        );
        assert_eq!(
            ledger
                .find(&TraceId::from_string("legacy".into()))
                .expect("find")
                .expect("legacy row")
                .project_path,
            ""
        );
    }

    #[test]
    fn acceptance_gate_evasion_and_fidelity_are_payload_free_and_session_bounded() {
        let directory = tempdir().expect("temporary directory");
        let ledger = Ledger::open(&directory.path().join("ledger.sqlite")).expect("ledger");
        let sentinel = "SENTINEL_SELECT_secret_path=/private/customer.sql";
        let evasion = EvasionAttribution {
            class: EvasionClass::E7FidelityHatch,
            wrapper_depth: 0,
            interpreter: None,
            path_form: EvasionPathForm::Bare,
            stage_count: 1,
            hatch_marker: true,
            avoidable: true,
            tier: EnforcementTier::T0TransparentRewrite,
            fidelity_reason: Some(FidelityReason::MachineProtocol),
            fidelity_validation: FidelityValidation::Valid,
        };
        for _ in 0..5 {
            ledger
                .record_operation_attributed_with_detail(
                    sentinel,
                    &format!(
                        "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=machine_protocol hzr rtk -- raw proto-dumper --json {sentinel}"
                    ),
                    25_000,
                    25_000,
                    1,
                    DetailedOperationAttribution {
                        attribution: OperationAttribution {
                            project_path: "/private/workspace",
                            agent: Some("claude-code:subagent-private-id"),
                            session_id: Some("session-private-id"),
                            channel: OperationChannel::HookCli,
                            measurement: OperationMeasurement::Estimated,
                            route: OperationRoute::Bypassed,
                        },
                        detail: None,
                        evasion: Some(&evasion),
                    },
                )
                .expect("record fidelity operation");
        }

        let evasion = ledger
            .evasion_summary(StatsQuery::default())
            .expect("evasion summary");
        assert_eq!(evasion.fidelity_operations, 5);
        assert_eq!(evasion.fidelity_delivered_tokens, 125_000);
        assert_eq!(evasion.fidelity_invalid_operations, 0);
        let usage = ledger
            .fidelity_session_usage("session-private-id", super::FidelityAllowance::default())
            .expect("fidelity usage");
        assert!(usage.exhausted);
        assert_eq!(usage.remaining_operations, 0);
        assert_eq!(usage.remaining_tokens, 0);
        let score = ledger
            .session_evasion_summary("session-private-id", super::FidelityAllowance::default())
            .expect("scorecard");
        assert_eq!(score.operations, 5);
        assert_eq!(score.agent.as_deref(), Some("claude-code"));
        assert!(
            score
                .agent_hash
                .as_deref()
                .is_some_and(|hash| hash.starts_with("hmac-sha256:"))
        );
        let json = serde_json::to_string(&(evasion, usage, score)).expect("aggregate JSON");
        for private in [
            sentinel,
            "/private/workspace",
            "session-private-id",
            "subagent-private-id",
        ] {
            assert!(!json.contains(private), "aggregate leaked {private}");
        }
        let stored: (
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = ledger
            .connection
            .query_row(
                "SELECT original_cmd, rtk_cmd, project_path, session_id, fidelity_reason
                   FROM commands LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("stored row");
        let stored_json = serde_json::to_string(&stored).expect("stored JSON");
        assert!(!stored_json.contains(sentinel));
        assert_eq!(stored.4.as_deref(), Some("machine_protocol"));

        let keyed_session_hash: String = ledger
            .connection
            .query_row("SELECT session_hash FROM commands LIMIT 1", [], |row| {
                row.get(0)
            })
            .expect("stored session pseudonym");
        assert!(keyed_session_hash.starts_with("hmac-sha256:"));
        assert_ne!(
            keyed_session_hash,
            super::privacy_identity_hash("session", "session-private-id")
        );
        let identity_version: String = ledger
            .connection
            .query_row(
                "SELECT value FROM ledger_privacy_meta WHERE key = 'identity_hmac_version'",
                [],
                |row| row.get(0),
            )
            .expect("identity HMAC version");
        assert_eq!(identity_version, super::IDENTITY_HMAC_KEY256_V2);

        // Rows written before the keyed-session migration cannot be rewritten because their
        // raw session IDs were intentionally scrubbed. Budget reads therefore match both the
        // current keyed pseudonym and the legacy domain-separated SHA-256 pseudonym.
        ledger
            .connection
            .execute(
                "UPDATE commands SET session_hash = ?1, accounting_policy_version = ?2
                  WHERE id = (SELECT MIN(id) FROM commands)",
                params![
                    super::privacy_identity_hash("session", "session-private-id"),
                    super::LEGACY_ACCOUNTING_POLICY_VERSION_V1,
                ],
            )
            .expect("legacy session fixture");
        let migrated_usage = ledger
            .fidelity_session_usage("session-private-id", super::FidelityAllowance::default())
            .expect("legacy-compatible fidelity usage");
        assert_eq!(migrated_usage.operations, 5);
        let current = ledger.efficiency_summary().expect("current-only claims");
        assert_eq!(current.operations, 4);
        assert_eq!(current.excluded_legacy_operations, 1);
        let current_stats = ledger
            .stats_collection(StatsQuery::default())
            .expect("current stats");
        assert_eq!(current_stats.snapshot.efficiency.operations, 4);
        assert_eq!(current_stats.snapshot.bypass.lifetime.total_operations, 4);
        assert_eq!(current_stats.snapshot.evasion.fidelity_operations, 4);
        assert_eq!(
            current_stats
                .snapshot
                .by_family
                .iter()
                .map(|family| family.operations)
                .sum::<u64>(),
            4
        );
        let compatibility_stats = ledger
            .stats_collection(StatsQuery {
                include_legacy_versions: true,
                ..StatsQuery::default()
            })
            .expect("legacy-inclusive stats");
        assert_eq!(compatibility_stats.snapshot.efficiency.operations, 5);
        assert_eq!(
            compatibility_stats
                .snapshot
                .bypass
                .lifetime
                .total_operations,
            5
        );
        assert_eq!(compatibility_stats.snapshot.evasion.fidelity_operations, 5);

        drop(ledger);
        let reopened = Ledger::open(&directory.path().join("ledger.sqlite")).expect("reopen");
        let reopened_usage = reopened
            .fidelity_session_usage("session-private-id", super::FidelityAllowance::default())
            .expect("stable keyed identity after restart");
        assert_eq!(reopened_usage.operations, 5);
    }

    #[test]
    fn identity_hmac_key_integrity_is_versioned_and_fail_closed() {
        assert!(super::PrivacyPseudonymizer::from_key("").is_err());
        assert!(super::PrivacyPseudonymizer::from_key("not-a-key").is_err());

        for (key, version) in [
            (String::new(), super::IDENTITY_HMAC_KEY256_V2),
            ("ab".repeat(32), "unknown_hmac_v9"),
            (
                "00000000-0000-4000-8000-000000000000".into(),
                super::IDENTITY_HMAC_KEY256_V2,
            ),
        ] {
            let directory = tempdir().expect("temporary directory");
            let path = directory.path().join("ledger.sqlite");
            drop(Ledger::open(&path).expect("initial ledger"));
            let connection = Connection::open(&path).expect("fixture connection");
            connection
                .execute(
                    "UPDATE ledger_privacy_meta SET value = ?1 WHERE key = 'identity_hmac_key'",
                    [key.as_str()],
                )
                .expect("corrupt key fixture");
            connection
                .execute(
                    "UPDATE ledger_privacy_meta SET value = ?1 WHERE key = 'identity_hmac_version'",
                    [version],
                )
                .expect("corrupt version fixture");
            drop(connection);
            assert!(
                matches!(
                    Ledger::open(&path),
                    Err(super::LedgerError::InvalidPrivacyIdentity(_))
                ),
                "key/version corruption must fail closed"
            );
        }

        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("legacy.sqlite");
        drop(Ledger::open(&path).expect("initial ledger"));
        let connection = Connection::open(&path).expect("fixture connection");
        let legacy_key = "00000000-0000-4000-8000-000000000000";
        connection
            .execute(
                "UPDATE ledger_privacy_meta SET value = ?1 WHERE key = 'identity_hmac_key'",
                [legacy_key],
            )
            .expect("legacy UUID key");
        connection
            .execute(
                "DELETE FROM ledger_privacy_meta WHERE key = 'identity_hmac_version'",
                [],
            )
            .expect("unversioned legacy fixture");
        drop(connection);
        let ledger = Ledger::open(&path).expect("legacy UUID key remains supported");
        let persisted: (String, String) = ledger
            .connection
            .query_row(
                "SELECT
                    (SELECT value FROM ledger_privacy_meta WHERE key = 'identity_hmac_key'),
                    (SELECT value FROM ledger_privacy_meta WHERE key = 'identity_hmac_version')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("persisted legacy key metadata");
        assert_eq!(persisted.0, legacy_key);
        assert_eq!(persisted.1, super::IDENTITY_HMAC_UUID_V1);
    }

    #[test]
    fn fidelity_reason_contradiction_is_typed_without_payload() {
        let directory = tempdir().expect("temporary directory");
        let ledger = Ledger::open(&directory.path().join("ledger.sqlite")).expect("ledger");
        let evasion = EvasionAttribution {
            class: EvasionClass::E7FidelityHatch,
            wrapper_depth: 0,
            interpreter: None,
            path_form: EvasionPathForm::Bare,
            stage_count: 1,
            hatch_marker: true,
            avoidable: true,
            tier: EnforcementTier::T4HatchQuarantine,
            fidelity_reason: Some(FidelityReason::Checksum),
            fidelity_validation: FidelityValidation::Contradicted,
        };
        ledger
            .record_operation_attributed_with_detail(
                "secret",
                "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=checksum hzr rtk -- raw proto-dumper secret",
                4,
                4,
                1,
                DetailedOperationAttribution {
                    attribution: OperationAttribution {
                        project_path: "",
                        agent: Some("codex"),
                        session_id: Some("s"),
                        channel: OperationChannel::HookCli,
                        measurement: OperationMeasurement::Estimated,
                        route: OperationRoute::Bypassed,
                    },
                    detail: None,
                    evasion: Some(&evasion),
                },
            )
            .expect("record");
        let validation: String = ledger
            .connection
            .query_row("SELECT fidelity_validation FROM commands", [], |row| {
                row.get(0)
            })
            .expect("validation");
        assert_eq!(validation, "contradicted");
    }

    #[test]
    fn acceptance_gate_policy_events_are_private_and_never_inflate_execution_accounting() {
        let directory = tempdir().expect("temporary directory");
        let ledger = Ledger::open(&directory.path().join("ledger.sqlite")).expect("ledger");
        let sentinel = "SENTINEL /private/path SELECT secret=value <<HEREDOC";
        let evasion = EvasionAttribution {
            class: EvasionClass::E7FidelityHatch,
            wrapper_depth: 1,
            interpreter: None,
            path_form: EvasionPathForm::AbsoluteSystem,
            stage_count: 2,
            hatch_marker: true,
            avoidable: true,
            tier: EnforcementTier::T4HatchQuarantine,
            fidelity_reason: None,
            fidelity_validation: FidelityValidation::MissingReason,
        };
        ledger
            .record_policy_event(super::PolicyEvent {
                project_path: sentinel,
                agent: Some(&format!("claude-code:{sentinel}")),
                session_id: Some(sentinel),
                evasion,
                decision: hzr_protocol::PolicyDecision::Ask,
                replacement_family: Some(sentinel),
            })
            .expect("policy event");

        let efficiency = ledger.efficiency_summary().expect("efficiency");
        assert_eq!(efficiency.operations, 0);
        assert_eq!(efficiency.total_observed_operations, 0);
        let evasion_summary = ledger
            .evasion_summary(StatsQuery::default())
            .expect("evasion summary");
        assert_eq!(evasion_summary.policy_attempts, 1);
        assert_eq!(evasion_summary.policy_by_class[0].attempts, 1);
        let session = ledger
            .session_evasion_summary(sentinel, super::FidelityAllowance::default())
            .expect("session score");
        assert_eq!(session.operations, 0);
        assert_eq!(session.policy_attempts, 1);
        assert_eq!(session.policy_asks, 1);
        let serialized = serde_json::to_string(&(evasion_summary, session)).expect("JSON");
        assert!(!serialized.contains(sentinel));
        let stored: (String, Option<String>, Option<String>, String, String) = ledger
            .connection
            .query_row(
                "SELECT project_hash, replacement_family, agent, producer_version,
                        accounting_policy_version FROM policy_events",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("stored event");
        assert!(stored.0.starts_with("sha256:"));
        assert_eq!(stored.1.as_deref(), Some("other"));
        assert_eq!(stored.2.as_deref(), Some("claude-code"));
        assert_eq!(stored.3, CURRENT_PRODUCER_VERSION);
        assert_eq!(stored.4, CURRENT_ACCOUNTING_POLICY_VERSION);
    }

    #[test]
    fn test_price_requires_actual_input_and_output() {
        let prices = PriceTable {
            input_microusd_per_million: 10_000_000,
            output_microusd_per_million: 20_000_000,
            cache_write_microusd_per_million: 0,
            cache_read_microusd_per_million: 0,
        };
        let usage = Usage {
            actual: ActualUsage {
                input_tokens: Some(1_000),
                output_tokens: Some(500),
                ..ActualUsage::default()
            },
            ..Usage::default()
        };

        assert_eq!(prices.cost_microusd(&usage), Some(20_000));
        assert_eq!(prices.cost_microusd(&Usage::default()), None);
    }

    #[test]
    fn provider_receipts_are_private_idempotent_and_session_scoped() {
        let directory = tempdir().expect("temp directory");
        let ledger = Ledger::open(&directory.path().join("ledger.sqlite")).expect("ledger");
        let catalog = crate::builtin_pricing_catalog().expect("catalog");
        let mut receipt = crate::ProviderEconomicReceipt {
            receipt_id: "external-receipt-private".into(),
            source: "provider-api".into(),
            observed_at_ms: 42,
            harness: "codex".into(),
            provider: "openai".into(),
            model: "gpt-5.6-sol".into(),
            method: "standard_short_context".into(),
            currency: "USD".into(),
            session_id: "session-private".into(),
            project_path: "/private/project".into(),
            baseline: crate::ProviderTokenUsage {
                input_tokens: 1_000_000,
                output_tokens: 100_000,
                ..crate::ProviderTokenUsage::default()
            },
            delivered: crate::ProviderTokenUsage {
                input_tokens: 500_000,
                output_tokens: 50_000,
                ..crate::ProviderTokenUsage::default()
            },
            actual_baseline_cost_microunits: Some(8_000_000),
            actual_delivered_cost_microunits: Some(4_000_000),
            enable_public_estimate: true,
        };

        let first = ledger
            .record_provider_receipt(&receipt, &catalog)
            .expect("first receipt");
        let replay = ledger
            .record_provider_receipt(&receipt, &catalog)
            .expect("idempotent replay");
        let summary = ledger
            .session_economic_summary("session-private")
            .expect("session summary");
        let other = ledger
            .session_economic_summary("other-session")
            .expect("other summary");

        assert!(first.recorded);
        assert!(!first.receipt_hash.contains("external-receipt-private"));
        assert!(replay.idempotent_replay);
        assert_eq!(summary.paired_receipts, 1);
        assert_eq!(
            summary
                .invoice_actual
                .expect("actual pair")
                .savings_microunits,
            4_000_000
        );
        assert_eq!(other.paired_receipts, 0);
        let stored: (String, String, String) = ledger
            .connection
            .query_row(
                "SELECT receipt_hash, session_hash, project_hash FROM provider_economic_receipts",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("stored receipt");
        assert!(stored.0.starts_with("hmac-sha256:"));
        assert!(stored.1.starts_with("hmac-sha256:"));
        assert!(stored.2.starts_with("sha256:"));

        receipt.delivered.input_tokens += 1;
        let conflict = ledger
            .record_provider_receipt(&receipt, &catalog)
            .expect_err("conflicting replay");
        assert!(conflict.to_string().contains("different content"));
    }
}
