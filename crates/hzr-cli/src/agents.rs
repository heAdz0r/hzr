//! `hzr agents` — install, enroll and inspect the optional agtx Agent Observatory.
//!
//! Two steps, deliberately separate. Installing the component downloads and
//! builds a pinned binary and enrolls nothing; enrolling a project turns
//! monitoring on for that project and installs nothing. A user who does only
//! one of them ends up with a clearly reported half-state rather than silent
//! monitoring they did not ask for.
//!
//! Nothing here starts an agtx process, writes to an agtx store, or touches
//! agent configuration. `disable` stops HZR from looking; it does not stop
//! anybody's agents.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};
use hzr_core::{AgtxProject, Config, read_bounded_regular_file};
use hzr_protocol::agents::{
    AGENT_OBSERVER_PATCH_IDENTITY, AGENT_SNAPSHOT_SCHEMA_VERSION, AgentLinkRequest,
    AgentSyncRequest, AgentUsageImportRequest, AgentUsageReceiptV1, AgentsStatusResponse,
};

use crate::cli::{AgentsCommand, AgentsComponentCommand, AgentsUsageCommand};
use crate::client::DaemonClient;
use crate::output::print_json;

/// Pinned upstream identity. Changing any of these is a deliberate re-pin that
/// also reruns the compatibility, privacy and economics fixtures.
pub const AGTX_REPOSITORY: &str = "https://github.com/fynnfluegge/agtx";
pub const AGTX_COMMIT: &str = "d307c4c182dff19a65370a50403185cb826f7f49";
pub const AGTX_VERSION: &str = "1.0.4";
pub const AGTX_PATCH: &str = "patches/agtx/1.0.4-readonly-observer.patch";
pub const AGTX_PATCH_SHA256: &str =
    "13ca1bbb4406eae4406ce8da3f9258a547a907c30d6a39d590d1ab1558cd0ea8";

const OBSERVER_BINARY: &str = "hzr-agtx-observer";
/// Import files are bounded well below the daemon's body limit.
const MAX_IMPORT_BYTES: u64 = 1_048_576;

/// Where HZR keeps the component. Never `PATH`.
#[must_use]
pub fn component_path(config: &Config) -> PathBuf {
    config
        .engines
        .directory
        .clone()
        .unwrap_or_else(|| config.data_dir.join("components"))
        .join(OBSERVER_BINARY)
}

