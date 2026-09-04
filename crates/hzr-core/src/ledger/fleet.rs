//! Fixed-window, privacy-safe fleet snapshots. No workspace filesystem is required.
use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};

use super::{
    Ledger, LedgerError, accounting_policy_predicate, privacy_safe_agent, privacy_safe_family,
    savings_neutral_sql_predicate,
};

#[derive(Clone, Copy, Debug)]
pub struct FleetStatsQuery<'a> {
    /// Inclusive Unix seconds.
    pub since_unix_seconds: i64,
    /// Exclusive Unix seconds; resolve once before collection.
    pub until_unix_seconds: i64,
    pub project_id: Option<&'a str>,
    pub include_legacy_versions: bool,
}

impl FleetStatsQuery<'_> {
    fn validate(&self) -> Result<(), LedgerError> {
        if self.since_unix_seconds < 0
            || self.until_unix_seconds <= self.since_unix_seconds
            || self.until_unix_seconds > i64::MAX / 1000
        {
            return Err(LedgerError::InvalidOperation(
                "fleet window must satisfy 0 <= since < until".into(),
            ));
        }
        if self
            .project_id
            .is_some_and(|id| !valid_project_id(id) && id != "unscoped")
        {
            return Err(LedgerError::InvalidOperation(
                "project ID must be sha256:<64 lowercase hexadecimal digits> or unscoped".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct FleetMetrics {
    pub recorded_operations: u64,
    pub explicit_delivery_operations: u64,
    pub explicit_delivery_tokens_estimated: u64,
    pub legacy_unknown_stage_operations: u64,
    pub measured_operations: u64,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub stage_excluded_operations: u64,
    pub native_unaccounted_operations: u64,
    pub unmeasured_operations: u64,
    pub excluded_legacy_operations: u64,
    /// Association only: a repeated command is not proof that filtering caused it.
    pub repeated_after_filter_operations: u64,
    pub repeated_after_filter_tokens_estimated: u64,
    pub execution_ms: u64,
}

impl FleetMetrics {
    fn add(&mut self, other: &Self) -> Result<(), LedgerError> {
        macro_rules! add {
            ($($field:ident),+ $(,)?) => { $(
                self.$field = self.$field.checked_add(other.$field).ok_or_else(|| LedgerError::InvalidOperation("fleet aggregate overflow".into()))?;
            )+ };
        }
        add!(
            recorded_operations,
            explicit_delivery_operations,
            explicit_delivery_tokens_estimated,
            legacy_unknown_stage_operations,
            measured_operations,
            baseline_tokens_estimated,
            delivered_tokens_estimated,
            net_avoided_tokens_estimated,
            stage_excluded_operations,
            native_unaccounted_operations,
            unmeasured_operations,
            excluded_legacy_operations,
            repeated_after_filter_operations,
            repeated_after_filter_tokens_estimated,
            execution_ms
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FleetDimension {
    pub key: String,
    pub metrics: FleetMetrics,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FleetProject {
    pub project_id: String,
    /// Registry state is sampled separately from the atomic ledger snapshot.
    pub registered: bool,
    pub workspace_exists: Option<bool>,
    pub metrics: FleetMetrics,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FleetOperationGroup {
    pub project_id: String,
    /// Agent ecosystem, not a physical machine identifier.
    pub host: String,
    pub family: String,
    pub route: String,
    pub metrics: FleetMetrics,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FleetStatsSnapshot {
    pub schema_version: u32,
    pub since_unix_seconds: i64,
    pub until_unix_seconds: i64,
    pub window_semantics: String,
    pub accounting_version_scope: String,
    pub selected_project_id: Option<String>,
    pub ledger_present: bool,
    pub consistency: String,
    pub project_attribution: String,
    pub host_attribution: String,
    pub host_coverage: String,
    pub measurement: String,
    pub producer_scope: String,
    pub delivery_coverage: String,
    pub economic_claim_ready: bool,
    pub economic_claim_blockers: Vec<String>,
    pub provider_receipts: u64,
    pub externally_verified_provider_receipts: u64,
    pub provider_tasks: Option<u64>,
    pub accepted_provider_tasks: Option<u64>,
    pub registry_warnings: u64,
    pub totals: FleetMetrics,
    pub by_project: Vec<FleetProject>,
    pub by_host: Vec<FleetDimension>,
    pub by_family: Vec<FleetDimension>,
    pub groups: Vec<FleetOperationGroup>,
}

impl FleetStatsSnapshot {
    fn empty(query: FleetStatsQuery<'_>) -> Self {
        Self {
            schema_version: 1,
            since_unix_seconds: query.since_unix_seconds,
            until_unix_seconds: query.until_unix_seconds,
            window_semantics: "[since, until)".into(),
            accounting_version_scope: if query.include_legacy_versions {
                "all"
            } else {
                "current_compatible"
            }
            .into(),
            selected_project_id: query.project_id.map(str::to_owned),
            ledger_present: false,
            consistency: "single_sqlite_read_transaction".into(),
            project_attribution: "exact_recorded_project_id; no ancestor double-counting".into(),
            host_attribution: "recorded_agent_ecosystem; physical_host_unknown".into(),
            host_coverage: "unknown; observed ledger rows are not all host operations".into(),
            measurement: "estimated_utf8_bytes_div_4_v1; not provider billing".into(),
            producer_scope: "internal_transport only; never add to explicit delivery".into(),
            delivery_coverage:
                "unknown_host_ack_and_unlinked_producers; zero delivery records mean unknown total"
                    .into(),
            economic_claim_ready: false,
            economic_claim_blockers: vec![
                "complete host delivery and input coverage are unproven".into(),
                "matched task-quality and causal economic evaluation are unavailable".into(),
            ],
            provider_receipts: 0,
            externally_verified_provider_receipts: 0,
            provider_tasks: query.project_id.is_none().then_some(0),
            accepted_provider_tasks: query.project_id.is_none().then_some(0),
            registry_warnings: 0,
            totals: FleetMetrics::default(),
            by_project: Vec::new(),
            by_host: Vec::new(),
            by_family: Vec::new(),
            groups: Vec::new(),
        }
    }

    /// Add registry-only projects without requiring their directories to still exist.
    pub fn include_registered_project(&mut self, project_id: String, exists: bool) {
        if !valid_project_id(&project_id)
            || self
                .selected_project_id
                .as_ref()
                .is_some_and(|id| id != &project_id)
        {
            return;
        }
        if let Some(project) = self
            .by_project
            .iter_mut()
            .find(|project| project.project_id == project_id)
        {
            project.registered = true;
            project.workspace_exists = Some(exists);
        } else {
            self.by_project.push(FleetProject {
                project_id,
                registered: true,
                workspace_exists: Some(exists),
                metrics: FleetMetrics::default(),
            });
            self.by_project
                .sort_by(|left, right| left.project_id.cmp(&right.project_id));
        }
    }

    /// Publish an owner-only JSON artifact atomically; no ledger mutation.
    pub fn export_atomic(&self, path: &Path) -> Result<(), LedgerError> {
        let bytes = serde_json::to_vec_pretty(self).map_err(LedgerError::Serialize)?;
        let path = if path
            .parent()
            .is_some_and(|parent| parent.as_os_str().is_empty())
        {
            Path::new(".").join(path)
        } else {
            path.to_path_buf()
        };
        super::atomic_write(&path, &bytes)
    }
}

fn valid_project_id(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

impl Ledger {
    pub fn fleet_stats_read_only(
        path: &Path,
        query: FleetStatsQuery<'_>,
    ) -> Result<FleetStatsSnapshot, LedgerError> {
        query.validate()?;
        if !path.exists() {
            return Ok(FleetStatsSnapshot::empty(query));
        }
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(LedgerError::Database)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(LedgerError::Database)?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(LedgerError::Database)?;
        let snapshot = fleet_snapshot(&transaction, query)?;
        transaction.commit().map_err(LedgerError::Database)?;
        Ok(snapshot)
    }
}

fn fleet_snapshot(
    connection: &Connection,
    query: FleetStatsQuery<'_>,
) -> Result<FleetStatsSnapshot, LedgerError> {
    let mut snapshot = FleetStatsSnapshot::empty(query);
    snapshot.ledger_present = true;
    let policy = accounting_policy_predicate(query.include_legacy_versions);
    let neutral = savings_neutral_sql_predicate("rtk_cmd");
    let sql = format!(
        "WITH windowed AS (
            SELECT *, ROW_NUMBER() OVER (PARTITION BY project_hash, session_hash ORDER BY timestamp, id) AS seq
              FROM commands
             WHERE CAST(strftime('%s', timestamp) AS INTEGER) >= ?1
               AND CAST(strftime('%s', timestamp) AS INTEGER) < ?2
               AND (?3 IS NULL OR COALESCE(NULLIF(project_hash, ''), 'unscoped') = ?3)
         ), scoped AS (
            SELECT *, ({policy}) AS selected,
                   COALESCE(accounting_stage, 'unknown') != 'internal_transport' AS excluded_stage,
                   CASE WHEN ({neutral}) THEN output_tokens ELSE input_tokens END AS baseline
              FROM windowed
         ), prepared AS (
            SELECT *, selected AND NOT excluded_stage AND measurement = 'estimated'
                       AND COALESCE(route, '') != 'native_unaccounted' AS measured
              FROM scoped
         ), repeats AS (
            SELECT *, MAX(CASE WHEN measured AND route = 'optimized' AND output_tokens < baseline THEN seq END)
                OVER (PARTITION BY project_hash, session_hash, command_hash ORDER BY seq
                      ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) AS previous_filtered_seq
              FROM prepared
         )
         SELECT COALESCE(NULLIF(project_hash, ''), 'unscoped'), COALESCE(agent, 'unknown'),
                COALESCE(operation_family, 'other'), COALESCE(route, 'unknown'),
                SUM(CASE WHEN selected THEN 1 ELSE 0 END),
                SUM(CASE WHEN measured THEN 1 ELSE 0 END),
                SUM(CASE WHEN measured THEN baseline ELSE 0 END),
                SUM(CASE WHEN measured THEN output_tokens ELSE 0 END),
                SUM(CASE WHEN measured THEN baseline-output_tokens ELSE 0 END),
                SUM(CASE WHEN selected AND excluded_stage THEN 1 ELSE 0 END),
                SUM(CASE WHEN selected AND NOT excluded_stage AND route = 'native_unaccounted' THEN 1 ELSE 0 END),
                SUM(CASE WHEN selected AND NOT excluded_stage AND measurement != 'estimated' THEN 1 ELSE 0 END),
                SUM(CASE WHEN selected THEN 0 ELSE 1 END),
                SUM(CASE WHEN measured AND NULLIF(session_hash, '') IS NOT NULL AND NULLIF(command_hash, '') IS NOT NULL
                          AND seq <= previous_filtered_seq + {rerun_window} THEN 1 ELSE 0 END),
                SUM(CASE WHEN measured AND NULLIF(session_hash, '') IS NOT NULL AND NULLIF(command_hash, '') IS NOT NULL
                          AND seq <= previous_filtered_seq + {rerun_window} THEN output_tokens ELSE 0 END),
                SUM(CASE WHEN selected THEN COALESCE(exec_time_ms, 0) ELSE 0 END),
                SUM(CASE WHEN selected AND accounting_stage IN ('final_delivery', 'standalone_delivery') AND measurement = 'estimated' THEN 1 ELSE 0 END),
                SUM(CASE WHEN selected AND accounting_stage IN ('final_delivery', 'standalone_delivery') AND measurement = 'estimated' THEN output_tokens ELSE 0 END),
                SUM(CASE WHEN selected AND (accounting_stage IS NULL OR accounting_stage NOT IN ('internal_transport', 'final_delivery', 'standalone_delivery', 'control_plane')) THEN 1 ELSE 0 END)
           FROM repeats GROUP BY 1,2,3,4 ORDER BY 1,2,3,4",
        rerun_window = super::RERUN_DETECTION_WINDOW_OPERATIONS,
    );
    let mut statement = connection.prepare(&sql).map_err(LedgerError::Database)?;
    let rows = statement
        .query_map(
            params![
                query.since_unix_seconds,
                query.until_unix_seconds,
                query.project_id
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    FleetMetrics {
                        recorded_operations: row.get(4)?,
                        measured_operations: row.get(5)?,
                        baseline_tokens_estimated: row.get(6)?,
                        delivered_tokens_estimated: row.get(7)?,
                        net_avoided_tokens_estimated: row.get(8)?,
                        stage_excluded_operations: row.get(9)?,
                        native_unaccounted_operations: row.get(10)?,
                        unmeasured_operations: row.get(11)?,
                        excluded_legacy_operations: row.get(12)?,
                        repeated_after_filter_operations: row.get(13)?,
                        repeated_after_filter_tokens_estimated: row.get(14)?,
                        execution_ms: row.get(15)?,
                        explicit_delivery_operations: row.get(16)?,
                        explicit_delivery_tokens_estimated: row.get(17)?,
                        legacy_unknown_stage_operations: row.get(18)?,
                    },
                ))
            },
        )
        .map_err(LedgerError::Database)?;
    let mut grouped: BTreeMap<(String, String, String, String), FleetMetrics> = BTreeMap::new();
    for row in rows {
        let (project, host, family, route, metrics) = row.map_err(LedgerError::Database)?;
        let project = if valid_project_id(&project) {
            project
        } else {
            "unscoped".into()
        };
        let host = if host == "unknown" {
            host
        } else {
            privacy_safe_agent(Some(&host)).unwrap_or_else(|| "unknown".into())
        };
        let family = privacy_safe_family(&family);
        let route = match route.as_str() {
            "optimized" | "bypassed" | "native_unaccounted" => route,
            _ => "unknown".into(),
        };
        grouped
            .entry((project, host, family, route))
            .or_default()
            .add(&metrics)?;
    }
    let mut projects: BTreeMap<String, FleetMetrics> = BTreeMap::new();
    let mut hosts: BTreeMap<String, FleetMetrics> = BTreeMap::new();
    let mut families: BTreeMap<String, FleetMetrics> = BTreeMap::new();
    for ((project_id, host, family, route), metrics) in grouped {
        snapshot.totals.add(&metrics)?;
        projects
            .entry(project_id.clone())
            .or_default()
            .add(&metrics)?;
        hosts.entry(host.clone()).or_default().add(&metrics)?;
        families.entry(family.clone()).or_default().add(&metrics)?;
        snapshot.groups.push(FleetOperationGroup {
            project_id,
            host,
            family,
            route,
            metrics,
        });
    }
    snapshot.by_project = projects
        .into_iter()
        .map(|(project_id, metrics)| FleetProject {
            project_id,
            registered: false,
            workspace_exists: None,
            metrics,
        })
        .collect();
    snapshot.by_host = hosts
        .into_iter()
        .map(|(key, metrics)| FleetDimension { key, metrics })
        .collect();
    snapshot.by_family = families
        .into_iter()
        .map(|(key, metrics)| FleetDimension { key, metrics })
        .collect();
    (
        snapshot.provider_receipts,
        snapshot.externally_verified_provider_receipts,
    ) = connection
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(CASE WHEN externally_verified = 1 THEN 1 ELSE 0 END), 0)
           FROM provider_economic_receipts WHERE observed_at_ms >= ?1 AND observed_at_ms < ?2
           AND (?3 IS NULL OR COALESCE(NULLIF(project_hash, ''), 'unscoped') = ?3)",
            params![
                query.since_unix_seconds * 1000,
                query.until_unix_seconds * 1000,
                query.project_id
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(LedgerError::Database)?;
    // Task receipts historically retain project_path, not project_hash. SQL hashes are unavailable;
    // explicit historical-ID selection therefore does not fabricate a project task count.
    if query.project_id.is_none() {
        let (tasks, accepted): (u64, u64) = connection
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(CASE WHEN outcome = 'accepted' THEN 1 ELSE 0 END),0)
             FROM usage_records WHERE created_at_ms >= ?1 AND created_at_ms < ?2",
                params![
                    query.since_unix_seconds * 1000,
                    query.until_unix_seconds * 1000
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(LedgerError::Database)?;
        snapshot.provider_tasks = Some(tasks);
        snapshot.accepted_provider_tasks = Some(accepted);
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests;
