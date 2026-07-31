use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use directories::ProjectDirs;

use crate::error::{MemoryError, Result};
use crate::types::IcmTransport;

#[derive(Debug, Clone)]
pub struct IcmConfig {
    pub executable: PathBuf,
    pub data_root: PathBuf,
    pub bind_addr: SocketAddr,
    pub expected_executable_sha256: Option<String>,
    pub startup_timeout: Duration,
    pub request_timeout: Duration,
    pub cli_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub circuit_failure_threshold: u32,
    pub circuit_reset_timeout: Duration,
    pub cli_fallback: bool,
    pub embeddings: bool,
    pub transport: IcmTransport,
}

impl IcmConfig {
    pub fn from_data_root(executable: impl Into<PathBuf>, data_root: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            data_root: data_root.into(),
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 11_435),
            expected_executable_sha256: None,
            startup_timeout: Duration::from_secs(120),
            request_timeout: Duration::from_secs(10),
            cli_timeout: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(5),
            circuit_failure_threshold: 3,
            circuit_reset_timeout: Duration::from_secs(20),
            cli_fallback: true,
            // A clean install must not turn the first durable write into an implicit,
            // unbounded model download. The daemon may opt in from its explicit config.
            embeddings: false,
            transport: IcmTransport::StdioMcp,
        }
    }

    pub fn discover(executable: impl Into<PathBuf>) -> Result<Self> {
        let dirs = ProjectDirs::from("dev", "headz0r", "hzr")
            .ok_or(MemoryError::DataDirectoryUnavailable)?;
        Ok(Self::from_data_root(executable, dirs.data_local_dir()))
    }

    pub fn validate(&self) -> Result<()> {
        if self.executable.as_os_str().is_empty() {
            return Err(MemoryError::InvalidConfig(
                "ICM executable must not be empty".into(),
            ));
        }
        if self.data_root.as_os_str().is_empty() {
            return Err(MemoryError::InvalidConfig(
                "HZR data root must not be empty".into(),
            ));
        }
        if !self.bind_addr.ip().is_loopback() {
            return Err(MemoryError::InvalidConfig(format!(
                "ICM must bind to loopback, got {}",
                self.bind_addr.ip()
            )));
        }
        if self.bind_addr.port() == 0 {
            return Err(MemoryError::InvalidConfig(
                "ICM bind port must be fixed and non-zero".into(),
            ));
        }
        for (name, timeout) in [
            ("startup_timeout", self.startup_timeout),
            ("request_timeout", self.request_timeout),
            ("cli_timeout", self.cli_timeout),
            ("shutdown_timeout", self.shutdown_timeout),
            ("circuit_reset_timeout", self.circuit_reset_timeout),
        ] {
            if timeout.is_zero() {
                return Err(MemoryError::InvalidConfig(format!(
                    "{name} must be non-zero"
                )));
            }
        }
        if self.circuit_failure_threshold == 0 {
            return Err(MemoryError::InvalidConfig(
                "circuit_failure_threshold must be non-zero".into(),
            ));
        }
        if let Some(checksum) = &self.expected_executable_sha256 {
            if checksum.len() != 64 || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(MemoryError::InvalidConfig(
                    "expected_executable_sha256 must contain 64 hexadecimal characters".into(),
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn executable_is_explicit_path(&self) -> bool {
        self.executable.components().count() > 1 || self.executable.is_absolute()
    }

    pub(crate) fn data_root(&self) -> &Path {
        &self.data_root
    }
}