pub async fn execute(
    config: &Config,
    config_path: &Path,
    command: AgentsCommand,
    json: bool,
) -> Result<ExitCode> {
    match command {
        AgentsCommand::Component { command } => match command {
            AgentsComponentCommand::Install {
                from_binary,
                source_dir,
                force,
            } => install_component(config, from_binary, source_dir, force, json).await,
            AgentsComponentCommand::Status => component_status(config, json).await,
        },
        AgentsCommand::Enable {
            project,
            agtx_data_dir,
        } => enable(config, config_path, &project, &agtx_data_dir, json).await,
        AgentsCommand::Disable { project } => disable(config, config_path, &project, json).await,
        AgentsCommand::Status => status(config, json).await,
        AgentsCommand::Sync { project } => sync(config, &project, json).await,
        AgentsCommand::Usage { command } => match command {
            AgentsUsageCommand::Import { file } => import_usage(config, &file, json).await,
        },
        AgentsCommand::Link {
            project,
            task,
            session,
            host,
        } => link(config, &project, &task, &session, &host, json).await,
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Ask an installed binary who it is.
///
/// Identity comes from the binary itself, never from its file name: a file
/// called `hzr-agtx-observer` proves nothing about what is inside it.
async fn probe(path: &Path) -> Option<(String, u32, String)> {
    let info = hzr_daemon::probe_agent_component(path.to_path_buf()).await;
    if !info.usable() {
        return None;
    }
    Some((
        info.version?,
        info.protocol_schema_version?,
        info.patch_identity?,
    ))
}

async fn component_status(config: &Config, json: bool) -> Result<ExitCode> {
    let path = component_path(config);
    let identity = probe(&path).await;
    let compatible = identity.as_ref().is_some_and(|(_, schema, patch)| {
        *schema == AGENT_SNAPSHOT_SCHEMA_VERSION && patch == AGENT_OBSERVER_PATCH_IDENTITY
    });
    if json {
        print_json(&serde_json::json!({
            "installed": path.is_file(),
            "path": path.display().to_string(),
            "version": identity.as_ref().map(|identity| identity.0.clone()),
            "protocol_schema_version": identity.as_ref().map(|identity| identity.1),
            "patch_identity": identity.as_ref().map(|identity| identity.2.clone()),
            "compatible": compatible,
            "upstream_commit": AGTX_COMMIT,
        }))?;
    } else {
        println!(
            "agents-component installed={} compatible={} path={} version={} patch={}",
            path.is_file(),
            compatible,
            path.display(),
            identity
                .as_ref()
                .map_or("-", |identity| identity.0.as_str()),
            identity
                .as_ref()
                .map_or("-", |identity| identity.2.as_str()),
        );
    }
    Ok(if compatible {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn repository_root() -> Option<PathBuf> {
    if let Ok(executable) = std::env::current_exe() {
        if let Some(root) = executable.parent().and_then(Path::parent) {
            let bundled = root.join("share/hzr");
            if bundled.join(AGTX_PATCH).is_file() && bundled.join("engines.lock.toml").is_file() {
                return Some(bundled);
            }
        }
    }
    let mut directory = std::env::current_dir().ok()?;
    loop {
        if directory.join("patches/agtx").is_dir() && directory.join("engines.lock.toml").is_file()
        {
            return Some(directory);
        }
        directory = directory.parent()?.to_path_buf();
    }
}

async fn install_component(
    config: &Config,
    from_binary: Option<PathBuf>,
    source_dir: Option<PathBuf>,
    force: bool,
    json: bool,
) -> Result<ExitCode> {
    let destination = component_path(config);
    if destination.is_file() && !force {
        let identity = probe(&destination).await;
        if identity.as_ref().is_some_and(|(_, schema, patch)| {
            *schema == AGENT_SNAPSHOT_SCHEMA_VERSION && patch == AGENT_OBSERVER_PATCH_IDENTITY
        }) {
            if !json {
                println!(
                    "agents-component already-installed path={}",
                    destination.display()
                );
            }
            return component_status(config, json).await;
        }
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create component directory {}", parent.display()))?;
    }

    let workspace;
    let built = match from_binary {
        // An operator-supplied build. Still probed: HZR installs an identity it
        // verified, not a path somebody typed.
        Some(path) => {
            let identity = probe(&path)
                .await
                .context("supplied binary does not report an identity")?;
            if identity.1 != AGENT_SNAPSHOT_SCHEMA_VERSION
                || identity.2 != AGENT_OBSERVER_PATCH_IDENTITY
            {
                bail!(
                    "supplied binary reports schema={} patch={}, expected schema={} patch={}",
                    identity.1,
                    identity.2,
                    AGENT_SNAPSHOT_SCHEMA_VERSION,
                    AGENT_OBSERVER_PATCH_IDENTITY
                );
            }
            path
        }
        None => {
            workspace = build_component(source_dir)?;
            workspace
                .path()
                .join("target/release")
                .join(OBSERVER_BINARY)
        }
    };

    let parent = destination
        .parent()
        .context("component directory is missing")?;
    let staged = tempfile::NamedTempFile::new_in(parent)?.into_temp_path();
    std::fs::copy(&built, &staged).with_context(|| format!("stage {}", built.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    let identity = probe(&staged)
        .await
        .context("staged observer has no valid identity")?;
    if identity.0 != AGTX_VERSION
        || identity.1 != AGENT_SNAPSHOT_SCHEMA_VERSION
        || identity.2 != AGENT_OBSERVER_PATCH_IDENTITY
    {
        bail!("staged observer identity does not match the pinned component");
    }
    std::fs::File::open(&staged)?.sync_all()?;
    staged
        .persist(&destination)
        .context("activate observer atomically")?;
    component_status(config, json).await
}

/// Build the observer from the pinned upstream commit plus the pinned patch.
fn build_component(source_dir: Option<PathBuf>) -> Result<tempfile::TempDir> {
    let root = repository_root().context(
        "run `hzr agents component install` from an HZR checkout, or pass --from-binary",
    )?;
    let patch = root.join(AGTX_PATCH);
    let digest = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(
        read_bounded_regular_file(&patch, 4 * 1_048_576)
            .with_context(|| format!("read {}", patch.display()))?,
    ));
    if digest != AGTX_PATCH_SHA256 {
        bail!(
            "patch digest mismatch for {}: expected {AGTX_PATCH_SHA256}, found {digest}",
            patch.display()
        );
    }

    // Never build from a predictable shared directory or execute untracked
    // files from an operator's checkout. Fetch the pinned tree into a private
    // temporary repository, leaving the supplied source untouched.
    let temporary = tempfile::Builder::new()
        .prefix("hzr-agtx-build-")
        .tempdir()?;
    let workspace = temporary.path();
    let source = match source_dir {
        Some(directory) => directory.canonicalize()?.to_string_lossy().into_owned(),
        None => AGTX_REPOSITORY.to_string(),
    };
    run(workspace, "git", &["init", "--quiet"])?;
    run(
        workspace,
        "git",
        &[
            "fetch",
            "--quiet",
            "--depth",
            "1",
            "--",
            &source,
            AGTX_COMMIT,
        ],
    )?;
    run(
        workspace,
        "git",
        &["checkout", "--quiet", "--detach", "FETCH_HEAD"],
    )?;

    let head = std::process::Command::new("git")
        .current_dir(workspace)
        .args(["rev-parse", "HEAD"])
        .output()
        .context("resolve the checked-out agtx commit")?;
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    if head != AGTX_COMMIT {
        bail!("agtx checkout is at {head}, expected the pinned {AGTX_COMMIT}");
    }

    let patch_argument = patch.to_string_lossy().into_owned();
    // `apply --check` first: a partially applied patch would leave a tree that
    // builds into something nobody pinned.
    run(workspace, "git", &["apply", "--check", &patch_argument])?;
    run(workspace, "git", &["apply", &patch_argument])?;

    eprintln!("building {OBSERVER_BINARY} (this compiles the pinned agtx source once)");
    run(
        workspace,
        "cargo",
        &["build", "--locked", "--release", "--bin", OBSERVER_BINARY],
    )?;
    let built = workspace.join("target/release").join(OBSERVER_BINARY);
    if !built.is_file() {
        bail!("build finished but {} is missing", built.display());
    }
    Ok(temporary)
}

fn run(directory: &Path, program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .current_dir(directory)
        .args(args)
        .status()
        .with_context(|| format!("run {program} {}", args.join(" ")))?;
    if !status.success() {
        bail!("{program} {} failed with {status}", args.join(" "));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Enrollment
// ---------------------------------------------------------------------------

fn canonical_existing(path: &Path, what: &str) -> Result<PathBuf> {
    let canonical = path.canonicalize().with_context(|| {
        format!(
            "{what} must be an existing absolute path: {}",
            path.display()
        )
    })?;
    if !canonical.is_dir() {
        bail!("{what} must be a directory: {}", canonical.display());
    }
    Ok(canonical)
}

async fn enable(
    config: &Config,
    config_path: &Path,
    project: &Path,
    data_dir: &Path,
    json: bool,
) -> Result<ExitCode> {
    let project = canonical_existing(project, "--project")?;
    let data_dir = canonical_existing(data_dir, "--agtx-data-dir")?;
    // An enrollment that names a store with no index is almost certainly a
    // typo, and monitoring nothing while reporting "enabled" is worse than
    // refusing now.
    if !data_dir.join("index.db").is_file() && !data_dir.join("projects").is_dir() {
        bail!(
            "{} does not look like an agtx data directory (no index.db and no projects/)",
            data_dir.display()
        );
    }
    let component = component_path(config);
    if !component.is_file() {
        eprintln!(
            "warning: the observer component is not installed at {}; monitoring will report missing_component until `hzr agents component install` runs",
            component.display()
        );
    }

    let mut updated = Config::load_or_default(config_path)?;
    updated.integrations.agtx.enroll(AgtxProject {
        project_path: project.clone(),
        data_dir: data_dir.clone(),
        enabled: true,
    });
    updated.write(config_path)?;
    let reload = reload_daemon(&updated).await;

    if json {
        print_json(&serde_json::json!({
            "enabled": true,
            "project": project.display().to_string(),
            "data_dir": data_dir.display().to_string(),
            "component_installed": component.is_file(),
            "daemon_reloaded": reload.is_some(),
        }))?;
    } else {
        println!(
            "agents-enabled project={} data-dir={} component-installed={} daemon-reloaded={}",
            project.display(),
            data_dir.display(),
            component.is_file(),
            reload.is_some(),
        );
        println!("monitoring is read-only: HZR never starts, moves, or answers an agtx task");
    }
    Ok(ExitCode::SUCCESS)
}

async fn disable(
    _config: &Config,
    config_path: &Path,
    project: &Path,
    json: bool,
) -> Result<ExitCode> {
    let project = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    let mut updated = Config::load_or_default(config_path)?;
    let changed = updated.integrations.agtx.disable_project(&project);
    updated.write(config_path)?;
    let reload = reload_daemon(&updated).await;
    if json {
        print_json(&serde_json::json!({
            "disabled": changed,
            "project": project.display().to_string(),
            "daemon_reloaded": reload.is_some(),
            "history_retained": true,
        }))?;
    } else {
        println!(
            "agents-disabled project={} changed={} daemon-reloaded={} history-retained=true",
            project.display(),
            changed,
            reload.is_some()
        );
        println!("agtx sessions and stored history are untouched");
    }
    Ok(ExitCode::SUCCESS)
}

/// Tell a running daemon to re-read the config. A daemon that is not running is
/// not an error: it will read the new file when it starts.
async fn reload_daemon(config: &Config) -> Option<AgentsStatusResponse> {
    let client = DaemonClient::from_config(config).ok()?;
    client.agents_reload(&config.integrations.agtx).await.ok()
}

// ---------------------------------------------------------------------------
// Reads and operations
// ---------------------------------------------------------------------------

async fn status(config: &Config, json: bool) -> Result<ExitCode> {
    let status = DaemonClient::from_config(config)?.agents_status().await?;
    if json {
        print_json(&status)?;
        return Ok(ExitCode::SUCCESS);
    }
    println!(
        "agents enabled={} poll-interval-ms={} component-installed={} component-version={} patch={}",
        status.enabled,
        status.poll_interval_ms,
        status.component.installed,
        status.component.version.as_deref().unwrap_or("-"),
        status.component.patch_identity.as_deref().unwrap_or("-"),
    );
    if let Some(platform) = status.component.unsupported_platform.as_deref() {
        println!("component-unavailable platform={platform}");
    }
    if status.enrollments.is_empty() {
        println!("no enrolled projects; monitoring needs both steps:");
        for (step, command) in onboarding_commands().iter().enumerate() {
            println!("  {}. {command}", step + 1);
        }
    }
    for enrollment in &status.enrollments {
        println!(
            "  project={} enabled={} state={} tasks={} lag-ms={} last-success-ms={} error={}",
            enrollment.project_id,
            enrollment.enabled,
            enrollment.state.as_str(),
            enrollment.observed_tasks,
            enrollment
                .lag_ms
                .map_or_else(|| "-".into(), |lag| lag.to_string()),
            enrollment
                .last_success_ms
                .map_or_else(|| "-".into(), |at| at.to_string()),
            enrollment.last_error_code.as_deref().unwrap_or("none"),
        );
    }
    Ok(ExitCode::SUCCESS)
}

async fn sync(config: &Config, project: &Path, json: bool) -> Result<ExitCode> {
    let project = canonical_existing(project, "--project")?;
    let response = DaemonClient::from_config(config)?
        .agents_sync(&AgentSyncRequest {
            project_path: project.to_string_lossy().into_owned(),
        })
        .await?;
    if json {
        print_json(&response)?;
    } else {
        println!(
            "agents-sync state={} pages={} tasks={} events={} tombstoned={} complete={} error={}",
            response.state.as_str(),
            response.pages,
            response.tasks_observed,
            response.events_recorded,
            response.tombstoned,
            response.complete,
            response.error_code.as_deref().unwrap_or("none"),
        );
        for warning in &response.warnings {
            println!("  warning {warning}");
        }
    }
    Ok(if response.error_code.is_some() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

async fn import_usage(config: &Config, file: &Path, json: bool) -> Result<ExitCode> {
    if !file.is_absolute() {
        bail!("--file must be an absolute path");
    }
    let bytes = read_bounded_regular_file(file, MAX_IMPORT_BYTES).with_context(|| {
        format!(
            "usage import must be a regular non-symlink file of at most {MAX_IMPORT_BYTES} bytes: {}",
            file.display()
        )
    })?;
    let mut request: AgentUsageImportRequest = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid usage import JSON in {}", file.display()))?;
    // Every receipt carries its own project path; the file is data, not a way
    // to reach a project the daemon is not monitoring.
    for receipt in &mut request.receipts {
        normalize_receipt(receipt)?;
    }
    let response = DaemonClient::from_config(config)?
        .agents_usage_import(&request)
        .await?;
    if json {
        print_json(&response)?;
    } else {
        println!(
            "agents-usage-import committed={} accepted={} replayed={} rejected={} conflicts={}",
            response.committed,
            response.accepted,
            response.replayed,
            response.rejected,
            response.conflicts,
        );
        for rejection in &response.rejections {
            println!(
                "  rejected {} {}",
                rejection.source_record_id, rejection.code
            );
        }
    }
    Ok(if response.committed {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Canonicalize an imported receipt's project path so it joins the same scope
/// the observer records, and refuse the cumulative kinds outright.
fn normalize_receipt(receipt: &mut AgentUsageReceiptV1) -> Result<()> {
    if receipt.usage_kind != "request_delta" {
        bail!(
            "receipt {} declares usage_kind={}; only per-request deltas are accepted, because summing cumulative session totals counts every earlier request again",
            receipt.source_record_id,
            receipt.usage_kind
        );
    }
    let path = PathBuf::from(&receipt.project_path);
    if let Ok(canonical) = path.canonicalize() {
        receipt.project_path = canonical.to_string_lossy().into_owned();
    }
    Ok(())
}

async fn link(
    config: &Config,
    project: &Path,
    task: &str,
    session: &str,
    host: &str,
    json: bool,
) -> Result<ExitCode> {
    let project = canonical_existing(project, "--project")?;
    let response = DaemonClient::from_config(config)?
        .agents_link(&AgentLinkRequest {
            project_path: project.to_string_lossy().into_owned(),
            source_task_id: task.to_string(),
            session_id: session.to_string(),
            host: host.to_string(),
        })
        .await?;
    if json {
        print_json(&response)?;
    } else {
        println!(
            "agents-link linked={} conflict={} task={} code={}",
            response.linked,
            response.conflict,
            response.task_id.as_deref().unwrap_or("-"),
            response.code.as_deref().unwrap_or("none"),
        );
        if response.conflict {
            println!(
                "this session is now claimed by more than one task; its spend stays shared and unallocated"
            );
        }
    }
    Ok(if response.linked {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// The exact commands the settings UI shows for an empty state.
#[must_use]
pub fn onboarding_commands() -> [&'static str; 2] {
    [
        "hzr agents component install",
        "hzr agents enable --project <absolute-worktree> --agtx-data-dir <absolute-agtx-data-dir>",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn component_activation_replaces_symlink_atomically_and_rejects_failed_binary() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = tempfile::tempdir().expect("temp");
        let config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        let destination = component_path(&config);
        std::fs::create_dir_all(destination.parent().expect("parent")).expect("mkdir");
        let victim = directory.path().join("unrelated");
        std::fs::write(&victim, "keep").expect("victim");
        symlink(&victim, &destination).expect("symlink");
        let source = directory.path().join("observer");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' '{} {} schema={} patch={}'\n",
            OBSERVER_BINARY,
            AGTX_VERSION,
            AGENT_SNAPSHOT_SCHEMA_VERSION,
            AGENT_OBSERVER_PATCH_IDENTITY
        );
        std::fs::write(&source, &script).expect("source");
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o755)).expect("mode");
        install_component(&config, Some(source.clone()), None, true, true)
            .await
            .expect("install");
        assert!(
            !std::fs::symlink_metadata(&destination)
                .expect("metadata")
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(&victim).expect("victim intact"),
            "keep"
        );
        let installed = std::fs::read(&destination).expect("installed");
        std::fs::write(&source, format!("{script}exit 7\n")).expect("bad source");
        assert!(
            install_component(&config, Some(source), None, true, true)
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(&destination).expect("still installed"),
            installed
        );
    }

    #[test]
    fn the_pinned_patch_digest_matches_the_checked_in_patch() {
        let Some(root) = repository_root() else {
            return;
        };
        let bytes = read_bounded_regular_file(&root.join(AGTX_PATCH), 4 * 1_048_576)
            .expect("pinned patch is present");
        let digest = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(bytes));
        assert_eq!(
            digest, AGTX_PATCH_SHA256,
            "the pinned patch changed without updating its digest"
        );
    }

    #[test]
    fn a_cumulative_receipt_is_refused_before_it_reaches_the_daemon() {
        let mut receipt = AgentUsageReceiptV1 {
            schema_version: 1,
            receipt_id: "r".into(),
            source: "s".into(),
            source_record_id: "R1".into(),
            observed_at_ms: 1,
            project_path: "/tmp".into(),
            host: "claude-code".into(),
            session_id: "sess".into(),
            task_id: None,
            request_id: None,
            provider: "p".into(),
            harness: "h".into(),
            model: "m".into(),
            billing_method: "standard".into(),
            currency: "USD".into(),
            request_input_tokens: None,
            usage_kind: "session_total".into(),
            usage: Default::default(),
            reported_cost_microunits: None,
            original_source_hash: None,
        };
        assert!(normalize_receipt(&mut receipt).is_err());
        receipt.usage_kind = "request_delta".into();
        assert!(normalize_receipt(&mut receipt).is_ok());
    }

    #[test]
    fn the_component_lives_in_hzrs_own_directory() {
        let mut config = Config {
            data_dir: PathBuf::from("/var/hzr"),
            ..Config::default()
        };
        config.engines.directory = None;
        assert_eq!(
            component_path(&config),
            PathBuf::from("/var/hzr/components/hzr-agtx-observer")
        );
        config.engines.directory = Some(PathBuf::from("/opt/hzr/engines"));
        assert_eq!(
            component_path(&config),
            PathBuf::from("/opt/hzr/engines/hzr-agtx-observer")
        );
    }

    #[test]
    fn onboarding_names_both_steps_in_order() {
        let steps = onboarding_commands();
        assert!(steps[0].contains("component install"));
        assert!(steps[1].contains("--agtx-data-dir"));
    }
}
