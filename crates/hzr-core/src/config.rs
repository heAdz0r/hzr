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
    pub activation: ActivationConfig,
    pub billing: BillingConfig,
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
            activation: ActivationConfig::default(),
            billing: BillingConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BillingConfig {
    /// Public list-price estimates are opt-in and never represent an invoice.
    pub public_estimate_enabled: bool,
    pub harness: String,
    pub provider: String,
    pub model: String,
    pub method: String,
    /// `input` by default; `cache_read` requires evidence that avoided context would be cached.
    pub pricing_basis: String,
    /// Optional strict schema-v1 catalog whose exact keys replace built-in entries.
    pub pricing_file: Option<PathBuf>,
}

impl BillingConfig {
    pub fn effective_pricing_basis(&self) -> &str {
        if self.pricing_basis.is_empty() {
            "input"
        } else {
            &self.pricing_basis
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let mut config: Self = toml::from_str(&content).map_err(ConfigError::Parse)?;
        config.migrate_pinned_engine_directory();
        config.validate()?;
        Ok(config)
    }

    /// Repair a configuration written before the engines path became upgrade-stable.
    ///
    /// Existing installs persisted the canonicalized `versions/<release>/engines`, which
    /// keeps launching the engines of the release that was current at first run. Rewriting
    /// it in memory on every load is enough — and safer than a one-shot file migration,
    /// because it also corrects a config copied between machines. The translation only
    /// applies when `current/engines` resolves to the very same directory, so a dangling
    /// or foreign `current` can never hijack engine execution.
    fn migrate_pinned_engine_directory(&mut self) {
        if self.engines.directory.is_none() {
            // A config first written by a source/debug binary predates the release bundle
            // and legitimately contains `directory = None`. Once the same config is loaded
            // by an installed bundle, adopt only a complete sibling engine directory.
            self.engines.directory = discover_bundle_engine_directory();
        }
        if let Some(directory) = self.engines.directory.clone() {
            let stable = stable_engine_directory(&directory);
            if stable != directory {
                self.engines.directory = Some(stable);
            }
        }
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
        if self.engines.grepai_watcher_limit == 0
            || self.engines.grepai_watcher_idle_ttl_seconds == 0
            || self.engines.grepai_watcher_sweep_seconds == 0
        {
            return Err(ConfigError::InvalidWatcherLifecycle);
        }
        if self.policy.input_token_budget() == 0 {
            return Err(ConfigError::InvalidPolicyBudget);
        }
        for workspace in &self.activation.enabled_workspaces {
            if !is_sha256(&workspace.repository_id)
                || !is_sha256(&workspace.worktree_id)
                || !workspace.root.is_absolute()
            {
                return Err(ConfigError::InvalidActivation);
            }
        }
        if self.billing.public_estimate_enabled {
            let fields = [
                self.billing.harness.as_str(),
                self.billing.provider.as_str(),
                self.billing.model.as_str(),
                self.billing.method.as_str(),
            ];
            if fields
                .into_iter()
                .any(|value| value.is_empty() || value.len() > 128 || !value.is_ascii())
                || !matches!(
                    self.billing.effective_pricing_basis(),
                    "input" | "cache_read"
                )
                || self
                    .billing
                    .pricing_file
                    .as_ref()
                    .is_some_and(|path| !path.is_absolute())
            {
                return Err(ConfigError::InvalidBilling);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationMode {
    #[default]
    All,
    Selected,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnabledWorkspace {
    pub repository_id: String,
    pub worktree_id: String,
    pub root: PathBuf,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ActivationConfig {
    pub mode: ActivationMode,
    pub enabled_workspaces: Vec<EnabledWorkspace>,
}

impl ActivationConfig {
    #[must_use]
    pub fn allows(&self, repository_id: &str, worktree_id: &str) -> bool {
        self.mode == ActivationMode::All
            || self.enabled_workspaces.iter().any(|workspace| {
                workspace.repository_id == repository_id && workspace.worktree_id == worktree_id
            })
    }

    pub fn enable(&mut self, workspace: EnabledWorkspace) {
        self.enabled_workspaces.retain(|enabled| {
            enabled.worktree_id != workspace.worktree_id && enabled.root != workspace.root
        });
        self.enabled_workspaces.push(workspace);
        self.enabled_workspaces
            .sort_by(|left, right| left.root.cmp(&right.root));
    }

    pub fn disable(&mut self, repository_id: &str, worktree_id: &str) -> bool {
        let before = self.enabled_workspaces.len();
        self.enabled_workspaces.retain(|workspace| {
            workspace.repository_id != repository_id || workspace.worktree_id != worktree_id
        });
        self.enabled_workspaces.len() != before
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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
    /// Enable ICM's embedding model. The default stays FTS-only so a clean install never
    /// blocks its first memory mutation on an implicit model download.
    pub icm_embeddings: bool,
    pub auto_index: bool,
    pub grepai_watcher_limit: usize,
    pub grepai_watcher_idle_ttl_seconds: u64,
    pub grepai_watcher_sweep_seconds: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            directory: discover_bundle_engine_directory(),
            strict_versions: true,
            auto_start_icm: true,
            icm_embeddings: false,
            auto_index: true,
            grepai_watcher_limit: 8,
            grepai_watcher_idle_ttl_seconds: 15 * 60,
            grepai_watcher_sweep_seconds: 30,
        }
    }
}

impl EngineConfig {
    pub fn binary(&self, name: &str) -> PathBuf {
        const MANAGED_BINARIES: [&str; 4] = ["grepai", "icm", "node", "rtk"];
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

/// Engine executables a bundle engine directory must contain to be usable.
const BUNDLE_ENGINES: [&str; 4] = ["rtk", "grepai", "icm", "node"];

fn has_all_engines(directory: &Path) -> bool {
    BUNDLE_ENGINES
        .iter()
        .all(|name| directory.join(name).is_file())
}

fn sibling_engine_directory(executable: &Path) -> Option<PathBuf> {
    // `current_exe()` may preserve the public `~/.local/bin/hzr` symlink on macOS.
    // Resolve it before deriving the bundle root or an installed binary will fall back
    // to PATH even though its complete private engine directory is present.
    let executable = executable
        .canonicalize()
        .unwrap_or_else(|_| executable.to_path_buf());
    let directory = executable.parent()?.parent()?.join("engines");
    if !has_all_engines(&directory) {
        return None;
    }
    // Canonicalize to reject traversal, then translate the physical location back to the
    // upgrade-stable one. Storing the canonical path directly would pin the config to
    // `versions/v0.4.6-<platform>/engines`, so after the next upgrade a new `hzr` would
    // keep launching the *previous* RTK/grepai/ICM/Node.
    let physical = std::fs::canonicalize(&directory).unwrap_or(directory);
    Some(stable_engine_directory(&physical))
}

/// Map `<root>/versions/<release>/engines` back to `<root>/current/engines` when that
/// indirection exists and resolves to the same directory.
///
/// `current_exe()` is already fully resolved on Linux (`/proc/self/exe`) and macOS, so a
/// bundle launched through `<root>/current/bin/hzr` reports the versioned path and the
/// `current` symlink is lost before we ever see it. Recovering it here is what makes an
/// upgrade actually switch engines; anything else is pinned to the release that happened
/// to be current at first run.
pub fn stable_engine_directory(physical: &Path) -> PathBuf {
    let Some(release_root) = physical.parent() else {
        return physical.to_path_buf();
    };
    let Some(versions_dir) = release_root.parent() else {
        return physical.to_path_buf();
    };
    if versions_dir.file_name().and_then(|name| name.to_str()) != Some("versions") {
        return physical.to_path_buf();
    }
    let Some(install_root) = versions_dir.parent() else {
        return physical.to_path_buf();
    };
    let stable = install_root.join("current").join("engines");
    // Only trust the stable path when it currently resolves to the same physical engines
    // directory. A dangling or foreign `current` must never redirect engine execution.
    // Both sides are canonicalized before comparison: the caller may pass an
    // uncanonicalized path, and on macOS `/var` versus `/private/var` would otherwise
    // make an identical directory look like a mismatch.
    let canonical_physical =
        std::fs::canonicalize(physical).unwrap_or_else(|_| physical.to_path_buf());
    let resolves_here = std::fs::canonicalize(&stable)
        .map(|resolved| resolved == canonical_physical)
        .unwrap_or(false);
    if resolves_here && has_all_engines(&stable) {
        stable
    } else {
        physical.to_path_buf()
    }
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

impl PolicyConfig {
    pub fn input_token_budget(&self) -> u64 {
        self.context_token_limit
            .saturating_sub(self.output_reserve)
            .saturating_sub(self.safety_margin)
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
    #[error("grepai watcher limit, idle TTL, and sweep interval must be greater than zero")]
    InvalidWatcherLifecycle,
    #[error("context token limit must exceed output reserve plus safety margin")]
    InvalidPolicyBudget,
    #[error("activation workspaces require absolute roots and lowercase SHA-256 identities")]
    InvalidActivation,
    #[error(
        "billing selection requires bounded ASCII identifiers, input/cache_read basis, and an absolute pricing_file"
    )]
    InvalidBilling,
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

    use super::{
        ActivationMode, Config, ConfigError, EnabledWorkspace, EngineConfig, PolicyConfig,
        discover_bundle_engine_directory, sibling_engine_directory, stable_engine_directory,
    };

    /// Build `<root>/versions/<release>/{bin,engines}` plus a `current` symlink, i.e. the
    /// exact layout `install.sh` produces.
    fn versioned_bundle(root: &Path, release: &str) -> std::path::PathBuf {
        let release_root = root.join("versions").join(release);
        let bin = release_root.join("bin");
        let engines = release_root.join("engines");
        fs::create_dir_all(&bin).expect("bin directory");
        fs::create_dir_all(&engines).expect("engines directory");
        fs::write(bin.join("hzr"), []).expect("hzr fixture");
        for engine in ["rtk", "grepai", "icm", "node"] {
            fs::write(engines.join(engine), []).expect("engine fixture");
        }
        release_root
    }

    #[cfg(unix)]
    fn point_current_at(root: &Path, release_root: &Path) {
        let current = root.join("current");
        let _ = fs::remove_file(&current);
        std::os::unix::fs::symlink(release_root, &current).expect("current symlink");
    }

    #[cfg(unix)]
    #[test]
    fn test_engine_directory_uses_upgrade_stable_current_path() {
        let directory = tempdir().expect("temporary directory");
        let root = directory.path();
        let release = versioned_bundle(root, "v0.4.6-darwin-arm64");
        point_current_at(root, &release);

        let resolved = stable_engine_directory(&release.join("engines"));
        assert_eq!(
            resolved,
            root.join("current").join("engines"),
            "a versioned path must translate to the stable current/ path"
        );
        assert!(
            !resolved.to_string_lossy().contains("versions/"),
            "pinning versions/ would keep old engines after an upgrade"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_pinned_engine_directory_is_migrated_on_load() {
        let directory = tempdir().expect("temporary directory");
        let root = directory.path();
        let release = versioned_bundle(root, "v0.4.6-darwin-arm64");
        point_current_at(root, &release);

        let config_path = root.join("config.toml");
        let mut config = Config::default();
        // Simulate a config written before the fix.
        config.engines.directory = Some(release.join("engines"));
        config.write(&config_path).expect("write legacy config");

        let loaded = Config::load(&config_path).expect("load migrates the pinned path");
        assert_eq!(
            loaded.engines.directory,
            Some(root.join("current").join("engines")),
            "loading must repair a config pinned to a physical release path"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_upgrade_switches_engines_through_current() {
        let directory = tempdir().expect("temporary directory");
        let root = directory.path();
        let old = versioned_bundle(root, "v0.4.6-darwin-arm64");
        point_current_at(root, &old);
        let stable = stable_engine_directory(&old.join("engines"));

        // Upgrade: a new release becomes current, exactly as install.sh repoints it.
        let new = versioned_bundle(root, "v0.4.6-darwin-arm64");
        fs::write(new.join("engines").join("rtk"), b"new").expect("new engine bytes");
        point_current_at(root, &new);

        assert_eq!(
            fs::read(stable.join("rtk")).expect("engine through stable path"),
            b"new",
            "the stable path must follow current/ to the upgraded engines"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_foreign_or_dangling_current_never_redirects_engines() {
        let directory = tempdir().expect("temporary directory");
        let root = directory.path();
        let release = versioned_bundle(root, "v0.4.6-darwin-arm64");
        let other = versioned_bundle(root, "v0.9.9-other");

        // `current` pointing at a different release must not capture this one.
        point_current_at(root, &other);
        assert_eq!(
            stable_engine_directory(&release.join("engines")),
            release.join("engines"),
            "a foreign current must not redirect engine execution"
        );

        // A dangling `current` must fall back to the physical path, not break.
        let current = root.join("current");
        fs::remove_file(&current).expect("remove current");
        std::os::unix::fs::symlink(root.join("versions/absent"), &current).expect("dangling");
        assert_eq!(
            stable_engine_directory(&release.join("engines")),
            release.join("engines")
        );
    }

    #[test]
    fn test_non_versioned_layout_is_left_unchanged() {
        let directory = tempdir().expect("temporary directory");
        let engines = directory.path().join("engines");
        assert_eq!(stable_engine_directory(&engines), engines);
    }

    #[test]
    fn test_absent_engine_directory_adopts_only_discovered_bundle() {
        let discovered = discover_bundle_engine_directory();
        let mut config = Config::default();
        config.engines.directory = None;

        config.migrate_pinned_engine_directory();

        assert_eq!(config.engines.directory, discovered);
    }

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
        for engine in ["rtk", "grepai", "icm", "node"] {
            fs::write(engines.join(engine), []).expect("write engine fixture");
        }

        assert_eq!(
            sibling_engine_directory(&executable),
            Some(fs::canonicalize(engines).expect("canonical engine directory"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_public_binary_symlink_discovers_private_bundle_engines() {
        let directory = tempdir().expect("temporary directory");
        let root = directory.path().join("install");
        let release = versioned_bundle(&root, "v0.4.6-darwin-arm64");
        point_current_at(&root, &release);
        let public = directory.path().join("bin/hzr");
        fs::create_dir_all(public.parent().expect("public parent")).expect("public directory");
        std::os::unix::fs::symlink(root.join("current/bin/hzr"), &public)
            .expect("public binary symlink");

        assert_eq!(
            sibling_engine_directory(&public),
            Some(
                fs::canonicalize(&root)
                    .expect("canonical install root")
                    .join("current/engines")
            )
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
    fn test_selected_activation_allows_only_explicit_workspaces_and_round_trips() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("config.toml");
        let mut config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        config.activation.mode = ActivationMode::Selected;
        config.activation.enabled_workspaces.push(EnabledWorkspace {
            repository_id: "a".repeat(64),
            worktree_id: "b".repeat(64),
            root: "/work/enabled".into(),
        });

        assert!(config.activation.allows(&"a".repeat(64), &"b".repeat(64)));
        assert!(!config.activation.allows(&"c".repeat(64), &"d".repeat(64)));

        config.write(&path).expect("config write");
        let loaded = Config::load(&path).expect("config load");
        assert_eq!(loaded.activation.mode, ActivationMode::Selected);
        assert_eq!(loaded.activation.enabled_workspaces.len(), 1);
    }

    #[test]
    fn test_clean_install_defaults_to_fts_only_memory() {
        let config = EngineConfig::default();

        assert!(!config.icm_embeddings);
        assert_eq!(config.grepai_watcher_limit, 8);
        assert_eq!(config.grepai_watcher_idle_ttl_seconds, 15 * 60);
        assert_eq!(config.grepai_watcher_sweep_seconds, 30);
    }

    #[test]
    fn test_config_rejects_disabled_watcher_lifecycle_budget() {
        let mut config = Config::default();
        config.engines.grepai_watcher_limit = 0;

        assert!(matches!(
            config.validate(),
            Err(ConfigError::InvalidWatcherLifecycle)
        ));
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
        assert_eq!(engines.binary("node"), Path::new("/opt/hzr/engines/node"));
        assert_eq!(engines.binary("git"), Path::new("git"));
        assert_eq!(engines.binary("rg"), Path::new("rg"));
    }

    #[test]
    fn test_policy_context_budget_reserves_output_and_safety_margin() {
        let policy = PolicyConfig {
            context_token_limit: 16_000,
            output_reserve: 2_000,
            safety_margin: 1_000,
            ..PolicyConfig::default()
        };

        assert_eq!(policy.input_token_budget(), 13_000);
    }

    #[test]
    fn test_config_rejects_policy_without_input_budget() {
        let mut config = Config::default();
        config.policy.context_token_limit = 3_000;
        config.policy.output_reserve = 2_000;
        config.policy.safety_margin = 1_000;

        assert!(matches!(
            config.validate(),
            Err(ConfigError::InvalidPolicyBudget)
        ));
    }
}
