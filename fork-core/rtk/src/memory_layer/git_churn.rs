//! Git churn frequency index: deterministic objective ranking signal.
//! Counts how often each file appears in `git log --all --name-only`,
//! caches the result per HEAD commit SHA to avoid re-scanning on each request.
//!
//! Replaces the subjective `f_affinity_score` (episode-based) with a pure
//! historical signal from git history — no interpretation, no staleness risk.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock}; // P1: in-process cache — avoid git log on every request

use anyhow::{Context, Result};

// ── Public types ──────────────────────────────────────────────────────────────

/// Frequency map cached per HEAD SHA.
/// Invalidates automatically when HEAD changes (new commit).
#[derive(Clone)] // P1: needed for in-process static cache
pub struct ChurnCache {
    /// HEAD commit SHA at time of build.
    pub head_sha: String,
    /// rel_path → number of times file appears in git log.
    pub freq_map: HashMap<String, u32>,
    /// Max count across all files (used for normalization).
    pub max_count: u32,
}

impl ChurnCache {
    /// Return log-normalized churn score in [0.0, 1.0].
    ///
    /// Uses `ln(count) / ln(max_count)` so that Cargo.lock (changed 100×)
    /// doesn't dominate over src files (changed 20×) — they stay comparable.
    pub fn score(&self, rel_path: &str) -> f32 {
        let count = *self.freq_map.get(rel_path).unwrap_or(&0) as f32;
        if count == 0.0 || self.max_count <= 1 {
            return 0.0;
        }
        let max = self.max_count as f32;
        // ln(count) / ln(max) — log-normalized in (0, 1]
        (count.ln() / max.ln()).clamp(0.0, 1.0)
    }
}

// ── In-process cache ─────────────────────────────────────────────────────────

// P1: persistent in-process cache keyed by repo_path.
// Invalidates automatically when HEAD SHA changes (new commit pushed).
static CHURN_CACHE: OnceLock<Mutex<HashMap<String, ChurnCache>>> = OnceLock::new();

fn churn_cache_global() -> &'static Mutex<HashMap<String, ChurnCache>> {
    CHURN_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Build a `ChurnCache` by scanning `git log --all --format="" --name-only`.
/// Returns `Ok(cache)` even on empty repos (all scores will be 0.0).
/// Results are cached in-process per (repo_path, HEAD SHA) — repeated calls are O(1).
pub fn load_churn(repo_root: &Path) -> Result<ChurnCache> {
    let head_sha = get_head_sha(repo_root).unwrap_or_else(|_| "unknown".to_string());
    let key = repo_root.to_string_lossy().to_string();

    // P1: fast path — return cached result if HEAD hasn't changed
    {
        let guard = churn_cache_global()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = guard.get(&key) {
            if cached.head_sha == head_sha {
                return Ok(cached.clone());
            }
        }
    }

    // Cache miss (first call or new commit): run git log
    let freq_map = build_freq_map(repo_root)?;
    let max_count = freq_map.values().copied().max().unwrap_or(0);
    let result = ChurnCache {
        head_sha,
        freq_map,
        max_count,
    };

    // Store in cache (lock re-acquired after drop above — no deadlock)
    churn_cache_global()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, result.clone());

    Ok(result)
}

/// Convenience free-function: `churn_score(cache, path)` mirrors the method.
pub fn churn_score(cache: &ChurnCache, rel_path: &str) -> f32 {
    cache.score(rel_path)
}

// ── Internals ─────────────────────────────────────────────────────────────────

fn get_head_sha(repo_root: &Path) -> Result<String> {
    let out = Command::new("git")
        .args(["-C", repo_root.to_str().unwrap_or("."), "rev-parse", "HEAD"])
        .output()
        .context("git rev-parse HEAD failed")?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// changed: streaming BufReader instead of loading full git log into RAM (performance fix)
fn build_freq_map(repo_root: &Path) -> Result<HashMap<String, u32>> {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let mut child = Command::new("git")
        .args([
            "-C",
            repo_root.to_str().unwrap_or("."),
            "log",
            "--all",
            "--format=", // empty commit header → only file names
            "--name-only",
            "--since=6 months ago", // M3: bound git log scan for large repos
        ])
        .stdout(Stdio::piped())
        .spawn()
        .context("git log spawn failed")?;

    let reader = BufReader::new(child.stdout.take().context("no stdout")?);
    let mut map: HashMap<String, u32> = HashMap::new();
    for line in reader.lines() {
        let line = line.context("git log read error")?;
        let trimmed = line.trim().to_string();
        if !trimmed.is_empty() {
            *map.entry(trimmed).or_insert(0) += 1;
        }
    }
    child.wait().context("git log wait failed")?;
    Ok(map)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cache(entries: &[(&str, u32)]) -> ChurnCache {
        let freq_map: HashMap<String, u32> =
            entries.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        let max_count = freq_map.values().copied().max().unwrap_or(0);
        ChurnCache {
            head_sha: "test_sha".to_string(),
            freq_map,
            max_count,
        }
    }

    #[test]
    fn test_score_zero_for_unknown_file() {
        let cache = make_cache(&[("src/auth.rs", 10), ("src/main.rs", 5)]);
        assert_eq!(cache.score("src/unknown.rs"), 0.0);
    }

    #[test]
    fn test_score_one_for_max_churn_file() {
        let cache = make_cache(&[("hot.rs", 100), ("cold.rs", 1)]);
        let s = cache.score("hot.rs");
        assert!(
            (s - 1.0).abs() < 1e-6,
            "max-churn file should score 1.0, got {s}"
        );
    }

    #[test]
    fn test_score_zero_for_single_occurrence() {
        // ln(1) = 0 → score is 0.0 even though file exists
        let cache = make_cache(&[("lone.rs", 1), ("hot.rs", 50)]);
        assert_eq!(cache.score("lone.rs"), 0.0, "count=1 → ln(1)/ln(max) = 0");
    }

    #[test]
    fn test_log_normalization_ordering() {
        // A changes 100×, B changes 10×, C changes 1×
        let cache = make_cache(&[("a.rs", 100), ("b.rs", 10), ("c.rs", 1)]);
        let sa = cache.score("a.rs");
        let sb = cache.score("b.rs");
        let sc = cache.score("c.rs");
        assert!(sa > sb, "a(100) should score higher than b(10)");
        assert!(sb > sc, "b(10) should score higher than c(1)");
        assert!(sa <= 1.0 && sb >= 0.0);
    }

    #[test]
    fn test_empty_cache_all_zeros() {
        let cache = make_cache(&[]);
        assert_eq!(cache.score("any.rs"), 0.0);
    }

    #[test]
    fn test_free_fn_churn_score_matches_method() {
        let cache = make_cache(&[("src/lib.rs", 42), ("src/main.rs", 100)]);
        assert_eq!(churn_score(&cache, "src/lib.rs"), cache.score("src/lib.rs"));
        assert_eq!(churn_score(&cache, "missing.rs"), cache.score("missing.rs"));
    }

    #[test]
    fn test_load_churn_real_repo() {
        // Integration test: actual git repo (the rtk repo itself).
        // Only tests that load_churn doesn't error and returns a non-empty map.
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let cache = load_churn(repo).expect("load_churn should succeed on a real repo");
        assert!(
            !cache.freq_map.is_empty(),
            "rtk repo should have non-empty churn map"
        );
        assert_ne!(cache.head_sha, "unknown", "should have a real HEAD sha");
        // src/main.rs changes a lot in this repo
        let main_score = cache.score("src/main.rs");
        assert!(main_score > 0.0, "src/main.rs should have non-zero churn");
    }
}
