use std::collections::HashSet;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{LazyLock, Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use hzr_core::{ActivationConfig, ActivationMode, Config, EnabledWorkspace, InstructionScope};
use hzr_index::{Deadlines, Workspace};
use serde::Serialize;

pub async fn discover(config: &Config, start: &Path) -> Result<Workspace> {
    Ok(Workspace::discover_managed_fast(
        start,
        Path::new("git"),
        &config.data_dir,
        Deadlines::default().version,
    )
    .await?)
}

pub async fn is_enabled(config: &Config, start: &Path) -> Result<bool> {
    if config.activation.mode == ActivationMode::All {
        return Ok(true);
    }
    let workspace = discover(config, start).await?;
    Ok(config.activation.allows(
        &workspace.identity.repository_id,
        &workspace.identity.worktree_id,
    ))
}

pub fn record(workspace: &Workspace) -> EnabledWorkspace {
    EnabledWorkspace {
        repository_id: workspace.identity.repository_id.clone(),
        worktree_id: workspace.identity.worktree_id.clone(),
        root: workspace.identity.root.clone(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionLocation {
    UserGlobal,
    WorkspaceShared,
    WorkspaceLocal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InstructionTarget {
    pub surface: crate::instructions::Surface,
    pub path: PathBuf,
    pub location: InstructionLocation,
}

impl InstructionTarget {
    pub fn is_local_codex(&self) -> bool {
        self.location == InstructionLocation::WorkspaceLocal
            && self.surface == crate::instructions::Surface::Codex
    }

    #[cfg(test)]
    pub fn path_is_local_codex(surface: crate::instructions::Surface, path: &Path) -> bool {
        surface == crate::instructions::Surface::Codex
            && path.file_name().and_then(|name| name.to_str()) == Some("AGENTS.override.md")
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InstructionDesiredState {
    pub desired: Vec<InstructionTarget>,
    pub obsolete: Vec<InstructionTarget>,
    pub excluded: Vec<PathBuf>,
}

fn workspace_targets(root: &Path, location: InstructionLocation) -> [InstructionTarget; 2] {
    let (claude, codex) = match location {
        InstructionLocation::WorkspaceShared => ("CLAUDE.md", "AGENTS.md"),
        InstructionLocation::WorkspaceLocal => ("CLAUDE.local.md", "AGENTS.override.md"),
        InstructionLocation::UserGlobal => {
            unreachable!("global targets do not belong to a workspace")
        }
    };
    [
        InstructionTarget {
            surface: crate::instructions::Surface::Claude,
            path: root.join(claude),
            location,
        },
        InstructionTarget {
            surface: crate::instructions::Surface::Codex,
            path: root.join(codex),
            location,
        },
    ]
}

fn global_targets() -> Result<[InstructionTarget; 2]> {
    Ok([
        InstructionTarget {
            surface: crate::instructions::Surface::Claude,
            path: crate::instructions::Surface::Claude.default_path()?,
            location: InstructionLocation::UserGlobal,
        },
        InstructionTarget {
            surface: crate::instructions::Surface::Codex,
            path: crate::instructions::Surface::Codex.default_path()?,
            location: InstructionLocation::UserGlobal,
        },
    ])
}

pub fn local_instruction_paths(
    root: &Path,
    scope: InstructionScope,
) -> [(crate::instructions::Surface, PathBuf); 2] {
    let location = match scope {
        InstructionScope::Shared => InstructionLocation::WorkspaceShared,
        InstructionScope::Local => InstructionLocation::WorkspaceLocal,
    };
    workspace_targets(root, location).map(|target| (target.surface, target.path))
}

pub fn instruction_desired_state(
    root: &Path,
    activation: ActivationMode,
    scope: InstructionScope,
) -> Result<InstructionDesiredState> {
    let globals = global_targets()?;
    let shared = workspace_targets(root, InstructionLocation::WorkspaceShared);
    let local = workspace_targets(root, InstructionLocation::WorkspaceLocal);
    let desired = match (activation, scope) {
        (_, InstructionScope::Local) => local.to_vec(),
        // Global activation is inherited by every workspace. Installing the same managed
        // contract into each workspace as well makes Claude and Codex ingest duplicate policy.
        (ActivationMode::All, InstructionScope::Shared) => globals.to_vec(),
        (ActivationMode::Selected, InstructionScope::Shared) => shared.to_vec(),
    };
    let mut obsolete = globals
        .iter()
        .chain(shared.iter())
        .chain(local.iter())
        .filter(|target| !desired.iter().any(|item| item.path == target.path))
        .cloned()
        .collect::<Vec<_>>();
    obsolete.sort_by(|left, right| left.path.cmp(&right.path));
    let excluded = if scope == InstructionScope::Local {
        local.iter().map(|target| target.path.clone()).collect()
    } else {
        Vec::new()
    };
    Ok(InstructionDesiredState {
        desired,
        obsolete,
        excluded,
    })
}

/// Deadline for one per-project Git probe. Matches `Deadlines::version`.
///
/// Doctor walks every registered workspace, so a repository whose `git` never returns —
/// an unreachable volume, a credential helper waiting on a terminal, a wedged hook — used
/// to stall the whole pass on that one project with nothing reported. A probe that outlives
/// this is killed and becomes a finding naming the project.
const GIT_PROBE_DEADLINE: Duration = Duration::from_secs(10);
const GIT_PROBE_POLL: Duration = Duration::from_millis(20);
/// Grace for draining stdout after the child exited; only a surviving descendant needs it.
const GIT_PROBE_DRAIN_GRACE: Duration = Duration::from_secs(2);
/// Projects with an unresponsive Git tolerated in one pass before the rest are refused.
///
/// A wedged Git is usually systemic (an unmounted volume, a machine-wide credential prompt),
/// so a 110-workspace fleet would otherwise cost 110 x the deadline before doctor finishes.
const GIT_PROBE_TIMEOUT_BUDGET: usize = 3;

/// Roots whose Git already missed the deadline in this pass.
///
/// Counting roots rather than probes keeps one wedged project to a single deadline — its
/// remaining probes are refused at once — and keeps the budget spendable by other projects.
static WEDGED_ROOTS: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Open a new fleet pass: a previous pass's timeouts must not refuse this one.
pub fn reset_git_probe_budget() {
    if let Ok(mut wedged) = WEDGED_ROOTS.lock() {
        wedged.clear();
    }
}

/// Run one non-interactive `git` in `root`, killing it at `deadline`.
///
/// `Ok(None)` means the deadline was exceeded and the child was killed; the caller decides
/// how to report it. Stdin is closed and every prompt path is disabled, so the probe can
/// never block waiting for a human that no longer has a terminal attached.
fn bounded_git(
    binary: &Path,
    root: &Path,
    args: &[&OsStr],
    capture: bool,
    deadline: Duration,
    operation: &str,
) -> Result<Option<Output>> {
    let mut command = Command::new(binary);
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(if capture {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::null())
        // A probe must never wait on a human: no terminal prompt, no askpass helper, and
        // no optional index lock another Git in the same repository is holding.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // Own process group: a hook or credential helper Git spawned must die with it.
        // A surviving grandchild keeps the pipe open and outlives the kill.
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to run git for {operation}"))?;
    // Drain the pipe on its own thread: the probe must not deadlock on a full pipe buffer
    // while the parent polls for exit, and must not later block on a drain that never ends.
    let (sender, drained) = mpsc::channel();
    match child.stdout.take() {
        Some(mut pipe) => {
            std::thread::spawn(move || {
                let mut buffer = Vec::new();
                let _ = pipe.read_to_end(&mut buffer);
                let _ = sender.send(buffer);
            });
        }
        None => drop(sender),
    }
    let expires = Instant::now() + deadline;
    let status = loop {
        match child
            .try_wait()
            .with_context(|| format!("failed to wait for git during {operation}"))?
        {
            Some(status) => break Some(status),
            None if Instant::now() >= expires => {
                terminate_group(&mut child);
                break None;
            }
            None => std::thread::sleep(GIT_PROBE_POLL),
        }
    };
    let Some(status) = status else {
        // The drain thread ends on its own once the killed group releases the pipe;
        // waiting for it here would reintroduce the stall the deadline just prevented.
        return Ok(None);
    };
    let stdout = match drained.recv_timeout(GIT_PROBE_DRAIN_GRACE) {
        Ok(buffer) => buffer,
        // Something Git spawned still owns the pipe: report the probe as unanswered
        // rather than block the pass on a descendant nobody is waiting for.
        Err(mpsc::RecvTimeoutError::Timeout) => return Ok(None),
        Err(mpsc::RecvTimeoutError::Disconnected) => Vec::new(),
    };
    Ok(Some(Output {
        status,
        stdout,
        stderr: Vec::new(),
    }))
}

/// SIGKILL the probe's whole process group, then reap the direct child.
fn terminate_group(child: &mut Child) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Why this probe must not run, if the pass already learned enough to skip it.
fn probe_refusal(
    wedged: &HashSet<PathBuf>,
    root: &Path,
    deadline: Duration,
    operation: &str,
) -> Option<String> {
    if wedged.contains(root) {
        return Some(format!(
            "{operation} skipped: this project's Git already exceeded {}s in this pass",
            deadline.as_secs()
        ));
    }
    if wedged.len() >= GIT_PROBE_TIMEOUT_BUDGET {
        return Some(format!(
            "{operation} skipped for {}: {} other project(s) already had an unresponsive Git in this pass, so the remainder are reported instead of stalling it",
            root.display(),
            wedged.len()
        ));
    }
    None
}

fn git_probe(root: &Path, args: &[&OsStr], capture: bool, operation: &str) -> Result<Output> {
    // A poisoned lock must not stop the diagnosis: fall through and probe.
    if let Some(reason) = WEDGED_ROOTS
        .lock()
        .ok()
        .and_then(|wedged| probe_refusal(&wedged, root, GIT_PROBE_DEADLINE, operation))
    {
        bail!("{reason}");
    }
    match bounded_git(
        Path::new("git"),
        root,
        args,
        capture,
        GIT_PROBE_DEADLINE,
        operation,
    )? {
        Some(output) => Ok(output),
        None => {
            let seen = WEDGED_ROOTS.lock().ok().map_or(1, |mut wedged| {
                wedged.insert(root.to_path_buf());
                wedged.len()
            });
            bail!(
                "{operation} did not answer within {}s in {}; that project's Git is unresponsive (unreachable volume, credential prompt, or a stuck hook) — repair it or unregister the workspace ({seen} unresponsive project(s) this pass)",
                GIT_PROBE_DEADLINE.as_secs(),
                root.display()
            )
        }
    }
}

pub fn is_tracked_shared_instruction(root: &Path, target: &InstructionTarget) -> Result<bool> {
    if target.location != InstructionLocation::WorkspaceShared || !root.join(".git").exists() {
        return Ok(false);
    }
    let relative = target.path.strip_prefix(root).with_context(|| {
        format!(
            "instruction target {} is outside workspace {}",
            target.path.display(),
            root.display()
        )
    })?;
    let output = git_probe(
        root,
        &[
            OsStr::new("ls-files"),
            OsStr::new("--error-unmatch"),
            OsStr::new("--"),
            relative.as_os_str(),
        ],
        false,
        "tracked-instruction check",
    )?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!(
            "git could not determine whether {} is tracked",
            target.path.display()
        ),
    }
}

const LOCAL_EXCLUDE_BEGIN: &str = "# hzr:begin local instructions";
const LOCAL_EXCLUDE_END: &str = "# hzr:end local instructions";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LocalExcludeReport {
    pub path: Option<PathBuf>,
    pub backup_path: Option<PathBuf>,
    pub changed: bool,
    pub installed: bool,
    pub before_sha256: Option<String>,
    pub after_sha256: Option<String>,
}

fn exclude_block(state: &InstructionDesiredState) -> String {
    let mut block = String::from(LOCAL_EXCLUDE_BEGIN);
    for path in &state.excluded {
        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            block.push('\n');
            block.push('/');
            block.push_str(name);
        }
    }
    block.push('\n');
    block.push_str(LOCAL_EXCLUDE_END);
    block
}

fn strip_exclude_block(text: &str) -> String {
    let Some(start) = text.find(LOCAL_EXCLUDE_BEGIN) else {
        return text.to_owned();
    };
    let Some(relative_end) = text[start..].find(LOCAL_EXCLUDE_END) else {
        return text.to_owned();
    };
    let end = start + relative_end + LOCAL_EXCLUDE_END.len();
    let mut stripped = String::with_capacity(text.len());
    stripped.push_str(text[..start].trim_end());
    let suffix = text[end..].trim_start_matches(['\r', '\n']);
    if !stripped.is_empty() && !suffix.is_empty() {
        stripped.push('\n');
    }
    stripped.push_str(suffix);
    if !stripped.is_empty() && !stripped.ends_with('\n') {
        stripped.push('\n');
    }
    stripped
}

pub fn local_exclude_path(root: &Path) -> Result<Option<PathBuf>> {
    if !root.join(".git").exists() {
        return Ok(None);
    }
    let output = git_probe(
        root,
        &[
            OsStr::new("rev-parse"),
            OsStr::new("--git-path"),
            OsStr::new("info/exclude"),
        ],
        true,
        "local-exclude probe",
    )?;
    if !output.status.success() {
        anyhow::bail!(
            "git could not resolve the local exclude file for {}",
            root.display()
        );
    }
    let raw = String::from_utf8(output.stdout)?;
    let reported = PathBuf::from(raw.trim());
    Ok(Some(if reported.is_absolute() {
        reported
    } else {
        root.join(reported)
    }))
}

pub fn reconcile_local_instruction_excludes(
    root: &Path,
    state: &InstructionDesiredState,
    dry_run: bool,
) -> Result<LocalExcludeReport> {
    let Some(path) = local_exclude_path(root)? else {
        return Ok(LocalExcludeReport {
            path: None,
            backup_path: None,
            changed: false,
            installed: false,
            before_sha256: None,
            after_sha256: None,
        });
    };
    let before = crate::adoption::read_optional(&path)?;
    let before_text = String::from_utf8(before.clone())?;
    let mut after = strip_exclude_block(&before_text);
    let installed = !state.excluded.is_empty();
    if installed {
        if !after.is_empty() && !after.ends_with("\n\n") {
            if after.ends_with('\n') {
                after.push('\n');
            } else {
                after.push_str("\n\n");
            }
        }
        after.push_str(&exclude_block(state));
        after.push('\n');
    }
    let changed = before != after.as_bytes();
    let backup_path = changed.then(|| crate::adoption::backup_path(&path, &before));
    if !dry_run {
        if let Some(backup) = backup_path.as_ref() {
            crate::adoption::commit(&path, &before, after.as_bytes(), backup, b"")?;
        }
    }
    Ok(LocalExcludeReport {
        path: Some(path),
        backup_path,
        changed,
        installed,
        before_sha256: Some(crate::adoption::sha256(&before)),
        after_sha256: Some(crate::adoption::sha256(after.as_bytes())),
    })
}

/// Снимок режима активации и списка явно включённых workspace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ActivationStatusReport {
    pub mode: ActivationMode,
    pub enabled_workspaces: Vec<EnabledWorkspace>,
}

impl ActivationStatusReport {
    #[must_use]
    pub fn from_config(activation: &ActivationConfig) -> Self {
        Self {
            mode: activation.mode,
            enabled_workspaces: activation.enabled_workspaces.clone(),
        }
    }
}

#[must_use]
pub fn render_status_text(report: &ActivationStatusReport) -> String {
    let mode = match report.mode {
        ActivationMode::All => "all",
        ActivationMode::Selected => "selected",
    };
    let mut output = format!("activation={mode}\n");
    match report.mode {
        ActivationMode::All => {
            output.push_str("enabled workspaces: all projects (no selection list)\n");
        }
        ActivationMode::Selected if report.enabled_workspaces.is_empty() => {
            output.push_str("enabled workspaces: none\n");
        }
        ActivationMode::Selected => {
            let _ = writeln!(
                output,
                "enabled workspaces ({}):",
                report.enabled_workspaces.len()
            );
            for workspace in &report.enabled_workspaces {
                let _ = writeln!(output, "  {}", workspace.root.display());
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use hzr_core::{ActivationMode, Config, EnabledWorkspace, InstructionScope};
    use tempfile::tempdir;

    use super::{
        ActivationStatusReport, GIT_PROBE_TIMEOUT_BUDGET, InstructionLocation, bounded_git,
        discover, instruction_desired_state, is_enabled, is_tracked_shared_instruction,
        probe_refusal, reconcile_local_instruction_excludes, record, render_status_text,
    };

    #[test]
    fn desired_state_makes_scope_transitions_explicit() {
        let directory = tempdir().expect("temporary directory");
        let local = instruction_desired_state(
            directory.path(),
            ActivationMode::All,
            InstructionScope::Local,
        )
        .expect("local desired state");
        assert_eq!(local.desired.len(), 2);
        assert!(
            local
                .desired
                .iter()
                .all(|target| target.location == InstructionLocation::WorkspaceLocal)
        );
        assert_eq!(local.excluded.len(), 2);
        assert_eq!(local.obsolete.len(), 4);

        let shared = instruction_desired_state(
            directory.path(),
            ActivationMode::Selected,
            InstructionScope::Shared,
        )
        .expect("shared desired state");
        assert!(
            shared
                .desired
                .iter()
                .all(|target| target.location == InstructionLocation::WorkspaceShared)
        );
        assert!(shared.excluded.is_empty());
        assert!(shared.obsolete.iter().any(|target| {
            target.location == InstructionLocation::WorkspaceLocal && target.is_local_codex()
        }));

        let global = instruction_desired_state(
            directory.path(),
            ActivationMode::All,
            InstructionScope::Shared,
        )
        .expect("global desired state");
        assert_eq!(global.desired.len(), 2);
        assert!(
            global
                .desired
                .iter()
                .all(|target| target.location == InstructionLocation::UserGlobal)
        );
        assert!(
            global
                .obsolete
                .iter()
                .any(|target| target.location == InstructionLocation::WorkspaceShared)
        );
    }

    #[test]
    fn local_exclude_is_installed_removed_and_preserves_user_bytes() {
        let directory = tempdir().expect("temporary directory");
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(directory.path())
            .status()
            .expect("git init");
        assert!(status.success());
        let exclude = directory.path().join(".git/info/exclude");
        std::fs::write(&exclude, "user-pattern\n").expect("user exclude");
        let local = instruction_desired_state(
            directory.path(),
            ActivationMode::Selected,
            InstructionScope::Local,
        )
        .expect("local state");
        let installed = reconcile_local_instruction_excludes(directory.path(), &local, false)
            .expect("install excludes");
        assert!(installed.changed);
        let installed_bytes = std::fs::read_to_string(&exclude).expect("installed exclude");
        assert!(installed_bytes.starts_with("user-pattern\n"));
        assert!(installed_bytes.contains("/CLAUDE.local.md"));
        assert!(installed_bytes.contains("/AGENTS.override.md"));

        let shared = instruction_desired_state(
            directory.path(),
            ActivationMode::Selected,
            InstructionScope::Shared,
        )
        .expect("shared state");
        let removed = reconcile_local_instruction_excludes(directory.path(), &shared, false)
            .expect("remove excludes");
        assert!(removed.changed);
        assert_eq!(
            std::fs::read_to_string(&exclude).expect("removed exclude"),
            "user-pattern\n"
        );
    }

    /// A `git` that never exits is the failure that used to stall doctor on one project.
    #[cfg(unix)]
    #[test]
    fn a_wedged_git_probe_is_killed_at_its_deadline() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempdir().expect("temporary directory");
        let stub = directory.path().join("git-stub");
        // `sh` outlives its own kill through the `sleep` grandchild unless the whole
        // process group is signalled, which is exactly what a Git hook or helper does.
        std::fs::write(&stub, "#!/bin/sh\nsleep 600\n").expect("stub git");
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))
            .expect("executable stub");

        let started = Instant::now();
        let answered = bounded_git(
            &stub,
            directory.path(),
            &[OsStr::new("rev-parse")],
            true,
            Duration::from_millis(200),
            "stub probe",
        )
        .expect("the probe itself must not fail");
        assert!(
            answered.is_none(),
            "a stub that never exits must not answer"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the probe ran for {:?}, so it was not bounded",
            started.elapsed()
        );
    }

    #[test]
    fn one_wedged_project_costs_one_deadline_and_a_wedged_fleet_stops_costing_any() {
        let mut wedged = HashSet::new();
        let first = PathBuf::from("/work/first");
        assert!(
            probe_refusal(&wedged, &first, Duration::from_secs(10), "probe").is_none(),
            "an unknown project is probed"
        );

        wedged.insert(first.clone());
        let repeat = probe_refusal(&wedged, &first, Duration::from_secs(10), "probe")
            .expect("a known-wedged project is refused at once");
        assert!(
            repeat.contains("this project's Git already exceeded"),
            "{repeat}"
        );
        assert!(
            probe_refusal(
                &wedged,
                Path::new("/work/second"),
                Duration::from_secs(10),
                "probe"
            )
            .is_none(),
            "the budget is not spent by one project"
        );

        for index in 1..GIT_PROBE_TIMEOUT_BUDGET {
            wedged.insert(PathBuf::from(format!("/work/wedged-{index}")));
        }
        let refused = probe_refusal(
            &wedged,
            Path::new("/work/second"),
            Duration::from_secs(10),
            "probe",
        )
        .expect("past the budget every remaining project is refused");
        assert!(
            refused.contains("already had an unresponsive Git"),
            "{refused}"
        );
    }

    #[test]
    fn tracked_shared_instructions_are_repository_owned() {
        let directory = tempdir().expect("temporary directory");
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(directory.path())
            .status()
            .expect("git init");
        assert!(status.success());
        std::fs::write(directory.path().join("AGENTS.md"), "tracked contract\n")
            .expect("AGENTS fixture");
        let status = std::process::Command::new("git")
            .args(["add", "AGENTS.md"])
            .current_dir(directory.path())
            .status()
            .expect("git add");
        assert!(status.success());
        let state = instruction_desired_state(
            directory.path(),
            ActivationMode::All,
            InstructionScope::Shared,
        )
        .expect("global desired state");
        let agents = state
            .obsolete
            .iter()
            .find(|target| target.path.ends_with("AGENTS.md"))
            .expect("shared Codex target");
        let claude = state
            .obsolete
            .iter()
            .find(|target| target.path.ends_with("CLAUDE.md"))
            .expect("shared Claude target");

        assert!(
            is_tracked_shared_instruction(directory.path(), agents)
                .expect("tracked instruction check")
        );
        assert!(
            !is_tracked_shared_instruction(directory.path(), claude)
                .expect("untracked instruction check")
        );
    }

    #[tokio::test]
    async fn selected_activation_is_exactly_scoped_to_the_enabled_workspace() {
        let directory = tempdir().expect("temporary directory");
        let enabled = directory.path().join("enabled");
        let baseline = directory.path().join("baseline");
        std::fs::create_dir_all(&enabled).expect("enabled directory");
        std::fs::create_dir_all(&baseline).expect("baseline directory");
        let mut config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        config.activation.mode = ActivationMode::Selected;
        let workspace = discover(&config, &enabled)
            .await
            .expect("workspace identity");
        config.activation.enable(record(&workspace));

        assert!(is_enabled(&config, &enabled).await.expect("enabled check"));
        assert!(
            !is_enabled(&config, &baseline)
                .await
                .expect("baseline check")
        );
    }

    #[test]
    fn status_report_mirrors_config_activation_without_mutation() {
        let mut config = Config::default();
        config.activation.mode = ActivationMode::Selected;
        config.activation.enabled_workspaces.push(EnabledWorkspace {
            repository_id: "a".repeat(64),
            worktree_id: "b".repeat(64),
            root: "/work/app".into(),
        });

        let report = ActivationStatusReport::from_config(&config.activation);
        assert_eq!(report.mode, ActivationMode::Selected);
        assert_eq!(report.enabled_workspaces.len(), 1);
        assert_eq!(
            report.enabled_workspaces[0].root,
            PathBuf::from("/work/app")
        );
        assert_eq!(config.activation.enabled_workspaces.len(), 1);
    }

    #[test]
    fn human_status_lists_selected_roots() {
        let report = ActivationStatusReport {
            mode: ActivationMode::Selected,
            enabled_workspaces: vec![EnabledWorkspace {
                repository_id: "a".repeat(64),
                worktree_id: "b".repeat(64),
                root: "/work/app".into(),
            }],
        };
        let text = render_status_text(&report);
        assert!(text.contains("activation=selected"));
        assert!(text.contains("enabled workspaces (1):"));
        assert!(text.contains("  /work/app"));
    }

    #[test]
    fn human_status_explains_all_mode() {
        let report = ActivationStatusReport {
            mode: ActivationMode::All,
            enabled_workspaces: Vec::new(),
        };
        let text = render_status_text(&report);
        assert!(text.contains("activation=all"));
        assert!(text.contains("all projects"));
    }
}
