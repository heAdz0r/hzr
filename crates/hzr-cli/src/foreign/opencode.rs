use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

type ProcessRow = (u32, u32, Vec<String>);

fn rows(text: &str) -> Vec<ProcessRow> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, rest) = line.split_once(char::is_whitespace)?;
            let (parent, command) = rest.trim_start().split_once(char::is_whitespace)?;
            Some((
                pid.parse().ok()?,
                parent.parse().ok()?,
                hzr_exec::parse_simple_shell(command.trim()).ok()?,
            ))
        })
        .collect()
}

fn snapshot() -> Result<Vec<ProcessRow>> {
    let output = Command::new("ps")
        .args(["-Ao", "pid=,ppid=,command="])
        .output()
        .context("cannot verify OpenCode engine process identity")?;
    anyhow::ensure!(output.status.success(), "process enumeration failed");
    Ok(rows(&String::from_utf8_lossy(&output.stdout)))
}

fn matches_registration(
    process: &ProcessRow,
    processes: &[ProcessRow],
    launches: &[Vec<String>],
) -> bool {
    let Some(program) = process.2.first() else {
        return false;
    };
    let is_opencode_child = processes
        .iter()
        .find(|row| row.0 == process.1)
        .and_then(|row| row.2.first())
        .is_some_and(|program| {
            Path::new(program)
                .file_name()
                .is_some_and(|name| name == "opencode")
        });
    is_opencode_child
        && launches.iter().any(|launch| {
            let Some(expected) = launch.first() else {
                return false;
            };
            let same_binary = if Path::new(expected).is_absolute() {
                program == expected
            } else {
                Path::new(program).file_name() == Some(std::ffi::OsStr::new(expected))
            };
            same_binary && process.2[1..] == launch[1..]
        })
}

#[cfg(unix)]
pub fn stop_disabled_opencode(launches: &[Vec<String>]) -> Result<Vec<u32>> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    use std::time::{Duration, Instant};

    let observed = snapshot()?;
    let candidates = observed
        .iter()
        .filter(|row| matches_registration(row, &observed, launches));
    let mut stopped = Vec::new();
    for candidate in candidates {
        let current = snapshot()?;
        if !current
            .iter()
            .any(|row| row == candidate && matches_registration(row, &current, launches))
        {
            continue;
        }
        let pid = Pid::from_raw(i32::try_from(candidate.0).context("invalid process id")?);
        match kill(pid, Signal::SIGTERM) {
            Ok(()) => {}
            Err(nix::errno::Errno::ESRCH) => {
                stopped.push(candidate.0);
                continue;
            }
            Err(error) => return Err(error).context("cannot stop disabled OpenCode ICM server"),
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if !snapshot()?.iter().any(|row| row == candidate) {
                stopped.push(candidate.0);
                break;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "disabled ICM process {} did not exit after SIGTERM; restart opencode and rerun doctor --fix",
                candidate.0
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Ok(stopped)
}

#[cfg(not(unix))]
pub fn stop_disabled_opencode(_launches: &[Vec<String>]) -> Result<Vec<u32>> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn verified_opencode_child_is_stopped_without_stopping_its_parent() {
        use std::os::unix::process::CommandExt;
        let directory = tempfile::tempdir().expect("fixture");
        let binary = directory.path().join("icm");
        std::os::unix::fs::symlink("/bin/sleep", &binary).expect("fixture executable");
        let script = directory.path().join("parent.sh");
        std::fs::write(&script, "\"$1\" 30 & wait\n").expect("parent script");
        let mut parent = Command::new("/bin/sh");
        parent.arg0("opencode").arg(&script).arg(&binary);
        let mut child = parent.spawn().expect("fixture parent");
        let launches = vec![vec![binary.to_string_lossy().into_owned(), "30".into()]];
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let current = snapshot().expect("process snapshot");
            if current
                .iter()
                .any(|row| matches_registration(row, &current, &launches))
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "fixture child not visible: {:?}",
                current
                    .iter()
                    .filter(|row| row.0 == child.id() || row.1 == child.id())
                    .collect::<Vec<_>>()
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let stopped = stop_disabled_opencode(&launches).expect("stop verified child");
        assert_eq!(stopped.len(), 1);
        assert_ne!(stopped[0], child.id());
        child.wait().expect("parent reaps child and exits");
    }

    #[test]
    fn only_exact_disabled_registration_with_opencode_parent_is_stoppable() {
        let processes = rows(
            "10 1 /opt/opencode\n11 10 /opt/icm serve\n12 1 /opt/hzrd\n13 12 /opt/icm serve\n14 10 /other/icm serve\n15 10 /opt/icm --db private serve",
        );
        let allowed = vec![vec!["/opt/icm".into(), "serve".into()]];
        let selected = processes
            .iter()
            .filter(|row| matches_registration(row, &processes, &allowed))
            .map(|row| row.0)
            .collect::<Vec<_>>();
        assert_eq!(selected, [11]);
        let without_parent = processes
            .iter()
            .filter(|row| row.0 != 10)
            .cloned()
            .collect::<Vec<_>>();
        assert!(!matches_registration(
            &processes[1],
            &without_parent,
            &allowed
        ));
    }
}
