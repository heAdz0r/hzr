use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use directories::BaseDirs;
use hzr_protocol::{
    AccountingAttribution, AccountingOperationKind, AccountingOperationMode, AccountingStage,
    TraceId, Usage,
};

use crate::operation::{
    OperationChannel, OperationMeasurement, OperationRoute, classify_operation,
    efficient_route_replacement, first_class_replacement, managed_raw_payload,
    raw_route_sql_predicate,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

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
    pub first_class_replacement_available: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StatsQuery<'a> {
    pub project_path: Option<&'a str>,
    /// Inclusive Unix-second cutoff shared by every section of one stats snapshot.
    pub since_unix_seconds: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StatsSnapshot {
    pub efficiency: EfficiencySummary,
    pub bypass: BypassSummary,
    pub provider_usage: LedgerSummary,
    pub by_family: Vec<OperationFamilySummary>,
}

/// A stats snapshot plus transient capability probes that must never be serialized.
#[derive(Clone, Debug)]
pub struct StatsCollection {
    pub snapshot: StatsSnapshot,
    capability_inputs: Vec<OperationCapabilityInput>,
}

#[derive(Clone, Debug)]
struct OperationCapabilityInput {
    command: String,
    targets: Vec<(String, OperationRoute)>,
}

impl StatsCollection {
    pub fn capability_commands(&self) -> impl ExactSizeIterator<Item = &str> {
        self.capability_inputs
            .iter()
            .map(|input| input.command.as_str())
    }

    /// Merge a cardinality-preserving fork-core response into the privacy-safe summary.
    /// A malformed response is ignored so capability reporting fails closed.
    pub fn apply_capabilities(&mut self, supported: &[bool]) -> bool {
        if supported.len() != self.capability_inputs.len() {
            return false;
        }
        for (input, supported) in self.capability_inputs.iter().zip(supported) {
            if !supported {
                continue;
            }
            for (family, route) in &input.targets {
                if let Some(summary) = self
                    .snapshot
                    .by_family
                    .iter_mut()
                    .find(|summary| summary.family == *family && summary.route == *route)
                {
                    summary.first_class_replacement_available = true;
                }
            }
        }
        true
    }
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
        _ => None,
    }
}

