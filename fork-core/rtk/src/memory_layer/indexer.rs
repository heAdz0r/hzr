// E0.1: Indexing/scanning extracted from mod.rs
use aho_corasick; // CR-07: multi-pattern search для find_cascade_dependents
use anyhow::{Context, Result};
use ignore::{WalkBuilder, WalkState};
use rayon::prelude::*; // parallel iterators for memory layer hot paths
use std::collections::{BTreeMap, HashMap, HashSet}; // E3.2: HashSet for cascade sets
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::cache::{
    canonical_project_root, epoch_secs, format_hash, hash_file, is_artifact_stale, load_artifact,
    project_cache_key,
};
use super::extractor;
use super::manifest;
use super::{
    BuildState, DeltaKind, DeltaSummary, FileArtifact, FileDelta, FileMeta, GraphSummary,
    ProjectArtifact, ScanStats, ARTIFACT_VERSION, EXCLUDED_DIRS,
};

pub(super) fn build_state(
    project: &Path,
    refresh: bool,
    cascade_enabled: bool, // E6.4: feature flag — skip cascade invalidation when false
    verbose: u8,
) -> Result<BuildState> {
    let project_root = canonical_project_root(project)?;
    let project_id = project_cache_key(&project_root);

    let previous = load_artifact(&project_root)?;
    let previous_exists = previous.is_some();
    let stale_previous = previous.as_ref().map(is_artifact_stale).unwrap_or(false);

    let previous_map: HashMap<String, FileArtifact> = previous
        .as_ref()
        .map(|artifact| {
            artifact
                .files
                .iter()
                .cloned()
                .map(|entry| (entry.rel_path.clone(), entry))
                .collect()
        })
        .unwrap_or_default();

    let current_files = scan_project_metadata(&project_root)?;
    let force_rehash = refresh || stale_previous;
    let (files, delta, scan_stats, total_bytes) = build_incremental_files(
        &current_files,
        &previous_map,
        force_rehash,
        cascade_enabled,
        verbose,
    )?; // E6.4: pass cascade flag

    let now = epoch_secs(SystemTime::now());
    let created_at = previous.as_ref().map(|a| a.created_at).unwrap_or(now);

    let dep_manifest = manifest::parse_dep_manifest(&project_root); // L4: parse fresh on every build
    let artifact = ProjectArtifact {
        version: ARTIFACT_VERSION,
        project_id: project_id.clone(),
        project_root: project_root.to_string_lossy().to_string(),
        created_at,
        updated_at: now,
        file_count: files.len(),
        total_bytes,
        files,
        dep_manifest, // L4: dependency manifest
    };

    let cache_hit = previous_exists && !refresh && !stale_previous && delta.changes.is_empty();
    let graph = summarize_graph(&artifact);

    Ok(BuildState {
        project_root,
        project_id,
        previous_exists,
        stale_previous,
        cache_hit,
        scan_stats,
        artifact,
        delta,
        graph,
    })
}

pub(super) fn artifact_is_dirty(project_root: &Path, artifact: &ProjectArtifact) -> Result<bool> {
    let current_files = scan_project_metadata(project_root)?;
    if current_files.len() != artifact.files.len() {
        return Ok(true);
    }

    let previous_map: HashMap<&str, &FileArtifact> = artifact
        .files
        .iter()
        .map(|entry| (entry.rel_path.as_str(), entry))
        .collect();

    for (rel_path, meta) in current_files {
        let Some(previous) = previous_map.get(rel_path.as_str()) else {
            return Ok(true);
        };

        if previous.size != meta.size || previous.mtime_ns != meta.mtime_ns {
            return Ok(true);
        }
    }

    Ok(false)
}

