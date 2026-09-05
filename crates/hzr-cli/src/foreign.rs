//! Read-only detection of engine processes outside verified HZR ownership.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessKind {
    Engine,
    Wrapper,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ForeignProcess {
    pub pid: u32,
    // 0.8.1: parent and argv[0] are retained so an orphan can be re-verified before it is stopped.
    pub parent: u32,
    pub engine: String,
    pub command: String,
    pub executable: String,
    pub kind: ProcessKind,
    pub managed: bool,
    /// Launched from an HZR installation that no longer exists (for example a removed release
    /// smoke fixture) and reparented to init. Only these may be stopped by `hzr doctor --fix`.
    pub orphaned: bool,
    pub orphan_reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ForeignReport {
    pub data_root: PathBuf,
    pub processes: Vec<ForeignProcess>,
    pub unmanaged_by_engine: BTreeMap<String, usize>,
    pub unmanaged_wrappers_by_engine: BTreeMap<String, usize>,
    // 0.8.1: orphaned HZR launches are reported separately from truly foreign processes.
    pub orphaned_by_engine: BTreeMap<String, usize>,
}

impl ForeignReport {
    pub fn unmanaged_active_total(&self) -> usize {
        self.unmanaged_by_engine.values().sum()
    }
    pub fn unmanaged_wrapper_total(&self) -> usize {
        self.unmanaged_wrappers_by_engine.values().sum()
    }
    pub fn orphaned_total(&self) -> usize {
        self.orphaned_by_engine.values().sum()
    }
    pub fn engine_summary(counts: &BTreeMap<String, usize>) -> String {
        counts
            .iter()
            .map(|(engine, count)| format!("{engine}={count}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrphanStopState {
    Terminated,
    Killed,
    AlreadyGone,
    IdentityChanged,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OrphanStopOutcome {
    pub pid: u32,
    pub engine: String,
    pub state: OrphanStopState,
    pub detail: String,
}

/// Path argument following `flag` (joined until the next option or subcommand word).
fn path_argument(tokens: &[String], flag: &str) -> Option<PathBuf> {
    let index = tokens.iter().position(|token| token == flag)?;
    let path = tokens[index + 1..]
        .iter()
        .take_while(|token| !token.starts_with('-') && !matches!(token.as_str(), "serve" | "watch"))
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// An orphan is proven only by an HZR launch layout plus a removed launcher: argv[0] must sit in
/// an HZR `engines` directory, the process must have lost its parent, and the executable or the
/// data it was started with must be gone. Every other process stays foreign and is never stopped.
fn orphan_reason(
    engine: &str,
    parent: u32,
    parent_alive: bool,
    tokens: &[String],
) -> Option<String> {
    if parent != 1 && parent_alive {
        return None;
    }
    let executable = Path::new(tokens.first()?);
    if !executable.is_absolute() {
        return None;
    }
    let components = executable
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    let hzr_layout = components.iter().any(|component| {
        *component == "hzr"
            || *component == "dev.headz0r.hzr"
            || component.starts_with("hzr-install-smoke.")
    });
    if !hzr_layout || !components.contains(&"engines") {
        return None;
    }
    if !executable.exists() {
        return Some(format!(
            "launcher installation removed: {} no longer exists",
            executable.display()
        ));
    }
    // A release smoke fixture is never a production installation; once its daemon is gone the
    // engine it launched has no owner even if the fixture directory was retained for inspection.
    if components
        .iter()
        .any(|component| component.starts_with("hzr-install-smoke."))
    {
        return Some(format!(
            "release smoke fixture launch without a daemon: {}",
            executable.display()
        ));
    }
    let data = match engine {
        "icm" => path_argument(tokens, "--db"),
        "grepai" => {
            path_argument(tokens, "--log-dir").or_else(|| path_argument(tokens, "--config"))
        }
        _ => None,
    }?;
    (!data.exists()).then(|| format!("launcher data removed: {} no longer exists", data.display()))
}

fn executable(token: &str) -> Option<&str> {
    Path::new(token).file_name().and_then(|name| name.to_str())
}

fn engine_command(tokens: &[String]) -> Option<(&'static str, &'static str, ProcessKind)> {
    for (index, token) in tokens.iter().enumerate() {
        let (engine, command) = match executable(token)? {
            "icm" => ("icm", "serve"),
            "grepai" => ("grepai", "watch"),
            _ => continue,
        };
        let mut arguments = tokens[index + 1..].iter();
        while let Some(argument) = arguments.next() {
            if argument == command {
                return Some((
                    engine,
                    command,
                    if index == 0 {
                        ProcessKind::Engine
                    } else {
                        ProcessKind::Wrapper
                    },
                ));
            }
            if argument == "--db" || argument == "--config" {
                arguments.next();
                // ps does not quote spaces in argv; skip the remainder of the option value.
                while let Some(next) = arguments.clone().next() {
                    if next == command || next.starts_with('-') {
                        break;
                    }
                    if matches!(
                        next.as_str(),
                        "recall" | "store" | "list" | "init" | "search" | "status"
                    ) {
                        return None;
                    }
                    arguments.next();
                }
            } else if !argument.starts_with('-') {
                break;
            }
        }
    }
    None
}

fn parse_ps(output: &str, data_root: &Path) -> Vec<ForeignProcess> {
    let rows = output
        .lines()
        .filter_map(|line| {
            let mut fields = line.trim_start().splitn(3, char::is_whitespace);
            let pid = fields.next()?.parse::<u32>().ok()?;
            let rest = line
                .trim_start()
                .strip_prefix(&pid.to_string())?
                .trim_start();
            let (parent, command) = rest.split_once(char::is_whitespace)?;
            let parent = parent.parse::<u32>().ok()?;
            let command = command.trim();
            let tokens = hzr_exec::parse_simple_shell(command)
                .unwrap_or_else(|_| command.split_whitespace().map(str::to_owned).collect());
            Some((pid, parent, tokens))
        })
        .collect::<Vec<_>>();
    rows.iter()
        .filter_map(|(pid, parent, tokens)| {
            let found = engine_command(tokens).or_else(|| {
                let shell = tokens.first().and_then(|token| executable(token));
                if matches!(shell, Some("sh" | "bash" | "zsh")) {
                    let body = tokens.windows(2).find(|pair| pair[0] == "-c")?;
                    let nested = hzr_exec::parse_simple_shell(&body[1]).ok()?;
                    engine_command(&nested)
                        .map(|(engine, command, _)| (engine, command, ProcessKind::Wrapper))
                } else {
                    None
                }
            })?;
            let (engine, command, kind) = found;
            let parent_row = rows.iter().find(|(candidate, _, _)| candidate == parent);
            let daemon_parent = parent_row.is_some_and(|(_, _, argv)| {
                argv.first().and_then(|token| executable(token)) == Some("hzrd")
            });
            let managed = kind == ProcessKind::Engine
                && daemon_parent
                && match engine {
                    "icm" => {
                        hzr_memory::is_managed_icm_process(data_root, *pid)
                            || legacy_icm_identity(data_root, *pid, tokens)
                    }
                    "grepai" => watcher_identity(data_root, *pid, tokens),
                    _ => false,
                };
            // 0.8.1: classify surviving HZR launches whose installation is gone.
            let orphan_reason = if managed || kind != ProcessKind::Engine {
                None
            } else {
                orphan_reason(engine, *parent, parent_row.is_some(), tokens)
            };
            Some(ForeignProcess {
                pid: *pid,
                parent: *parent,
                engine: engine.to_owned(),
                command: format!("{engine} {command} <arguments omitted>"),
                executable: tokens.first().cloned().unwrap_or_default(),
                kind,
                managed,
                orphaned: orphan_reason.is_some(),
                orphan_reason,
            })
        })
        .collect()
}

fn legacy_icm_identity(data_root: &Path, pid: u32, tokens: &[String]) -> bool {
    let pid_path = data_root.join("memory/icm/runtime/icm.pid");
    if !std::fs::symlink_metadata(&pid_path)
        .is_ok_and(|metadata| metadata.file_type().is_file() && metadata.len() <= 32)
        || !std::fs::read_to_string(&pid_path)
            .is_ok_and(|recorded| recorded.trim().parse::<u32>().ok() == Some(pid))
    {
        return false;
    }
    let Some(index) = tokens.iter().position(|token| token == "--db") else {
        return false;
    };
    let path = tokens[index + 1..]
        .iter()
        .take_while(|token| !token.starts_with('-') && token.as_str() != "serve")
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    match (
        Path::new(&path).canonicalize(),
        data_root.join("memory/icm/memories.db").canonicalize(),
    ) {
        (Ok(actual), Ok(expected)) => actual == expected,
        _ => false,
    }
}

fn watcher_identity(data_root: &Path, pid: u32, tokens: &[String]) -> bool {
    let Some(index) = tokens.iter().position(|token| token == "--log-dir") else {
        return false;
    };
    let path = tokens[index + 1..]
        .iter()
        .take_while(|token| !token.starts_with('-'))
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let Ok(root) = data_root.join("workspaces").canonicalize() else {
        return false;
    };
    let Ok(runtime) = Path::new(&path).canonicalize() else {
        return false;
    };
    if !runtime.starts_with(root) {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(runtime) else {
        return false;
    };
    entries.take(32).filter_map(Result::ok).any(|entry| {
        if entry.path().extension().and_then(|value| value.to_str()) != Some("ready")
            || !entry.file_type().is_ok_and(|kind| kind.is_file())
            || !entry.metadata().is_ok_and(|meta| meta.len() <= 256)
        {
            return false;
        }
        std::fs::read_to_string(entry.path()).is_ok_and(|content| {
            let mut lines = content.lines();
            lines.next() == Some("ready")
                && lines.next().and_then(|line| line.parse::<u32>().ok()) == Some(pid)
        })
    })
}

pub fn scan(data_root: &Path) -> Result<ForeignReport> {
    let output = std::process::Command::new("ps")
        .args(["-Ao", "pid=,ppid=,command="])
        .output()
        .context("failed to enumerate processes with ps")?;
    anyhow::ensure!(
        output.status.success(),
        "process enumeration failed with {}",
        output.status
    );
    Ok(report_from_processes(
        data_root,
        parse_ps(&String::from_utf8_lossy(&output.stdout), data_root),
    ))
}

fn report_from_processes(data_root: &Path, processes: Vec<ForeignProcess>) -> ForeignReport {
    let mut unmanaged_by_engine = BTreeMap::new();
    let mut unmanaged_wrappers_by_engine = BTreeMap::new();
    let mut orphaned_by_engine = BTreeMap::new();
    for process in &processes {
        if process.managed {
            continue;
        }
        // 0.8.1: an orphaned HZR launch is not counted as a foreign duplicate owner.
        let target = match (process.kind, process.orphaned) {
            (ProcessKind::Engine, true) => &mut orphaned_by_engine,
            (ProcessKind::Engine, false) => &mut unmanaged_by_engine,
            (ProcessKind::Wrapper, _) => &mut unmanaged_wrappers_by_engine,
        };
        *target.entry(process.engine.clone()).or_insert(0) += 1;
    }
    ForeignReport {
        data_root: data_root.to_path_buf(),
        processes,
        unmanaged_by_engine,
        unmanaged_wrappers_by_engine,
        orphaned_by_engine,
    }
}

/// Stop the orphaned HZR launches in `report`. Each PID is re-read from `ps` and must still
/// carry the same argv[0] and orphan classification, so a recycled PID is never signalled.
#[cfg(unix)]
pub fn stop_orphaned(report: &ForeignReport) -> Vec<OrphanStopOutcome> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    use std::time::{Duration, Instant};

    let mut outcomes = Vec::new();
    for process in report.processes.iter().filter(|process| process.orphaned) {
        let pid = Pid::from_raw(process.pid as i32);
        let current = std::process::Command::new("ps")
            .args(["-o", "pid=,ppid=,command=", "-p", &process.pid.to_string()])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| parse_ps(&String::from_utf8_lossy(&output.stdout), &report.data_root))
            .unwrap_or_default();
        let Some(live) = current
            .into_iter()
            .find(|candidate| candidate.pid == process.pid)
        else {
            outcomes.push(OrphanStopOutcome {
                pid: process.pid,
                engine: process.engine.clone(),
                state: OrphanStopState::AlreadyGone,
                detail: "process exited before it was signalled".into(),
            });
            continue;
        };
        if !live.orphaned || live.executable != process.executable {
            outcomes.push(OrphanStopOutcome {
                pid: process.pid,
                engine: process.engine.clone(),
                state: OrphanStopState::IdentityChanged,
                detail: "PID no longer identifies the orphaned launch; left untouched".into(),
            });
            continue;
        }
        let wait_exit = |limit: Duration| {
            let deadline = Instant::now() + limit;
            loop {
                if kill(pid, None) == Err(nix::errno::Errno::ESRCH) {
                    return true;
                }
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        let outcome = match kill(pid, Signal::SIGTERM) {
            Err(nix::errno::Errno::ESRCH) => OrphanStopOutcome {
                pid: process.pid,
                engine: process.engine.clone(),
                state: OrphanStopState::AlreadyGone,
                detail: "process exited before it was signalled".into(),
            },
            Err(error) => OrphanStopOutcome {
                pid: process.pid,
                engine: process.engine.clone(),
                state: OrphanStopState::Failed,
                detail: format!("SIGTERM failed: {error}"),
            },
            Ok(()) if wait_exit(Duration::from_secs(3)) => OrphanStopOutcome {
                pid: process.pid,
                engine: process.engine.clone(),
                state: OrphanStopState::Terminated,
                detail: process
                    .orphan_reason
                    .clone()
                    .unwrap_or_else(|| "orphaned HZR launch".into()),
            },
            Ok(()) => match kill(pid, Signal::SIGKILL) {
                Ok(()) | Err(nix::errno::Errno::ESRCH) if wait_exit(Duration::from_secs(2)) => {
                    OrphanStopOutcome {
                        pid: process.pid,
                        engine: process.engine.clone(),
                        state: OrphanStopState::Killed,
                        detail: "ignored SIGTERM for 3s; SIGKILL delivered".into(),
                    }
                }
                Ok(()) => OrphanStopOutcome {
                    pid: process.pid,
                    engine: process.engine.clone(),
                    state: OrphanStopState::Failed,
                    detail: "SIGKILL delivered but the process is still listed".into(),
                },
                Err(error) => OrphanStopOutcome {
                    pid: process.pid,
                    engine: process.engine.clone(),
                    state: OrphanStopState::Failed,
                    detail: format!("SIGKILL failed: {error}"),
                },
            },
        };
        outcomes.push(outcome);
    }
    outcomes
}

#[cfg(not(unix))]
pub fn stop_orphaned(report: &ForeignReport) -> Vec<OrphanStopOutcome> {
    report
        .processes
        .iter()
        .filter(|process| process.orphaned)
        .map(|process| OrphanStopOutcome {
            pid: process.pid,
            engine: process.engine.clone(),
            state: OrphanStopState::Failed,
            detail: "stopping orphaned engines is unsupported on this platform".into(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ProcessKind, parse_ps, report_from_processes};
    use std::path::Path;

    #[test]
    fn detects_options_before_subcommand_and_redacts_secrets() {
        let processes = parse_ps(
            "225 1 /managed/icm --db /tmp/old smoke/memories.db --no-embeddings serve --token SECRET\n226 1 grepai --config /tmp/config watch\n227 1 icm recall serve\n228 1 printf icm-like\n",
            Path::new("/tmp/data"),
        );
        assert_eq!(processes.len(), 2);
        assert!(
            processes
                .iter()
                .all(|p| p.kind == ProcessKind::Engine && !p.managed)
        );
        assert!(!format!("{processes:?}").contains("SECRET"));
    }

    #[test]
    fn command_text_or_data_root_substrings_never_prove_ownership() {
        let processes = parse_ps(
            "225 1 /managed/icm --db /tmp/data/memory/icm/memories.db serve\n226 10 grepai watch --log-dir /tmp/data-evil/workspaces/x\n10 1 hzrd\n",
            Path::new("/tmp/data"),
        );
        assert_eq!(processes.len(), 2);
        assert!(processes.iter().all(|p| !p.managed));
    }

    #[test]
    fn distinguishes_shell_and_client_wrappers() {
        let processes = parse_ps(
            "225 1 /usr/local/bin/icm serve\n226 1 /bin/sh -c 'icm --db /tmp/db serve'\n227 1 /Applications/Claude.app/Claude --mcp /usr/local/bin/icm serve\n",
            Path::new("/tmp/data"),
        );
        assert_eq!(processes.len(), 3);
        assert_eq!(
            processes
                .iter()
                .filter(|p| p.kind == ProcessKind::Wrapper)
                .count(),
            2
        );
    }

    #[test]
    fn verifies_watcher_pid_marker_and_parent() {
        let root = tempfile::tempdir().expect("root");
        let runtime = root.path().join("workspaces/a/runtime");
        std::fs::create_dir_all(&runtime).expect("runtime");
        std::fs::write(runtime.join("watch.ready"), "ready\n225\n").expect("marker");
        let input = format!(
            "10 1 /managed/hzrd\n225 10 /managed/grepai watch --log-dir {}\n226 1 /managed/grepai watch --log-dir {}\n",
            runtime.display(),
            runtime.display()
        );
        let processes = parse_ps(&input, root.path());
        assert!(processes[0].managed);
        assert!(!processes[1].managed);
    }

    #[test]
    fn legacy_icm_requires_pid_database_and_daemon_parent() {
        let root = tempfile::tempdir().expect("root");
        let database = root.path().join("memory/icm/memories.db");
        let runtime = root.path().join("memory/icm/runtime");
        std::fs::create_dir_all(&runtime).expect("runtime");
        std::fs::write(&database, "").expect("database");
        std::fs::write(runtime.join("icm.pid"), "225").expect("PID");
        let input = format!(
            "10 1 /managed/hzrd\n225 10 /managed/icm --db {} --no-embeddings serve\n226 10 /managed/icm --db {} serve\n225 1 /managed/icm --db {} serve\n",
            database.display(),
            database.display(),
            database.display()
        );
        let processes = parse_ps(&input, root.path());
        assert_eq!(processes.len(), 3);
        assert!(processes[0].managed);
        assert!(!processes[1].managed);
        assert!(!processes[2].managed);
    }

    // 0.8.1: a removed installation whose engine child survives is an orphan, not a foreign owner.
    #[test]
    fn removed_hzr_launch_reparented_to_init_is_an_orphan() {
        let root = tempfile::tempdir().expect("root");
        let removed = root
            .path()
            .join("hzr-install-smoke.abc/home/.local/share/hzr/current/engines/icm");
        let removed_db = root
            .path()
            .join("hzr-install-smoke.abc/home/data/memories.db");
        let input = format!(
            "225 1 {} --db {} --no-embeddings serve --http 127.0.0.1:1 --token SECRET\n226 1 /usr/local/bin/icm --db /tmp/x serve\n42 1 /bin/sleep 100\n227 42 {} --db {} serve\n",
            removed.display(),
            removed_db.display(),
            removed.display(),
            removed_db.display()
        );
        let processes = parse_ps(&input, Path::new("/tmp/data"));
        assert_eq!(processes.len(), 3);
        assert!(processes[0].orphaned, "{:?}", processes[0]);
        assert!(
            processes[0]
                .orphan_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("no longer exists"))
        );
        assert!(
            !processes[1].orphaned,
            "a non-HZR path is foreign, never an orphan"
        );
        assert!(
            !processes[2].orphaned,
            "a live parent that is not hzrd keeps ownership ambiguous"
        );
        let report = report_from_processes(Path::new("/tmp/data"), processes);
        assert_eq!(report.orphaned_total(), 1);
        assert_eq!(report.unmanaged_active_total(), 2);
    }

    #[test]
    fn retained_smoke_fixture_launch_without_parent_is_an_orphan() {
        let root = tempfile::tempdir().expect("root");
        let engines = root
            .path()
            .join("hzr-install-smoke.keep/home/.local/share/hzr/current/engines");
        std::fs::create_dir_all(&engines).expect("engines");
        let binary = engines.join("icm");
        std::fs::write(&binary, "").expect("binary");
        let database = root.path().join("hzr-install-smoke.keep/memories.db");
        std::fs::write(&database, "").expect("database");
        let input = format!(
            "225 1 {} --db {} serve\n",
            binary.display(),
            database.display()
        );
        let processes = parse_ps(&input, Path::new("/tmp/data"));
        assert_eq!(processes.len(), 1);
        assert!(processes[0].orphaned, "{:?}", processes[0]);
    }

    #[test]
    fn existing_hzr_launch_with_live_data_is_not_an_orphan() {
        let root = tempfile::tempdir().expect("root");
        let engines = root.path().join("share/hzr/current/engines");
        std::fs::create_dir_all(&engines).expect("engines");
        let binary = engines.join("icm");
        std::fs::write(&binary, "").expect("binary");
        let database = root.path().join("memories.db");
        std::fs::write(&database, "").expect("database");
        let input = format!(
            "225 1 {} --db {} serve\n",
            binary.display(),
            database.display()
        );
        let processes = parse_ps(&input, Path::new("/tmp/data"));
        assert_eq!(processes.len(), 1);
        assert!(!processes[0].orphaned);
        assert!(!processes[0].managed);
    }

    #[test]
    fn ignores_malformed_lines_and_unrelated_commands() {
        assert!(parse_ps("bad icm serve\n99\n10 1 hzr doctor --json\n", Path::new("")).is_empty());
    }
}
