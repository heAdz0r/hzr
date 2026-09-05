//! 0.8.1: post-upgrade reference-state reconciliation.
//!
//! A new HZR version must leave every registered workspace, index, engine and client pin in
//! the reference state without the operator visiting each project. The SessionStart hook has a
//! ten-second budget while a fleet pass takes longer, so the first `hzr init --if-needed` on a
//! new version records a marker and launches one detached `hzr doctor --reconcile-fleet --fix`
//! whose JSON report lands under `reports/`. `hzr update` runs the same pass in the foreground.
//! Doctor reports the marker as `reference_state` so an interrupted pass stays visible.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fs2::FileExt;
use hzr_core::Config;
use serde::{Deserialize, Serialize};

use crate::diagnostics::{CheckStatus, DoctorCheck, DoctorReport};

/// Environment variable carrying the marker path into the doctor pass that must record it.
pub const MARKER_ENV: &str = "HZR_REFERENCE_STATE_MARKER";
const SCHEMA_VERSION: u16 = 1;
/// A scheduled pass older than this is treated as interrupted and scheduled again.
const STALE_SCHEDULE_SECS: u64 = 15 * 60;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceState {
    Scheduled,
    Complete,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReferenceStateSummary {
    pub healthy: bool,
    pub workspaces_scanned: usize,
    pub stale_registrations_pruned: usize,
    pub orphaned_engines_stopped: usize,
    pub remaining_errors: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReferenceStateMarker {
    pub schema_version: u16,
    pub hzr_version: String,
    pub state: ReferenceState,
    pub scheduled_at_unix: u64,
    #[serde(default)]
    pub completed_at_unix: Option<u64>,
    #[serde(default)]
    pub report_path: Option<PathBuf>,
    #[serde(default)]
    pub summary: Option<ReferenceStateSummary>,
    #[serde(default)]
    pub error: Option<String>,
}

/// What `hzr init --if-needed` decided about the reconciliation pass.
#[derive(Clone, Debug, Serialize)]
pub struct ScheduleOutcome {
    pub action: &'static str,
    pub hzr_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ScheduleOutcome {
    pub fn failed(error: anyhow::Error) -> Self {
        Self {
            action: "failed",
            hzr_version: env!("CARGO_PKG_VERSION").to_owned(),
            report_path: None,
            error: Some(format!("{error:#}")),
        }
    }
}

pub fn marker_path(config: &Config) -> PathBuf {
    config.data_dir.join("runtime/reference-state.json")
}

fn lock_path(config: &Config) -> PathBuf {
    config.data_dir.join("runtime/reference-state.lock")
}

fn report_path(config: &Config, version: &str) -> PathBuf {
    config
        .data_dir
        .join("reports")
        .join(format!("reference-state-{version}.json"))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn read_marker(config: &Config) -> Result<Option<ReferenceStateMarker>> {
    let path = marker_path(config);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let marker: ReferenceStateMarker = serde_json::from_slice(&bytes)
        .with_context(|| format!("{} is not a valid reference-state marker", path.display()))?;
    if marker.schema_version != SCHEMA_VERSION {
        anyhow::bail!(
            "{} has unsupported schema version {}",
            path.display(),
            marker.schema_version
        );
    }
    Ok(Some(marker))
}

fn write_marker(config: &Config, marker: &ReferenceStateMarker) -> Result<()> {
    let path = marker_path(config);
    let parent = path
        .parent()
        .context("reference-state marker has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let mut bytes = serde_json::to_vec_pretty(marker)?;
    bytes.push(b'\n');
    let temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to stage {}", path.display()))?;
    fs::write(temporary.path(), &bytes)?;
    temporary
        .persist(&path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

/// Decide whether this version still needs its reconciliation pass and, if so, launch it
/// detached. Returns `None` when the pass already completed for the running version.
pub fn schedule_if_needed(config: &Config, workspace: &Path) -> Result<Option<ScheduleOutcome>> {
    let version = env!("CARGO_PKG_VERSION");
    let lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path(config))
        .context("failed to open the reference-state lock")?;
    if lock.try_lock_exclusive().is_err() {
        return Ok(Some(ScheduleOutcome {
            action: "in_progress",
            hzr_version: version.to_owned(),
            report_path: None,
            error: None,
        }));
    }
    let outcome = (|| {
        let now = unix_now();
        match read_marker(config) {
            Ok(Some(marker)) if marker.hzr_version == version => match marker.state {
                ReferenceState::Complete => return Ok(None),
                ReferenceState::Scheduled
                    if now.saturating_sub(marker.scheduled_at_unix) < STALE_SCHEDULE_SECS =>
                {
                    return Ok(Some(ScheduleOutcome {
                        action: "already_scheduled",
                        hzr_version: version.to_owned(),
                        report_path: marker.report_path,
                        error: None,
                    }));
                }
                ReferenceState::Scheduled | ReferenceState::Failed => {}
            },
            // A different version, no marker, or an unreadable marker all mean: run the pass.
            Ok(_) | Err(_) => {}
        }
        let report = report_path(config, version);
        let reports_dir = report
            .parent()
            .context("reference-state report has no parent directory")?;
        fs::create_dir_all(reports_dir)
            .with_context(|| format!("failed to create {}", reports_dir.display()))?;
        write_marker(
            config,
            &ReferenceStateMarker {
                schema_version: SCHEMA_VERSION,
                hzr_version: version.to_owned(),
                state: ReferenceState::Scheduled,
                scheduled_at_unix: now,
                completed_at_unix: None,
                report_path: Some(report.clone()),
                summary: None,
                error: None,
            },
        )?;
        let executable =
            std::env::current_exe().context("cannot resolve the HZR executable to schedule")?;
        let stdout = fs::File::create(&report)
            .with_context(|| format!("failed to create {}", report.display()))?;
        let stderr = fs::File::create(report.with_extension("log"))
            .with_context(|| format!("failed to create the log next to {}", report.display()))?;
        let mut command = Command::new(executable);
        command
            .args([
                "doctor",
                "--reconcile-fleet",
                "--fix",
                "--json",
                "--workspace",
            ])
            .arg(workspace)
            .env(MARKER_ENV, marker_path(config))
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Own process group: the hook's shell exiting must not take the pass with it.
            command.process_group(0);
        }
        command
            .spawn()
            .context("failed to launch the detached reference-state doctor pass")?;
        Ok(Some(ScheduleOutcome {
            action: "scheduled",
            hzr_version: version.to_owned(),
            report_path: Some(report),
            error: None,
        }))
    })();
    // fs2's trait method: std's `File::unlock` is stable only from Rust 1.89 (MSRV is 1.85).
    let _ = FileExt::unlock(&lock);
    outcome
}

fn summary_from_report(report: &DoctorReport) -> ReferenceStateSummary {
    ReferenceStateSummary {
        healthy: report.healthy,
        workspaces_scanned: report
            .fleet_reconcile
            .as_ref()
            .map_or(0, |fleet| fleet.workspaces_scanned),
        stale_registrations_pruned: report.fleet_reconcile.as_ref().map_or(0, |fleet| {
            fleet
                .stale_registrations
                .iter()
                .filter(|entry| entry.state == "pruned")
                .count()
        }),
        orphaned_engines_stopped: report.orphan_cleanup.as_ref().map_or(0, |outcomes| {
            outcomes
                .iter()
                .filter(|outcome| {
                    matches!(
                        outcome.state,
                        crate::foreign::OrphanStopState::Terminated
                            | crate::foreign::OrphanStopState::Killed
                    )
                })
                .count()
        }),
        remaining_errors: report
            .checks
            .iter()
            .filter(|check| check.status == CheckStatus::Error)
            .map(|check| check.name.clone())
            .collect(),
    }
}

/// Record the outcome of a doctor pass that was launched as the reference-state
/// reconciliation. Only the marker path this data root owns is ever written.
pub fn record_completion_from_env(config: &Config, report: &DoctorReport) {
    let Ok(requested) = std::env::var(MARKER_ENV) else {
        return;
    };
    record_completion_for_marker(config, Path::new(&requested), report);
}

fn record_completion_for_marker(config: &Config, requested: &Path, report: &DoctorReport) {
    if requested != marker_path(config) {
        eprintln!(
            "HZR reference-state marker {} is outside this data root; not recorded",
            requested.display()
        );
        return;
    }
    if let Err(error) = record_completion(config, report) {
        eprintln!("HZR reference-state completion was not recorded: {error:#}");
    }
}

pub fn record_completion(config: &Config, report: &DoctorReport) -> Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let previous = read_marker(config).ok().flatten();
    let summary = summary_from_report(report);
    let state = if summary.remaining_errors.is_empty() {
        ReferenceState::Complete
    } else {
        ReferenceState::Failed
    };
    write_marker(
        config,
        &ReferenceStateMarker {
            schema_version: SCHEMA_VERSION,
            hzr_version: version.to_owned(),
            state,
            scheduled_at_unix: previous
                .as_ref()
                .filter(|marker| marker.hzr_version == version)
                .map_or_else(unix_now, |marker| marker.scheduled_at_unix),
            completed_at_unix: Some(unix_now()),
            report_path: previous.and_then(|marker| marker.report_path),
            error: (state == ReferenceState::Failed).then(|| {
                format!(
                    "doctor still reports errors: {}",
                    summary.remaining_errors.join(", ")
                )
            }),
            summary: Some(summary),
        },
    )
}

/// Run the reconciliation pass in the foreground with `binary` (used after `hzr update`).
pub fn run_foreground(config: &Config, binary: &Path, json: bool) -> Result<bool> {
    let mut command = Command::new(binary);
    command
        .args(["doctor", "--reconcile-fleet", "--fix"])
        .env(MARKER_ENV, marker_path(config));
    if json {
        command.arg("--json");
    }
    let status = command
        .status()
        .with_context(|| format!("failed to run {} doctor", binary.display()))?;
    Ok(status.success())
}

pub fn reference_state_check(config: &Config) -> DoctorCheck {
    let version = env!("CARGO_PKG_VERSION");
    let remedy = "run `hzr doctor --reconcile-fleet --fix`";
    let (status, detail) = match read_marker(config) {
        Err(error) => (
            CheckStatus::Warning,
            format!("reference-state marker unreadable: {error:#}; {remedy}"),
        ),
        Ok(None) => (
            CheckStatus::Warning,
            format!(
                "post-upgrade reconciliation has not run for {version}; the next session start schedules it, or {remedy}"
            ),
        ),
        Ok(Some(marker)) if marker.hzr_version != version => (
            CheckStatus::Warning,
            format!(
                "post-upgrade reconciliation last completed for {}, not {version}; the next session start schedules it, or {remedy}",
                marker.hzr_version
            ),
        ),
        Ok(Some(marker)) => match marker.state {
            ReferenceState::Scheduled => (
                CheckStatus::Warning,
                format!(
                    "post-upgrade reconciliation for {version} was scheduled {}s ago and has not reported; if it is not running, {remedy}",
                    unix_now().saturating_sub(marker.scheduled_at_unix)
                ),
            ),
            ReferenceState::Failed => (
                CheckStatus::Warning,
                format!(
                    "post-upgrade reconciliation for {version} finished with errors ({}); {remedy}",
                    marker.error.unwrap_or_else(|| "unspecified".into())
                ),
            ),
            ReferenceState::Complete => {
                let summary = marker.summary.as_ref();
                (
                    CheckStatus::Pass,
                    format!(
                        "fleet reconciled for {version}: {} workspace(s) verified, {} stale registration(s) pruned, {} orphaned engine(s) stopped",
                        summary.map_or(0, |summary| summary.workspaces_scanned),
                        summary.map_or(0, |summary| summary.stale_registrations_pruned),
                        summary.map_or(0, |summary| summary.orphaned_engines_stopped)
                    ),
                )
            }
        },
    };
    DoctorCheck {
        name: "reference_state".into(),
        status,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(directory: &Path) -> Config {
        Config {
            data_dir: directory.join("data"),
            ..Config::default()
        }
    }

    #[test]
    fn missing_marker_and_other_versions_warn_and_completion_passes() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config = config(directory.path());
        fs::create_dir_all(config.data_dir.join("runtime")).expect("runtime");
        assert_eq!(reference_state_check(&config).status, CheckStatus::Warning);

        write_marker(
            &config,
            &ReferenceStateMarker {
                schema_version: SCHEMA_VERSION,
                hzr_version: "0.0.1".into(),
                state: ReferenceState::Complete,
                scheduled_at_unix: 1,
                completed_at_unix: Some(2),
                report_path: None,
                summary: None,
                error: None,
            },
        )
        .expect("marker");
        let check = reference_state_check(&config);
        assert_eq!(check.status, CheckStatus::Warning);
        assert!(check.detail.contains("0.0.1"), "{}", check.detail);

        let report = DoctorReport {
            hzr_version: env!("CARGO_PKG_VERSION").into(),
            config_path: PathBuf::new(),
            data_dir: config.data_dir.clone(),
            workspace: PathBuf::new(),
            healthy: true,
            readiness: crate::diagnostics::ReadinessReport::default(),
            checks: vec![DoctorCheck {
                name: "daemon".into(),
                status: CheckStatus::Pass,
                detail: String::new(),
            }],
            client_workspace_bindings: Vec::new(),
            response_codec_coverage: Vec::new(),
            repair: None,
            fidelity_reconcile: None,
            fleet_reconcile: None,
            orphan_cleanup: None,
        };
        record_completion(&config, &report).expect("record");
        let check = reference_state_check(&config);
        assert_eq!(check.status, CheckStatus::Pass, "{}", check.detail);
        let marker = read_marker(&config).expect("read").expect("marker");
        assert_eq!(marker.state, ReferenceState::Complete);
        assert_eq!(marker.hzr_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn errors_in_the_pass_are_recorded_as_failed_not_hidden() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config = config(directory.path());
        fs::create_dir_all(config.data_dir.join("runtime")).expect("runtime");
        let report = DoctorReport {
            hzr_version: env!("CARGO_PKG_VERSION").into(),
            config_path: PathBuf::new(),
            data_dir: config.data_dir.clone(),
            workspace: PathBuf::new(),
            healthy: false,
            readiness: crate::diagnostics::ReadinessReport::default(),
            checks: vec![DoctorCheck {
                name: "foreign_engine_processes".into(),
                status: CheckStatus::Error,
                detail: String::new(),
            }],
            client_workspace_bindings: Vec::new(),
            response_codec_coverage: Vec::new(),
            repair: None,
            fidelity_reconcile: None,
            fleet_reconcile: None,
            orphan_cleanup: None,
        };
        record_completion(&config, &report).expect("record");
        let check = reference_state_check(&config);
        assert_eq!(check.status, CheckStatus::Warning);
        assert!(
            check.detail.contains("foreign_engine_processes"),
            "{}",
            check.detail
        );
    }

    #[test]
    fn marker_outside_the_data_root_is_never_written() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config = config(directory.path());
        fs::create_dir_all(config.data_dir.join("runtime")).expect("runtime");
        let foreign = directory.path().join("elsewhere.json");
        let report = DoctorReport {
            hzr_version: env!("CARGO_PKG_VERSION").into(),
            config_path: PathBuf::new(),
            data_dir: config.data_dir.clone(),
            workspace: PathBuf::new(),
            healthy: true,
            readiness: crate::diagnostics::ReadinessReport::default(),
            checks: Vec::new(),
            client_workspace_bindings: Vec::new(),
            response_codec_coverage: Vec::new(),
            repair: None,
            fidelity_reconcile: None,
            fleet_reconcile: None,
            orphan_cleanup: None,
        };
        record_completion_for_marker(&config, &foreign, &report);
        assert!(!foreign.exists());
        assert!(!marker_path(&config).exists());
    }
}
