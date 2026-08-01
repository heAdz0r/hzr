use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::process::Stdio;
use std::time::{Duration, Instant};

use fs2::FileExt;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::client::IcmClient;
use crate::config::IcmConfig;
use crate::error::{MemoryError, Result};
use crate::installation::verify_installation;
use crate::layout::IcmLayout;
use crate::mcp;
use crate::types::{IcmTransport, ServiceHealth};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceStatus {
    Stopped,
    Running { pid: u32, health: ServiceHealth },
    Attached { health: ServiceHealth },
    Unready { pid: Option<u32>, reason: String },
    Exited { code: Option<i32> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartOutcome {
    Started { pid: u32, health: ServiceHealth },
    AlreadyRunning { pid: u32, health: ServiceHealth },
    Attached { health: ServiceHealth },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    Stopped,
    AlreadyStopped,
    Detached,
}

enum ManagedProcess {
    Stopped,
    Owned { child: Child, lock: File, pid: u32 },
    Attached { lock: Option<File> },
}

enum LockAcquisition {
    Owned(File),
    Attached(ServiceHealth),
}

pub struct IcmSupervisor {
    config: IcmConfig,
    layout: IcmLayout,
    client: IcmClient,
    lifecycle: Mutex<()>,
    process: Mutex<ManagedProcess>,
}

impl std::fmt::Debug for IcmSupervisor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IcmSupervisor")
            .field("executable", &self.config.executable)
            .field("endpoint", &self.client.endpoint())
            .field("database", &self.layout.database)
            .finish_non_exhaustive()
    }
}

impl IcmSupervisor {
    pub fn new(config: IcmConfig) -> Result<Self> {
        config.validate()?;
        let layout = IcmLayout::prepare(config.data_root())?;
        let token = layout.load_or_create_token()?;
        let client = IcmClient::new(config.clone(), layout.clone(), token, mcp::shared())?;
        Ok(Self {
            config,
            layout,
            client,
            lifecycle: Mutex::new(()),
            process: Mutex::new(ManagedProcess::Stopped),
        })
    }

    pub fn client(&self) -> IcmClient {
        self.client.clone()
    }

    pub fn layout(&self) -> &IcmLayout {
        &self.layout
    }

    pub async fn start(&self) -> Result<StartOutcome> {
        let _lifecycle = self.lifecycle.lock().await;
        self.start_locked().await
    }

    async fn start_locked(&self) -> Result<StartOutcome> {
        let mut process = self.process.lock().await;
        match &mut *process {
            ManagedProcess::Owned { child, pid, .. } => {
                if let Some(status) = child.try_wait().map_err(process_control)? {
                    let code = status.code();
                    release_owned(&mut process, &self.layout);
                    tracing::warn!(
                        ?code,
                        "previous ICM process had exited; starting a replacement"
                    );
                } else {
                    let health = self.client.readiness().await?;
                    return Ok(StartOutcome::AlreadyRunning { pid: *pid, health });
                }
            }
            ManagedProcess::Attached { .. } => match self.client.readiness().await {
                Ok(health) => return Ok(StartOutcome::Attached { health }),
                Err(error) => {
                    tracing::warn!(%error, "attached ICM process is no longer ready");
                    *process = ManagedProcess::Stopped;
                }
            },
            ManagedProcess::Stopped => {}
        }

        verify_installation(&self.config).await?;
        let lock = match self.acquire_lock().await? {
            LockAcquisition::Owned(lock) => lock,
            LockAcquisition::Attached(health) => {
                *process = ManagedProcess::Attached { lock: None };
                return Ok(StartOutcome::Attached { health });
            }
        };
        let orphan_health =
            if self.config.transport == IcmTransport::Http && self.layout.pid_file.exists() {
                self.client.readiness().await.ok()
            } else {
                None
            };
        if let Some(health) = orphan_health {
            *process = ManagedProcess::Attached { lock: Some(lock) };
            return Ok(StartOutcome::Attached { health });
        }
        let token = self.layout.load_or_create_token()?;
        let mut child = self.spawn(&token)?;
        let pid = child.id().ok_or_else(|| MemoryError::UnexpectedResponse {
            operation: "process start",
            message: "spawned ICM process did not expose a process ID".into(),
        })?;
        if let Err(error) = fs::write(&self.layout.pid_file, pid.to_string()) {
            let _ = terminate_child(&mut child, self.config.shutdown_timeout).await;
            return Err(MemoryError::Io {
                operation: "write ICM PID file",
                path: self.layout.pid_file.clone(),
                source: error,
            });
        }

        let health = match self.wait_ready(&mut child).await {
            Ok(health) => health,
            Err(error) => {
                let _ = terminate_child(&mut child, self.config.shutdown_timeout).await;
                let _ = fs::remove_file(&self.layout.pid_file);
                fs2::FileExt::unlock(&lock).map_err(process_control)?;
                return Err(error);
            }
        };
        *process = ManagedProcess::Owned { child, lock, pid };
        Ok(StartOutcome::Started { pid, health })
    }

