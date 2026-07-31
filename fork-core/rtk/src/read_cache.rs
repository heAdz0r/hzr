//! Read cache: filesystem-based cache for filtered read output.
//! Extracted from read.rs (PR-2).

use crate::filter::FilterLevel;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const READ_CACHE_VERSION: u32 = 1;
const READ_CACHE_MAX_ENTRIES: usize = 512;

// ── Public API ──────────────────────────────────────────────

/// Check whether read cache should be used for the given parameters.
pub fn should_use_read_cache(
    level: FilterLevel,
    from: Option<usize>,
    to: Option<usize>,
    max_lines: Option<usize>,
    line_numbers: bool,
    _dedup: bool,
) -> bool {
    level != FilterLevel::None
        && from.is_none()
        && to.is_none()
        && max_lines.is_none()
        && !line_numbers
}

/// Build a cache key incorporating path, metadata, and read options.
pub fn build_read_cache_key(
    file: &Path,
    level: FilterLevel,
    from: Option<usize>,
    to: Option<usize>,
    max_lines: Option<usize>,
    line_numbers: bool,
    dedup: bool,
) -> Result<String> {
    let canonical = fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let metadata =
        fs::metadata(file).with_context(|| format!("Failed to stat {}", file.display()))?;
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
        .map(|dur| dur.as_nanos())
        .unwrap_or(0);

    #[cfg(unix)]
    let (dev, ino) = (metadata.dev(), metadata.ino());
    #[cfg(not(unix))]
    let (dev, ino) = (0u64, 0u64);

    Ok(format!(
        "v={READ_CACHE_VERSION}|path={}|size={}|mtime_ns={modified_ns}|dev={dev}|ino={ino}|level={level}|from={:?}|to={:?}|max_lines={:?}|line_numbers={line_numbers}|dedup={dedup}",
        canonical.display(),
        metadata.len(),
        from,
        to,
        max_lines
    ))
}

/// Try to load cached output for the given key.
pub fn load_read_cache(key: &str) -> Option<String> {
    let path = read_cache_path(key);
    let raw = fs::read_to_string(path).ok()?;
    let entry: ReadCacheEntry = serde_json::from_str(&raw).ok()?;
    if entry.key != key {
        return None;
    }
    Some(entry.output)
}

/// Store output in the cache for the given key.
pub fn store_read_cache(key: &str, output: &str) {
    let dir = read_cache_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let entry = ReadCacheEntry {
        key: key.to_string(),
        output: output.to_string(),
    };
    let serialized = match serde_json::to_string(&entry) {
        Ok(value) => value,
        Err(_) => return,
    };
    let final_path = read_cache_path(key);
    let tmp_name = format!(
        ".tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let tmp_path = dir.join(tmp_name);
    if fs::write(&tmp_path, serialized).is_ok() {
        let _ = fs::rename(tmp_path, final_path);
        prune_read_cache(&dir);
    }
}

// ── Internal helpers ────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct ReadCacheEntry {
    key: String,
    output: String,
}

fn read_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("rtk")
        .join("read-cache")
}

fn read_cache_path(key: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    let digest = hasher.finish();
    read_cache_dir().join(format!("{digest:016x}.json"))
}

fn prune_read_cache(dir: &Path) {
    let mut files = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    return None;
                }
                let modified = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                Some((path, modified))
            })
            .collect::<Vec<_>>(),
        Err(_) => return,
    };

    if files.len() <= READ_CACHE_MAX_ENTRIES {
        return;
    }

    files.sort_by_key(|(_, modified)| *modified);
    let remove_count = files.len().saturating_sub(READ_CACHE_MAX_ENTRIES);
    for (path, _) in files.into_iter().take(remove_count) {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn should_use_cache_only_for_filtered_default_reads() {
        assert!(should_use_read_cache(
            FilterLevel::Minimal,
            None,
            None,
            None,
            false,
            false
        ));
        assert!(!should_use_read_cache(
            FilterLevel::None,
            None,
            None,
            None,
            false,
            false
        ));
        assert!(!should_use_read_cache(
            FilterLevel::Minimal,
            Some(1),
            None,
            None,
            false,
            false
        ));
        assert!(!should_use_read_cache(
            FilterLevel::Minimal,
            None,
            None,
            None,
            true,
            false
        ));
    }

    #[test]
    fn cache_key_varies_by_level() -> Result<()> {
        let mut file = NamedTempFile::new()?;
        writeln!(file, "data")?;
        let key_min = build_read_cache_key(
            file.path(),
            FilterLevel::Minimal,
            None,
            None,
            None,
            false,
            false,
        )?;
        let key_aggr = build_read_cache_key(
            file.path(),
            FilterLevel::Aggressive,
            None,
            None,
            None,
            false,
            false,
        )?;
        assert_ne!(key_min, key_aggr);
        Ok(())
    }

    #[test]
    fn cache_key_varies_by_dedup() -> Result<()> {
        let mut file = NamedTempFile::new()?;
        writeln!(file, "data")?;
        let key_plain = build_read_cache_key(
            file.path(),
            FilterLevel::Minimal,
            None,
            None,
            None,
            false,
            false,
        )?;
        let key_dedup = build_read_cache_key(
            file.path(),
            FilterLevel::Minimal,
            None,
            None,
            None,
            false,
            true,
        )?;
        assert_ne!(key_plain, key_dedup);
        Ok(())
    }

    #[test]
    fn store_and_load_cache_roundtrip() {
        let key = "test-roundtrip-key-unique-12345";
        let output = "cached output content";
        store_read_cache(key, output);
        let loaded = load_read_cache(key);
        assert_eq!(loaded, Some(output.to_string()));
        // cleanup
        let path = read_cache_path(key);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn load_cache_returns_none_for_missing_key() {
        let result = load_read_cache("nonexistent-key-987654321");
        assert!(result.is_none());
    }
}
