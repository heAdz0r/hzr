use std::io;
use std::time::Duration;

use tokio::process::{Child, Command};

#[cfg(unix)]
pub(crate) fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.as_std_mut().process_group(0);
}

#[cfg(not(unix))]
pub(crate) fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
pub(crate) struct ProcessGroupGuard {
    id: Option<nix::unistd::Pid>,
}

#[cfg(unix)]
impl ProcessGroupGuard {
    pub(crate) fn new(child: &Child) -> io::Result<Self> {
        let id = child
            .id()
            .ok_or_else(|| io::Error::other("spawned Node process has no process ID"))?;
        let id = i32::try_from(id)
            .map_err(|_| io::Error::other("Node process ID exceeds the platform range"))?;
        Ok(Self {
            id: Some(nix::unistd::Pid::from_raw(id)),
        })
    }

    pub(crate) fn finish(&mut self) -> io::Result<()> {
        self.signal(nix::sys::signal::Signal::SIGKILL)?;
        self.id = None;
        Ok(())
    }

    pub(crate) async fn terminate(&mut self, child: &mut Child, grace: Duration) -> io::Result<()> {
        if self.signal(nix::sys::signal::Signal::SIGTERM).is_err() {
            let _ = child.start_kill();
        }

        let wait = tokio::time::timeout(grace, child.wait()).await;
        let kill = self.signal(nix::sys::signal::Signal::SIGKILL);
        if kill.is_ok() {
            self.id = None;
        } else {
            let _ = child.start_kill();
        }
        let waited = match wait {
            Ok(result) => result.map(|_| ()),
            Err(_) => child.wait().await.map(|_| ()),
        };
        kill?;
        waited
    }

    fn signal(&self, signal: nix::sys::signal::Signal) -> io::Result<()> {
        let Some(id) = self.id else {
            return Ok(());
        };
        match nix::sys::signal::killpg(id, signal) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
            Err(error) => Err(io::Error::from(error)),
        }
    }
}

#[cfg(unix)]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        let _ = self.signal(nix::sys::signal::Signal::SIGKILL);
    }
}

#[cfg(not(unix))]
pub(crate) struct ProcessGroupGuard;

#[cfg(not(unix))]
impl ProcessGroupGuard {
    pub(crate) fn new(_child: &Child) -> io::Result<Self> {
        Ok(Self)
    }

    pub(crate) fn finish(&mut self) -> io::Result<()> {
        Ok(())
    }

    pub(crate) async fn terminate(
        &mut self,
        child: &mut Child,
        _grace: Duration,
    ) -> io::Result<()> {
        child.kill().await
    }
}