    pub async fn stop(&self) -> Result<StopOutcome> {
        let _lifecycle = self.lifecycle.lock().await;
        self.stop_locked().await
    }

    async fn stop_locked(&self) -> Result<StopOutcome> {
        let managed = {
            let mut process = self.process.lock().await;
            std::mem::replace(&mut *process, ManagedProcess::Stopped)
        };
        match managed {
            ManagedProcess::Stopped => Ok(StopOutcome::AlreadyStopped),
            ManagedProcess::Attached { lock } => {
                if let Some(lock) = lock {
                    fs2::FileExt::unlock(&lock).map_err(process_control)?;
                }
                Ok(StopOutcome::Detached)
            }
            ManagedProcess::Owned {
                mut child, lock, ..
            } => {
                self.client.disconnect_mcp().await;
                let termination = terminate_child(&mut child, self.config.shutdown_timeout).await;
                let _ = fs::remove_file(&self.layout.pid_file);
                let unlock = fs2::FileExt::unlock(&lock).map_err(process_control);
                termination?;
                unlock?;
                Ok(StopOutcome::Stopped)
            }
        }
    }

    pub async fn restart(&self) -> Result<StartOutcome> {
        let _lifecycle = self.lifecycle.lock().await;
        if matches!(&*self.process.lock().await, ManagedProcess::Attached { .. }) {
            return Err(MemoryError::NotProcessOwner);
        }
        self.stop_locked().await?;
        self.start_locked().await
    }

    pub async fn status(&self) -> ServiceStatus {
        let mut process = self.process.lock().await;
        match &mut *process {
            ManagedProcess::Stopped => ServiceStatus::Stopped,
            ManagedProcess::Attached { .. } => match self.client.readiness().await {
                Ok(health) => ServiceStatus::Attached { health },
                Err(error) => ServiceStatus::Unready {
                    pid: None,
                    reason: error.to_string(),
                },
            },
            ManagedProcess::Owned { child, pid, .. } => match child.try_wait() {
                Ok(Some(exit)) => {
                    let code = exit.code();
                    release_owned(&mut process, &self.layout);
                    ServiceStatus::Exited { code }
                }
                Err(error) => ServiceStatus::Unready {
                    pid: Some(*pid),
                    reason: error.to_string(),
                },
                Ok(None) => match self.client.readiness().await {
                    Ok(health) => ServiceStatus::Running { pid: *pid, health },
                    Err(error) => ServiceStatus::Unready {
                        pid: Some(*pid),
                        reason: error.to_string(),
                    },
                },
            },
        }
    }

