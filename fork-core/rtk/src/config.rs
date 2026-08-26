use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub tracking: TrackingConfig,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub filters: FilterConfig,
    #[serde(default)]
    pub grepai: GrepaiConfig, // grepai integration settings
    #[serde(default)]
    pub tee: crate::tee::TeeConfig, // upstream sync: tee raw output config
    #[serde(default)]
    pub mem: MemConfig, // memory layer config (cache TTL, project limit, symbol limit)
    #[serde(default)]
    pub hooks: HooksConfig, // fork: ported from upstream v0.42.4 (rewrite engine config)
    #[serde(default)]
    pub gh: GhConfig,
}

/// GitHub CLI routing options.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct GhConfig {
    /// Route `gh <cmd> --json …` through the lossless repacker instead of
    /// leaving it as exact passthrough. Off by default — see [`gh_pack_json`].
    #[serde(default)]
    pub pack_json: bool,
}

/// Hook rewrite engine config (fork: ported from upstream v0.42.4).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct HooksConfig {
    /// Commands to exclude from auto-rewrite (e.g. ["curl", "playwright"]).
    #[serde(default)]
    pub exclude_commands: Vec<String>,
    /// Wrapper prefixes transparently stripped before rewrite, then re-prepended
    /// (e.g. ["docker exec mycontainer", "poetry run"]). Literal match, not pattern.
    #[serde(default)]
    pub transparent_prefixes: Vec<String>,
    /// Suppress informational stderr diagnostics — notes about commands that
    /// *succeeded*. In a terminal they are a useful signal; inside an agent the
    /// host merges stderr into the model's context, so a note about a command
    /// that worked costs more tokens than the filter saved. Real errors and
    /// truncation warnings are never suppressed.
    #[serde(default)]
    pub quiet: bool,
}

/// True when `gh <cmd> --json …` may be routed through the lossless repacker.
///
/// Off by default and opt-in via `RTK_GH_PACK_JSON=1` or `[gh] pack_json = true`.
/// `gh api` is packed unconditionally because its previous rendering was a lossy
/// preview; `--json`, by contrast, is currently exact passthrough, and emitting
/// a notation the caller did not ask for is a product decision rather than a bug
/// fix — so it stays behind a switch until measured.
pub fn gh_pack_json() -> bool {
    static PACK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PACK.get_or_init(|| {
        if std::env::var("RTK_GH_PACK_JSON").ok().as_deref() == Some("1") {
            return true;
        }
        Config::load()
            .map(|config| config.gh.pack_json)
            .unwrap_or(false)
    })
}

/// True when informational diagnostics should be suppressed: `RTK_QUIET=1`, or
/// `[hooks] quiet = true` in `config.toml`.
///
/// `Config::load()` re-reads and re-parses the file on every call and this sits
/// on the command path, so the answer is cached for the process lifetime — one
/// read, not one per diagnostic.
pub fn quiet_diagnostics() -> bool {
    static QUIET: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *QUIET.get_or_init(|| {
        if std::env::var("RTK_QUIET").ok().as_deref() == Some("1") {
            return true;
        }
        Config::load()
            .map(|config| config.hooks.quiet)
            .unwrap_or(false)
    })
}

/// `eprintln!` for informational diagnostics, silenced by [`quiet_diagnostics`].
#[macro_export]
macro_rules! rtk_info {
    ($($arg:tt)*) => {
        if !$crate::config::quiet_diagnostics() {
            eprintln!($($arg)*);
        }
    };
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TrackingConfig {
    pub enabled: bool,
    pub history_days: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_path: Option<PathBuf>,
}

impl Default for TrackingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            history_days: 90,
            database_path: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DisplayConfig {
    pub colors: bool,
    pub emoji: bool,
    pub max_width: usize,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            colors: true,
            emoji: true,
            max_width: 120,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FilterConfig {
    pub ignore_dirs: Vec<String>,
    pub ignore_files: Vec<String>,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            ignore_dirs: vec![
                ".git".into(),
                "node_modules".into(),
                "target".into(),
                "__pycache__".into(),
                ".venv".into(),
                "vendor".into(),
            ],
            ignore_files: vec!["*.lock".into(), "*.min.js".into(), "*.min.css".into()],
        }
    }
}

