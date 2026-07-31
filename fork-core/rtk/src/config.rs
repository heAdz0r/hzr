use anyhow::Result;
use serde::{Deserialize, Serialize};
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
    let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    Ok(config_dir.join("rtk").join("config.toml"))
}

pub fn show_config() -> Result<()> {
    let path = get_config_path()?;
    println!("Config: {}", path.display());
    println!();

    if path.exists() {
        let config = Config::load()?;
        println!("{}", toml::to_string_pretty(&config)?);
    } else {
        println!("(default config, file not created)");
        println!();
        let config = Config::default();
        println!("{}", toml::to_string_pretty(&config)?);
    }

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
}
