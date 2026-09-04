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
    pub engine: String,
    pub command: String,
    pub kind: ProcessKind,
    pub managed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ForeignReport {
    pub data_root: PathBuf,
    pub processes: Vec<ForeignProcess>,
    pub unmanaged_by_engine: BTreeMap<String, usize>,
    pub unmanaged_wrappers_by_engine: BTreeMap<String, usize>,
}

impl ForeignReport {
    pub fn unmanaged_active_total(&self) -> usize {
        self.unmanaged_by_engine.values().sum()
    }
    pub fn unmanaged_wrapper_total(&self) -> usize {
        self.unmanaged_wrappers_by_engine.values().sum()
    }
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
            let daemon_parent = rows.iter().any(|(candidate, _, argv)| {
                candidate == parent
                    && argv.first().and_then(|token| executable(token)) == Some("hzrd")
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
            Some(ForeignProcess {
                pid: *pid,
                engine: engine.to_owned(),
                command: format!("{engine} {command} <arguments omitted>"),
                kind,
                managed,
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
    let processes = parse_ps(&String::from_utf8_lossy(&output.stdout), data_root);
    let mut unmanaged_by_engine = BTreeMap::new();
    let mut unmanaged_wrappers_by_engine = BTreeMap::new();
    for process in &processes {
        if !process.managed {
            let target = match process.kind {
                ProcessKind::Engine => &mut unmanaged_by_engine,
                ProcessKind::Wrapper => &mut unmanaged_wrappers_by_engine,
            };
            *target.entry(process.engine.clone()).or_insert(0) += 1;
        }
    }
    Ok(ForeignReport {
        data_root: data_root.to_path_buf(),
        processes,
        unmanaged_by_engine,
        unmanaged_wrappers_by_engine,
    })
}

#[cfg(test)]
mod tests {
    use super::{ProcessKind, parse_ps};
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

    #[test]
    fn ignores_malformed_lines_and_unrelated_commands() {
        assert!(parse_ps("bad icm serve\n99\n10 1 hzr doctor --json\n", Path::new("")).is_empty());
    }
}
