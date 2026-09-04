use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{IcmConfig, IcmLayout, MemoryError, Result};

const STATE_LIMIT: u64 = 16 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProcessIdentity {
    pub pid: u32,
    pub start: String,
}

impl ProcessIdentity {
    pub fn capture(pid: u32) -> Result<Option<Self>> {
        if pid == 0 || pid > i32::MAX as u32 {
            return Err(MemoryError::OwnershipUncertain(
                "invalid ICM process ID".into(),
            ));
        }
        process_start(pid).map(|start| start.map(|start| Self { pid, start }))
    }

    #[cfg(unix)]
    pub fn spawned(pid: u32) -> Result<Self> {
        Self::capture(pid)?.ok_or_else(|| {
            MemoryError::OwnershipUncertain("spawned ICM exited before identity commit".into())
        })
    }

    #[cfg(not(unix))]
    pub fn spawned(pid: u32) -> Result<Self> {
        Ok(Self {
            pid,
            start: "unverifiable-on-this-platform".into(),
        })
    }

    pub fn is_current(&self) -> Result<bool> {
        Ok(Self::capture(self.pid)?.as_ref() == Some(self))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeState {
    version: u32,
    database: PathBuf,
    pub executable: PathBuf,
    executable_sha256: String,
    pub endpoint: SocketAddr,
    pub process: Option<ProcessIdentity>,
}

impl RuntimeState {
    pub fn starting(config: &IcmConfig, layout: &IcmLayout) -> Result<Self> {
        Ok(Self {
            version: 1,
            database: layout.database.clone(),
            executable: config.executable.clone(),
            executable_sha256: executable_sha256(config)?,
            endpoint: config.bind_addr,
            process: None,
        })
    }

    pub fn matches_executable(&self, config: &IcmConfig) -> Result<bool> {
        Ok(self.executable == config.executable
            && self.executable_sha256 == executable_sha256(config)?)
    }

    pub fn validate(&self, layout: &IcmLayout) -> Result<()> {
        if self.version != 1
            || self.database != layout.database
            || !self.endpoint.ip().is_loopback()
            || self.endpoint.port() == 0
            || self.executable.as_os_str().is_empty()
            || self.executable_sha256.len() != 64
            || !self
                .executable_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.process.as_ref().is_some_and(|process| {
                process.pid == 0
                    || process.pid > i32::MAX as u32
                    || process.start.is_empty()
                    || process.start.len() > 512
            })
        {
            return Err(MemoryError::OwnershipUncertain(
                "ICM runtime identity does not match its data root".into(),
            ));
        }
        Ok(())
    }

    pub fn load(layout: &IcmLayout) -> Result<Option<Self>> {
        let path = layout.runtime_dir.join("service.json");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(state_io("inspect ICM runtime identity", &path, error)),
        };
        if !metadata.file_type().is_file() || metadata.len() > STATE_LIMIT {
            return Err(MemoryError::OwnershipUncertain(
                "ICM runtime identity must be a bounded regular file".into(),
            ));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(nix::libc::O_NOFOLLOW);
        }
        let file = options
            .open(&path)
            .map_err(|error| state_io("open ICM runtime identity", &path, error))?;
        let mut bytes = Vec::new();
        file.take(STATE_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| state_io("read ICM runtime identity", &path, error))?;
        if bytes.len() as u64 > STATE_LIMIT {
            return Err(MemoryError::OwnershipUncertain(
                "ICM runtime identity exceeds its size limit".into(),
            ));
        }
        let state: Self = serde_json::from_slice(&bytes).map_err(|_| {
            MemoryError::OwnershipUncertain("ICM runtime identity is malformed".into())
        })?;
        state.validate(layout)?;
        Ok(Some(state))
    }

    pub fn persist(&self, layout: &IcmLayout) -> Result<()> {
        self.validate(layout)?;
        let path = layout.runtime_dir.join("service.json");
        let temporary = layout
            .runtime_dir
            .join(format!(".service-{}.json", Uuid::new_v4()));
        let encoded =
            serde_json::to_vec(self).map_err(|error| MemoryError::UnexpectedResponse {
                operation: "encode runtime identity",
                message: error.to_string(),
            })?;
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&encoded)?;
            file.sync_all()?;
            fs::rename(&temporary, &path)?;
            #[cfg(unix)]
            fs::File::open(&layout.runtime_dir)?.sync_all()?;
            Ok::<_, std::io::Error>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|error| state_io("persist ICM runtime identity", &path, error))
    }

    pub fn remove(layout: &IcmLayout) {
        let _ = fs::remove_file(layout.runtime_dir.join("service.json"));
    }
}

