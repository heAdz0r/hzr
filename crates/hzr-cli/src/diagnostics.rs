use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use hzr_agent::{IntegrationLayout, preflight};
use hzr_core::{
    Config, DEFAULT_FIDELITY_OPERATION_ALLOWANCE, DEFAULT_FIDELITY_TOKEN_ALLOWANCE, locked_engines,
};
use hzr_index::{
    Deadlines, IndexGeneration, IndexMigrationOutcome, IndexPlacement, IndexStatus, Workspace,
    migrate_legacy_index, registered_workspaces,
};
use hzr_protocol::{EngineState, PROTOCOL_VERSION};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tokio::time::timeout;

use crate::cli::ServiceCommand;
use crate::client::DaemonClient;
use crate::fleet_exemption;
use crate::{
    activation, adoption, client_config, foreign, hook_runner, instructions, prefix, service,
};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repair: Option<IndexMigrationOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fidelity_reconcile: Option<hzr_protocol::FidelityReconcileReceipt>,
}

pub async fn repair_legacy_index(
    config: &Config,
    workspace: &Path,
) -> hzr_index::Result<Option<IndexMigrationOutcome>> {
    let deadlines = Deadlines::default();
    let discovered = Workspace::discover_managed(
        workspace,
        Path::new("git"),
        &config.data_dir,
        deadlines.version,
    )
    .await?;
    if !matches!(
        discovered.placement()?,
        IndexPlacement::LegacyProject { .. }
    ) {
        return Ok(None);
    }
    migrate_legacy_index(
        workspace,
        Path::new("git"),
        &config.data_dir,
        deadlines.version,
    )
    .await
    .map(Some)
}