/// E6.4: Feature flags for the memory layer — per-feature enable/disable.
/// All flags default to `true` (opt-out), except `strict_by_default` (opt-in).
#[derive(Debug, Serialize, Deserialize)]
pub struct MemFeatureFlags {
    /// Enable L2 type_graph extraction (type relations; slower). Default: true.
    #[serde(default = "bool_true")]
    pub type_graph: bool,
    /// Enable L5 test_map file classification. Default: true.
    #[serde(default = "bool_true")]
    pub test_map: bool,
    /// Enable L4 dep_manifest parsing (Cargo.toml/package.json/pyproject). Default: true.
    #[serde(default = "bool_true")]
    pub dep_manifest: bool,
    /// Enable E3.2 cascade invalidation through import graph. Default: true.
    #[serde(default = "bool_true")]
    pub cascade_invalidation: bool,
    /// Enable E3.3 git delta queries (`--since REV`). Default: true.
    #[serde(default = "bool_true")]
    pub git_delta: bool,
    /// Apply `--strict` mode by default in `rtk memory explore`. Default: false.
    #[serde(default)]
    pub strict_by_default: bool,
    /// PRD R1: Enable graph-first plan pipeline (default: true). Default: true.
    #[serde(default = "bool_true")]
    pub graph_first_plan: bool, // ADDED: PRD graph-first pipeline switch
    /// PRD R1: Fail-open to legacy pipeline on graph-first error (default: true).
    #[serde(default = "bool_true")]
    pub plan_fail_open: bool, // ADDED: PRD fail-open fallback flag
}

fn bool_true() -> bool {
    true
}

impl Default for MemFeatureFlags {
    fn default() -> Self {
        Self {
            type_graph: true,
            test_map: true,
            dep_manifest: true,
            cascade_invalidation: true,
            git_delta: true,
            strict_by_default: false,
            graph_first_plan: true, // ADDED: default on
            plan_fail_open: true,   // ADDED: default on
        }
    }
}

/// Memory layer configuration (§9 PRD: cache + symbol limits)
#[derive(Debug, Serialize, Deserialize)]
pub struct MemConfig {
    #[serde(default = "MemConfig::default_cache_ttl_secs")]
    pub cache_ttl_secs: u64, // seconds before artifact is considered stale (default: 86400)
    #[serde(default = "MemConfig::default_cache_max_projects")]
    pub cache_max_projects: usize, // max cached projects in mem.db (default: 64)
    #[serde(default = "MemConfig::default_max_symbols_per_file")]
    pub max_symbols_per_file: usize, // L3: cap symbols per file (default: 64)
    #[serde(default)]
    pub features: MemFeatureFlags, // E6.4: per-feature enable/disable flags
    /// PRD R1: max candidates passed to graph-first pipeline (default: 60).
    #[serde(default = "MemConfig::default_plan_candidate_cap")]
    pub plan_candidate_cap: usize, // ADDED: PRD R1 hard cap
    /// PRD R2: max candidates passed to semantic stage (default: 30).
    #[serde(default = "MemConfig::default_plan_semantic_cap")]
    pub plan_semantic_cap: usize, // ADDED: PRD R2 semantic cap
    /// PRD R3: minimum final score to retain candidate (default: 0.12).
    #[serde(default = "MemConfig::default_plan_min_final_score")]
    pub plan_min_final_score: f32, // ADDED: PRD R3 threshold
}

impl MemConfig {
    fn default_cache_ttl_secs() -> u64 {
        86400 // 24 h — matches CACHE_TTL_SECS compile-time fallback
    }
    fn default_cache_max_projects() -> usize {
        64 // matches CACHE_MAX_PROJECTS compile-time fallback
    }
    fn default_max_symbols_per_file() -> usize {
        64 // matches MAX_SYMBOLS_PER_FILE compile-time fallback
    }
    fn default_plan_candidate_cap() -> usize {
        60
    } // ADDED: PRD R1
    fn default_plan_semantic_cap() -> usize {
        30
    } // ADDED: PRD R2
    fn default_plan_min_final_score() -> f32 {
        0.12
    } // ADDED: PRD R3
}

impl Default for MemConfig {
    fn default() -> Self {
        Self {
            cache_ttl_secs: Self::default_cache_ttl_secs(),
            cache_max_projects: Self::default_cache_max_projects(),
            max_symbols_per_file: Self::default_max_symbols_per_file(),
            features: MemFeatureFlags::default(), // E6.4
            plan_candidate_cap: Self::default_plan_candidate_cap(), // ADDED
            plan_semantic_cap: Self::default_plan_semantic_cap(), // ADDED
            plan_min_final_score: Self::default_plan_min_final_score(), // ADDED
        }
    }
}

/// grepai external semantic search integration
#[derive(Debug, Serialize, Deserialize)]
pub struct GrepaiConfig {
    /// Enable grepai delegation in `rtk rgai` (default: true)
    pub enabled: bool,
    /// Auto-init projects on first `rtk rgai` if grepai is installed (default: true)
    pub auto_init: bool,
    /// Custom binary path override (default: auto-detect via PATH)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_path: Option<PathBuf>,
}

