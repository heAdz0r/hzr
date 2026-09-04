//! Producer output and adapter delivery are separate, non-additive dimensions.
use rusqlite::params;
use serde::{Deserialize, Serialize};
use super::{Ledger, LedgerError, StatsQuery, accounting_policy_predicate, privacy_identity_hash};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeliverySummary {
    /// Explicit final/standalone adapter payload records; not provider receipts.
    pub operations: u64,
    /// None means no measured delivery evidence, never an inferred zero.
    pub tokens_estimated: Option<u64>,
    pub legacy_unknown_stage_operations: u64,
    pub coverage: String,
    pub complete: bool,
}

impl Default for DeliverySummary {
    fn default() -> Self {
        Self {
            operations: 0,
            tokens_estimated: None,
            legacy_unknown_stage_operations: 0,
            coverage: "unknown_host_ack_and_unlinked_producers".into(),
            complete: false,
        }
    }
}

impl Ledger {
    pub(super) fn delivery_summary(
        &self, query: StatsQuery<'_>, session: Option<(&str, &str)>,
    ) -> Result<DeliverySummary, LedgerError> {
        let project = query.project_path.map(|value| privacy_identity_hash("project", value));
        self.delivery_summary_for_scope(project.as_deref(), query.since_unix_seconds, query.include_legacy_versions, session)
    }

    pub(super) fn delivery_summary_for_scope(
        &self, project_hash: Option<&str>, since: Option<i64>, include_legacy: bool,
        session: Option<(&str, &str)>,
    ) -> Result<DeliverySummary, LedgerError> {
        let policy = accounting_policy_predicate(include_legacy);
        let sql = format!(
            "SELECT
                COALESCE(SUM(CASE WHEN accounting_stage IN ('final_delivery', 'standalone_delivery')
                    AND measurement = 'estimated' THEN 1 ELSE 0 END), 0),
                SUM(CASE WHEN accounting_stage IN ('final_delivery', 'standalone_delivery')
                    AND measurement = 'estimated' THEN output_tokens END),
                COALESCE(SUM(CASE WHEN accounting_stage IS NULL OR accounting_stage NOT IN
                    ('internal_transport', 'final_delivery', 'standalone_delivery', 'control_plane')
                    THEN 1 ELSE 0 END), 0)
             FROM commands WHERE ({policy})
                AND (?1 IS NULL OR instr('|' || project_scope_hashes || '|', '|' || ?1 || '|') > 0)
                AND (?2 IS NULL OR CAST(strftime('%s', timestamp) AS INTEGER) >= ?2)
                AND (?3 IS NULL OR session_hash IN (?3, ?4))"
        );
        self.connection.query_row(&sql, params![project_hash, since, session.map(|v| v.0), session.map(|v| v.1)], |row| {
            Ok(DeliverySummary {
                operations: row.get(0)?,
                tokens_estimated: row.get(1)?,
                legacy_unknown_stage_operations: row.get(2)?,
                ..DeliverySummary::default()
            })
        }).map_err(LedgerError::Database)
    }
}