pub(super) fn build_git_delta(
    project_root: &Path,
    since_rev: &str,
    verbose: u8,
) -> Result<DeltaSummary> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["diff", "--name-status", "--find-renames"])
        .arg(format!("{since_rev}..HEAD"))
        .arg("--")
        .output()
        .with_context(|| {
            format!(
                "Failed to run git diff for delta since '{}' in {}",
                since_rev,
                project_root.display()
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "git diff failed for --since '{}': {}",
            since_rev,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut changes: Vec<FileDelta> = Vec::new();

    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 2 {
            continue;
        }

        let status = fields[0];
        let (change, rel_path) = if status.starts_with('R') {
            if fields.len() < 3 {
                continue;
            }
            (DeltaKind::Modified, fields[2])
        } else if status.starts_with('A') {
            (DeltaKind::Added, fields[1])
        } else if status.starts_with('M') {
            (DeltaKind::Modified, fields[1])
        } else if status.starts_with('D') {
            (DeltaKind::Removed, fields[1])
        } else {
            continue;
        };

        let rel = normalize_rel_path(Path::new(rel_path));
        if should_skip_rel_path(Path::new(&rel)) {
            continue;
        }

        let (old_hash, new_hash) = hashes_for_git_delta(project_root, &rel, change);
        changes.push(FileDelta {
            path: rel,
            change,
            old_hash,
            new_hash,
        });
    }

    changes.sort_by(|a, b| a.path.cmp(&b.path));
    let delta = summarize_delta(changes);

    if verbose > 0 {
        eprintln!(
            "memory.delta since={} +{} ~{} -{}",
            since_rev, delta.added, delta.modified, delta.removed
        );
    }

    Ok(delta)
}

fn hashes_for_git_delta(
    project_root: &Path,
    rel_path: &str,
    change: DeltaKind,
) -> (Option<String>, Option<String>) {
    if matches!(change, DeltaKind::Removed) {
        return (None, None);
    }

    let abs = project_root.join(PathBuf::from(rel_path));
    let hash = hash_file(&abs).ok().map(format_hash);
    (None, hash)
}

/// E3.2: Compute all module-name stems for a file path for import-graph matching.
/// E.g. "src/memory_layer/cache.rs" → ["cache", "cache.rs",
///      "src/memory_layer/cache", "src::memory_layer::cache",
///      "crate::src::memory_layer::cache", "./cache"]
fn module_stems_for_path(rel_path: &str) -> Vec<String> {
    let p = Path::new(rel_path);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(rel_path);
    let name_with_ext = p.file_name().and_then(|s| s.to_str()).unwrap_or(rel_path);

    let extensions = [
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".py", ".go",
    ];
    let mut no_ext = rel_path;
    for ext in &extensions {
        if let Some(stripped) = rel_path.strip_suffix(ext) {
            no_ext = stripped;
            break;
        }
    }

    let double_colon = no_ext.replace(['/', '\\'], "::");
    let mut stems = vec![
        stem.to_string(),
        name_with_ext.to_string(),
        no_ext.to_string(),   // "src/memory_layer/cache"
        double_colon.clone(), // "src::memory_layer::cache"
        format!("./{stem}"),  // relative TS/JS import
    ];
    // Rust: add crate:: prefix variant
    if !double_colon.starts_with("crate::") {
        stems.push(format!("crate::{double_colon}"));
    }
    stems
}

/// E3.2: Find files in `previous_map` that import any of the `changed_paths`.
/// Uses Aho-Corasick multi-pattern search for O(N*M*(I+S)) instead of O(N*M*S*L).
///
/// CR-07: заменяем O(N×M×S×L) nested contains() на AhoCorasick.
/// Один проход AhoCorasick по строке импорта находит все stems одновременно —
/// O(|import| + |stems|) вместо O(|import| × |stems|) для каждой пары.
fn find_cascade_dependents(
    changed_paths: &HashSet<String>,
    previous_map: &HashMap<String, FileArtifact>,
) -> HashSet<String> {
    let changed_stems: Vec<String> = changed_paths
        .iter()
        .flat_map(|p| module_stems_for_path(p))
        .collect();

    if changed_stems.is_empty() {
        return HashSet::new();
    }

    // CR-07: строим AhoCorasick один раз до par_iter — O(S) construction,
    // затем каждый поиск по import-строке O(|import| + matches) вместо O(|import|×S).
    let ac = match aho_corasick::AhoCorasick::new(&changed_stems) {
        Ok(ac) => ac,
        Err(_) => {
            // fallback: оригинальный алгоритм если AhoCorasick не смог собраться
            return previous_map
                .par_iter()
                .filter(|(rel_path, _)| !changed_paths.contains(*rel_path))
                .filter_map(|(rel_path, artifact)| {
                    let is_dependent = artifact.imports.iter().any(|import| {
                        !import.starts_with("self:")
                            && changed_stems
                                .iter()
                                .any(|stem| import.contains(stem.as_str()))
                    });
                    if is_dependent {
                        Some(rel_path.clone())
                    } else {
                        None
                    }
                })
                .collect();
        }
    };

    // parallel: каждый файл независим — par_iter + AhoCorasick::is_match() (thread-safe)
    previous_map
        .par_iter()
        .filter(|(rel_path, _)| !changed_paths.contains(*rel_path))
        .filter_map(|(rel_path, artifact)| {
            let is_dependent = artifact.imports.iter().any(|import| {
                !import.starts_with("self:") && ac.is_match(import) // CR-07: O(|import|) per check
            });
            if is_dependent {
                Some(rel_path.clone())
            } else {
                None
            }
        })
        .collect()
}

fn build_incremental_files(
    current_files: &BTreeMap<String, FileMeta>,
    previous_map: &HashMap<String, FileArtifact>,
    force_rehash: bool,
    cascade_enabled: bool, // E6.4: feature flag — when false, skip import-graph cascade pass
    verbose: u8,
) -> Result<(Vec<FileArtifact>, DeltaSummary, ScanStats, u64)> {
    // E3.2: Pre-pass — identify metadata-changed files, then expand via import graph cascade.
    let cascade_paths: HashSet<String> = if cascade_enabled
        && !force_rehash
        && !previous_map.is_empty()
    {
        // E6.4: guard
        let metadata_changed: HashSet<String> = current_files
            .iter()
            .filter_map(|(rel_path, meta)| {
                match previous_map.get(rel_path) {
                    Some(prev) if prev.size == meta.size && prev.mtime_ns == meta.mtime_ns => None,
                    _ => Some(rel_path.clone()), // modified or new file
                }
            })
            .collect();
        let deps = find_cascade_dependents(&metadata_changed, previous_map);
        if verbose > 0 && !deps.is_empty() {
            eprintln!(
                "memory.cascade: {} dependent files queued for rehash (from {} changed)",
                deps.len(),
                metadata_changed.len()
            );
        }
        deps
    } else {
        HashSet::new()
    };

    // Parallel: hash_file() + analyze_file() are I/O+CPU bound and fully independent per file.
    // par_iter() replaces the sequential for-loop; accumulation stays sequential after collect.
    struct FileEntry {
        artifact: FileArtifact,
        delta: Option<FileDelta>,
        reused: bool,
    }

    let raw: Vec<Result<Option<FileEntry>>> = current_files
        .par_iter()
        .map(|(rel_path, meta)| -> Result<Option<FileEntry>> {
            match previous_map.get(rel_path) {
                Some(previous) => {
                    let metadata_match =
                        previous.size == meta.size && previous.mtime_ns == meta.mtime_ns;
                    // E3.2: also force rehash if this file is a cascade dependent
                    if metadata_match && !force_rehash && !cascade_paths.contains(rel_path) {
                        return Ok(Some(FileEntry {
                            artifact: previous.clone(),
                            delta: None,
                            reused: true,
                        })); // wrapped in Some
                    }

                    let current_hash = hash_file(&meta.abs_path)
                        .with_context(|| format!("Failed to hash {}", meta.abs_path.display()))?;

                    if current_hash == previous.hash {
                        let mut kept = previous.clone();
                        kept.size = meta.size;
                        kept.mtime_ns = meta.mtime_ns;
                        return Ok(Some(FileEntry {
                            artifact: kept,
                            delta: None,
                            reused: false,
                        })); // wrapped in Some
                    }

                    let mut next = previous.clone();
                    let analysis =
                        extractor::analyze_file(&meta.abs_path, meta.size, current_hash)?;
                    next.size = meta.size;
                    next.mtime_ns = meta.mtime_ns;
                    next.hash = current_hash;
                    next.language = analysis.language;
                    next.line_count = analysis.line_count;
                    next.imports = analysis.imports;
                    next.pub_symbols = analysis.pub_symbols; // L3: refresh API surface
                    next.type_relations = analysis.type_relations; // L2: refresh type graph
                    let delta = FileDelta {
                        path: rel_path.clone(),
                        change: DeltaKind::Modified,
                        old_hash: Some(format_hash(previous.hash)),
                        new_hash: Some(format_hash(current_hash)),
                    };
                    Ok(Some(FileEntry {
                        artifact: next,
                        delta: Some(delta),
                        reused: false,
                    })) // wrapped in Some
                }
                None => {
                    // Race condition guard: temp files (e.g. atomic writes by Qdrant)
                    // may disappear between FS discovery and hashing. Skip silently.
                    let current_hash = match hash_file(&meta.abs_path) {
                        Ok(h) => h,
                        Err(e) => {
                            if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                                if io_err.kind() == std::io::ErrorKind::NotFound {
                                    return Ok(None); // transient temp file — skip
                                }
                            }
                            return Err(e).with_context(|| {
                                format!(
                                    "Failed to hash newly discovered file {}",
                                    meta.abs_path.display()
                                )
                            });
                        }
                    };
                    let analysis =
                        extractor::analyze_file(&meta.abs_path, meta.size, current_hash)?;
                    let artifact = FileArtifact {
                        rel_path: rel_path.clone(),
                        size: meta.size,
                        mtime_ns: meta.mtime_ns,
                        hash: current_hash,
                        language: analysis.language,
                        line_count: analysis.line_count,
                        imports: analysis.imports,
                        pub_symbols: analysis.pub_symbols, // L3: public API surface
                        type_relations: analysis.type_relations, // L2: type graph edges
                    };
                    let delta = FileDelta {
                        path: rel_path.clone(),
                        change: DeltaKind::Added,
                        old_hash: None,
                        new_hash: Some(format_hash(current_hash)),
                    };
                    Ok(Some(FileEntry {
                        // wrapped in Some
                        artifact,
                        delta: Some(delta),
                        reused: false,
                    }))
                }
            }
        })
        .collect();

    // Sequential accumulation of parallel results into scan_stats + file lists.
    // Note: unlike the previous sequential loop, all files are processed before the
    // first error is propagated (par_iter does not short-circuit). This is intentional:
    // errors (hash/IO failures) are rare and wasting redundant work is preferable to
    // the added complexity of early-exit parallel iteration.
    let mut files: Vec<FileArtifact> = Vec::with_capacity(current_files.len());
    let mut changes: Vec<FileDelta> = Vec::new();
    let mut scan_stats = ScanStats {
        scanned_files: current_files.len(),
        ..ScanStats::default()
    };
    let mut total_bytes: u64 = 0;

    for entry_result in raw {
        let Some(entry) = entry_result? else { continue }; // skip None (transient files)
        total_bytes = total_bytes.saturating_add(entry.artifact.size);
        if entry.reused {
            scan_stats.reused_entries += 1;
        } else {
            scan_stats.rehashed_entries += 1;
        }
        if let Some(delta) = entry.delta {
            changes.push(delta);
        }
        files.push(entry.artifact);
    }

    for (rel_path, previous) in previous_map {
        if !current_files.contains_key(rel_path) {
            changes.push(FileDelta {
                path: rel_path.clone(),
                change: DeltaKind::Removed,
                old_hash: Some(format_hash(previous.hash)),
                new_hash: None,
            });
        }
    }

    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    changes.sort_by(|a, b| a.path.cmp(&b.path));

    if verbose > 0 {
        eprintln!(
            "memory.index files={} reused={} rehashed={} delta={} (+{} ~{} -{})",
            current_files.len(),
            scan_stats.reused_entries,
            scan_stats.rehashed_entries,
            changes.len(),
            changes
                .iter()
                .filter(|c| matches!(c.change, DeltaKind::Added))
                .count(),
            changes
                .iter()
                .filter(|c| matches!(c.change, DeltaKind::Modified))
                .count(),
            changes
                .iter()
                .filter(|c| matches!(c.change, DeltaKind::Removed))
                .count(),
        );
    }

    let delta = summarize_delta(changes);
    Ok((files, delta, scan_stats, total_bytes))
}

