use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use hzr_protocol::{TraceId, Usage};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
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
                    ON usage_records(created_at_ms DESC);",
            )
            .map_err(LedgerError::Database)?;
        Ok(Self { connection })
    }

    pub fn record(&self, record: &LedgerRecord) -> Result<(), LedgerError> {
        self.connection
            .execute(
                "INSERT OR REPLACE INTO usage_records (
                    trace_id, created_at_ms, provider, model,
                    actual_input, actual_output, actual_reasoning,
                    actual_cache_write, actual_cache_read,
                    estimated_input, estimated_output, estimate_method,
                    turns, retries, latency_ms, outcome, policy_version, cost_microusd
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                    ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18
                 )",
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
                    SUM(CASE WHEN outcome = 'accepted' THEN 1 ELSE 0 END),
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
}

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("failed to create ledger directory {path}: {source}")]
    Directory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("ledger database error: {0}")]
    Database(rusqlite::Error),
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use hzr_protocol::{ActualUsage, EstimatedUsage, TraceId, Usage};
    use tempfile::tempdir;

    use super::{Ledger, LedgerRecord, PriceTable};

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
            policy_version: "0.1.0".into(),
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
