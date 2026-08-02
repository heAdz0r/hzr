use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use hzr_agent::{IntegrationLayout, preflight};
use hzr_core::{Config, locked_engines};
use hzr_index::{Deadlines, IndexPlacement, Workspace};
use hzr_protocol::{EngineState, PROTOCOL_VERSION};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tokio::time::timeout;

use crate::cli::ServiceCommand;
use crate::client::DaemonClient;
use crate::{adoption, client_config, foreign, hook_runner, instructions, prefix, service};

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
    match adoption::default_settings_path().and_then(|path| adoption::status(&path)) {
        Ok(status) if status.conflict || status.hzr_entries > 2 => checks.push(check(
            "hook_ownership",
            CheckStatus::Error,
            format!(
                "HZR={} RTK={}; exactly two HZR handlers and zero RTK handlers are allowed",
                status.hzr_entries, status.rtk_entries
            ),
        )),
        Ok(status) if status.installed => checks.push(check(
            "hook_ownership",
            CheckStatus::Pass,
            "one HZR dispatcher plus one SessionStart initializer",
        )),
        Ok(status) => checks.push(check(
            "hook_ownership",
            CheckStatus::Warning,
            format!(
                "HZR={} RTK={}; run `hzr install --dry-run`",
                status.hzr_entries, status.rtk_entries
            ),
        )),
        Err(error) => checks.push(check("hook_ownership", CheckStatus::Warning, error)),
    }
    if let Ok(status) = adoption::default_settings_path().and_then(|path| adoption::status(&path)) {
        if status.external_icm_entries > 0 {
            // A direct `icm hook` writes to a store HZR does not supervise: that is a
            // second durable memory layer, which §6.5 gives HZR sole ownership of.
            checks.push(check(
                "external_icm_hooks",
                CheckStatus::Error,
                format!(
                    "{} external ICM hook(s) bypass HZR memory ownership; run `hzr install` \
                     (or `--keep-external-icm` to accept the duplicate deliberately)",
                    status.external_icm_entries
                ),
            ));
        }
    }
    // `hzr` must be reachable by name: hooks and the CLAUDE.md contract both call it.
    match prefix::default_prefix() {
        Ok(prefix_dir) => {
            let installed = prefix_dir.join("hzr").exists();
            let on_path = prefix::is_on_path(&prefix_dir);
            checks.push(match (installed, on_path) {
                (true, true) => check(
                    "hzr_on_path",
                    CheckStatus::Pass,
                    format!("{}", prefix_dir.join("hzr").display()),
                ),
                (true, false) => check(
                    "hzr_on_path",
                    CheckStatus::Error,
                    format!(
                        "{} is not on PATH; add it to the shell profile",
                        prefix_dir.display()
                    ),
                ),
                (false, _) => check(
                    "hzr_on_path",
                    CheckStatus::Error,
                    format!(
                        "no durable `hzr` in {}; run `hzr install --prefix {}`",
                        prefix_dir.display(),
                        prefix_dir.display()
                    ),
                ),
            });
        }
        Err(error) => checks.push(check("hzr_on_path", CheckStatus::Warning, error)),
    }
    // Agent instructions are what make an agent *prefer* hzr; hooks alone only
    // rewrite Bash, so a missing contract is a real adoption gap, not cosmetic.
    for surface in [instructions::Surface::Claude, instructions::Surface::Codex] {
        let name = match surface {
            instructions::Surface::Claude => "claude_instructions",
            instructions::Surface::Codex => "codex_instructions",
        };
        // A BEGIN marker alone is not adoption: the referenced contract must be readable
        // and no legacy mandate may survive beside it. Marker + conflict is a failure.
        match surface
            .default_path()
            .and_then(|path| instructions::audit(&path))
        {
            Ok(report) if report.healthy() => {
                checks.push(check(name, CheckStatus::Pass, report.path.display()))
            }
            Ok(report) => {
                let mut reasons = Vec::new();
                if !report.installed {
                    reasons.push("HZR contract block is absent".to_owned());
                }
                if report.installed && !report.contract_readable {
                    reasons.push(match &report.contract_path {
                        Some(contract) => {
                            format!("referenced contract {} is unreadable", contract.display())
                        }
                        None => "block references no contract asset".to_owned(),
                    });
                }
                if !report.conflicting_mandates.is_empty() {
                    reasons.push(format!(
                        "legacy directives still active outside the managed block: {}",
                        report.conflicting_mandates.join(", ")
                    ));
                }
                checks.push(check(
                    name,
                    CheckStatus::Error,
                    format!(
                        "{}: {}; run `hzr install --force`",
                        report.path.display(),
                        reasons.join("; ")
                    ),
                ));
            }
            Err(error) => checks.push(check(name, CheckStatus::Warning, error)),
        }
    }
    // Direct client ICM registration is a second memory writer regardless of what the
    // instruction files say, so it is audited separately from the text.
    match client_config::direct_icm_registrations() {
        Ok(found) if found.is_empty() => checks.push(check(
            "client_mcp_ownership",
            CheckStatus::Pass,
            "no direct ICM registration in client MCP configs",
        )),
        Ok(found) => checks.push(check(
            "client_mcp_ownership",
            CheckStatus::Error,
            direct_icm_registration_detail(&found),
        )),
        Err(error) => checks.push(check("client_mcp_ownership", CheckStatus::Warning, error)),
    }
    // Neither host exposes a global request/response interception point. Keep that
    // boundary machine-visible so an MCP/tool migration cannot be mistaken for codec
    // coverage or credited as delivered token savings.
    for client in ["claude", "codex"] {
        checks.push(check(
            format!("{client}_global_codec"),
            CheckStatus::Warning,
            "unintercepted: the host exposes no global request/response hook; HZR records no codec savings for this path",
        ));
    }
    // Report only. Stopping foreign processes stays an explicit user decision (§11).
    match foreign::scan(&config.data_dir) {
        Ok(report) => {
            if report.unmanaged_active_total() > 0 {
                let detail = report
                    .unmanaged_by_engine
                    .iter()
                    .map(|(engine, count)| format!("{engine}={count}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                checks.push(check(
                    "foreign_engine_processes",
                    CheckStatus::Error,
                    format!(
                        "{} unmanaged engine process(es) ({detail}) duplicate HZR ownership; \
                         stop them yourself — HZR never kills external processes",
                        report.unmanaged_active_total()
                    ),
                ));
            } else {
                checks.push(check(
                    "foreign_engine_processes",
                    CheckStatus::Pass,
                    "none detected",
                ));
            }
            if report.unmanaged_wrapper_total() > 0 {
                let detail = report
                    .unmanaged_wrappers_by_engine
                    .iter()
                    .map(|(engine, count)| format!("{engine}={count}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                checks.push(check(
                    "foreign_engine_wrappers",
                    CheckStatus::Warning,
                    format!(
                        "{} client wrapper(s) still mention direct engines ({detail}); restart clients after config migration",
                        report.unmanaged_wrapper_total()
                    ),
                ));
            } else {
                checks.push(check(
                    "foreign_engine_wrappers",
                    CheckStatus::Pass,
                    "none detected",
                ));
            }
        }
        Err(error) => {
            checks.push(check(
                "foreign_engine_processes",
                CheckStatus::Warning,
                &error,
            ));
            checks.push(check(
                "foreign_engine_wrappers",
                CheckStatus::Warning,
                error,
            ));
        }
    }
    match hook_runner::degraded_rewrite_coverage(config) {
        // A closed gap is a pass, not a permanent warning — the ledger is whole again and
        // the history stays visible in the detail line.
        Ok(coverage) if coverage.complete && coverage.lifetime_rewrites == 0 => checks.push(
            check("degraded_rewrites", CheckStatus::Pass, "none recorded"),
        ),
        Ok(coverage) if coverage.complete => checks.push(check(
            "degraded_rewrites",
            CheckStatus::Pass,
            format!(
                "{} historical daemon-free rewrite(s), all reconciled",
                coverage.lifetime_rewrites
            ),
        )),
        Ok(coverage) => checks.push(check(
            "degraded_rewrites",
            CheckStatus::Warning,
            format!(
                "{} daemon-free rewrite(s) are not in the ledger; the next managed rewrite reconciles them",
                coverage.unreconciled_rewrites
            ),
        )),
        Err(error) => checks.push(check("degraded_rewrites", CheckStatus::Warning, error)),
    }
    let global_binary_exists = prefix::default_prefix()
        .map(|directory| directory.join("hzr").exists())
        .unwrap_or(false);
    match service::execute(ServiceCommand::Status) {
        Ok(report) if report.active && !report.binary.to_string_lossy().contains("/versions/") => {
            checks.push(check(
                "daemon_service",
                CheckStatus::Pass,
                format!(
                    "{:?} owns {} through {}",
                    report.manager,
                    report.binary.display(),
                    report.definition.display()
                ),
            ));
        }
        Ok(report) => checks.push(check(
            "daemon_service",
            if global_binary_exists {
                CheckStatus::Error
            } else {
                CheckStatus::Warning
            },
            format!(
                "{:?} service is inactive or not stable (binary {}); run `hzr daemon service install`",
                report.manager,
                report.binary.display()
            ),
        )),
        Err(error) => checks.push(check(
            "daemon_service",
            if global_binary_exists {
                CheckStatus::Error
            } else {
                CheckStatus::Warning
            },
            error,
        )),
    }
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
    checks.extend(attest_active_bundle(config));

    let integration = integration_layout(config);
    let node = std::env::var_os("HZR_NODE")
        .map(PathBuf::from)
        .unwrap_or_else(|| config.engines.binary("node"));
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
                    let fts_only = health.engines.iter().any(|engine| {
                        engine.name == "icm"
                            && engine.state == EngineState::Degraded
                            && engine.detail.as_deref().is_some_and(|detail| {
                                detail.contains("FTS-only mode; embeddings are disabled")
                            })
                    });
                    let unexpected_degraded = health.engines.iter().any(|engine| {
                        engine.state == EngineState::Degraded
                            && !(engine.name == "icm"
                                && engine.detail.as_deref().is_some_and(|detail| {
                                    detail.contains("FTS-only mode; embeddings are disabled")
                                }))
                    });
                    let status = if !compatible || unexpected_degraded {
                        CheckStatus::Error
                    } else if fts_only {
                        CheckStatus::Warning
                    } else {
                        CheckStatus::Pass
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

fn attest_active_bundle(config: &Config) -> Vec<DoctorCheck> {
    const ARTIFACTS: [(&str, &str); 10] = [
        ("hzr", "bin/hzr"),
        ("hzrd", "bin/hzrd"),
        ("rtk", "engines/rtk"),
        ("grepai", "engines/grepai"),
        ("icm", "engines/icm"),
        ("node", "runtime/node/bin/node"),
        ("caveman_bridge", "engines/caveman-code/bridge.mjs"),
        ("contract", "share/hzr/HZR.md"),
        ("hzr_tdd_skill", "share/hzr/skills/hzr-tdd/SKILL.md"),
        (
            "hzr_tdd_patterns",
            "share/hzr/skills/hzr-tdd/references/testing-patterns.md",
        ),
    ];
    let Some(engine_directory) = &config.engines.directory else {
        return vec![check(
            "bundle_attestation",
            strict_status(config),
            "no self-contained engine directory is configured",
        )];
    };
    let Some(root) = engine_directory.parent() else {
        return vec![check(
            "bundle_attestation",
            CheckStatus::Error,
            format!(
                "engine directory has no bundle root: {}",
                engine_directory.display()
            ),
        )];
    };
    let manifest_path = root.join("share/hzr/BUNDLE_MANIFEST.sha256");
    let manifest = match std::fs::read_to_string(&manifest_path) {
        Ok(manifest) => manifest,
        Err(error) => {
            return vec![check(
                "bundle_attestation",
                strict_status(config),
                format!("failed to read {}: {error}", manifest_path.display()),
            )];
        }
    };
    let expected: BTreeMap<&str, &str> = manifest
        .lines()
        .filter_map(|line| {
            let (digest, path) = line.split_once(char::is_whitespace)?;
            Some((path.trim().trim_start_matches("./"), digest))
        })
        .collect();
    let canonical_root = match root.canonicalize() {
        Ok(root) => root,
        Err(error) => {
            return vec![check(
                "bundle_attestation",
                CheckStatus::Error,
                format!("failed to resolve {}: {error}", root.display()),
            )];
        }
    };

    ARTIFACTS
        .into_iter()
        .map(|(name, relative)| {
            let path = root.join(relative);
            let resolved = match path.canonicalize() {
                Ok(resolved) if resolved.starts_with(&canonical_root) => resolved,
                Ok(resolved) => {
                    return check(
                        format!("bundle_{name}"),
                        CheckStatus::Error,
                        format!(
                            "{} resolves outside the active bundle to {}",
                            path.display(),
                            resolved.display()
                        ),
                    );
                }
                Err(error) => {
                    return check(
                        format!("bundle_{name}"),
                        CheckStatus::Error,
                        format!("failed to resolve {}: {error}", path.display()),
                    );
                }
            };
            let Some(expected_digest) = expected.get(relative) else {
                return check(
                    format!("bundle_{name}"),
                    CheckStatus::Error,
                    format!("{relative} is absent from {}", manifest_path.display()),
                );
            };
            match std::fs::read(&resolved) {
                Ok(bytes) => {
                    let actual = hex::encode(Sha256::digest(bytes));
                    if actual == *expected_digest {
                        check(
                            format!("bundle_{name}"),
                            CheckStatus::Pass,
                            format!("{} sha256={actual}", path.display()),
                        )
                    } else {
                        check(
                            format!("bundle_{name}"),
                            CheckStatus::Error,
                            format!(
                                "{} digest mismatch: expected {}, got {actual}",
                                path.display(),
                                expected_digest
                            ),
                        )
                    }
                }
                Err(error) => check(
                    format!("bundle_{name}"),
                    CheckStatus::Error,
                    format!("failed to read {}: {error}", path.display()),
                ),
            }
        })
        .collect()
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

fn direct_icm_registration_detail(found: &[String]) -> String {
    format!(
        "direct ICM MCP registration bypasses HZR memory ownership in: {}; \
         run `hzr install --dry-run`, then `hzr install --force` to replace it; \
         `hzr mcp config --client <client>` only prints a manual snippet",
        found.join(", ")
    )
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
    use std::fs;

    use hzr_core::Config;
    use sha2::{Digest, Sha256};

    use super::{
        CheckStatus, attest_active_bundle, bounded, direct_icm_registration_detail,
        integration_layout,
    };

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

    #[test]
    fn test_direct_icm_repair_names_the_mutating_install_command() {
        let detail = direct_icm_registration_detail(&["codex".to_owned()]);

        assert!(detail.contains("`hzr install --force` to replace it"));
        assert!(detail.contains("`hzr mcp config --client <client>` only prints"));
    }

    #[test]
    fn test_bundle_attestation_detects_tampered_runtime() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path().join("current");
        let artifacts = [
            "bin/hzr",
            "bin/hzrd",
            "engines/rtk",
            "engines/grepai",
            "engines/icm",
            "runtime/node/bin/node",
            "engines/caveman-code/bridge.mjs",
            "share/hzr/HZR.md",
            "share/hzr/skills/hzr-tdd/SKILL.md",
            "share/hzr/skills/hzr-tdd/references/testing-patterns.md",
        ];
        let mut manifest = String::new();
        for relative in artifacts {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("artifact parent"))
                .expect("artifact directory");
            fs::write(&path, relative.as_bytes()).expect("artifact");
            let digest = hex::encode(Sha256::digest(relative.as_bytes()));
            manifest.push_str(&format!("{digest}  ./{relative}\n"));
        }
        fs::write(root.join("share/hzr/BUNDLE_MANIFEST.sha256"), manifest).expect("manifest");
        let mut config = Config::default();
        config.engines.directory = Some(root.join("engines"));

        let checks = attest_active_bundle(&config);
        assert!(checks.iter().all(|check| check.status == CheckStatus::Pass));
        assert!(
            checks
                .iter()
                .any(|check| check.name == "bundle_hzr_tdd_skill")
        );
        assert!(
            checks
                .iter()
                .any(|check| check.name == "bundle_hzr_tdd_patterns")
        );

        fs::write(root.join("engines/icm"), b"tampered").expect("tamper fixture");
        let checks = attest_active_bundle(&config);
        let icm = checks
            .iter()
            .find(|check| check.name == "bundle_icm")
            .expect("ICM attestation");
        assert_eq!(icm.status, CheckStatus::Error);
        assert!(icm.detail.contains("digest mismatch"));
    }
}