fn parse_accounting_stage(value: &str) -> Option<AccountingStage> {
    match value {
        "internal_transport" => Some(AccountingStage::InternalTransport),
        "final_delivery" => Some(AccountingStage::FinalDelivery),
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
    pub input_per_million_usd: f64,
    pub output_per_million_usd: f64,
    pub cache_write_per_million_usd: f64,
    pub cache_read_per_million_usd: f64,
}

impl PriceTable {
    pub fn cost_microusd(self, usage: &Usage) -> Option<u64> {
        let input = usage.actual.input_tokens?;
        let output = usage.actual.output_tokens?;
        let cache_write = usage.actual.cache_write_tokens.unwrap_or_default();
        let cache_read = usage.actual.cache_read_tokens.unwrap_or_default();
        let usd = input as f64 * self.input_per_million_usd / 1_000_000.0
            + output as f64 * self.output_per_million_usd / 1_000_000.0
            + cache_write as f64 * self.cache_write_per_million_usd / 1_000_000.0
            + cache_read as f64 * self.cache_read_per_million_usd / 1_000_000.0;
        Some((usd * 1_000_000.0).round() as u64)
    }
}

impl Ledger {
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
        }
        let connection = Connection::open(path).map_err(LedgerError::Database)?;
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
                    source_bytes INTEGER
                 );
                 CREATE INDEX IF NOT EXISTS idx_timestamp ON commands(timestamp);
                 CREATE INDEX IF NOT EXISTS idx_project_path_timestamp
                    ON commands(project_path, timestamp);
                 CREATE TABLE IF NOT EXISTS parse_failures (
                    id INTEGER PRIMARY KEY,
                    timestamp TEXT NOT NULL,
                    raw_command TEXT NOT NULL,
                    error_message TEXT NOT NULL,
                    fallback_succeeded INTEGER NOT NULL DEFAULT 0
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
        ] {
            let _ = connection.execute(&format!("ALTER TABLE commands ADD COLUMN {column}"), []);
        }
        // Идемпотентно: существующие БД получают колонку; повторный ALTER безопасно игнорируется.
        let _ = connection.execute(
            "ALTER TABLE usage_records ADD COLUMN project_path TEXT NOT NULL DEFAULT ''",
            [],
        );
        migrate_legacy_ledgers(&connection, path)?;
        Ok(Self { connection })
    }

    pub fn record(&self, record: &LedgerRecord) -> Result<(), LedgerError> {
        self.connection
            .execute(
                "INSERT INTO usage_records (
                    trace_id, created_at_ms, provider, model,
                    actual_input, actual_output, actual_reasoning,
                    actual_cache_write, actual_cache_read,
                    estimated_input, estimated_output, estimate_method,
                    turns, retries, latency_ms, outcome, policy_version, cost_microusd,
                    project_path
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                    ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19
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
                    record.project_path.as_str(),
                ],
            )
            .map_err(LedgerError::Database)?;
        Ok(())
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

    /// Collect one snapshot and the distinct raw commands requiring canonical fork capability
    /// classification. The command inputs stay in this non-serializable transient type.
    pub fn stats_collection(&self, query: StatsQuery<'_>) -> Result<StatsCollection, LedgerError> {
        let (by_family, capability_inputs) =
            self.operation_family_summary(query.project_path, query.since_unix_seconds)?;
        Ok(StatsCollection {
            snapshot: StatsSnapshot {
                efficiency: self
                    .efficiency_summary_scoped(query.project_path, query.since_unix_seconds)?,
                bypass: self.bypass_summary_scoped(query.project_path, query.since_unix_seconds)?,
                provider_usage: self
                    .summary_scoped(query.project_path, query.since_unix_seconds)?,
                by_family,
            },
            capability_inputs,
        })
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
                    project_path != ''
                    AND (project_path = ?1
                         OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2)
                 ))
                   AND (?3 IS NULL OR created_at_ms >= ?3 * 1000)",
                params![
                    project_path,
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
        self.efficiency_summary_scoped(None, None)
    }

    pub fn efficiency_summary_for_project(
        &self,
        project_path: &str,
    ) -> Result<EfficiencySummary, LedgerError> {
        self.efficiency_summary_scoped(Some(project_path), None)
    }

    fn efficiency_summary_scoped(
        &self,
        project_path: Option<&str>,
        since_unix_seconds: Option<i64>,
    ) -> Result<EfficiencySummary, LedgerError> {
        let raw_predicate = raw_route_sql_predicate("rtk_cmd");
        let neutral_predicate = format!("({raw_predicate}) OR rtk_cmd = 'rtk write'");
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
               AND COALESCE(accounting_stage, 'internal_transport') != 'final_delivery'
               AND (?1 IS NULL OR project_path = ?1
                    OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2)
               AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)"
        );
        let mut summary = self
            .connection
            .query_row(
                &totals_query,
                params![
                    project_path,
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
               AND COALESCE(accounting_stage, 'internal_transport') != 'final_delivery'
               AND (?1 IS NULL OR project_path = ?1
                    OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2)
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
                    project_path,
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
        let (total, native, unmeasured) = self
            .connection
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(route = 'native_unaccounted'), 0),
                        COALESCE(SUM(measurement = 'unmeasured' AND route = 'bypassed'), 0)
                   FROM commands
                  WHERE (?1 IS NULL OR project_path = ?1
                         OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2)
                    AND COALESCE(accounting_stage, 'internal_transport') != 'final_delivery'
                    AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)",
                params![project_path, scope_separator, since_unix_seconds],
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
        let mut channels = self
            .connection
            .prepare_cached(
                "SELECT channel, COUNT(*) FROM commands
                  WHERE (?1 IS NULL OR project_path = ?1
                         OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2)
                    AND COALESCE(accounting_stage, 'internal_transport') != 'final_delivery'
                    AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)
                  GROUP BY channel",
            )
            .map_err(LedgerError::Database)?;
        summary.by_channel = channels
            .query_map(
                params![
                    project_path,
                    std::path::MAIN_SEPARATOR.to_string(),
                    since_unix_seconds
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
            )
            .map_err(LedgerError::Database)?
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(LedgerError::Database)?;
        summary.by_mode = self.operation_modes_summary(project_path, since_unix_seconds)?;
        Ok(summary)
    }

    fn operation_modes_summary(
        &self,
        project_path: Option<&str>,
        since_unix_seconds: Option<i64>,
    ) -> Result<Vec<OperationModeSummary>, LedgerError> {
        let mut statement = self
            .connection
            .prepare_cached(
                "SELECT operation_kind, operation_mode, accounting_stage, COUNT(*),
                        COALESCE(SUM(output_tokens), 0)
                   FROM commands
                  WHERE operation_kind IS NOT NULL
                    AND operation_mode IS NOT NULL
                    AND accounting_stage IS NOT NULL
                    AND (?1 IS NULL OR project_path = ?1
                         OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2)
                    AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)
                  GROUP BY operation_kind, operation_mode, accounting_stage
                  ORDER BY operation_kind, operation_mode, accounting_stage",
            )
            .map_err(LedgerError::Database)?;
        let rows = statement
            .query_map(
                params![
                    project_path,
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
        self.connection
            .execute(
                "INSERT INTO commands (
                    timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens,
                    saved_tokens, savings_pct, exec_time_ms, project_path, agent, session_id,
                    channel, measurement, route, operation_kind, operation_mode,
                    accounting_stage, requested_mode, effective_mode, search_strategy,
                    search_fallback_code, search_include_content, result_limit, path_scope_count,
                    filter_level, range_from, range_to, source_bytes
                 ) VALUES (
                    datetime('now'), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                    ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27
                 )",
                params![
                    original_command,
                    recorded_command,
                    input_tokens,
                    output_tokens,
                    saved,
                    savings_pct,
                    execution_ms,
                    attribution.project_path,
                    attribution.agent,
                    attribution.session_id,
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
        self.bypass_summary_scoped(None, None)
    }

    pub fn bypass_summary_for_project(
        &self,
        project_path: &str,
    ) -> Result<BypassSummary, LedgerError> {
        self.bypass_summary_scoped(Some(project_path), None)
    }

    fn bypass_summary_scoped(
        &self,
        project_path: Option<&str>,
        since_unix_seconds: Option<i64>,
    ) -> Result<BypassSummary, LedgerError> {
        let (total_operations, total_delivered) = self
            .connection
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(output_tokens), 0)
                 FROM commands
                 WHERE (?1 IS NULL OR project_path = ?1
                        OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2)
                   AND COALESCE(accounting_stage, 'internal_transport') != 'final_delivery'
                   AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)",
                params![
                    project_path,
                    std::path::MAIN_SEPARATOR.to_string(),
                    since_unix_seconds
                ],
                |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
            )
            .map_err(LedgerError::Database)?;
        let query = format!(
            "SELECT rtk_cmd, COUNT(*), COALESCE(SUM(output_tokens), 0)
             FROM commands
             WHERE ({})
               AND COALESCE(accounting_stage, 'internal_transport') != 'final_delivery'
               AND (?1 IS NULL OR project_path = ?1
                    OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2)
               AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)
             GROUP BY rtk_cmd",
            raw_route_sql_predicate("rtk_cmd")
        );
        let mut statement = self
            .connection
            .prepare(&query)
            .map_err(LedgerError::Database)?;
        let groups = statement
            .query_map(
                params![
                    project_path,
                    std::path::MAIN_SEPARATOR.to_string(),
                    since_unix_seconds
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, u64>(2)?,
                    ))
                },
            )
            .map_err(LedgerError::Database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LedgerError::Database)?;

        let mut by_tool: BTreeMap<String, BypassTool> = BTreeMap::new();
        let mut heaviest: BTreeMap<String, u64> = BTreeMap::new();
        let mut operations = 0;
        let mut delivered = 0;
        for (command, executions, delivered_tokens) in groups {
            let classification = classify_operation(&command);
            operations += executions;
            delivered += delivered_tokens;
            let entry = by_tool
                .entry(classification.operation.clone())
                .or_insert_with(|| BypassTool {
                    tool: classification.operation.clone(),
                    executions: 0,
                    delivered_tokens_estimated: 0,
                    example_command: command.clone(),
                    replacement: None,
                    rationale: None,
                });
            entry.executions += executions;
            entry.delivered_tokens_estimated += delivered_tokens;
            // The costliest concrete invocation becomes the worked example, so the
            // suggestion an operator reads is the one that would have saved the most.
            let previous = heaviest
                .entry(classification.operation.clone())
                .or_default();
            if delivered_tokens >= *previous {
                *previous = delivered_tokens;
                entry.example_command = command.clone();
                entry.replacement = classification
                    .replacement
                    .as_ref()
                    .map(|replacement| replacement.suggestion.clone());
                entry.rationale = classification
                    .replacement
                    .as_ref()
                    .map(|replacement| replacement.rationale.to_owned());
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
    ) -> Result<(Vec<OperationFamilySummary>, Vec<OperationCapabilityInput>), LedgerError> {
        let mut statement = self
            .connection
            .prepare_cached(
                "SELECT rtk_cmd, route, operation_kind, COUNT(*),
                        COALESCE(SUM(output_tokens), 0)
                   FROM commands
                  WHERE (?1 IS NULL OR project_path = ?1
                         OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2)
                    AND (?3 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?3)
                    AND COALESCE(accounting_stage, 'internal_transport') != 'final_delivery'
                  GROUP BY rtk_cmd, route, operation_kind",
            )
            .map_err(LedgerError::Database)?;
        let rows = statement
            .query_map(
                params![
                    project_path,
                    std::path::MAIN_SEPARATOR.to_string(),
                    since_unix_seconds
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, u64>(3)?,
                        row.get::<_, u64>(4)?,
                    ))
                },
            )
            .map_err(LedgerError::Database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LedgerError::Database)?;

        let mut families = BTreeMap::<(String, String), OperationFamilySummary>::new();
        let mut capability_targets = BTreeMap::<String, Vec<(String, OperationRoute)>>::new();
        for (command, stored_route, stored_operation, operations, delivered) in rows {
            let classification = classify_operation(&command);
            let route = route_from_ledger(stored_route.as_deref(), classification.route);
            let family = stored_operation
                .as_deref()
                .and_then(parse_operation_kind)
                .map(|operation| operation.as_str().to_owned())
                .unwrap_or(classification.operation);
            let replacement_available = route == OperationRoute::Optimized
                || first_class_replacement(&command).is_some()
                || efficient_route_replacement(&command).is_some();
            if route == OperationRoute::Bypassed && !replacement_available {
                let candidate = managed_raw_payload(&command).unwrap_or(&command).to_owned();
                let target = (family.clone(), route);
                let targets = capability_targets.entry(candidate).or_default();
                if !targets.contains(&target) {
                    targets.push(target);
                }
            }
            let key = (family.clone(), route.as_str().to_owned());
            let summary = families
                .entry(key)
                .or_insert_with(|| OperationFamilySummary {
                    family,
                    route,
                    operations: 0,
                    delivered_tokens_estimated: 0,
                    first_class_replacement_available: false,
                });
            summary.operations = summary.operations.saturating_add(operations);
            summary.delivered_tokens_estimated =
                summary.delivered_tokens_estimated.saturating_add(delivered);
            summary.first_class_replacement_available |= replacement_available;
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
        let capability_inputs = capability_targets
            .into_iter()
            .map(|(command, targets)| OperationCapabilityInput { command, targets })
            .collect();
        Ok((summaries, capability_inputs))
    }

    pub fn project_activity(
        &self,
        project_path: &str,
    ) -> Result<ProjectActivitySummary, LedgerError> {
        let raw_predicate = raw_route_sql_predicate("rtk_cmd");
        let measured_predicate =
            "measurement = 'estimated' AND COALESCE(route, '') != 'native_unaccounted'";
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
             WHERE project_path = ?1
                OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2"
        );
        let mut summary = self
            .connection
            .query_row(
                &activity_query,
                params![project_path, std::path::MAIN_SEPARATOR.to_string()],
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
                        recent_operations: Vec::new(),
                    })
                },
            )
            .map_err(LedgerError::Database)?;
        summary.unscoped_operations = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM commands WHERE project_path = ''",
                [],
                |row| row.get(0),
            )
            .map_err(LedgerError::Database)?;
        summary.raw_operations = self
            .connection
            .query_row(
                &format!(
                    "SELECT COUNT(*)
                     FROM commands
                     WHERE (project_path = ?1
                            OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2)
                       AND measurement = 'estimated'
                       AND COALESCE(route, '') != 'native_unaccounted'
                       AND ({})",
                    raw_route_sql_predicate("rtk_cmd")
                ),
                params![project_path, std::path::MAIN_SEPARATOR.to_string()],
                |row| row.get(0),
            )
            .map_err(LedgerError::Database)?;
        summary.optimized_operations = self
            .connection
            .query_row(
                &format!(
                    "SELECT COUNT(*)
                     FROM commands
                     WHERE (project_path = ?1
                            OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2)
                       AND measurement = 'estimated'
                       AND COALESCE(route, '') != 'native_unaccounted'
                       AND NOT ({})",
                    raw_route_sql_predicate("rtk_cmd")
                ),
                params![project_path, std::path::MAIN_SEPARATOR.to_string()],
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
                 WHERE project_path = ?1
                    OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2",
                params![project_path, std::path::MAIN_SEPARATOR.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(LedgerError::Database)?;
        let mut statement = self
            .connection
            .prepare_cached(
                "SELECT id, timestamp, original_cmd, rtk_cmd, project_path, agent, session_id,
                        input_tokens, output_tokens, input_tokens - output_tokens, exec_time_ms,
                        COALESCE(route, '')
                 FROM commands
                 WHERE project_path = ?1
                    OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2
                 ORDER BY id DESC
                 LIMIT 24",
            )
            .map_err(LedgerError::Database)?;
        summary.recent_operations = statement
            .query_map(
                params![project_path, std::path::MAIN_SEPARATOR.to_string()],
                |row| {
                    let command: String = row.get(3)?;
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
                        original_command: row.get(2)?,
                        recorded_command: command,
                        working_directory: row.get(4)?,
                        agent: row.get(5)?,
                        session_id: row.get(6)?,
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
        AccountingSearchStrategy, AccountingStage, ActualUsage, EstimatedUsage, SearchFallbackCode,
        TraceId, Usage,
    };
    use rusqlite::{Connection, params};
    use tempfile::tempdir;

    use super::{
        DetailedOperationAttribution, Ledger, LedgerRecord, OperationAttribution, PriceTable,
        ProjectOperationRoute, StatsQuery, operation_identity,
    };

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
                    measurement, route, operation_kind
                 ) VALUES (datetime(?1, 'unixepoch'), '', ?2, ?3, ?3, 0, 0, 0, '',
                           'hook_cli', 'estimated', ?4, ?5)",
                params![timestamp, command, delivered, route, operation_kind],
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
        assert!(families[1].first_class_replacement_available);
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

        let mut collection = ledger
            .stats_collection(StatsQuery {
                project_path: None,
                since_unix_seconds: Some(cutoff),
            })
            .expect("seven-day snapshot");
        let commands = collection
            .capability_commands()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(
            commands.len(),
            6,
            "only distinct in-window commands are probed"
        );
        assert!(!commands.iter().any(|command| command.contains("terraform")));
        let supported = commands
            .iter()
            .map(|command| {
                ["bun", "git", "ssh", "gh", "cargo"]
                    .iter()
                    .any(|family| command.starts_with(family))
            })
            .collect::<Vec<_>>();
        assert!(collection.apply_capabilities(&supported));
        assert!(!collection.apply_capabilities(&supported[..supported.len() - 1]));
        let families = collection.snapshot.by_family;

        for family in ["bun", "git", "ssh", "gh", "cargo"] {
            let summary = families
                .iter()
                .find(|summary| summary.family == family)
                .expect("dedicated legacy family");
            assert!(summary.first_class_replacement_available, "{family}");
        }
        assert!(families.iter().any(|summary| {
            summary.family == "unknown-tool" && !summary.first_class_replacement_available
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
        let summary = ledger.efficiency_summary().expect("efficiency summary");
        assert_eq!(summary.by_mode.len(), 1);
        assert_eq!(
            summary.by_mode[0].mode,
            AccountingOperationMode::SearchExact
        );
        assert_eq!(summary.by_mode[0].delivered_tokens_estimated, 8);
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
                    },
                )
                .expect("record stage");
        }

        let summary = ledger.efficiency_summary().expect("efficiency summary");
        assert_eq!(summary.operations, 1);
        assert_eq!(summary.baseline_tokens_estimated, 100);
        assert_eq!(summary.delivered_tokens_estimated, 20);
        assert_eq!(summary.total_observed_operations, 1);
        assert_eq!(summary.by_mode.len(), 2);
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
            summary.operations, 1,
            "native and unmeasured rows leave the ratio"
        );
        assert_eq!(summary.native_unaccounted_operations, 1);
        assert_eq!(summary.unmeasured_bypass_operations, 1);
        assert_eq!(summary.accounted_operations, 2);
        assert_eq!(summary.total_observed_operations, 3);
        assert_eq!(summary.by_channel.get("hook_cli"), Some(&2));
        assert_eq!(summary.by_channel.get("native_host"), Some(&1));
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
    fn test_proxy_ledger_rows_are_classified_as_raw() {
        assert_eq!(
            operation_identity("rtk proxy sed -n 1,20p file"),
            (
                "sed".into(),
                ProjectOperationRoute::Raw,
                Some("hzr rtk -- read file --from 1 --to 20".into()),
                Some(
                    "hzr read streams the requested span with filtering instead of the whole slice"
                        .into()
                ),
            )
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
        let ledger = Ledger::open(&directory.path().join("usage.sqlite")).expect("ledger open");
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
        let ledger = Ledger::open(&directory.path().join("usage.sqlite")).expect("ledger open");
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

        let gain = ledger
            .efficiency_summary_for_project("/work/a")
            .expect("project efficiency");
        let bypass = ledger
            .bypass_summary_for_project("/work/a")
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
        let summary = ledger.efficiency_summary().expect("efficiency summary");

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
            policy_version: "0.4.3".into(),
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
            policy_version: "0.4.3".into(),
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
        assert_eq!(loaded.project_path, "/work/a");
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
                policy_version: "0.4.3".into(),
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
    fn test_price_requires_actual_input_and_output() {
        let prices = PriceTable {
            input_per_million_usd: 10.0,
            output_per_million_usd: 20.0,
            cache_write_per_million_usd: 0.0,
            cache_read_per_million_usd: 0.0,
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
}