pub(super) fn scan_project_metadata(project_root: &Path) -> Result<BTreeMap<String, FileMeta>> {
    // mpsc channel: each worker thread owns its own Sender clone — zero lock contention.
    // Factory (mkf) is called once per thread under the hood; workers send independently.
    let (tx, rx) = mpsc::channel::<(String, FileMeta)>();
    let project_root_buf = project_root.to_path_buf();

    WalkBuilder::new(project_root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .follow_links(false)
        .build_parallel()
        .run(|| {
            // Clone sender for this worker thread. Factory is called once per thread
            // (not once per entry), so each thread gets exactly one Sender clone.
            let tx = tx.clone();
            let project_root = project_root_buf.clone();
            Box::new(move |entry_result| {
                let entry = match entry_result {
                    Ok(e) => e,
                    Err(_) => return WalkState::Continue,
                };
                let file_type = match entry.file_type() {
                    Some(ft) => ft,
                    None => return WalkState::Continue,
                };
                if !file_type.is_file() {
                    return WalkState::Continue;
                }
                let abs_path = entry.path();
                let rel_path = match abs_path.strip_prefix(&project_root) {
                    Ok(rel) => rel,
                    Err(_) => return WalkState::Continue,
                };
                if should_skip_rel_path(rel_path) {
                    return WalkState::Continue;
                }
                // Use DirEntry::metadata() (lstat — consistent with follow_links(false))
                // instead of fs::metadata() (stat — follows symlinks).
                let metadata = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => return WalkState::Continue,
                };
                let rel = normalize_rel_path(rel_path);
                let mtime_ns = metadata
                    .modified()
                    .ok()
                    .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos() as u64)
                    .unwrap_or(0);
                let _ = tx.send((
                    rel,
                    FileMeta {
                        abs_path: abs_path.to_path_buf(),
                        size: metadata.len(),
                        mtime_ns,
                    },
                ));
                WalkState::Continue
            })
        });

    // All worker threads are joined synchronously by run(). Drop the original sender
    // here to close the channel so rx.into_iter() terminates instead of blocking.
    drop(tx);
    Ok(rx.into_iter().collect())
}