    async fn acquire_lock(&self) -> Result<LockAcquisition> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.layout.lock_file)
            .map_err(|source| MemoryError::Io {
                operation: "open ICM supervisor lock",
                path: self.layout.lock_file.clone(),
                source,
            })?;
        match lock.try_lock_exclusive() {
            Ok(()) => Ok(LockAcquisition::Owned(lock)),
            Err(source) if source.kind() == ErrorKind::WouldBlock => {
                if self.config.transport == IcmTransport::StdioMcp {
                    return Err(MemoryError::SupervisorLockHeld {
                        lock_path: self.layout.lock_file.clone(),
                    });
                }
                if let Some(health) = self.wait_external_ready().await {
                    return Ok(LockAcquisition::Attached(health));
                }
                Err(MemoryError::SupervisorLockHeld {
                    lock_path: self.layout.lock_file.clone(),
                })
            }
            Err(source) => Err(MemoryError::Io {
                operation: "lock ICM supervisor",
                path: self.layout.lock_file.clone(),
                source,
            }),
        }
    }

    async fn wait_external_ready(&self) -> Option<ServiceHealth> {
        let deadline = Instant::now() + self.config.startup_timeout;
        loop {
            if let Ok(health) = self.client.readiness().await {
                return Some(health);
            }
            if Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn spawn(&self, token: &str) -> Result<Child> {
        let log = self.layout.open_log()?;
        let stderr = log.try_clone().map_err(|source| MemoryError::Io {
            operation: "clone ICM log handle",
            path: self.layout.log_file.clone(),
            source,
        })?;
        let mut command = Command::new(&self.config.executable);
        command
            .arg("--db")
            .arg(&self.layout.database)
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true);
        if !self.config.embeddings {
            command.arg("--no-embeddings");
        }
        match self.config.transport {
            IcmTransport::StdioMcp => {
                command
                    .arg("serve")
                    .arg("--compact")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped());
            }
            IcmTransport::Http => {
                command
                    .arg("serve")
                    .arg("--http")
                    .arg(self.config.bind_addr.to_string())
                    .arg("--token")
                    .arg(token)
                    .stdin(Stdio::null())
                    .stdout(Stdio::from(log));
            }
        }
        command
            .spawn()
            .map_err(|source| MemoryError::ProcessSpawn { source })
    }

    async fn wait_ready(&self, child: &mut Child) -> Result<ServiceHealth> {
        if self.config.transport == IcmTransport::StdioMcp {
            return mcp::attach(
                self.client.mcp(),
                child,
                self.config.startup_timeout,
                self.config.embeddings,
            )
            .await;
        }
        let deadline = Instant::now() + self.config.startup_timeout;
        loop {
            if let Some(status) = child.try_wait().map_err(process_control)? {
                return Err(MemoryError::ProcessExited { status });
            }
            if let Ok(health) = self.client.readiness().await {
                return Ok(health);
            }
            if Instant::now() >= deadline {
                return Err(MemoryError::StartupTimeout {
                    endpoint: self.client.endpoint().to_owned(),
                    timeout: self.config.startup_timeout,
                });
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

impl Drop for IcmSupervisor {
    fn drop(&mut self) {
        if let Ok(mut process) = self.process.try_lock() {
            if let ManagedProcess::Owned {
                child,
                lock,
                pid: _,
            } = &mut *process
            {
                let _ = child.start_kill();
                let _ = fs::remove_file(&self.layout.pid_file);
                let _ = fs2::FileExt::unlock(lock);
            }
            if let ManagedProcess::Attached { lock: Some(lock) } = &*process {
                let _ = fs2::FileExt::unlock(lock);
            }
            *process = ManagedProcess::Stopped;
        }
    }
}

fn release_owned(process: &mut ManagedProcess, layout: &IcmLayout) {
    if let ManagedProcess::Owned { lock, .. } = process {
        let _ = fs2::FileExt::unlock(lock);
    }
    let _ = fs::remove_file(&layout.pid_file);
    *process = ManagedProcess::Stopped;
}

fn process_control(source: std::io::Error) -> MemoryError {
    MemoryError::ProcessControl { source }
}

#[cfg(unix)]
async fn terminate_child(child: &mut Child, timeout: Duration) -> Result<()> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    if let Some(id) = child.id() {
        if let Err(error) = kill(Pid::from_raw(id as i32), Signal::SIGTERM) {
            tracing::debug!(%error, "SIGTERM failed; process may already be gone");
        }
    }
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(result) => result.map(|_| ()).map_err(process_control),
        Err(_) => {
            child.kill().await.map_err(process_control)?;
            child.wait().await.map(|_| ()).map_err(process_control)
        }
    }
}

#[cfg(not(unix))]
async fn terminate_child(child: &mut Child, _timeout: Duration) -> Result<()> {
    child.kill().await.map_err(process_control)?;
    child.wait().await.map(|_| ()).map_err(process_control)
}