fn hook_ownership_check(status: adoption::HookStatus) -> DoctorCheck {
    if status.conflict || status.hzr_entries > 6 {
        check(
            "hook_ownership",
            CheckStatus::Error,
            format!(
                "HZR={} RTK={}; exactly six HZR handlers and zero RTK handlers are allowed",
                status.hzr_entries, status.rtk_entries
            ),
        )
    } else if status.installed {
        check(
            "hook_ownership",
            CheckStatus::Pass,
            "native-aware dispatcher, SessionStart, observer, prompt nudge, and bounded stop scorecards",
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

fn fidelity_allowance_check() -> DoctorCheck {
    check(
        "fidelity_allowance",
        CheckStatus::Pass,
        format!(
            "per-session hatch allowance active: {} operations or {} delivered tokens; current-session usage appears in the Stop scorecard",
            DEFAULT_FIDELITY_OPERATION_ALLOWANCE, DEFAULT_FIDELITY_TOKEN_ALLOWANCE
        ),
    )
}

fn fidelity_durability_check(config: &Config) -> DoctorCheck {
    let directory = config.data_dir.join("ledger/fidelity-pending");
    match hzr_daemon::inspect_fidelity_pending(&directory) {
        Ok(status) => fidelity_durability_status_check(&directory, status),
        Err(error) => check(
            "fidelity_durability",
            CheckStatus::Error,
            format!("cannot inspect {}: {error}", directory.display()),
        ),
    }
}

fn fidelity_durability_status_check(
    directory: &Path,
    status: hzr_daemon::FidelityDurabilityStatus,
) -> DoctorCheck {
    if status.healthy() && status.reserved == 0 {
        return check(
            "fidelity_durability",
            CheckStatus::Pass,
            "no pending fidelity reservations, unknown executions, replay backlog, or corrupt records",
        );
    }
    if status.healthy() {
        return check(
            "fidelity_durability",
            CheckStatus::Warning,
            format!(
                "{} provably pre-execution reservation(s) are pending in {}; stale reservations auto-expire after 5 minutes",
                status.reserved,
                directory.display()
            ),
        );
    }
    check(
        "fidelity_durability",
        CheckStatus::Error,
        format!(
            "reserved={}, executing_unknown={}, executed_pending_replay={}, corrupt={} in {}; unknown_ids={}; never retry or delete an unknown execution because it may already have been billed - reconcile it with `hzr doctor --resolve-fidelity <ID> --acknowledge-executed` (records zero unmeasured tokens) or only after proof use `hzr doctor --resolve-fidelity <ID> --prove-not-executed`; restart hzrd to idempotently replay executed records; preserve corrupt records because corruption blocks new fidelity execution",
            status.reserved,
            status.executing_unknown,
            status.executed_pending_replay,
            status.corrupt,
            directory.display(),
            status.unknown_reservation_ids.join(",")
        ),
    )
}

fn instruction_health_check(
    name: &str,
    surface: instructions::Surface,
    path: &Path,
) -> DoctorCheck {
    match instructions::audit(surface, path) {
        Ok(report) if report.healthy() => check(name, CheckStatus::Pass, report.path.display()),
        Ok(report) => {
            let mut reasons = Vec::new();
            let mut managed_repair_needed = false;
            if !report.installed {
                reasons.push("HZR contract block is absent".to_owned());
                managed_repair_needed = true;
            }
            if report.installed && !report.current {
                reasons.push("managed routing policy is stale".to_owned());
                managed_repair_needed = true;
            }
            if report.installed && !report.contract_readable {
                managed_repair_needed = true;
                reasons.push(match &report.contract_path {
                    Some(contract) => {
                        format!("referenced contract {} is unreadable", contract.display())
                    }
                    None => "block references no contract asset".to_owned(),
                });
            }
            if !report.conflicting_mandates.is_empty() {
                reasons.push(format!(
                    "directives still active outside the managed block: {}",
                    report.conflicting_mandates.join(", ")
                ));
            }
            let remediation = match (
                managed_repair_needed,
                report.conflicting_mandates.is_empty(),
            ) {
                (true, true) => "run `hzr init --if-needed`",
                (true, false) => {
                    "run `hzr init --if-needed` for the managed block, then manually edit the listed user-authored lines and re-run `hzr doctor`; HZR will not rewrite those lines"
                }
                (false, false) => {
                    "manually edit the listed user-authored lines and re-run `hzr doctor`; `hzr init` intentionally will not rewrite those lines"
                }
                (false, true) => "re-run `hzr doctor`",
            };
            check(
                name,
                CheckStatus::Error,
                format!(
                    "{}: {}; {remediation}",
                    report.path.display(),
                    reasons.join("; ")
                ),
            )
        }
        Err(error) => check(name, CheckStatus::Warning, error),
    }
}

fn workspace_instruction_health_check(
    name: &str,
    surface: instructions::Surface,
    path: &Path,
) -> DoctorCheck {
    match instructions::audit(surface, path) {
        Ok(report) if !report.installed => check(
            name,
            CheckStatus::Warning,
            format!(
                "{}: managed project contract is absent; run `hzr init --if-needed`",
                path.display()
            ),
        ),
        _ => instruction_health_check(name, surface, path),
    }
}

fn fleet_instruction_health_checks(config: &Config, current_workspace: &Path) -> Vec<DoctorCheck> {
    let snapshot = registered_workspaces(&config.data_dir);
    let mut checks = Vec::new();
    let mut stale_paths = Vec::new();
    let mut conflict_count = 0_usize;
    for warning in snapshot.warnings {
        checks.push(check(
            "fleet_instruction_registry",
            CheckStatus::Warning,
            format!("{}: {}", warning.path.display(), warning.detail),
        ));
    }
    let mut waived_projects = Vec::new();
    for registration in snapshot.registrations {
        if registration.root == current_workspace {
            continue;
        }
        // A waiver is read once per project and reported, never inferred from the path.
        let exemptions = match fleet_exemption::load(&registration.root) {
            Ok(exemptions) => exemptions,
            Err(error) => {
                checks.push(check(
                    "fleet_instruction_exemption",
                    CheckStatus::Error,
                    format!("{error:#}; an unauditable waiver is not honoured"),
                ));
                fleet_exemption::FleetExemptions::default()
            }
        };
        for (surface, path) in activation::local_instruction_paths(&registration.root) {
            let audit = match instructions::audit(surface, &path) {
                Ok(audit) => audit,
                Err(error) => {
                    checks.push(check(
                        format!("fleet_{}_instructions", surface.as_str()),
                        CheckStatus::Warning,
                        format!("{}: {error}", path.display()),
                    ));
                    continue;
                }
            };
            if !audit.installed && audit.conflicting_mandates.is_empty() {
                continue;
            }
            // A declared waiver removes only the rules it names. Anything left is still a
            // finding, so a policy file cannot broaden itself into a blanket opt-out.
            let unwaived: Vec<&String> = audit
                .conflicting_mandates
                .iter()
                .filter(|conflict| !exemptions.covers(conflict))
                .collect();
            if unwaived.is_empty() {
                if !audit.conflicting_mandates.is_empty() {
                    waived_projects.push(format!(
                        "{} ({} directive(s); {})",
                        path.display(),
                        audit.conflicting_mandates.len(),
                        exemptions.summary()
                    ));
                }
                // A waiver covers directives, never the managed block itself: a stale or
                // unreadable contract stays a finding even in an exempt project.
                if !audit.installed || !audit.current || !audit.contract_readable {
                    stale_paths.push(path);
                }
                continue;
            }
            conflict_count = conflict_count.saturating_add(1);
            if conflict_count > 32 {
                continue;
            }
            let mut finding = instruction_health_check(
                &format!("fleet_{}_instructions", surface.as_str()),
                surface,
                &path,
            );
            if finding.status != CheckStatus::Pass {
                finding.detail.push_str(&format!(
                    "; the managed block can be refreshed with `cd {} && hzr init --if-needed`; manually apply each listed line remediation because direct user-authored conflict lines are never auto-rewritten",
                    registration.root.display()
                ));
            }
            checks.push(finding);
        }
    }
    if !stale_paths.is_empty() {
        stale_paths.sort();
        let examples = stale_paths
            .iter()
            .take(8)
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        checks.push(check(
            "fleet_stale_managed_contracts",
            CheckStatus::Error,
            format!(
                "{} registered instruction files have a stale/unreadable managed block; examples: {}; reconcile each owning workspace explicitly with `hzr init --if-needed` ({} more not expanded)",
                stale_paths.len(),
                examples,
                stale_paths.len().saturating_sub(8)
            ),
        ));
    }
    if !waived_projects.is_empty() {
        waived_projects.sort();
        checks.push(check(
            "fleet_instruction_exemptions",
            CheckStatus::Warning,
            format!(
                "{} instruction file(s) keep a direct engine directive under a declared `{}` waiver, so they are reported rather than silently passed: {}",
                waived_projects.len(),
                fleet_exemption::POLICY_RELATIVE_PATH,
                waived_projects.join("; ")
            ),
        ));
    }
    if conflict_count > 32 {
        checks.push(check(
            "fleet_instruction_conflicts_truncated",
            CheckStatus::Error,
            format!(
                "{} additional registered instruction files contain direct engine/native mandates; output is capped at 32 detailed files",
                conflict_count - 32
            ),
        ));
    }
    if checks.is_empty() {
        checks.push(check(
            "fleet_instructions",
            CheckStatus::Pass,
            "registered workspace instruction files have no stale managed block or direct engine mandate",
        ));
    }
    checks
}

pub async fn doctor(config_path: &Path, config: &Config, workspace: &Path) -> DoctorReport {
    let mut checks = Vec::new();
    checks.push(install_transaction_check(config_path, config, workspace));
    match adoption::default_settings_path().and_then(|path| adoption::status(&path)) {
        Ok(status) => checks.push(hook_ownership_check(status)),
        Err(error) => checks.push(check("hook_ownership", CheckStatus::Warning, error)),
    }
    match adoption::default_settings_path().and_then(|path| adoption::status(&path)) {
        Ok(status) => checks.push(match status.native_tool_mode {
            Some(adoption::NativeToolMode::Observe) => check(
                "native_tool_mode",
                CheckStatus::Warning,
                "observe mode leaves native Read/Grep unsteered; run `hzr install --force --native-tool-mode steer`",
            ),
            Some(mode) => check(
                "native_tool_mode",
                CheckStatus::Pass,
                mode.as_str().to_owned(),
            ),
            None => check(
                "native_tool_mode",
                CheckStatus::Warning,
                "native tool policy is not installed; run `hzr install --force`",
            ),
        }),
        Err(error) => checks.push(check("native_tool_mode", CheckStatus::Warning, error)),
    }
    checks.push(fidelity_allowance_check());
    checks.push(fidelity_durability_check(config));
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
    // rewrite Bash. All-project activation uses the two global surfaces. Selected
    // activation uses workspace-local surfaces so disabled projects stay untouched.
    let workspace_enabled = activation::is_enabled(config, workspace)
        .await
        .unwrap_or(false);
    match config.activation.mode {
        hzr_core::ActivationMode::All => {
            for surface in [instructions::Surface::Claude, instructions::Surface::Codex] {
                let name = match surface {
                    instructions::Surface::Claude => "claude_instructions",
                    instructions::Surface::Codex => "codex_instructions",
                };
                match surface.default_path() {
                    Ok(path) => checks.push(instruction_health_check(name, surface, &path)),
                    Err(error) => checks.push(check(name, CheckStatus::Warning, error)),
                }
            }
            for (surface, path) in activation::local_instruction_paths(workspace) {
                checks.push(workspace_instruction_health_check(
                    match surface {
                        instructions::Surface::Claude => "workspace_claude_instructions",
                        instructions::Surface::Codex => "workspace_codex_instructions",
                    },
                    surface,
                    &path,
                ));
            }
        }
        hzr_core::ActivationMode::Selected if workspace_enabled => {
            for (surface, path) in activation::local_instruction_paths(workspace) {
                checks.push(workspace_instruction_health_check(
                    match surface {
                        instructions::Surface::Claude => "workspace_claude_instructions",
                        instructions::Surface::Codex => "workspace_codex_instructions",
                    },
                    surface,
                    &path,
                ));
            }
        }
        hzr_core::ActivationMode::Selected => checks.push(check(
            "workspace_instructions",
            CheckStatus::Pass,
            "workspace is disabled; no local HZR contract is required",
        )),
    }
    checks.extend(fleet_instruction_health_checks(config, workspace));
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
        Ok(mut statuses) => {
            let project_codex_path = client_config::project_codex_path(workspace);
            match client_config::status(client_config::Client::Codex, &project_codex_path) {
                Ok(project_codex) if project_codex.registered => {
                    // Trusted project configuration has precedence over the user-global Codex
                    // registration. Audit the effective pin instead of reporting a stale global
                    // fallback that Codex will not launch in this workspace.
                    statuses =
                        effective_workspace_mcp_statuses(statuses, Some(project_codex), None);
                }
                Ok(_) => {}
                Err(error) => checks.push(check(
                    "project_codex_mcp",
                    CheckStatus::Error,
                    format!(
                        "invalid project Codex MCP {}: {error}",
                        project_codex_path.display()
                    ),
                )),
            }
            // Claude Code stores its per-project servers inside the same user config. The
            // project scope wins for this workspace; the user-global entry is only a fallback.
            match client_config::claude_code_project_status(workspace) {
                Ok(project_claude_code) => {
                    statuses =
                        effective_workspace_mcp_statuses(statuses, None, project_claude_code);
                }
                Err(error) => checks.push(check(
                    "project_claude_code_mcp",
                    CheckStatus::Error,
                    format!("invalid project Claude Code MCP scope: {error}"),
                )),
            }
            checks.push(workspace_binding_check(&statuses, workspace));
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
            // Ownership alone does not mean semantic search works: cold/missing artifacts
            // only showed up in context-plan warnings before this probe existed.
            match index_status_snapshot(&discovered) {
                Ok(status) => checks.push(index_readiness_check(&status)),
                Err(error) => checks.push(check("index_readiness", CheckStatus::Warning, error)),
            }
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
        repair: None,
        fidelity_reconcile: None,
    }
}

fn effective_workspace_mcp_statuses(
    mut global: Vec<client_config::ClientMcpStatus>,
    project_codex: Option<client_config::ClientMcpStatus>,
    project_claude_code: Option<client_config::ClientMcpStatus>,
) -> Vec<client_config::ClientMcpStatus> {
    if let Some(project_codex) = project_codex.filter(|status| status.registered) {
        global.retain(|status| status.client != client_config::Client::Codex);
        global.push(project_codex);
    }
    if let Some(project_claude_code) = project_claude_code.filter(|status| status.registered) {
        global.retain(|status| status.client != client_config::Client::ClaudeCode);
        global.push(project_claude_code);
    }
    global
}

fn install_transaction_check(config_path: &Path, config: &Config, workspace: &Path) -> DoctorCheck {
    let path = config.data_dir.join("runtime/install-transaction.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return check(
                "install_transaction",
                CheckStatus::Warning,
                "no install transaction journal; run `hzr install --dry-run` to inspect desired state",
            );
        }
        Err(error) => return check("install_transaction", CheckStatus::Error, error),
    };
    let journal: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(journal) => journal,
        Err(error) => {
            return check(
                "install_transaction",
                CheckStatus::Error,
                format!("invalid install journal {}: {error}", path.display()),
            );
        }
    };
    let expected_stages = crate::INSTALL_STAGES
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let planned = journal["planned_stages"].as_array().map(|stages| {
        stages
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
    });
    let completed = journal["completed_stages"].as_array().map(|stages| {
        stages
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
    });
    let valid_plan = planned.as_ref().is_some_and(|stages| {
        stages.len() == crate::INSTALL_STAGES.len()
            && stages
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                == expected_stages
    });
    let valid_completed = completed.as_ref().is_some_and(|stages| {
        stages.len() == journal["completed_stages"].as_array().map_or(0, Vec::len)
            && stages
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len()
                == stages.len()
            && stages.iter().all(|stage| expected_stages.contains(stage))
    });
    let valid_digest = journal["plan_sha256"].as_str().is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    });
    let state = journal["state"].as_str();
    let owner_config = journal["config_path"].as_str();
    let owner_workspace = journal["workspace"].as_str();
    if journal["schema_version"].as_u64() != Some(u64::from(crate::INSTALL_JOURNAL_SCHEMA_VERSION))
        || !matches!(state, Some("applying" | "recovering" | "complete"))
        || !valid_plan
        || !valid_completed
        || !valid_digest
        || owner_config.is_none()
        || owner_workspace.is_none()
    {
        return check(
            "install_transaction",
            CheckStatus::Error,
            format!(
                "install journal {} has invalid schema or receipt metadata",
                path.display()
            ),
        );
    }
    let owner_config = PathBuf::from(owner_config.expect("validated owner config"));
    let owner_workspace = PathBuf::from(owner_workspace.expect("validated owner workspace"));
    let current_config = if config_path.is_absolute() {
        config_path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(config_path)
    };
    let current_workspace =
        std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let journal_workspace =
        std::fs::canonicalize(&owner_workspace).unwrap_or_else(|_| owner_workspace.clone());
    if owner_config != current_config || journal_workspace != current_workspace {
        return check(
            "install_transaction",
            CheckStatus::Error,
            format!(
                "install journal {} belongs to config {} workspace {}, not current config {} workspace {}",
                path.display(),
                owner_config.display(),
                owner_workspace.display(),
                current_config.display(),
                current_workspace.display()
            ),
        );
    }
    if state == Some("complete") {
        let completed_all = completed.as_ref().is_some_and(|stages| {
            stages.len() == crate::INSTALL_STAGES.len()
                && stages
                    .iter()
                    .copied()
                    .collect::<std::collections::HashSet<_>>()
                    == expected_stages
        });
        if !completed_all {
            return check(
                "install_transaction",
                CheckStatus::Error,
                format!(
                    "complete install journal {} is missing stage receipts",
                    path.display()
                ),
            );
        }
        return check(
            "install_transaction",
            CheckStatus::Pass,
            format!("complete forward-recovery journal at {}", path.display()),
        );
    }
    check(
        "install_transaction",
        CheckStatus::Error,
        format!(
            "incomplete forward-recovery install transaction at {}; recover the same desired state with `hzr --config {} install --force --workspace {}`",
            path.display(),
            shell_quote_diagnostic(&owner_config.to_string_lossy()),
            shell_quote_diagnostic(&owner_workspace.to_string_lossy())
        ),
    )
}

