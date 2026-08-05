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

fn hook_ownership_check(status: adoption::HookStatus) -> DoctorCheck {
    if status.conflict || status.hzr_entries > 3 {
        check(
            "hook_ownership",
            CheckStatus::Error,
            format!(
                "HZR={} RTK={}; exactly three HZR handlers and zero RTK handlers are allowed",
                status.hzr_entries, status.rtk_entries
            ),
        )
    } else if status.installed {
        check(
            "hook_ownership",
            CheckStatus::Pass,
            "one HZR dispatcher, one SessionStart initializer, and one PostToolUse observer",
        )
    } else {
        check(
            "hook_ownership",
            CheckStatus::Warning,
            format!(
                "HZR={} RTK={}; run `hzr install --dry-run`",
                status.hzr_entries, status.rtk_entries
            ),
        )
    }
}

pub async fn doctor(config_path: &Path, config: &Config, workspace: &Path) -> DoctorReport {
    let mut checks = Vec::new();
    match adoption::default_settings_path().and_then(|path| adoption::status(&path)) {
        Ok(status) => checks.push(hook_ownership_check(status)),
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
    // Ownership is not enough: a server HZR owns can still be bound to a directory that is
    // not the project, which is invisible until a recall comes back empty. Claude Code is
    // audited but never written by install, so a missing HZR MCP there is its own check —
    // otherwise hooks can look healthy while `hzr mcp status` shows registered=false.
    match client_config::status_all() {
        Ok(statuses) => {
            checks.push(workspace_binding_check(&statuses));
            checks.push(claude_code_mcp_check(&statuses));
        }
        Err(error) => {
            checks.push(check("client_mcp_workspace", CheckStatus::Warning, &error));
            checks.push(check("claude_code_mcp", CheckStatus::Warning, error));
        }
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
            let duplicate_detail = discovered
                .duplicate_index_dirs
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            checks.push(if discovered.duplicate_index_dirs.is_empty() {
                check("grepai_duplicates", CheckStatus::Pass, "none found")
            } else if matches!(
                discovered.require_single_index(),
                Err(hzr_index::IndexError::DuplicateIndexes { .. })
            ) {
                check("grepai_duplicates", CheckStatus::Error, duplicate_detail)
            } else {
                check("grepai_duplicates", CheckStatus::Warning, duplicate_detail)
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

/// Claude Code's state file is audited, never rewritten by `hzr install`. When HZR MCP is
/// absent there, hooks can still look healthy while the agent has no native HZR tools —
/// the same gap `hzr mcp status` already shows as `claude-code registered=false`.
fn claude_code_mcp_check(statuses: &[client_config::ClientMcpStatus]) -> DoctorCheck {
    match statuses
        .iter()
        .find(|status| status.client == client_config::Client::ClaudeCode)
    {
        Some(status) if status.registered => check(
            "claude_code_mcp",
            CheckStatus::Pass,
            format!("HZR MCP registered at {}", status.path.display()),
        ),
        Some(status) => check(
            "claude_code_mcp",
            CheckStatus::Warning,
            format!(
                "HZR MCP is not registered for Claude Code ({}); {}",
                status.path.display(),
                client_config::Client::ClaudeCode.direct_icm_remediation()
            ),
        ),
        None => check(
            "claude_code_mcp",
            CheckStatus::Warning,
            format!(
                "Claude Code MCP status unavailable; {}",
                client_config::Client::ClaudeCode.direct_icm_remediation()
            ),
        ),
    }
}

/// Report registered MCP servers whose project namespace is decided by the client's working
/// directory rather than pinned.
///
/// A registration used to be judged only on existing, which hid the worst binding failure
/// there is: the Claude desktop app launches MCP servers from `/`, so an unpinned server
/// wrote every memory into the namespace of the filesystem root while looking healthy. This
/// is a warning and not an error because an unpinned server bound to a real repository still
/// works — it is the *silence* that was wrong, not the configuration in every case.
fn workspace_binding_check(statuses: &[client_config::ClientMcpStatus]) -> DoctorCheck {
    let unpinned: Vec<&str> = statuses
        .iter()
        .filter(|status| status.registered && status.pinned_workspace.is_none())
        .map(|status| status.client.as_str())
        .collect();

    if unpinned.is_empty() {
        return check(
            "client_mcp_workspace",
            CheckStatus::Pass,
            "every registered MCP server pins its project workspace",
        );
    }

    check(
        "client_mcp_workspace",
        CheckStatus::Warning,
        format!(
            "{} registered without `--workspace`, so the memory namespace comes from the \
             directory the client launches from; the desktop app uses `/` and Codex uses a \
             per-session directory, and stores made there are unreachable from the project. \
             Re-register with `hzr mcp config --client <client> --workspace <dir>`",
            unpinned.join(", ")
        ),
    )
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

/// Each entry already carries the remediation for its own client, because they differ: HZR
/// rewrites Codex and the desktop app, and must never rewrite Claude Code's own state file.
/// A single generic instruction here would be wrong for whichever client it did not fit.
fn direct_icm_registration_detail(found: &[String]) -> String {
    format!(
        "direct ICM MCP registration bypasses HZR memory ownership in: {}",
        found.join("; ")
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

    use crate::client_config::{Client, ClientMcpStatus};

    use super::{
        CheckStatus, attest_active_bundle, bounded, claude_code_mcp_check,
        direct_icm_registration_detail, hook_ownership_check, integration_layout,
        workspace_binding_check,
    };

    #[test]
    fn test_doctor_accepts_the_dispatch_init_and_observer_hooks() {
        let status = crate::adoption::HookStatus {
            settings_path: "/tmp/settings.json".into(),
            hzr_entries: 3,
            rtk_entries: 0,
            external_icm_entries: 0,
            installed: true,
            conflict: false,
        };

        let check = hook_ownership_check(status);
        assert_eq!(check.status, CheckStatus::Pass);
        assert!(check.detail.contains("PostToolUse observer"));
    }

    fn registration(client: Client, pinned: Option<&str>) -> ClientMcpStatus {
        ClientMcpStatus {
            client,
            path: std::path::PathBuf::from("/tmp/config"),
            config_exists: true,
            registered: true,
            command: Some("/opt/hzr/bin/hzr".into()),
            args: vec!["mcp".into(), "serve".into()],
            direct_icm_registrations: 0,
            pinned_workspace: pinned.map(str::to_owned),
            lifecycle: "client_managed_stdio",
            started_by_init: false,
        }
    }

    /// An unpinned registration means the memory namespace is whatever directory the client
    /// launched from. That produced the worst observed failure — the desktop app launches
    /// from `/`, so its stores were unreachable from the repository they described — and
    /// nothing reported it, because a registration was judged only on being present.
    #[test]
    fn test_doctor_reports_an_unpinned_client_workspace() {
        let pinned = workspace_binding_check(&[registration(
            Client::ClaudeDesktop,
            Some("/Users/andrew/code/app"),
        )]);
        assert_eq!(pinned.status, CheckStatus::Pass);

        let unpinned = workspace_binding_check(&[registration(Client::ClaudeDesktop, None)]);
        assert_eq!(unpinned.status, CheckStatus::Warning);
        assert!(
            unpinned.detail.contains("--workspace"),
            "the warning must name the fix, got: {}",
            unpinned.detail
        );
        assert!(
            unpinned.detail.contains("claude-desktop"),
            "the warning must name the client, got: {}",
            unpinned.detail
        );
    }

    /// A client with no `hzr` registration at all is not an unpinned one; reporting it here
    /// would duplicate the ownership check and bury the real signal.
    #[test]
    fn test_an_unregistered_client_is_not_reported_as_unpinned() {
        let mut status = registration(Client::Codex, None);
        status.registered = false;

        assert_eq!(
            workspace_binding_check(&[status]).status,
            CheckStatus::Pass,
            "only registered servers can have a workspace binding"
        );
    }

    /// Hooks alone do not register the Claude Code MCP server. `hzr mcp status` can report
    /// `claude-code registered=false` while hook ownership looks healthy, and doctor used to
    /// stay silent because the only Claude Code remediation path was the direct-ICM check.
    /// Install never writes that file, so the warning must name `claude mcp add`.
    #[test]
    fn test_doctor_warns_when_claude_code_hzr_mcp_is_absent() {
        let mut absent = registration(Client::ClaudeCode, None);
        absent.registered = false;
        absent.path = std::path::PathBuf::from("/tmp/.claude.json");

        let check = claude_code_mcp_check(&[absent]);
        assert_eq!(check.name, "claude_code_mcp");
        assert_eq!(check.status, CheckStatus::Warning);
        assert!(
            check.detail.contains("claude mcp add"),
            "must name the CLI that mutates Claude Code state, got: {}",
            check.detail
        );
        assert!(
            check.detail.contains("/tmp/.claude.json"),
            "must name the audited path, got: {}",
            check.detail
        );

        let present = claude_code_mcp_check(&[registration(
            Client::ClaudeCode,
            Some("/Users/andrew/code/app"),
        )]);
        assert_eq!(present.status, CheckStatus::Pass);
    }

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

    /// The repair instruction must be the one that actually mutates the file, not the one that
    /// prints a snippet — and it differs by client, because HZR rewrites Codex and the desktop
    /// app but must never rewrite Claude Code's own state file. A single generic instruction
    /// was wrong for whichever client it did not fit, so each entry now carries its own and
    /// the detail line only joins them.
    #[test]
    fn test_direct_icm_repair_names_the_command_that_fits_each_client() {
        assert!(
            Client::Codex
                .direct_icm_remediation()
                .contains("`hzr install --force`")
        );
        assert!(
            Client::Codex
                .direct_icm_remediation()
                .contains("only prints a snippet")
        );
        assert!(
            Client::ClaudeCode
                .direct_icm_remediation()
                .contains("claude mcp remove icm"),
            "HZR never writes this file, so `hzr install` cannot be the fix"
        );

        let detail = direct_icm_registration_detail(&[
            format!(
                "codex (/x, 1 registration(s) — {})",
                Client::Codex.direct_icm_remediation()
            ),
            format!(
                "claude-code (/y, 1 registration(s) — {})",
                Client::ClaudeCode.direct_icm_remediation()
            ),
        ]);
        assert!(detail.contains("`hzr install --force`"));
        assert!(detail.contains("claude mcp remove icm"));
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
