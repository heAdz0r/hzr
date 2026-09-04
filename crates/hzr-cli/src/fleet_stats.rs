use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use hzr_core::{Config, FleetStatsQuery, Ledger, privacy_identity_hash};

use crate::cli::{AccountingVersion, StatsDuration};

pub struct FleetOptions<'a> {
    pub since: Option<&'a StatsDuration>,
    pub since_unix: Option<i64>,
    pub until: Option<i64>,
    pub project_id: Option<&'a str>,
    pub export: Option<&'a Path>,
    pub accounting_version: AccountingVersion,
    pub json: bool,
}

pub fn show(config: &Config, options: FleetOptions<'_>) -> Result<()> {
    let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
    let (since, until) = window(options.since, options.since_unix, options.until, now)?;
    let mut report = Ledger::fleet_stats_read_only(
        &config.data_dir.join("ledger/hzr.sqlite"),
        FleetStatsQuery {
            since_unix_seconds: since,
            until_unix_seconds: until,
            project_id: options.project_id,
            include_legacy_versions: options.accounting_version == AccountingVersion::All,
        },
    )?;
    let registry = hzr_index::registered_workspaces(&config.data_dir);
    report.registry_warnings = registry.warnings.len() as u64;
    for registration in registry.registrations {
        let id = privacy_identity_hash("project", &registration.root.to_string_lossy());
        report.include_registered_project(id, registration.root.is_dir());
    }
    if let Some(path) = options.export {
        report.export_atomic(path)?;
    }
    if options.json {
        crate::output::print_json(&report)?;
    } else {
        crate::stats_output::print_fleet(&report)?;
        if let Some(path) = options.export {
            println!("Snapshot exported to {}", path.display());
        }
    }
    Ok(())
}

fn window(
    since: Option<&StatsDuration>,
    since_unix: Option<i64>,
    until: Option<i64>,
    now: i64,
) -> Result<(i64, i64)> {
    anyhow::ensure!(
        since.is_none() || since_unix.is_none(),
        "choose --since or --since-unix"
    );
    let until = until.unwrap_or(now);
    let since = match (since, since_unix) {
        (_, Some(since)) => since,
        (Some(duration), None) => until
            .checked_sub(i64::try_from(duration.seconds())?)
            .unwrap_or(-1),
        (None, None) => {
            anyhow::bail!("fleet snapshots require --since <duration> or --since-unix <seconds>")
        }
    };
    anyhow::ensure!(
        since >= 0 && until > since,
        "fleet window must satisfy 0 <= since < until"
    );
    Ok((since, until))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_window_is_independent_of_execution_time() -> Result<()> {
        assert_eq!(window(None, Some(100), Some(200), 999)?, (100, 200));
        assert_eq!(window(None, Some(100), Some(200), 1999)?, (100, 200));
        assert!(window(None, None, None, 200).is_err());
        assert!(window(None, Some(200), Some(100), 999).is_err());
        assert!(window(None, Some(-1), Some(100), 999).is_err());
        Ok(())
    }
}