impl Default for GrepaiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_init: true,
            binary_path: None,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = get_config_path()?;

        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    /// Ignore lists actually used by the system commands: the built-in noise set
    /// merged with `[filters].ignore_dirs` / `ignore_files` from `config.toml`.
    ///
    /// Those two config keys existed but had no reader — a user could set them
    /// and nothing happened. The built-ins stay as the floor so the default
    /// behaviour is unchanged; config entries are added on top, de-duplicated
    /// and order-stable so the generated ignore patterns are deterministic.
    pub fn merged_ignore_dirs(builtin: &[&str]) -> Vec<String> {
        Self::merge_ignore_list(builtin, |filters| &filters.ignore_dirs)
    }

    pub fn merged_ignore_files(builtin: &[&str]) -> Vec<String> {
        Self::merge_ignore_list(builtin, |filters| &filters.ignore_files)
    }

    fn merge_ignore_list(
        builtin: &[&str],
        select: fn(&FilterConfig) -> &Vec<String>,
    ) -> Vec<String> {
        let mut merged: Vec<String> = builtin.iter().map(|entry| (*entry).to_string()).collect();
        // Config I/O is deliberately kept off the hot path: one cached read per
        // process, not one per command that needs the list.
        static CACHED: std::sync::OnceLock<Option<Config>> = std::sync::OnceLock::new();
        let cached = CACHED.get_or_init(|| Config::load().ok());
        if let Some(config) = cached {
            for entry in select(&config.filters) {
                if !merged.iter().any(|existing| existing == entry) {
                    merged.push(entry.clone());
                }
            }
        }
        merged
    }

    pub fn save(&self) -> Result<()> {
        let path = get_config_path()?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    pub fn create_default() -> Result<PathBuf> {
        let config = Config::default();
        config.save()?;
        get_config_path()
    }
}

fn get_config_path() -> Result<PathBuf> {
    let config_dir = match dirs::config_dir() {
        Some(path) if path.is_absolute() => path,
        Some(path) => std::env::current_dir()?.join(path),
        None => std::env::current_dir()?.join(".config"),
    };
    Ok(config_dir.join("rtk").join("config.toml"))
}

const CONFIG_OUTPUT_SCHEMA_VERSION: u32 = 2;

#[derive(Serialize)]
struct ConfigOutput<'a> {
    schema_version: u32,
    config_path: &'a std::path::Path,
    config_exists: bool,
    config_sha256: Option<String>,
    config: &'a Config,
}

fn config_json(
    path: &std::path::Path,
    exists: bool,
    config_sha256: Option<String>,
    config: &Config,
) -> Result<String> {
    Ok(serde_json::to_string_pretty(&ConfigOutput {
        schema_version: CONFIG_OUTPUT_SCHEMA_VERSION,
        config_path: path,
        config_exists: exists,
        config_sha256,
        config,
    })?)
}

pub fn show_config(format: &str) -> Result<()> {
    let path = get_config_path()?;
    let exists = path.exists();
    let (config, config_sha256) = if exists {
        let bytes = std::fs::read(&path)?;
        let config: Config = toml::from_str(std::str::from_utf8(&bytes)?)?;
        (config, Some(format!("{:x}", Sha256::digest(&bytes))))
    } else {
        (Config::default(), None)
    };

    if format == "json" {
        println!("{}", config_json(&path, exists, config_sha256, &config)?);
        return Ok(());
    }

    println!("Config: {}", path.display());
    println!();
    if !exists {
        println!("(default config, file not created)");
        println!();
    }
    println!("{}", toml::to_string_pretty(&config)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grepai_config_defaults_are_enabled_with_auto_init() {
        let cfg = Config::default();
        assert!(cfg.grepai.enabled);
        assert!(cfg.grepai.auto_init);
        assert_eq!(cfg.grepai.binary_path, None);
    }

    #[test]
    fn config_json_is_typed_versioned_and_machine_parseable() {
        let config = Config::default();
        let output = config_json(
            std::path::Path::new("/tmp/rtk/config.toml"),
            false,
            None,
            &config,
        )
        .expect("config JSON");
        let value: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert_eq!(value["schema_version"], 2);
        assert_eq!(value["config_path"], "/tmp/rtk/config.toml");
        assert_eq!(value["config_exists"], false);
        assert!(value["config_sha256"].is_null());
        assert_eq!(value["config"]["grepai"]["enabled"], true);
        assert_eq!(value["config"]["grepai"]["auto_init"], true);
    }
}
