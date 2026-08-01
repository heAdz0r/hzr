use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::process::{Child, Command};
use tokio::time::{Instant, sleep, timeout};

use crate::error::{IndexError, Result};
use crate::grepai::Deadlines;
use crate::owner::IndexOwner;
use crate::process;
use crate::workspace::Workspace;

pub(crate) async fn start(
    binary: &Path,
    workspace: &Workspace,
    deadlines: &Deadlines,
    isolated_worktree: bool,
) -> Result<WatchHandle> {
    let owner = IndexOwner::acquire(workspace)?;
    let runtime_dir = watch_runtime_dir(workspace);
    std::fs::create_dir_all(&runtime_dir).map_err(|source| IndexError::Io {
        operation: "create grepai watch runtime",
        path: runtime_dir.clone(),
        source,
    })?;
    let log_path = runtime_dir.join("grepai-watch.log");
    let stdout = open_log(&log_path)?;
    let stderr = stdout.try_clone().map_err(|source| IndexError::Io {
        operation: "clone grepai watch log handle",
        path: log_path.clone(),
        source,
    })?;
    let mut command = Command::new(binary);
    command
        .args(["watch", "--no-ui", "--log-dir"])
        .arg(&runtime_dir);
    if isolated_worktree {
        command.arg(crate::grepai::SINGLE_WORKTREE_WATCH_FLAG);
    }
    command
        .current_dir(&workspace.identity.root)
        .env("GREPAI_BACKGROUND", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|source| IndexError::CommandUnavailable {
            operation: "start grepai watch",
            program: binary.to_path_buf(),
            source,
        })?;
    let pid = child.id().ok_or_else(|| IndexError::InvalidEngineOutput {
        engine: "grepai",
        operation: "start watch",
        detail: "spawned watcher has no process id".into(),
    })?;
    if let Err(error) = wait_until_ready(
        &mut child,
        pid,
        &runtime_dir,
        &log_path,
        deadlines.watch_start,
    )
    .await
    {
        let _ = child.kill().await;
        return Err(error);
    }

    Ok(WatchHandle {
        binary: binary.to_path_buf(),
        root: workspace.identity.root.clone(),
        runtime_dir,
        log_path,
        child,
        _owner: owner,
        stop_deadline: deadlines.watch_stop,
        started_at: Instant::now(),
        stopped: false,
    })
}

pub struct WatchHandle {
    binary: PathBuf,
    root: PathBuf,
    runtime_dir: PathBuf,
    log_path: PathBuf,
    child: Child,
    _owner: IndexOwner,
    stop_deadline: Duration,
    started_at: Instant,
    stopped: bool,
}

impl WatchHandle {
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    pub fn uptime(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub fn is_running(&mut self) -> Result<bool> {
        self.child
            .try_wait()
            .map(|status| status.is_none())
            .map_err(|source| IndexError::Io {
                operation: "check grepai watch process",
                path: self.binary.clone(),
                source,
            })
    }

    pub async fn shutdown(mut self) -> Result<()> {
        let args = [
            OsString::from("watch"),
            OsString::from("--stop"),
            OsString::from("--log-dir"),
            self.runtime_dir.as_os_str().to_owned(),
        ];
        let stop_result = process::output(
            &self.binary,
            &args,
            &self.root,
            self.stop_deadline,
            "stop grepai watch",
        )
        .await
        .and_then(|output| process::require_success(output, "stop grepai watch"));

        if stop_result.is_ok() {
            match timeout(self.stop_deadline, self.child.wait()).await {
                Ok(Ok(_)) => {
                    self.stopped = true;
                    return Ok(());
                }
                Ok(Err(source)) => {
                    let _ = self.child.kill().await;
                    self.stopped = true;
                    return Err(IndexError::Io {
                        operation: "wait for grepai watch shutdown",
                        path: self.binary.clone(),
                        source,
                    });
                }
                Err(_) => {
                    let _ = self.child.kill().await;
                    let _ = self.child.wait().await;
                    self.stopped = true;
                    return Err(IndexError::DeadlineExceeded {
                        operation: "stop grepai watch",
                        duration: self.stop_deadline,
                    });
                }
            }
        }

        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        self.stopped = true;
        stop_result.map(|_| ())
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        if !self.stopped {
            let _ = self.child.start_kill();
        }
    }
}

fn open_log(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|source| IndexError::Io {
            operation: "create grepai watch log",
            path: path.to_path_buf(),
            source,
        })
}

fn watch_runtime_dir(workspace: &Workspace) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    workspace.index.directory.join("hzr-runtime").join(format!(
        "{}-{}-{nanos}",
        &workspace.identity.worktree_id[..12],
        std::process::id()
    ))
}

async fn wait_until_ready(
    child: &mut Child,
    pid: u32,
    runtime_dir: &Path,
    log_path: &Path,
    deadline: Duration,
) -> Result<()> {
    let expires = Instant::now() + deadline;
    loop {
        if ready_pid(runtime_dir)? == Some(pid) {
            return Ok(());
        }
        if let Some(status) = child.try_wait().map_err(|source| IndexError::Io {
            operation: "check grepai watch startup",
            path: log_path.to_path_buf(),
            source,
        })? {
            return Err(IndexError::WatchExited {
                code: status.code(),
                log_path: log_path.to_path_buf(),
            });
        }
        if Instant::now() >= expires {
            return Err(IndexError::DeadlineExceeded {
                operation: "start grepai watch",
                duration: deadline,
            });
        }
        sleep(Duration::from_millis(50)).await;
    }
}

fn ready_pid(runtime_dir: &Path) -> Result<Option<u32>> {
    let entries = std::fs::read_dir(runtime_dir).map_err(|source| IndexError::Io {
        operation: "read grepai watch runtime",
        path: runtime_dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| IndexError::Io {
            operation: "read grepai watch runtime entry",
            path: runtime_dir.to_path_buf(),
            source,
        })?;
        if entry.path().extension().and_then(|value| value.to_str()) != Some("ready") {
            continue;
        }
        let content = std::fs::read_to_string(entry.path()).map_err(|source| IndexError::Io {
            operation: "read grepai ready marker",
            path: entry.path(),
            source,
        })?;
        let mut lines = content.lines();
        if lines.next() != Some("ready") {
            return Err(IndexError::InvalidEngineOutput {
                engine: "grepai",
                operation: "read watch ready marker",
                detail: format!("invalid marker at {}", entry.path().display()),
            });
        }
        let pid = lines
            .next()
            .ok_or_else(|| IndexError::InvalidEngineOutput {
                engine: "grepai",
                operation: "read watch ready marker",
                detail: format!("missing pid at {}", entry.path().display()),
            })?
            .parse::<u32>()
            .map_err(|error| IndexError::InvalidEngineOutput {
                engine: "grepai",
                operation: "read watch ready marker",
                detail: error.to_string(),
            })?;
        return Ok(Some(pid));
    }
    Ok(None)
}
