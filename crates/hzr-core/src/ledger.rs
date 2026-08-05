use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use directories::BaseDirs;
use hzr_protocol::{TraceId, Usage};

use crate::operation::{
    OperationChannel, OperationMeasurement, OperationRoute, classify_operation,
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
    pub by_command: Vec<EfficiencyCommandSummary>,
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
                    cost_microusd INTEGER
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
                    route TEXT
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
                    turns, retries, latency_ms, outcome, policy_version, cost_microusd
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                    ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18
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
                        outcome, policy_version, cost_microusd
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
                    })
                },
            )
            .optional()
            .map_err(LedgerError::Database)
    }

    pub fn summary(&self) -> Result<LedgerSummary, LedgerError> {
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
                 FROM usage_records",
                [],
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
        self.efficiency_summary_scoped(None)
    }

    pub fn efficiency_summary_for_project(
        &self,
        project_path: &str,
    ) -> Result<EfficiencySummary, LedgerError> {
        self.efficiency_summary_scoped(Some(project_path))
    }

    fn efficiency_summary_scoped(
        &self,
        project_path: Option<&str>,
    ) -> Result<EfficiencySummary, LedgerError> {
        let raw_predicate = raw_route_sql_predicate("rtk_cmd");
        let totals_query = format!(
            "SELECT
                COUNT(*),
                COALESCE(SUM(CASE WHEN ({raw_predicate})
                                  THEN output_tokens ELSE input_tokens END), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(CASE WHEN NOT ({raw_predicate}) AND input_tokens > output_tokens
                                  THEN input_tokens - output_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN NOT ({raw_predicate}) AND output_tokens > input_tokens
                                  THEN output_tokens - input_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN ({raw_predicate})
                                  THEN 0 ELSE input_tokens - output_tokens END), 0),
                COALESCE(SUM(exec_time_ms), 0)
             FROM commands
             WHERE measurement = 'estimated'
               AND COALESCE(route, '') != 'native_unaccounted'
               AND (?1 IS NULL OR project_path = ?1
                    OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2)"
        );
        let mut summary = self
            .connection
            .query_row(
                &totals_query,
                params![project_path, std::path::MAIN_SEPARATOR.to_string()],
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
                        by_command: Vec::new(),
                    })
                },
            )
            .map_err(LedgerError::Database)?;
        let by_command_query = format!(
            "SELECT
                rtk_cmd,
                COUNT(*),
                COALESCE(SUM(CASE WHEN ({raw_predicate})
                                  THEN output_tokens ELSE input_tokens END), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(CASE WHEN NOT ({raw_predicate}) AND input_tokens > output_tokens
                                  THEN input_tokens - output_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN NOT ({raw_predicate}) AND output_tokens > input_tokens
                                  THEN output_tokens - input_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN ({raw_predicate})
                                  THEN 0 ELSE input_tokens - output_tokens END), 0),
                COALESCE(AVG(exec_time_ms), 0)
             FROM commands
             WHERE measurement = 'estimated'
               AND COALESCE(route, '') != 'native_unaccounted'
               AND (?1 IS NULL OR project_path = ?1
                    OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2)
             GROUP BY rtk_cmd
             ORDER BY SUM(CASE WHEN ({raw_predicate})
                               THEN 0 ELSE input_tokens - output_tokens END) DESC"
        );
        let mut statement = self
            .connection
            .prepare_cached(&by_command_query)
            .map_err(LedgerError::Database)?;
        summary.by_command = statement
            .query_map(
                params![project_path, std::path::MAIN_SEPARATOR.to_string()],
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
                         OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2)",
                params![project_path, scope_separator],
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
                  GROUP BY channel",
            )
            .map_err(LedgerError::Database)?;
        summary.by_channel = channels
            .query_map(
                params![project_path, std::path::MAIN_SEPARATOR.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
            )
            .map_err(LedgerError::Database)?
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(LedgerError::Database)?;
        Ok(summary)
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
        if attribution.measurement == OperationMeasurement::Unmeasured
            && (input_tokens != 0 || output_tokens != 0)
        {
            return Err(LedgerError::InvalidOperation(
                "unmeasured operations cannot carry invented token counts".into(),
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
                    channel, measurement, route
                 ) VALUES (
                    datetime('now'), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13
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
        self.bypass_summary_scoped(None)
    }

    pub fn bypass_summary_for_project(
        &self,
        project_path: &str,
    ) -> Result<BypassSummary, LedgerError> {
        self.bypass_summary_scoped(Some(project_path))
    }

    fn bypass_summary_scoped(
        &self,
        project_path: Option<&str>,
    ) -> Result<BypassSummary, LedgerError> {
        let (total_operations, total_delivered) = self
            .connection
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(output_tokens), 0)
                 FROM commands
                 WHERE (?1 IS NULL OR project_path = ?1
                        OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2)",
                params![project_path, std::path::MAIN_SEPARATOR.to_string()],
                |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
            )
            .map_err(LedgerError::Database)?;
        let query = format!(
            "SELECT rtk_cmd, COUNT(*), COALESCE(SUM(output_tokens), 0)
             FROM commands
             WHERE ({})
               AND (?1 IS NULL OR project_path = ?1
                    OR substr(project_path, 1, length(?1) + 1) = ?1 || ?2)
             GROUP BY rtk_cmd",
            raw_route_sql_predicate("rtk_cmd")
        );
        let mut statement = self
            .connection
            .prepare(&query)
            .map_err(LedgerError::Database)?;
        let groups = statement
            .query_map(
                params![project_path, std::path::MAIN_SEPARATOR.to_string()],
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
    use hzr_protocol::{ActualUsage, EstimatedUsage, TraceId, Usage};
    use rusqlite::Connection;
    use tempfile::tempdir;

    use super::{
        Ledger, LedgerRecord, OperationAttribution, PriceTable, ProjectOperationRoute,
        operation_identity,
    };

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
            policy_version: "0.3.7".into(),
            cost_microusd: Some(50),
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