/// Legacy PID files only fence startup; they never authorize attachment or signalling.
pub(crate) fn legacy_pid(layout: &IcmLayout) -> Result<Option<u32>> {
    let path = &layout.pid_file;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(state_io("inspect legacy ICM PID", path, error)),
    };
    if !metadata.file_type().is_file() || metadata.len() > 32 {
        return Err(MemoryError::OwnershipUncertain(
            "legacy ICM PID must be a bounded regular file".into(),
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut recorded = String::new();
    options
        .open(path)
        .map_err(|error| state_io("open legacy ICM PID", path, error))?
        .take(33)
        .read_to_string(&mut recorded)
        .map_err(|error| state_io("read legacy ICM PID", path, error))?;
    let pid = recorded
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0 && *pid <= i32::MAX as u32);
    if recorded.len() > 32 || pid.is_none() {
        return Err(MemoryError::OwnershipUncertain(
            "legacy ICM PID file is invalid".into(),
        ));
    }
    Ok(pid)
}

fn executable_sha256(config: &IcmConfig) -> Result<String> {
    let path = if config.executable_is_explicit_path() {
        config.executable.clone()
    } else {
        std::env::var_os("PATH")
            .and_then(|value| {
                std::env::split_paths(&value)
                    .map(|directory| directory.join(&config.executable))
                    .find(|path| path.is_file())
            })
            .ok_or_else(|| {
                MemoryError::OwnershipUncertain("ICM executable is not resolvable".into())
            })?
    };
    crate::installation::sha256_file(&path)
}

fn state_io(
    operation: &'static str,
    path: &std::path::Path,
    source: std::io::Error,
) -> MemoryError {
    MemoryError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(target_os = "linux")]
fn process_start(pid: u32) -> Result<Option<String>> {
    let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(MemoryError::ProcessControl { source: error }),
    };
    let fields = stat
        .rsplit_once(')')
        .map(|(_, rest)| rest.split_whitespace().collect::<Vec<_>>())
        .ok_or_else(|| MemoryError::OwnershipUncertain("invalid process identity".into()))?;
    if fields
        .first()
        .is_some_and(|state| *state == "Z" || *state == "X")
    {
        return Ok(None);
    }
    let start = fields
        .get(19)
        .ok_or_else(|| MemoryError::OwnershipUncertain("missing process start identity".into()))?;
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|source| MemoryError::ProcessControl { source })?;
    Ok(Some(format!("{}:{start}", boot.trim())))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_start(pid: u32) -> Result<Option<String>> {
    let output = std::process::Command::new("/bin/ps")
        .args([
            "-p",
            &pid.to_string(),
            "-o",
            "lstart=",
            "-o",
            "stat=",
            "-o",
            "comm=",
        ])
        .env("LC_ALL", "C")
        .output()
        .map_err(|source| MemoryError::ProcessControl { source })?;
    if !output.status.success() && output.stdout.is_empty() {
        if output.status.code() == Some(1) {
            use nix::sys::signal::kill;
            use nix::unistd::Pid;
            if kill(Pid::from_raw(pid as i32), None) == Err(nix::errno::Errno::ESRCH) {
                return Ok(None);
            }
        }
        return Err(MemoryError::OwnershipUncertain(
            "process identity probe failed".into(),
        ));
    }
    let output = String::from_utf8_lossy(&output.stdout);
    let mut fields = output.split_whitespace();
    let start = fields.by_ref().take(5).collect::<Vec<_>>().join(" ");
    let state = fields.next().ok_or_else(|| {
        MemoryError::OwnershipUncertain("process identity probe was empty".into())
    })?;
    if state.starts_with('Z') {
        return Ok(None);
    }
    let executable = fields.collect::<Vec<_>>().join(" ");
    if start.is_empty() || executable.is_empty() {
        return Err(MemoryError::OwnershipUncertain(
            "process identity probe was incomplete".into(),
        ));
    }
    // exec may legitimately replace a script interpreter without changing process ownership.
    Ok(Some(start))
}

#[cfg(not(unix))]
fn process_start(_pid: u32) -> Result<Option<String>> {
    Err(MemoryError::OwnershipUncertain(
        "orphan identity inspection is unsupported on this platform".into(),
    ))
}

/// A diagnostic match requires the persisted PID and start identity, never a command substring.
pub fn is_managed_icm_process(data_root: &std::path::Path, pid: u32) -> bool {
    let root = match data_root.canonicalize() {
        Ok(root) => root.join("memory/icm"),
        Err(_) => return false,
    };
    let layout = IcmLayout {
        database: root.join("memories.db"),
        runtime_dir: root.join("runtime"),
        lock_file: root.join("runtime/supervisor.lock"),
        pid_file: root.join("runtime/icm.pid"),
        token_file: root.join("auth.token"),
        token_lock_file: root.join("runtime/token.lock"),
        log_file: root.join("icm.log"),
        root,
    };
    RuntimeState::load(&layout)
        .ok()
        .flatten()
        .and_then(|state| state.process)
        .is_some_and(|process| process.pid == pid && process.is_current().unwrap_or(false))
}