pub(super) fn should_skip_rel_path(path: &Path) -> bool {
    if path
        .components()
        .any(|component| EXCLUDED_DIRS.contains(&component.as_os_str().to_string_lossy().as_ref()))
    {
        return true;
    }

    let rel = normalize_rel_path(path);
    rel.ends_with(".rtk-lock")
}

/// Returns true if a FS event on `abs_path` should trigger re-indexing. // E3.1
/// Skips paths inside excluded dirs (target, node_modules, .git, etc.) and .rtk-lock files.
pub(super) fn should_watch_abs_path(project_root: &Path, abs_path: &Path) -> bool {
    abs_path
        .strip_prefix(project_root)
        .map(|rel| !should_skip_rel_path(rel))
        .unwrap_or(false) // outside project root — ignore
}

fn normalize_rel_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(super) fn summarize_delta(changes: Vec<FileDelta>) -> DeltaSummary {
    let added = changes
        .iter()
        .filter(|item| matches!(item.change, DeltaKind::Added))
        .count();
    let modified = changes
        .iter()
        .filter(|item| matches!(item.change, DeltaKind::Modified))
        .count();
    let removed = changes
        .iter()
        .filter(|item| matches!(item.change, DeltaKind::Removed))
        .count();

    DeltaSummary {
        added,
        modified,
        removed,
        changes,
    }
}

