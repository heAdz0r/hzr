use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use hzr_agent::{IntegrationLayout, preflight};
use hzr_core::{Config, locked_engines};
use hzr_index::{Deadlines, IndexPlacement, Workspace};
use hzr_protocol::{EngineState, PROTOCOL_VERSION};
use serde::Serialize;
use tokio::process::Command;
use tokio::time::timeout;

use crate::client::DaemonClient;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DoctorReport {
    pub hzr_version: String,
    pub config_path: PathBuf,
    pub data_dir: PathBuf,
    pub workspace: PathBuf,
    pub healthy: bool,
    pub checks: Vec<DoctorCheck>,
}

pub async fn doctor(config_path: &Path, config: &Config, workspace: &Path) -> DoctorReport {
    let mut checks = Vec::new();
    checks.push(if config_path.is_file() {
        check("config", CheckStatus::Pass, config_path.display())
    } else {
        check(
            "config",
            CheckStatus::Warning,
            format!("{} is absent; defaults are active", config_path.display()),
        )
    });
    checks.push(if config.data_dir.is_dir() {
        check("data_root", CheckStatus::Pass, config.data_dir.display())
    } else {
        check(
            "data_root",
            CheckStatus::Warning,
            format!("{} is absent; run `hzr init`", config.data_dir.display()),
        )
    });

    match locked_engines() {
        Ok(manifest) => {
            checks.push(check(
                "engine_lock",
                CheckStatus::Pass,
                format!("{} pinned components", manifest.engine.len()),
            ));
            for pin in manifest
                .engine
                .iter()
                .filter(|pin| !pin.binary.is_empty() && pin.name != "caveman-code")
            {
                checks.push(
                    inspect_engine(
                        &pin.name,
                        &pin.version,
                        &config.engines.binary(&pin.binary),
                        config.engines.strict_versions,
                    )
                    .await,
                );
            }
        }
        Err(error) => checks.push(check("engine_lock", CheckStatus::Error, error)),
    }

    let integration = integration_layout(config);
    let node = std::env::var_os("HZR_NODE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("node"));
    match preflight(&node, &integration).await {
        Ok(report) => checks.push(check(
            "caveman_code",
            CheckStatus::Pass,
            format!(
                "Node {}; caveman-code {} ({})",
                report.node_version,
                report.runtime.version,
                report.runtime.installed_package.display()
            ),
        )),
        Err(error) => checks.push(check("caveman_code", strict_status(config), error)),
    }

    let deadlines = Deadlines::default();
    match Workspace::discover_managed(
        workspace,
        Path::new("git"),
        &config.data_dir,
        deadlines.version,
    )
    .await
    {
        Ok(discovered) => {
            match discovered.placement() {
                Ok(IndexPlacement::ManagedSymlink { link, target }) => checks.push(check(
                    "grepai_ownership",
                    CheckStatus::Pass,
                    format!("{} -> {}", link.display(), target.display()),
                )),
                Ok(IndexPlacement::Missing {
                    intended_directory, ..
                }) => checks.push(check(
                    "grepai_ownership",
                    CheckStatus::Pass,
                    format!(
                        "uninitialized; canonical target is {}",
                        intended_directory.display()
                    ),
                )),
                Ok(IndexPlacement::LegacyProject { directory }) => checks.push(check(
                    "grepai_ownership",
                    CheckStatus::Warning,
                    format!("legacy project index at {}", directory.display()),
                )),
                Ok(placement) => checks.push(check(
                    "grepai_ownership",
                    CheckStatus::Error,
                    format!("conflicting placement: {placement:?}"),
                )),
                Err(error) => checks.push(check("grepai_ownership", CheckStatus::Error, error)),
            }
            checks.push(if discovered.duplicate_index_dirs.is_empty() {
                check("grepai_duplicates", CheckStatus::Pass, "none found")
            } else {
                check(
                    "grepai_duplicates",
                    CheckStatus::Error,
                    discovered
                        .duplicate_index_dirs
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                )
            });
        }
        Err(error) => checks.push(check("grepai_ownership", CheckStatus::Error, error)),
    }

    let token_path = config.data_dir.join("runtime/hzrd.token");
    if token_path.exists() {
        match DaemonClient::from_config(config) {
            Ok(client) => match client.health().await {
                Ok(health) => {
                    let compatible = health.protocol_version == PROTOCOL_VERSION
                        && health.hzr_version == env!("CARGO_PKG_VERSION");
                    let status = if compatible && health.state != EngineState::Degraded {
                        CheckStatus::Pass
                    } else {
                        CheckStatus::Error
                    };
                    checks.push(check(
                        "daemon",
                        status,
                        format!(
                            "HZR {}, protocol {}, state {:?}",
                            health.hzr_version, health.protocol_version, health.state
                        ),
                    ));
                }
                Err(error) => checks.push(check("daemon", CheckStatus::Warning, error)),
            },
            Err(error) => checks.push(check("daemon_token", CheckStatus::Error, error)),
        }
    } else {
        checks.push(check(
            "daemon",
            CheckStatus::Warning,
            "not initialized; run `hzr daemon serve`",
        ));
    }

    let healthy = checks
        .iter()
        .all(|check| check.status != CheckStatus::Error);
    DoctorReport {
        hzr_version: env!("CARGO_PKG_VERSION").into(),
        config_path: config_path.to_path_buf(),
        data_dir: config.data_dir.clone(),
        workspace: workspace.to_path_buf(),
        healthy,
        checks,
    }
}

pub fn integration_layout(config: &Config) -> IntegrationLayout {
    if let Some(root) = std::env::var_os("HZR_CAVEMAN_CODE_DIR") {
        return IntegrationLayout::new(PathBuf::from(root));
    }
    if let Some(engine_directory) = &config.engines.directory {
        return IntegrationLayout::new(engine_directory.join("caveman-code"));
    }
    IntegrationLayout::new(config.data_dir.join("engines/caveman-code"))
}

pub fn resolve_binary(candidate: &Path) -> Option<PathBuf> {
    if candidate.components().count() > 1 || candidate.is_absolute() {
        return is_executable(candidate).then(|| candidate.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|directory| binary_candidates(&directory, candidate))
        .find(|path| is_executable(path))
}

async fn inspect_engine(name: &str, expected: &str, candidate: &Path, strict: bool) -> DoctorCheck {
    let Some(binary) = resolve_binary(candidate) else {
        return check(
            format!("engine_{name}"),
            if strict {
                CheckStatus::Error
            } else {
                CheckStatus::Warning
            },
            format!("{} is not executable or not on PATH", candidate.display()),
        );
    };
    let version_argument = if name == "grepai" {
        "version"
    } else {
        "--version"
    };
    let child = Command::new(&binary)
        .arg(version_argument)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let output = match child {
        Ok(child) => match timeout(Duration::from_secs(5), child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                return check(format!("engine_{name}"), CheckStatus::Error, error);
            }
            Err(_) => {
                return check(
                    format!("engine_{name}"),
                    CheckStatus::Error,
                    "version probe exceeded five seconds",
                );
            }
        },
        Err(error) => return check(format!("engine_{name}"), CheckStatus::Error, error),
    };
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let rendered = rendered.trim();
    if !output.status.success() {
        return check(
            format!("engine_{name}"),
            CheckStatus::Error,
            format!(
                "{} exited with {}: {rendered}",
                binary.display(),
                output.status
            ),
        );
    }
    if !rendered.contains(expected) {
        return check(
            format!("engine_{name}"),
            if strict {
                CheckStatus::Error
            } else {
                CheckStatus::Warning
            },
            format!(
                "expected {expected}, got {} from {}",
                bounded(rendered),
                binary.display()
            ),
        );
    }
    check(
        format!("engine_{name}"),
        CheckStatus::Pass,
        format!("{}: {}", binary.display(), bounded(rendered)),
    )
}

fn strict_status(config: &Config) -> CheckStatus {
    if config.engines.strict_versions {
        CheckStatus::Error
    } else {
        CheckStatus::Warning
    }
}

fn check(
    name: impl Into<String>,
    status: CheckStatus,
    detail: impl std::fmt::Display,
) -> DoctorCheck {
    DoctorCheck {
        name: name.into(),
        status,
        detail: detail.to_string(),
    }
}

fn bounded(value: &str) -> &str {
    let mut boundary = value.len().min(512);
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &value[..boundary]
}

fn binary_candidates(directory: &Path, candidate: &Path) -> Vec<PathBuf> {
    let path = directory.join(candidate);
    #[cfg(windows)]
    {
        let mut candidates = vec![path.clone()];
        if path.extension().is_none() {
            candidates
                .extend(["exe", "cmd", "bat"].map(|extension| path.with_extension(extension)));
        }
        candidates
    }
    #[cfg(not(windows))]
    {
        vec![path]
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use hzr_core::Config;

    use super::{bounded, integration_layout};

    #[test]
    fn test_bounded_diagnostic_respects_utf8_boundary() {
        let value = "€".repeat(300);
        let bounded = bounded(&value);

        assert_eq!(bounded.len(), 510);
        assert_eq!(bounded.chars().count(), 170);
    }

    #[test]
    fn test_integration_layout_prefers_relocatable_bundle_engine() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let engines = directory.path().join("engines");
        let integration = engines.join("caveman-code");
        let mut config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        config.engines.directory = Some(engines);

        assert_eq!(integration_layout(&config).root(), integration);
    }
}