fn shell_quote_diagnostic(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn attest_active_bundle(config: &Config) -> Vec<DoctorCheck> {
    const ARTIFACTS: [(&str, &str); 11] = [
        ("hzr", "bin/hzr"),
        ("hzrd", "bin/hzrd"),
        ("rtk", "engines/rtk"),
        ("grepai", "engines/grepai"),
        ("icm", "engines/icm"),
        ("node", "runtime/node/bin/node"),
        ("caveman_bridge", "engines/caveman-code/bridge.mjs"),
        ("contract", "share/hzr/HZR.md"),
        ("agent_capabilities", "share/hzr/agent-capabilities.json"),
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

/// Снимок артефактов индекса без запуска grepai — doctor только читает файлы.
fn index_status_snapshot(workspace: &Workspace) -> Result<IndexStatus, String> {
    let initialized = workspace.index.config.is_file();
    Ok(IndexStatus {
        placement: workspace.placement().map_err(|error| error.to_string())?,
        initialized,
        vectors_present: workspace.index.vectors.is_file(),
        symbols_present: workspace.index.symbols.is_file(),
        repository_graph_present: workspace.index.repository_graph.is_file(),
        duplicate_index_dirs: workspace.duplicate_index_dirs.clone(),
        generation: initialized
            .then(|| IndexGeneration::read(workspace))
            .transpose()
            .map_err(|error| error.to_string())?,
    })
}

/// Готовность семантического индекса для doctor: init + vectors/symbols/graph.
///
/// Совпадает с тем, что оператор видит в `hzr index status`, и даёт remediation до
/// первого `context plan` warning о cold warm-up.
fn index_readiness_check(status: &IndexStatus) -> DoctorCheck {
    if !status.initialized {
        return check(
            "index_readiness",
            CheckStatus::Warning,
            "semantic index is not initialized; run `hzr index init --workspace .` \
             (the first semantic query also starts the hzrd watcher warm-up)",
        );
    }

    let mut missing = Vec::new();
    if !status.vectors_present {
        missing.push("vectors");
    }
    if !status.symbols_present {
        missing.push("symbols");
    }
    if !status.repository_graph_present {
        missing.push("repository_graph");
    }
    if !missing.is_empty() {
        return check(
            "index_readiness",
            CheckStatus::Warning,
            format!(
                "semantic index is not ready (missing {}); wait for the hzrd watcher or run a \
                 semantic query to warm, then check with `hzr index status --workspace .`",
                missing.join(", ")
            ),
        );
    }

    check(
        "index_readiness",
        CheckStatus::Pass,
        "ready; vectors, symbols, and repository graph are present",
    )
}

/// Report registered MCP servers whose project namespace is decided by the client's working
/// directory rather than pinned.
///
/// A registration used to be judged only on existing, which hid the worst binding failure
/// there is: the Claude desktop app launches MCP servers from `/`, so an unpinned server
/// wrote every memory into the namespace of the filesystem root while looking healthy. This
/// is a warning and not an error because an unpinned server bound to a real repository still
/// works — it is the *silence* that was wrong, not the configuration in every case.
fn workspace_binding_check(
    statuses: &[client_config::ClientMcpStatus],
    workspace: &Path,
) -> DoctorCheck {
    let expected = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let mismatched = statuses
        .iter()
        .filter(|status| status.registered)
        .filter_map(|status| {
            let pinned = status.pinned_workspace.as_deref()?;
            let pinned_path = PathBuf::from(pinned);
            let actual = std::fs::canonicalize(&pinned_path).unwrap_or(pinned_path);
            (actual != expected).then(|| format!("{}={}", status.client.as_str(), actual.display()))
        })
        .collect::<Vec<_>>();
    if !mismatched.is_empty() {
        return check(
            "client_mcp_workspace",
            CheckStatus::Error,
            format!(
                "registered MCP workspace mismatch: expected {}; found {}. Re-register every client for the current project before using project-scoped tools",
                expected.display(),
                mismatched.join(", ")
            ),
        );
    }
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
             Re-register with `hzr mcp config --client <client> --workspace <dir> --apply`",
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
    use std::path::Path;

    use hzr_core::Config;
    use hzr_index::{Deadlines, IndexMigrationOutcome, IndexPlacement, IndexStatus, Workspace};
    use sha2::{Digest, Sha256};

    use crate::client_config::{Client, ClientMcpStatus};
    use crate::instructions::{self, Surface};

    use super::{
        CheckStatus, attest_active_bundle, bounded, claude_code_mcp_check,
        direct_icm_registration_detail, effective_workspace_mcp_statuses, fidelity_allowance_check,
        fidelity_durability_status_check, fleet_instruction_health_checks, hook_ownership_check,
        index_readiness_check, install_transaction_check, instruction_health_check,
        integration_layout, repair_legacy_index, workspace_binding_check,
        workspace_instruction_health_check,
    };

    #[test]
    fn acceptance_gate_doctor_renders_fidelity_allowance() {
        let result = fidelity_allowance_check();
        assert_eq!(result.status, CheckStatus::Pass);
        assert!(result.detail.contains("5 operations"));
        assert!(result.detail.contains("100000 delivered tokens"));
        assert!(result.detail.contains("Stop scorecard"));
    }

    #[test]
    fn doctor_distinguishes_reserved_unknown_replay_and_corrupt_fidelity_records() {
        let directory = Path::new("/data/ledger/fidelity-pending");
        let reserved = fidelity_durability_status_check(
            directory,
            hzr_daemon::FidelityDurabilityStatus {
                reserved: 1,
                ..Default::default()
            },
        );
        assert_eq!(reserved.status, CheckStatus::Warning);
        assert!(reserved.detail.contains("pre-execution"));
        assert!(reserved.detail.contains("auto-expire after 5 minutes"));

        let unsafe_pending = fidelity_durability_status_check(
            directory,
            hzr_daemon::FidelityDurabilityStatus {
                executing_unknown: 1,
                executed_pending_replay: 1,
                corrupt: 1,
                ..Default::default()
            },
        );
        assert_eq!(unsafe_pending.status, CheckStatus::Error);
        assert!(unsafe_pending.detail.contains("never retry or delete"));
        assert!(unsafe_pending.detail.contains("idempotently replay"));
        assert!(
            unsafe_pending
                .detail
                .contains("blocks new fidelity execution")
        );
    }

    #[test]
    fn acceptance_gate_doctor_rejects_stale_managed_instructions() {
        let fixture = tempfile::tempdir().expect("fixture directory");
        let contract = fixture.path().join("HZR.md");
        let target = fixture.path().join("AGENTS.md");
        fs::write(&contract, "contract").expect("contract fixture");
        instructions::install(Surface::Codex, &target, &contract, false, true)
            .expect("managed instruction fixture");
        let stale = fs::read_to_string(&target)
            .expect("managed instructions")
            .replace("raw` is forbidden", "raw` is preferred");
        fs::write(&target, stale).expect("stale instructions");

        let result = instruction_health_check("codex_instructions", Surface::Codex, &target);
        assert_eq!(result.status, CheckStatus::Error);
        assert!(result.detail.contains("managed routing policy is stale"));
        assert!(result.detail.contains("hzr init --if-needed"));
    }

    #[tokio::test]
    async fn acceptance_gate_doctor_audits_registered_workspace_direct_engine_mandates() {
        let fixture = tempfile::tempdir().expect("fixture directory");
        let current = fixture.path().join("current");
        let fleet = fixture.path().join("fleet");
        let contract = fixture.path().join("HZR.md");
        fs::create_dir_all(&current).expect("current workspace");
        fs::create_dir_all(&fleet).expect("fleet workspace");
        fs::write(&contract, "contract").expect("contract fixture");
        let config = Config {
            data_dir: fixture.path().join("data"),
            ..Default::default()
        };
        let workspace = Workspace::discover_managed(
            &fleet,
            Path::new("git"),
            &config.data_dir,
            Deadlines::default().version,
        )
        .await
        .expect("workspace discovery");
        workspace
            .ensure_managed_location()
            .expect("managed placement");
        workspace.register().expect("workspace registration");
        let target = fleet.join("CLAUDE.md");
        fs::write(
            &target,
            "# Rules\nUse Bash: grepai callers important_symbol\n",
        )
        .expect("user instruction fixture");
        instructions::install(Surface::Claude, &target, &contract, false, true)
            .expect("managed instruction fixture");

        let checks = fleet_instruction_health_checks(&config, &current);
        let finding = checks
            .iter()
            .find(|check| check.detail.contains("direct-grepai at line 2"))
            .expect("fleet conflict finding");
        assert_eq!(finding.status, CheckStatus::Error);
        assert!(finding.detail.contains(&target.display().to_string()));
        assert!(finding.detail.contains("remediation:"));
    }

    #[test]
    fn acceptance_gate_doctor_truthfully_marks_user_conflicts_as_manual_only() {
        let fixture = tempfile::tempdir().expect("fixture directory");
        let contract = fixture.path().join("HZR.md");
        let target = fixture.path().join("AGENTS.md");
        fs::write(&contract, "contract").expect("contract fixture");
        fs::write(&target, "Run `grepai trace important_symbol`.\n").expect("user rules");
        instructions::install(Surface::Codex, &target, &contract, false, true)
            .expect("managed instruction fixture");

        let result = instruction_health_check("codex_instructions", Surface::Codex, &target);
        assert_eq!(result.status, CheckStatus::Error);
        assert!(
            result
                .detail
                .contains("manually edit the listed user-authored lines")
        );
        assert!(
            result
                .detail
                .contains("`hzr init` intentionally will not rewrite those lines")
        );
        assert!(!result.detail.contains("run `hzr init --if-needed`"));
    }

    #[test]
    fn acceptance_gate_doctor_warns_for_missing_project_contract() {
        let fixture = tempfile::tempdir().expect("fixture directory");
        let target = fixture.path().join("AGENTS.md");
        let result = workspace_instruction_health_check(
            "workspace_codex_instructions",
            Surface::Codex,
            &target,
        );
        assert_eq!(result.status, CheckStatus::Warning);
        assert!(result.detail.contains("managed project contract is absent"));
        assert!(result.detail.contains("hzr init --if-needed"));
    }

    fn index_status(
        initialized: bool,
        vectors: bool,
        symbols: bool,
        repository_graph: bool,
    ) -> IndexStatus {
        IndexStatus {
            placement: IndexPlacement::Missing {
                project_entry: "/tmp/project/.grepai".into(),
                intended_directory: "/tmp/data/workspaces/x/.grepai".into(),
                managed: true,
            },
            initialized,
            vectors_present: vectors,
            symbols_present: symbols,
            repository_graph_present: repository_graph,
            duplicate_index_dirs: Vec::new(),
            generation: None,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_doctor_fix_migrates_one_legacy_index_with_a_backup() {
        let fixture = tempfile::tempdir().expect("fixture directory");
        let workspace = fixture.path().join("workspace");
        let legacy = workspace.join(".grepai");
        fs::create_dir_all(&legacy).expect("legacy index directory");
        fs::write(legacy.join("config.yaml"), "version: 1\n").expect("legacy config");
        fs::write(legacy.join("index.gob"), b"vectors").expect("legacy vectors");
        fs::write(legacy.join("symbols.gob"), b"symbols").expect("legacy symbols");
        let config = Config {
            data_dir: fixture.path().join("data"),
            ..Config::default()
        };
        config.ensure_layout().expect("HZR data layout");

        let outcome = repair_legacy_index(&config, &workspace)
            .await
            .expect("doctor repair")
            .expect("legacy repair outcome");
        assert!(matches!(outcome, IndexMigrationOutcome::Applied { .. }));
        let manifest = match outcome {
            IndexMigrationOutcome::Applied { manifest, .. }
            | IndexMigrationOutcome::AlreadyApplied { manifest, .. } => manifest,
        };

        assert!(
            fs::symlink_metadata(workspace.join(".grepai"))
                .expect("managed project link")
                .file_type()
                .is_symlink()
        );
        assert!(std::path::Path::new(&manifest.backup.display).is_dir());
    }

    /// Операторы не должны узнавать о холодном индексе только из warning'а context plan.
    #[test]
    fn test_doctor_warns_when_semantic_index_is_not_initialized() {
        let check = index_readiness_check(&index_status(false, false, false, false));
        assert_eq!(check.name, "index_readiness");
        assert_eq!(check.status, CheckStatus::Warning);
        assert!(
            check.detail.contains("hzr index init"),
            "remediation must name index init, got: {}",
            check.detail
        );
    }

    #[test]
    fn test_doctor_warns_when_semantic_index_artifacts_are_incomplete() {
        let check = index_readiness_check(&index_status(true, false, false, false));
        assert_eq!(check.status, CheckStatus::Warning);
        assert!(
            check.detail.contains("not ready"),
            "detail must say not ready, got: {}",
            check.detail
        );
        assert!(
            check.detail.contains("repository_graph")
                || check.detail.contains("vectors")
                || check.detail.contains("symbols"),
            "detail must name missing artifacts, got: {}",
            check.detail
        );
        assert!(
            check.detail.contains("hzrd") || check.detail.contains("warm"),
            "detail must point at watcher/warm remediation, got: {}",
            check.detail
        );
    }

    #[test]
    fn test_doctor_passes_when_semantic_index_artifacts_are_present() {
        let check = index_readiness_check(&index_status(true, true, true, true));
        assert_eq!(check.status, CheckStatus::Pass);
        assert!(check.detail.contains("ready") || check.detail.contains("present"));
    }

    #[test]
    fn test_doctor_accepts_the_dispatch_init_and_observer_hooks() {
        let status = crate::adoption::HookStatus {
            settings_path: "/tmp/settings.json".into(),
            hzr_entries: 6,
            rtk_entries: 0,
            external_icm_entries: 0,
            installed: true,
            conflict: false,
            native_tool_mode: Some(crate::adoption::NativeToolMode::Steer),
        };

        let check = hook_ownership_check(status);
        assert_eq!(check.status, CheckStatus::Pass);
        assert!(check.detail.contains("observer"));
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
        let workspace = std::path::Path::new("/Users/andrew/code/app");
        let pinned = workspace_binding_check(
            &[registration(
                Client::ClaudeDesktop,
                Some("/Users/andrew/code/app"),
            )],
            workspace,
        );
        assert_eq!(pinned.status, CheckStatus::Pass);

        let unpinned =
            workspace_binding_check(&[registration(Client::ClaudeDesktop, None)], workspace);
        assert_eq!(unpinned.status, CheckStatus::Warning);
        assert!(
            unpinned.detail.contains("--workspace") && unpinned.detail.contains("--apply"),
            "the warning must name the apply fix, got: {}",
            unpinned.detail
        );
        assert!(
            unpinned.detail.contains("claude-desktop"),
            "the warning must name the client, got: {}",
            unpinned.detail
        );
    }

    #[test]
    fn test_doctor_rejects_a_pinned_workspace_other_than_the_current_project() {
        let check = workspace_binding_check(
            &[registration(
                Client::Codex,
                Some("/Users/andrew/code/other"),
            )],
            std::path::Path::new("/Users/andrew/code/app"),
        );

        assert_eq!(check.status, CheckStatus::Error);
        assert!(check.detail.contains("workspace mismatch"));
        assert!(check.detail.contains("codex=/Users/andrew/code/other"));
        assert!(check.detail.contains("expected /Users/andrew/code/app"));
    }

    #[test]
    fn project_codex_registration_takes_precedence_over_stale_global_codex_pin() {
        let workspace = std::path::Path::new("/Users/andrew/code/app");
        let statuses = effective_workspace_mcp_statuses(
            vec![
                registration(Client::Codex, Some("/Users/andrew/code/other")),
                registration(Client::ClaudeDesktop, Some("/Users/andrew/code/app")),
            ],
            Some(registration(Client::Codex, Some("/Users/andrew/code/app"))),
            None,
        );
        let check = workspace_binding_check(&statuses, workspace);
        assert_eq!(check.status, CheckStatus::Pass);
    }

    #[test]
    fn project_claude_code_scope_takes_precedence_over_stale_global_claude_pin() {
        let workspace = std::path::Path::new("/Users/andrew/code/app");
        let statuses = effective_workspace_mcp_statuses(
            vec![
                registration(Client::ClaudeCode, Some("/Users/andrew/code/other")),
                registration(Client::Codex, Some("/Users/andrew/code/app")),
            ],
            None,
            Some(registration(
                Client::ClaudeCode,
                Some("/Users/andrew/code/app"),
            )),
        );
        let check = workspace_binding_check(&statuses, workspace);
        assert_eq!(
            check.status,
            CheckStatus::Pass,
            "the per-project Claude Code scope is what launches here: {}",
            check.detail
        );
    }

    #[test]
    fn install_journal_desired_state_fails_closed_and_names_exact_recovery() {
        let fixture = tempfile::tempdir().expect("fixture");
        let workspace = fixture.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let config_path = fixture.path().join("config.toml");
        let config = Config {
            data_dir: fixture.path().join("data"),
            ..Config::default()
        };
        config.ensure_layout().expect("layout");
        let journal_path = config.data_dir.join("runtime/install-transaction.json");
        let journal = serde_json::json!({
            "schema_version": crate::INSTALL_JOURNAL_SCHEMA_VERSION,
            "state": "recovering",
            "config_path": config_path,
            "workspace": workspace,
            "plan_sha256": "a".repeat(64),
            "planned_stages": crate::INSTALL_STAGES,
            "completed_stages": ["config", "workspace"],
            "attempt": 2
        });
        fs::write(
            &journal_path,
            serde_json::to_vec(&journal).expect("journal JSON"),
        )
        .expect("journal fixture");
        let incomplete = install_transaction_check(&config_path, &config, &workspace);
        assert_eq!(incomplete.status, CheckStatus::Error);
        assert!(incomplete.detail.contains("install --force --workspace"));
        assert!(
            incomplete
                .detail
                .contains(workspace.to_str().expect("workspace"))
        );

        let mut corrupt = journal;
        corrupt["schema_version"] = serde_json::json!(999);
        fs::write(
            &journal_path,
            serde_json::to_vec(&corrupt).expect("journal JSON"),
        )
        .expect("corrupt journal fixture");
        let corrupt = install_transaction_check(&config_path, &config, &workspace);
        assert_eq!(corrupt.status, CheckStatus::Error);
        assert!(
            corrupt
                .detail
                .contains("invalid schema or receipt metadata")
        );

        let incomplete_complete = serde_json::json!({
            "schema_version": crate::INSTALL_JOURNAL_SCHEMA_VERSION,
            "state": "complete",
            "config_path": config_path,
            "workspace": workspace,
            "plan_sha256": "b".repeat(64),
            "planned_stages": crate::INSTALL_STAGES,
            "completed_stages": ["config"],
            "attempt": 1
        });
        fs::write(
            &journal_path,
            serde_json::to_vec(&incomplete_complete).expect("journal JSON"),
        )
        .expect("incomplete complete journal");
        let missing = install_transaction_check(&config_path, &config, &workspace);
        assert_eq!(missing.status, CheckStatus::Error);
        assert!(missing.detail.contains("missing stage receipts"));

        let wrong_workspace = fixture.path().join("other-workspace");
        let wrong_owner = serde_json::json!({
            "schema_version": crate::INSTALL_JOURNAL_SCHEMA_VERSION,
            "state": "complete",
            "config_path": config_path,
            "workspace": wrong_workspace,
            "plan_sha256": "c".repeat(64),
            "planned_stages": crate::INSTALL_STAGES,
            "completed_stages": crate::INSTALL_STAGES,
            "attempt": 1
        });
        fs::write(
            &journal_path,
            serde_json::to_vec(&wrong_owner).expect("journal JSON"),
        )
        .expect("wrong owner journal");
        let wrong = install_transaction_check(&config_path, &config, &workspace);
        assert_eq!(wrong.status, CheckStatus::Error);
        assert!(wrong.detail.contains("not current config"));
    }

    /// A client with no `hzr` registration at all is not an unpinned one; reporting it here
    /// would duplicate the ownership check and bury the real signal.
    #[test]
    fn test_an_unregistered_client_is_not_reported_as_unpinned() {
        let mut status = registration(Client::Codex, None);
        status.registered = false;

        assert_eq!(
            workspace_binding_check(&[status], std::path::Path::new("/work/app")).status,
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

    /// The repair instruction must be the one that actually mutates the file — and it differs
    /// by client, because HZR rewrites Codex and the desktop app but must never rewrite Claude
    /// Code's own state file. A single generic instruction was wrong for whichever client it
    /// did not fit, so each entry now carries its own and the detail line only joins them.
    #[test]
    fn test_direct_icm_repair_names_the_command_that_fits_each_client() {
        assert!(
            Client::Codex
                .direct_icm_remediation()
                .contains("`hzr install --force`")
        );
        assert!(
            Client::Codex.direct_icm_remediation().contains("--apply"),
            "writable clients must name the mcp config apply path"
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
            "share/hzr/agent-capabilities.json",
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