pub(super) fn summarize_graph(artifact: &ProjectArtifact) -> GraphSummary {
    let edges = artifact
        .files
        .iter()
        .map(|file| file.imports.len())
        .sum::<usize>();

    GraphSummary {
        nodes: artifact.files.len(),
        edges,
    }
}

#[cfg(test)]
mod tests {
    use super::super::cache::project_cache_key;
    use super::super::{FileArtifact, ProjectArtifact, ARTIFACT_VERSION};
    use super::*;
    use tempfile::TempDir;

    fn make_artifact_for_dir(dir: &std::path::Path) -> ProjectArtifact {
        let current_files = scan_project_metadata(dir).expect("scan");
        let project_id = project_cache_key(dir);
        let mut files: Vec<FileArtifact> = current_files
            .iter()
            .map(|(rel, meta)| FileArtifact {
                rel_path: rel.clone(),
                size: meta.size,
                mtime_ns: meta.mtime_ns,
                hash: 0,
                language: None,
                line_count: None,
                imports: vec![],
                pub_symbols: vec![],
                type_relations: vec![],
            })
            .collect();
        files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        ProjectArtifact {
            version: ARTIFACT_VERSION,
            project_id,
            project_root: dir.to_string_lossy().to_string(),
            created_at: 0,
            updated_at: 0,
            file_count: files.len(),
            total_bytes: files.iter().map(|f| f.size).sum(),
            files,
            dep_manifest: None,
        }
    }

