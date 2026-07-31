use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use hzr_protocol::CodecProfile;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub schema_version: u16,
    pub data_dir: PathBuf,
    pub daemon: DaemonConfig,
    pub engines: EngineConfig,
    pub policy: PolicyConfig,
    pub privacy: PrivacyConfig,
}

impl Default for Config {
    fn default() -> Self {
        let paths = ConfigPaths::discover();
        Self {
            schema_version: 1,
            data_dir: paths.data_dir,
            daemon: DaemonConfig::default(),
            engines: EngineConfig::default(),
            policy: PolicyConfig::default(),
            privacy: PrivacyConfig::default(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let config: Self = toml::from_str(&content).map_err(ConfigError::Parse)?;
        config.validate()?;
        Ok(config)
    }

    pub fn load_or_default(path: &Path) -> Result<Self, ConfigError> {
        if path.exists() {
            Self::load(path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn write(&self, path: &Path) -> Result<(), ConfigError> {
        self.validate()?;
        let parent = path
            .parent()
            .filter(|directory| !directory.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if parent != Path::new(".") {
            fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
            set_private_directory_permissions(parent)?;
        }

        let content = toml::to_string_pretty(self).map_err(ConfigError::Serialize)?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|source| ConfigError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        temporary
            .write_all(content.as_bytes())
            .map_err(|source| ConfigError::Write {
                path: temporary.path().to_path_buf(),
                source,
            })?;
        set_private_permissions(temporary.path())?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|source| ConfigError::Write {
                path: temporary.path().to_path_buf(),
                source,
            })?;
        temporary
            .persist(path)
            .map_err(|error| ConfigError::Write {
                path: path.to_path_buf(),
                source: error.error,
            })?;
        sync_directory(parent).map_err(|source| ConfigError::Write {
            path: parent.to_path_buf(),
            source,
        })
    }

    pub fn ensure_layout(&self) -> Result<(), ConfigError> {
        for directory in [
            self.data_dir.clone(),
            self.data_dir.join("runtime"),
            self.data_dir.join("workspaces"),
            self.data_dir.join("memory/icm"),
            self.data_dir.join("ledger"),
            self.data_dir.join("engines"),
        ] {
            fs::create_dir_all(&directory).map_err(|source| ConfigError::Write {
                path: directory.clone(),
                source,
            })?;
            set_private_directory_permissions(&directory)?;
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.schema_version != 1 {
            return Err(ConfigError::Schema(self.schema_version));
        }
        if !self.daemon.bind.ip().is_loopback() {
            return Err(ConfigError::NonLoopbackBind(self.daemon.bind));
        }
        if self.daemon.request_limit_bytes == 0 {
            return Err(ConfigError::InvalidRequestLimit);
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ConfigPaths {
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
}

impl ConfigPaths {
    pub fn discover() -> Self {
        if let Some(project) = ProjectDirs::from("dev", "headz0r", "hzr") {
            return Self {
                config_file: project.config_dir().join("config.toml"),
                data_dir: project.data_local_dir().to_path_buf(),
            };
        }

        let base = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".hzr");
        Self {
            config_file: base.join("config.toml"),
            data_dir: base.join("data"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    pub bind: std::net::SocketAddr,
    pub request_limit_bytes: usize,
    pub request_timeout_ms: u64,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            bind: std::net::SocketAddr::from(([127, 0, 0, 1], 47_391)),
            request_limit_bytes: 1_048_576,
            request_timeout_ms: 30_000,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    pub directory: Option<PathBuf>,
    pub strict_versions: bool,
    pub auto_start_icm: bool,
    pub auto_index: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            directory: discover_bundle_engine_directory(),
            strict_versions: true,
            auto_start_icm: true,
            auto_index: true,
        }
    }
}

impl EngineConfig {
    pub fn binary(&self, name: &str) -> PathBuf {
        const MANAGED_BINARIES: [&str; 3] = ["grepai", "icm", "rtk"];
        if MANAGED_BINARIES.contains(&name) {
            return self
                .directory
                .as_ref()
                .map_or_else(|| PathBuf::from(name), |directory| directory.join(name));
        }
        PathBuf::from(name)
    }
}

fn discover_bundle_engine_directory() -> Option<PathBuf> {
    if let Some(directory) = std::env::var_os("HZR_ENGINES_DIR") {
        if !directory.is_empty() {
            return Some(PathBuf::from(directory));
        }
    }
    let executable = std::env::current_exe().ok()?;
    sibling_engine_directory(&executable)
}

fn sibling_engine_directory(executable: &Path) -> Option<PathBuf> {
    let directory = executable.parent()?.parent()?.join("engines");
    ["rtk", "grepai", "icm"]
        .iter()
        .all(|name| directory.join(name).is_file())
        .then(|| std::fs::canonicalize(&directory).unwrap_or(directory))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PolicyConfig {
    pub codec_profile: CodecProfile,
    pub context_token_limit: u64,
    pub output_reserve: u64,
    pub safety_margin: u64,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            codec_profile: CodecProfile::Adaptive,
            context_token_limit: 16_000,
            output_reserve: 2_000,
            safety_margin: 1_000,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacyConfig {
    pub telemetry: bool,
    pub raw_retention_seconds: u64,
    pub redact_secrets: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            telemetry: false,
            raw_retention_seconds: 0,
            redact_secrets: true,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config: {0}")]
    Parse(toml::de::Error),
    #[error("failed to serialize config: {0}")]
    Serialize(toml::ser::Error),
    #[error("unsupported config schema {0}")]
    Schema(u16),
    #[error("daemon must bind to loopback, got {0}")]
    NonLoopbackBind(std::net::SocketAddr),
    #[error("daemon request limit must be greater than zero")]
    InvalidRequestLimit,
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<(), ConfigError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| {
        ConfigError::Write {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), ConfigError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
        ConfigError::Write {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), std::io::Error> {
    fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<(), ConfigError> {
    Ok(())
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), ConfigError> {
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::SocketAddr;
    use std::path::Path;

    use tempfile::tempdir;

    use super::{Config, ConfigError, EngineConfig, sibling_engine_directory};

    #[test]
    fn test_bundle_engine_directory_requires_all_managed_engines() {
        let directory = tempdir().expect("temporary directory");
        let bin = directory.path().join("bin");
        let engines = directory.path().join("engines");
        fs::create_dir_all(&bin).expect("create bin directory");
        fs::create_dir_all(&engines).expect("create engine directory");
        let executable = bin.join("hzr");
        fs::write(&executable, []).expect("write executable fixture");

        assert!(sibling_engine_directory(&executable).is_none());
        for engine in ["rtk", "grepai", "icm"] {
            fs::write(engines.join(engine), []).expect("write engine fixture");
        }

        assert_eq!(
            sibling_engine_directory(&executable),
            Some(fs::canonicalize(engines).expect("canonical engine directory"))
        );
    }

    #[test]
    fn test_config_round_trip() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("config.toml");
        let config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };

        config.write(&path).expect("config write");
        let loaded = Config::load(&path).expect("config load");

        assert_eq!(loaded.schema_version, 1);
        assert_eq!(loaded.data_dir, config.data_dir);
    }

    #[test]
    fn test_config_rejects_non_loopback_bind() {
        let mut config = Config::default();
        config.daemon.bind = SocketAddr::from(([0, 0, 0, 0], 47_391));

        assert!(matches!(
            config.validate(),
            Err(ConfigError::NonLoopbackBind(_))
        ));
    }

    #[test]
    fn test_engine_directory_does_not_shadow_system_tools() {
        let engines = EngineConfig {
            directory: Some("/opt/hzr/engines".into()),
            ..EngineConfig::default()
        };

        assert_eq!(
            engines.binary("grepai"),
            Path::new("/opt/hzr/engines/grepai")
        );
        assert_eq!(engines.binary("git"), Path::new("git"));
        assert_eq!(engines.binary("rg"), Path::new("rg"));
    }
}
