//! Detection of engine processes HZR does not supervise.
//!
//! PRD §4.3 forbids automatic cleanup and §11 forbids stopping external processes
//! implicitly, because a wrongly-killed watcher loses in-flight index state. But an
//! undetected duplicate is worse than a reported one: several `icm serve` processes
//! mean several writers to the memory store, and a stray `grepai watch` re-scans a
//! tree HZR already owns. So this module only *reports*, and stopping stays an
//! explicit, separately-confirmed operation.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Serialize;

/// Engine process shapes worth reporting, matched against the full command line.
const WATCHED: [(&str, &str); 2] = [("icm", "icm serve"), ("grepai", "grepai watch")];

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
    /// True when the command line points inside the HZR data root, i.e. HZR started it.
    pub managed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ForeignReport {
    pub data_root: PathBuf,
    pub processes: Vec<ForeignProcess>,
    /// Unmanaged count per engine — the number that duplicates HZR ownership.
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

/// Parse `ps -Ao pid=,command=` output. Kept separate from process execution so the
/// classification logic is testable without spawning anything.
fn parse_ps(output: &str, data_root: &str) -> Vec<ForeignProcess> {
    let mut processes = Vec::new();
    for line in output.lines() {
        let line = line.trim_start();
        let Some((pid, command)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        let command = command.trim();
        // Never report ourselves: `hzr doctor` contains these needles in its own argv.
        if command.contains("hzr doctor") || command.contains("hooks dispatch") {
            continue;
        }
        for (engine, needle) in WATCHED {
            if command.contains(needle) {
                let executable = command
                    .split_whitespace()
                    .next()
                    .and_then(|path| std::path::Path::new(path).file_name())
                    .and_then(|name| name.to_str());
                processes.push(ForeignProcess {
                    pid,
                    engine: engine.to_owned(),
                    command: command.to_owned(),
                    kind: if executable == Some(engine) {
                        ProcessKind::Engine
                    } else {
                        ProcessKind::Wrapper
                    },
                    managed: !data_root.is_empty() && command.contains(data_root),
                });
                break;
            }
        }
    }
    processes
}

pub fn scan(data_root: &std::path::Path) -> Result<ForeignReport> {
    let output = std::process::Command::new("ps")
        .args(["-Ao", "pid=,command="])
        .output()
        .context("failed to enumerate processes with `ps`")?;
    let text = String::from_utf8_lossy(&output.stdout);
    let root = data_root.to_string_lossy().to_string();
    let processes = parse_ps(&text, &root);

    let mut unmanaged_by_engine: BTreeMap<String, usize> = BTreeMap::new();
    let mut unmanaged_wrappers_by_engine: BTreeMap<String, usize> = BTreeMap::new();
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

    const ROOT: &str = "/home/u/Library/Application Support/dev.headz0r.hzr";

    #[test]
    fn test_detects_duplicate_icm_and_legacy_grepai() {
        let output = "\
  225 /usr/local/bin/icm serve
 8542 /usr/local/bin/icm serve
64443 /usr/local/bin/grepai watch
  777 /usr/bin/unrelated --icm-like
";
        let found = parse_ps(output, ROOT);
        assert_eq!(found.len(), 3, "only real engine processes count");
        assert_eq!(found.iter().filter(|p| p.engine == "icm").count(), 2);
        assert_eq!(found.iter().filter(|p| p.engine == "grepai").count(), 1);
        assert!(found.iter().all(|p| !p.managed));
        assert!(found.iter().all(|p| p.kind == ProcessKind::Engine));
    }

    #[test]
    fn test_distinguishes_client_wrapper_from_active_engine() {
        let output = "\
  225 /usr/local/bin/icm serve
  226 /bin/sh -c /usr/local/bin/icm serve
  227 /Applications/Claude.app/Claude --mcp /usr/local/bin/icm serve
";
        let found = parse_ps(output, ROOT);
        assert_eq!(found.len(), 3);
        assert_eq!(
            found
                .iter()
                .filter(|process| process.kind == ProcessKind::Engine)
                .count(),
            1
        );
        assert_eq!(
            found
                .iter()
                .filter(|process| process.kind == ProcessKind::Wrapper)
                .count(),
            2
        );
    }

    #[test]
    fn test_hzr_owned_watcher_is_not_reported_as_foreign() {
        let output = format!("72793 grepai watch --no-ui --log-dir {ROOT}/workspaces/a/b/index\n");
        let found = parse_ps(&output, ROOT);
        assert_eq!(found.len(), 1);
        assert!(
            found[0].managed,
            "a watcher inside the HZR data root is ours, not foreign"
        );
    }

    #[test]
    fn test_scan_never_reports_itself() {
        let output =
            "  10 /usr/local/bin/hzr doctor --json\n  11 /usr/local/bin/hzr hooks dispatch\n";
        assert!(
            parse_ps(output, ROOT).is_empty(),
            "HZR must not flag its own invocation"
        );
    }

    #[test]
    fn test_malformed_lines_are_skipped() {
        let output = "not-a-pid icm serve\n\n   \n99\n";
        assert!(parse_ps(output, ROOT).is_empty());
    }

    #[test]
    fn test_empty_data_root_marks_everything_foreign() {
        let output = "225 /usr/local/bin/icm serve\n";
        let found = parse_ps(output, "");
        assert_eq!(found.len(), 1);
        assert!(
            !found[0].managed,
            "without a known data root nothing may be assumed managed"
        );
    }
}