    #[test]
    fn scan_project_metadata_parallel_returns_all_files_and_excludes_rtk_lock() {
        // TDD: parallel walk must return same set as sequential; .rtk-lock must be excluded.
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("lib.rs"), "pub fn add() {}").unwrap();
        std::fs::write(dir.path().join("config.rs"), "// config").unwrap();
        std::fs::write(dir.path().join("secret.rtk-lock"), "lock").unwrap();

        let result = scan_project_metadata(dir.path()).expect("scan must succeed");

        assert_eq!(
            result.len(),
            3,
            "expected 3 .rs files, got {:?}",
            result.keys().collect::<Vec<_>>()
        );
        assert!(result.contains_key("main.rs"), "main.rs must be present");
        assert!(result.contains_key("lib.rs"), "lib.rs must be present");
        assert!(
            result.contains_key("config.rs"),
            "config.rs must be present"
        );
        assert!(
            !result.contains_key("secret.rtk-lock"),
            ".rtk-lock must be excluded"
        );
        // rel_paths must not have leading separator
        for (rel, meta) in &result {
            assert!(
                !rel.starts_with('/') && !rel.starts_with('\\'),
                "rel_path must be relative: {rel}"
            );
            assert!(meta.size > 0, "size must be > 0 for {rel}");
        }
    }

    #[test]
    fn artifact_is_dirty_detects_mtime_change() {
        let dir = TempDir::new().unwrap();
        let src_file = dir.path().join("main.rs");
        std::fs::write(&src_file, "fn main() {}").unwrap();

        let artifact = make_artifact_for_dir(dir.path());
        // Initially clean
        assert!(!artifact_is_dirty(dir.path(), &artifact).unwrap());

        // Ensure measurable mtime delta
        std::thread::sleep(std::time::Duration::from_millis(20));
        // Modify file content → triggers mtime change
        std::fs::write(&src_file, "fn main() { println!(\"changed\"); }").unwrap();

        assert!(artifact_is_dirty(dir.path(), &artifact).unwrap());
    }

    #[test]
    fn artifact_is_dirty_clean_state() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }",
        )
        .unwrap();

        let artifact = make_artifact_for_dir(dir.path());
        assert!(
            !artifact_is_dirty(dir.path(), &artifact).unwrap(),
            "freshly scanned artifact should not be dirty"
        );
    }

    #[test]
    fn build_git_delta_fails_without_git_repo() {
        let dir = TempDir::new().unwrap();
        // tmpdir has no .git — git command must fail
        let result = build_git_delta(dir.path(), "HEAD~3", 0);
        assert!(result.is_err(), "should fail: no git repo in tmpdir");
    }

    // ── E3.2 cascade tests ───────────────────────────────────────────────────

    #[test]
    fn module_stems_for_rust_path() {
        // E3.2: stem generation must include all common Rust import variants
        let stems = module_stems_for_path("src/memory_layer/cache.rs");
        assert!(stems.contains(&"cache".to_string()));
        assert!(stems.contains(&"cache.rs".to_string()));
        assert!(stems.contains(&"src/memory_layer/cache".to_string()));
        assert!(stems.contains(&"src::memory_layer::cache".to_string()));
        assert!(stems.contains(&"crate::src::memory_layer::cache".to_string()));
    }

    #[test]
    fn find_cascade_dependents_finds_direct_importer() {
        // E3.2: file B imports module A; when A changes, B must be in cascade set
        let mut previous_map: HashMap<String, FileArtifact> = HashMap::new();

        let file_a = FileArtifact {
            rel_path: "src/cache.rs".to_string(),
            size: 100,
            mtime_ns: 1,
            hash: 1,
            language: Some("rust".to_string()),
            line_count: Some(10),
            imports: vec![],
            pub_symbols: vec![],
            type_relations: vec![],
        };
        let file_b = FileArtifact {
            rel_path: "src/main.rs".to_string(),
            size: 200,
            mtime_ns: 2,
            hash: 2,
            language: Some("rust".to_string()),
            line_count: Some(20),
            // main.rs imports cache module (as recorded by extract_imports)
            imports: vec!["crate::cache".to_string()],
            pub_symbols: vec![],
            type_relations: vec![],
        };
        previous_map.insert(file_a.rel_path.clone(), file_a);
        previous_map.insert(file_b.rel_path.clone(), file_b);

        let mut changed = HashSet::new();
        changed.insert("src/cache.rs".to_string()); // A changed

        let deps = find_cascade_dependents(&changed, &previous_map);
        assert!(
            deps.contains("src/main.rs"),
            "main.rs imports cache, should be in cascade set; got: {:?}",
            deps
        );
        assert!(
            !deps.contains("src/cache.rs"),
            "changed file itself must not be in cascade set"
        );
    }

    #[test]
    fn build_incremental_files_stats_and_deltas() {
        // TDD: parallel impl must produce identical scan_stats, total_bytes, and change list.
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() {}").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn b() {}").unwrap();
        std::fs::write(dir.path().join("c.rs"), "fn c() {}").unwrap();

        let current_files = scan_project_metadata(dir.path()).expect("scan");
        assert_eq!(current_files.len(), 3);

        // a.rs: supply correct size+mtime → metadata_match=true → reused (no hash IO)
        let a_meta = current_files.get("a.rs").unwrap();
        // b.rs: supply wrong mtime → metadata_match=false → rehashed → Modified delta
        let b_meta = current_files.get("b.rs").unwrap();

        let mut previous_map: HashMap<String, FileArtifact> = HashMap::new();
        previous_map.insert(
            "a.rs".to_string(),
            FileArtifact {
                rel_path: "a.rs".to_string(),
                size: a_meta.size,
                mtime_ns: a_meta.mtime_ns,
                hash: 9999,
                language: None,
                line_count: None,
                imports: vec![],
                pub_symbols: vec![],
                type_relations: vec![],
            },
        );
        previous_map.insert(
            "b.rs".to_string(),
            FileArtifact {
                rel_path: "b.rs".to_string(),
                size: b_meta.size,
                mtime_ns: b_meta.mtime_ns.wrapping_add(1),
                hash: 0,
                language: None,
                line_count: None,
                imports: vec![],
                pub_symbols: vec![],
                type_relations: vec![],
            },
        );
        // c.rs not in previous_map → Added delta

        let (files, delta, stats, total_bytes) =
            build_incremental_files(&current_files, &previous_map, false, false, 0).unwrap();

        assert_eq!(files.len(), 3, "all 3 files must be in output");
        assert!(
            files.windows(2).all(|w| w[0].rel_path <= w[1].rel_path),
            "files must be sorted"
        );
        assert_eq!(stats.reused_entries, 1, "a.rs metadata matched → reuse");
        assert_eq!(stats.rehashed_entries, 2, "b.rs + c.rs must be rehashed");
        assert_eq!(delta.added, 1, "c.rs is new → Added");
        assert_eq!(delta.modified, 1, "b.rs mtime changed → Modified");
        assert_eq!(delta.removed, 0, "nothing removed");
        assert!(total_bytes > 0);
    }

    #[test]
    fn find_cascade_dependents_large_set_deterministic() {
        // Regression: parallel impl must return same results as sequential for large input.
        let mut previous_map: HashMap<String, FileArtifact> = HashMap::new();
        // 50 files that import "auth" → all should be in cascade set
        for i in 0..50u64 {
            previous_map.insert(
                format!("src/user_{i}.rs"),
                FileArtifact {
                    rel_path: format!("src/user_{i}.rs"),
                    size: 100,
                    mtime_ns: i,
                    hash: i,
                    language: Some("rust".to_string()),
                    line_count: Some(10),
                    imports: vec!["crate::auth".to_string()],
                    pub_symbols: vec![],
                    type_relations: vec![],
                },
            );
        }
        // 50 files that do NOT import "auth" → must not appear
        for i in 0..50u64 {
            previous_map.insert(
                format!("src/unrelated_{i}.rs"),
                FileArtifact {
                    rel_path: format!("src/unrelated_{i}.rs"),
                    size: 50,
                    mtime_ns: i,
                    hash: 1000 + i,
                    language: Some("rust".to_string()),
                    line_count: Some(5),
                    imports: vec!["std::fmt".to_string()],
                    pub_symbols: vec![],
                    type_relations: vec![],
                },
            );
        }
        let mut changed = HashSet::new();
        changed.insert("src/auth.rs".to_string());

        let deps = find_cascade_dependents(&changed, &previous_map);

        assert_eq!(deps.len(), 50, "expected 50 dependents, got {}", deps.len());
        for i in 0..50u64 {
            assert!(
                deps.contains(&format!("src/user_{i}.rs")),
                "missing user_{i}.rs"
            );
            assert!(
                !deps.contains(&format!("src/unrelated_{i}.rs")),
                "false positive unrelated_{i}.rs"
            );
        }
    }

    #[test]
    fn find_cascade_dependents_empty_on_no_match() {
        // E3.2: no file imports the changed file → cascade set is empty
        let mut previous_map: HashMap<String, FileArtifact> = HashMap::new();
        previous_map.insert(
            "src/util.rs".to_string(),
            FileArtifact {
                rel_path: "src/util.rs".to_string(),
                size: 50,
                mtime_ns: 1,
                hash: 1,
                language: Some("rust".to_string()),
                line_count: Some(5),
                imports: vec!["std::collections::HashMap".to_string()],
                pub_symbols: vec![],
                type_relations: vec![],
            },
        );

        let mut changed = HashSet::new();
        changed.insert("src/unrelated.rs".to_string());

        let deps = find_cascade_dependents(&changed, &previous_map);
        assert!(
            deps.is_empty(),
            "no file imports unrelated.rs; expected empty set"
        );
    }
}
