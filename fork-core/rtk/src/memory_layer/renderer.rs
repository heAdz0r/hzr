// E0.1: Response building and text rendering extracted from mod.rs
use anyhow::{bail, Context, Result};
use std::collections::HashMap;

use super::{
    BuildState, CacheStatus, ContextSlice, DeltaKind, DeltaPayload, DeltaSummary, DetailLevel,
    DetailLimits, FileArtifact, FileSurface, ImportStat, LayerFlags, MemoryResponse,
    ModuleIndexEntry, PathStat, ProjectArtifact, ProjectStats, QueryType, TestMapEntry,
    TypeRelation, ENTRY_POINT_HINTS,
};

/// E6.4: Mask LayerFlags with feature flags — flags can only be disabled, never enabled.
pub(super) fn apply_feature_flags(
    mut flags: LayerFlags,
    feat: &crate::config::MemFeatureFlags,
) -> LayerFlags {
    if !feat.type_graph {
        flags.l2_type_graph = false; // E6.4: disable L2 type_graph
    }
    if !feat.test_map {
        flags.l5_test_map = false; // E6.4: disable L5 test_map
    }
    if !feat.dep_manifest {
        flags.l4_dep_manifest = false; // E6.4: disable L4 dep_manifest
    }
    flags
}

pub(super) fn build_response(
    command: &str,
    state: &BuildState,
    detail: DetailLevel,
    refresh: bool,
    delta: &DeltaSummary,
    query_type: QueryType,                     // E2.3
    features: &crate::config::MemFeatureFlags, // E6.4
) -> MemoryResponse {
    let limits = limits_for_detail(detail);
    let layers = apply_feature_flags(layers_for(query_type), features); // E6.4: apply feature mask

    let cache_status = if refresh {
        CacheStatus::Refreshed
    } else if state.stale_previous {
        CacheStatus::StaleRebuild
    } else if state.cache_hit {
        CacheStatus::Hit
    } else if state.previous_exists && !delta.changes.is_empty() {
        // Previous artifact exists, not stale, but files changed since last index (watcher missed event)
        CacheStatus::DirtyRebuild
    } else {
        CacheStatus::Miss
    };

    let stats = ProjectStats {
        file_count: state.artifact.file_count,
        total_bytes: state.artifact.total_bytes,
        reused_entries: state.scan_stats.reused_entries,
        rehashed_entries: state.scan_stats.rehashed_entries,
        scanned_files: state.scan_stats.scanned_files,
    };

    let mut delta_files = if layers.l6_change_digest {
        delta.changes.clone()
    } else {
        Vec::new()
    };
    if delta_files.len() > limits.max_changes {
        delta_files.truncate(limits.max_changes);
    }

    let delta_payload = if layers.l6_change_digest {
        Some(DeltaPayload {
            added: delta.added,
            modified: delta.modified,
            removed: delta.removed,
            files: delta_files,
        })
    } else {
        None
    };

    let empty_delta = DeltaSummary {
        added: 0,
        modified: 0,
        removed: 0,
        changes: Vec::new(),
    };
    let context_delta = if layers.l6_change_digest {
        delta
    } else {
        &empty_delta
    };

    let context = build_context_slice(&state.artifact, context_delta, limits, layers); // E6.4: pass pre-computed layers (feature-masked)

    // P0 dirty-blocking: derive freshness from cache_status (PRD §8)
    // build_state always rebuilds from current FS, so DirtyRebuild/StaleRebuild data is fresh
    // after rebuild. Label "rebuilt" to distinguish from cache Hit ("fresh").
    let freshness = match cache_status {
        CacheStatus::Hit => "fresh",
        CacheStatus::Refreshed => "fresh", // explicit refresh => fresh
        CacheStatus::StaleRebuild => "rebuilt", // was stale, now rebuilt from FS
        CacheStatus::DirtyRebuild => "rebuilt", // was dirty, now rebuilt from FS
        CacheStatus::Miss => "fresh",      // first index => fresh
    };

    MemoryResponse {
        command: command.to_string(),
        project_root: state.project_root.to_string_lossy().to_string(),
        project_id: state.project_id.clone(),
        artifact_version: state.artifact.version,
        detail,
        cache_status,
        cache_hit: state.cache_hit,
        freshness, // P0: explicit freshness state for consumers (PRD §8)
        stats,
        delta: delta_payload,
        context,
        graph: state.graph.clone(),
    }
}

pub(super) fn build_context_slice(
    artifact: &ProjectArtifact,
    delta: &DeltaSummary,
    limits: DetailLimits,
    layers: LayerFlags, // E6.4: pre-computed (feature-masked) layer flags
) -> ContextSlice {
    // E6.4: layers already computed and feature-masked by caller (build_response or compute_gain_stats)

    // L0: project map (entry_points + hot_paths)
    let entry_points = if layers.l0_project_map {
        select_entry_points(&artifact.files, limits.max_entry_points)
    } else {
        Vec::new()
    };
    let hot_paths = if layers.l0_project_map {
        select_hot_paths(artifact, delta, limits.max_hot_paths)
    } else {
        Vec::new()
    };

    // top_imports
    let top_imports = if layers.top_imports {
        select_top_imports(&artifact.files, limits.max_imports)
    } else {
        Vec::new()
    };

    // L3: api_surface — show recently changed files or entry points
    // On initial indexing delta contains ALL files — fall back to entry_points.
    let api_surface = if layers.l3_api_surface {
        let use_delta =
            !delta.changes.is_empty() && delta.changes.len() <= limits.max_api_files * 4;
        let surface_paths: Vec<&str> = if use_delta {
            delta
                .changes
                .iter()
                .filter(|c| !matches!(c.change, DeltaKind::Removed))
                .map(|c| c.path.as_str())
                .collect()
        } else {
            entry_points.iter().map(|s| s.as_str()).collect()
        };
        build_api_surface(
            artifact,
            &surface_paths,
            limits.max_api_files,
            limits.max_api_symbols,
        )
    } else {
        Vec::new()
    };

    // L1: module_index — compact list of module exports
    let module_index = if layers.l1_module_index {
        build_module_index(artifact, limits.max_modules, limits.max_module_exports)
    } else {
        Vec::new()
    };

    // L2: type_graph — type relationships (implements/extends/contains/alias)
    let type_graph = if layers.l2_type_graph {
        build_type_graph(artifact, limits.max_modules) // T3.1: respect DetailLevel limits
    } else {
        Vec::new()
    };

    // L4: dep_manifest — dependency manifest from cached artifact
    let dep_manifest = if layers.l4_dep_manifest {
        artifact.dep_manifest.clone()
    } else {
        None
    };

    // L5: test_map — list of test files with kind classification
    let test_map = if layers.l5_test_map {
        build_test_map(artifact, limits.max_modules) // T3.1: respect DetailLevel limits
    } else {
        Vec::new()
    };

    ContextSlice {
        entry_points,
        hot_paths,
        top_imports,
        api_surface,
        module_index,
        type_graph, // L2
        dep_manifest,
        test_map, // L5
    }
}

pub(super) fn select_entry_points(files: &[FileArtifact], max: usize) -> Vec<String> {
    let mut picked: Vec<String> = ENTRY_POINT_HINTS
        .iter()
        .filter_map(|hint| {
            files
                .iter()
                .find(|file| file.rel_path == *hint || file.rel_path.ends_with(hint))
                .map(|file| file.rel_path.clone())
        })
        .collect();

    if picked.len() < max {
        for file in files {
            if picked.len() >= max {
                break;
            }
            if is_hidden_rel_path(&file.rel_path) {
                continue;
            }
            let stem = std::path::Path::new(&file.rel_path) // T3.2: filename stem match
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if (stem == "main" || stem == "index") && !picked.contains(&file.rel_path) {
                picked.push(file.rel_path.clone());
            }
        }
    }

    if picked.len() < max {
        for file in files {
            if picked.len() >= max {
                break;
            }
            if is_hidden_rel_path(&file.rel_path) {
                continue;
            }
            if !picked.contains(&file.rel_path) {
                picked.push(file.rel_path.clone());
            }
        }
    }

    picked.truncate(max);
    picked
}

fn select_hot_paths(artifact: &ProjectArtifact, delta: &DeltaSummary, max: usize) -> Vec<PathStat> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    if !delta.changes.is_empty() {
        for change in &delta.changes {
            let top = top_level_path(&change.path);
            *counts.entry(top).or_insert(0) += 1;
        }
    } else {
        for file in &artifact.files {
            let top = top_level_path(&file.rel_path);
            *counts.entry(top).or_insert(0) += 1;
        }
    }

    let mut ranked: Vec<PathStat> = counts
        .into_iter()
        .map(|(path, count)| PathStat { path, count })
        .collect();
    ranked.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.path.cmp(&b.path)));
    ranked.truncate(max);
    ranked
}

fn select_top_imports(files: &[FileArtifact], max: usize) -> Vec<ImportStat> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    for file in files {
        for import in &file.imports {
            if import.starts_with("self:")    // T3.3: filter Rust internals
                || import.starts_with("super::")
                || import.starts_with("crate::")
                || import.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
            {
                continue;
            }
            *counts.entry(import.clone()).or_insert(0) += 1;
        }
    }

    let mut ranked: Vec<ImportStat> = counts
        .into_iter()
        .map(|(module, count)| ImportStat { module, count })
        .collect();
    ranked.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.module.cmp(&b.module)));
    ranked.truncate(max);
    ranked
}

pub(super) fn top_level_path(rel_path: &str) -> String {
    rel_path
        .split('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(".")
        .to_string()
}

fn is_hidden_rel_path(rel_path: &str) -> bool {
    rel_path.split('/').any(|segment| segment.starts_with('.'))
}

/// Build the L3 api_surface slice: public symbols for given paths.
/// Falls back to top-N files by symbol count when preferred paths yield nothing.
fn build_api_surface(
    artifact: &ProjectArtifact,
    paths: &[&str],
    max_files: usize,
    max_symbols: usize,
) -> Vec<FileSurface> {
    let to_surface = |fa: &FileArtifact| -> FileSurface {
        FileSurface {
            path: fa.rel_path.clone(),
            lang: fa.language.clone().unwrap_or_else(|| "?".to_string()),
            symbols: fa.pub_symbols.iter().take(max_symbols).cloned().collect(),
        }
    };

    // Try the requested paths first
    let file_map: HashMap<&str, &FileArtifact> = artifact
        .files
        .iter()
        .map(|f| (f.rel_path.as_str(), f))
        .collect();

    let from_paths: Vec<FileSurface> = paths
        .iter()
        .filter_map(|p| file_map.get(p).copied())
        .filter(|fa| !fa.pub_symbols.is_empty())
        .take(max_files)
        .map(to_surface)
        .collect();

    if !from_paths.is_empty() {
        return from_paths;
    }

    // Fallback: top-N files with most public symbols, primary language first.
    // Detect the project's primary language by file count to avoid showing
    // benchmark/test helper files from secondary languages (e.g. Python benchmarks
    // in a Rust project) ahead of the core source.
    let primary_lang: Option<&str> = {
        let mut lang_counts: HashMap<&str, usize> = HashMap::new();
        for fa in &artifact.files {
            if let Some(lang) = fa.language.as_deref() {
                if !fa.pub_symbols.is_empty() {
                    *lang_counts.entry(lang).or_insert(0) += 1;
                }
            }
        }
        lang_counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(lang, _)| lang)
    };

    let mut ranked: Vec<&FileArtifact> = artifact
        .files
        .iter()
        .filter(|fa| !fa.pub_symbols.is_empty())
        .collect();

    // Sort: primary-language files first, then by symbol count within each group. // This prevents secondary-language helpers (benchmarks, scripts) from
    // outranking core source files in mixed-language projects.
    ranked.sort_by(|a, b| {
        let a_primary = a.language.as_deref() == primary_lang;
        let b_primary = b.language.as_deref() == primary_lang;
        b_primary
            .cmp(&a_primary)
            .then_with(|| b.pub_symbols.len().cmp(&a.pub_symbols.len()))
    });

    ranked.into_iter().take(max_files).map(to_surface).collect()
}

/// L1: Build module_index — compact per-module export list derived from pub_symbols.
pub(super) fn build_module_index(
    artifact: &ProjectArtifact,
    max_modules: usize,
    max_exports: usize,
) -> Vec<ModuleIndexEntry> {
    let mut entries: Vec<ModuleIndexEntry> = artifact
        .files
        .iter()
        .filter(|f| !f.pub_symbols.is_empty())
        .map(|f| ModuleIndexEntry {
            module: f.rel_path.clone(),
            lang: f.language.clone().unwrap_or_else(|| "?".to_string()),
            exports: f
                .pub_symbols
                .iter()
                .take(max_exports)
                .map(|s| s.name.clone())
                .collect(),
        })
        .collect();
    entries.sort_by(|a, b| a.module.cmp(&b.module));
    entries.truncate(max_modules);
    entries
}

/// E2.3: Determine which artifact layers to include based on query_type.
pub(super) fn layers_for(qt: QueryType) -> LayerFlags {
    match qt {
        QueryType::General => LayerFlags {
            l0_project_map: true,
            l1_module_index: true,
            l2_type_graph: true, // L2: general — include type relationships
            l3_api_surface: true,
            l4_dep_manifest: true,
            l5_test_map: true,
            l6_change_digest: true,
            top_imports: true,
        },
        QueryType::Bugfix => LayerFlags {
            l0_project_map: false,
            l1_module_index: true,
            l2_type_graph: false, // L2: bugfix — omit (focus on API + delta)
            l3_api_surface: true,
            l4_dep_manifest: false,
            l5_test_map: true,
            l6_change_digest: true,
            top_imports: false,
        },
        QueryType::Feature => LayerFlags {
            l0_project_map: true,
            l1_module_index: true,
            l2_type_graph: true, // L2: feature — PRD §7.2 includes type_graph
            l3_api_surface: true,
            l4_dep_manifest: true,
            l5_test_map: true,
            l6_change_digest: false,
            top_imports: true,
        },
        QueryType::Refactor => LayerFlags {
            l0_project_map: false,
            l1_module_index: true,
            l2_type_graph: true, // L2: refactor — PRD §7.2 includes type_graph
            l3_api_surface: true,
            l4_dep_manifest: false,
            l5_test_map: true,
            l6_change_digest: false,
            top_imports: false,
        },
        QueryType::Incident => LayerFlags {
            l0_project_map: false,
            l1_module_index: false,
            l2_type_graph: false, // L2: incident — omit (focus on API + deps + delta)
            l3_api_surface: true,
            l4_dep_manifest: true,
            l5_test_map: false,
            l6_change_digest: true,
            top_imports: false,
        },
    }
}

pub(super) fn limits_for_detail(detail: DetailLevel) -> DetailLimits {
    match detail {
        DetailLevel::Compact => DetailLimits {
            max_changes: 8,
            max_entry_points: 5,
            max_hot_paths: 5,
            max_imports: 5,
            max_api_files: 5,      // L3: compact — top 5 files
            max_api_symbols: 8,    // L3: compact — 8 pub symbols each
            max_modules: 10,       // L1: compact — top 10 modules
            max_module_exports: 8, // L1: compact — 8 exports each
        },
        DetailLevel::Normal => DetailLimits {
            max_changes: 32,
            max_entry_points: 10,
            max_hot_paths: 10,
            max_imports: 12,
            max_api_files: 10,
            max_api_symbols: 20,
            max_modules: 24,
            max_module_exports: 16,
        },
        DetailLevel::Verbose => DetailLimits {
            max_changes: 256,
            max_entry_points: 32,
            max_hot_paths: 32,
            max_imports: 32,
            max_api_files: 32,
            max_api_symbols: 64,
            max_modules: 128,
            max_module_exports: 64,
        },
    }
}

pub(super) fn print_response(response: &MemoryResponse, format: &str) -> Result<()> {
    match format {
        "text" => {
            println!("{}", render_text(response));
            Ok(())
        }
        "json" => {
            println!(
                "{}",
                serde_json::to_string_pretty(response)
                    .context("Failed to serialize memory response as JSON")?
            );
            Ok(())
        }
        _ => bail!("Unsupported format '{}'. Use 'text' or 'json'.", format),
    }
}

pub(super) fn render_text(response: &MemoryResponse) -> String {
    let mut out = String::new();
    out.push_str("# format: entry_points=read-first hot_paths(N)=git-freq api_surface=pub-symbols[lang] delta=+add~mod-rm\n"); // T3.5: format legend
                                                                                                                               // P0: include freshness in header line (PRD §8 — consumers can verify data validity)
    out.push_str(&format!(
        "memory.{} project={} id={} cache={} freshness={}\n",
        response.command,
        response.project_root,
        response.project_id,
        cache_status_label(response.cache_status),
        response.freshness
    ));
    out.push_str(&format!(
        "stats files={} bytes={}\n", // T3.4: remove cache telemetry from LLM context
        response.stats.file_count,
        format_bytes(response.stats.total_bytes),
    ));
    if let Some(delta) = &response.delta {
        out.push_str(&format!(
            "delta +{} ~{} -{}\n",
            delta.added, delta.modified, delta.removed
        ));

        if !delta.files.is_empty() {
            out.push_str("changes\n");
            for change in &delta.files {
                out.push_str(&format!(
                    "{} {}\n",
                    delta_marker(change.change),
                    change.path
                ));
            }
        }
    }

    if !response.context.entry_points.is_empty() {
        out.push_str("entry_points ");
        out.push_str(&response.context.entry_points.join(", "));
        out.push('\n');
    }

    if !response.context.hot_paths.is_empty() {
        let parts: Vec<String> = response
            .context
            .hot_paths
            .iter()
            .map(|item| format!("{}({})", item.path, item.count))
            .collect();
        out.push_str("hot_paths ");
        out.push_str(&parts.join(", "));
        out.push('\n');
    }

    if !response.context.top_imports.is_empty() {
        let parts: Vec<String> = response
            .context
            .top_imports
            .iter()
            .map(|item| format!("{}({})", item.module, item.count))
            .collect();
        out.push_str("top_imports ");
        out.push_str(&parts.join(", "));
        out.push('\n');
    }

    if !response.context.api_surface.is_empty() {
        out.push_str("api_surface\n");
        for surface in &response.context.api_surface {
            let sym_str: Vec<String> = surface
                .symbols
                .iter()
                .map(|s| {
                    if let Some(sig) = &s.sig {
                        format!("{}{}({})", s.name, sig, s.kind)
                    } else {
                        s.name.to_string()
                    }
                })
                .collect();
            out.push_str(&format!(
                "  {}[{}]: {}\n",
                surface.path,
                surface.lang,
                sym_str.join(", ")
            ));
        }
    }

    // L1: module_index
    if !response.context.module_index.is_empty() {
        out.push_str("module_index\n");
        for entry in &response.context.module_index {
            out.push_str(&format!(
                "  {}[{}]: {}\n",
                entry.module,
                entry.lang,
                entry.exports.join(", ")
            ));
        }
    }

    // L2: type_graph
    if !response.context.type_graph.is_empty() {
        out.push_str("type_graph\n");
        for rel in &response.context.type_graph {
            out.push_str(&format!(
                "  {} --{}--> {}\n",
                rel.source, rel.relation, rel.target
            ));
        }
    }

    // L5: test_map
    if !response.context.test_map.is_empty() {
        out.push_str("test_map\n");
        for entry in &response.context.test_map {
            out.push_str(&format!("  {}[{}]\n", entry.path, entry.kind));
        }
    }

    // L4: dep_manifest
    if let Some(deps) = &response.context.dep_manifest {
        if !deps.runtime.is_empty() {
            let names: Vec<&str> = deps.runtime.iter().map(|d| d.name.as_str()).collect();
            out.push_str(&format!("deps_runtime {}\n", names.join(", ")));
        }
        if !deps.dev.is_empty() {
            let names: Vec<&str> = deps.dev.iter().map(|d| d.name.as_str()).collect();
            out.push_str(&format!("deps_dev {}\n", names.join(", ")));
        }
        if !deps.build.is_empty() {
            let names: Vec<&str> = deps.build.iter().map(|d| d.name.as_str()).collect();
            out.push_str(&format!("deps_build {}\n", names.join(", ")));
        }
    }

    out
}

fn delta_marker(kind: DeltaKind) -> &'static str {
    match kind {
        DeltaKind::Added => "+",
        DeltaKind::Modified => "~",
        DeltaKind::Removed => "-",
    }
}

fn cache_status_label(status: CacheStatus) -> &'static str {
    match status {
        CacheStatus::Hit => "hit",
        CacheStatus::Miss => "miss",
        CacheStatus::Refreshed => "refreshed",
        CacheStatus::StaleRebuild => "stale_rebuild",
        CacheStatus::DirtyRebuild => "dirty_rebuild", // L5: files changed within TTL
    }
}

// ── L2: type_graph helpers ───────────────────────────────────────────────────

/// Build the L2 type_graph: collect all type relations from cached file artifacts, capped at `max`.
fn build_type_graph(artifact: &ProjectArtifact, max: usize) -> Vec<TypeRelation> {
    let mut all: Vec<TypeRelation> = artifact
        .files
        .iter()
        .flat_map(|f| f.type_relations.iter().cloned())
        .collect();
    // Deduplicate by (source, target, relation)
    all.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then_with(|| a.target.cmp(&b.target))
            .then_with(|| a.relation.cmp(&b.relation))
    });
    all.dedup_by(|a, b| a.source == b.source && a.target == b.target && a.relation == b.relation);
    all.truncate(max);
    all
}

// ── L5: test_map helpers ──────────────────────────────────────────────────────

/// Build an ordered list of test files found in the artifact, capped at `max`.
fn build_test_map(artifact: &ProjectArtifact, max: usize) -> Vec<TestMapEntry> {
    artifact
        .files
        .iter()
        .filter(|f| is_test_file(&f.rel_path))
        .take(max)
        .map(|f| TestMapEntry {
            path: f.rel_path.clone(),
            kind: test_file_kind(&f.rel_path),
        })
        .collect()
}

fn is_test_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    let segments: Vec<&str> = lower.split('/').collect();

    // Test directory segments (check all except last component)
    if segments.len() > 1 {
        for seg in &segments[..segments.len() - 1] {
            if matches!(*seg, "tests" | "__tests__" | "test" | "spec") {
                return true;
            }
        }
    }

    // Filename suffixes
    let filename = segments.last().copied().unwrap_or("");
    filename.ends_with("_test.rs")
        || filename.ends_with("_test.py")
        || filename.ends_with(".test.ts")
        || filename.ends_with(".spec.ts")
        || filename.ends_with(".test.tsx")
        || filename.ends_with(".spec.tsx")
        || filename.ends_with(".test.js")
        || filename.ends_with(".spec.js")
}

fn test_file_kind(path: &str) -> String {
    let lower = path.to_lowercase();
    if lower.contains("e2e") || lower.contains("playwright") || lower.contains("cypress") {
        "e2e".to_string()
    } else if lower.contains("integration") {
        "integration".to_string()
    } else {
        "unit".to_string()
    }
}

pub(super) fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1}GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1}MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{}B", bytes)
    }
}

/// E6.4: Unit tests for apply_feature_flags — verify that disabling a feature zeroes out
/// the corresponding LayerFlag regardless of what query_type sets.
#[cfg(test)]
mod feature_flag_tests {
    use super::*;
    use crate::config::MemFeatureFlags;

    fn all_on() -> LayerFlags {
        layers_for(super::super::QueryType::General) // General enables all flags
    }

    #[test]
    fn default_feature_flags_leave_layers_unchanged() {
        // E6.4: default flags must be a no-op mask
        let base = all_on();
        let feat = MemFeatureFlags::default();
        let masked = apply_feature_flags(base, &feat);
        assert!(masked.l2_type_graph, "type_graph default=true must keep L2");
        assert!(masked.l5_test_map, "test_map default=true must keep L5");
        assert!(
            masked.l4_dep_manifest,
            "dep_manifest default=true must keep L4"
        );
    }

    #[test]
    fn type_graph_false_disables_l2() {
        // E6.4: type_graph=false must zero L2 regardless of query_type
        let base = all_on();
        let feat = MemFeatureFlags {
            type_graph: false,
            ..MemFeatureFlags::default()
        };
        let masked = apply_feature_flags(base, &feat);
        assert!(
            !masked.l2_type_graph,
            "L2 must be disabled when type_graph=false"
        );
        assert!(masked.l5_test_map, "L5 must be unaffected");
        assert!(masked.l4_dep_manifest, "L4 must be unaffected");
    }

    #[test]
    fn test_map_false_disables_l5() {
        // E6.4: test_map=false must zero L5 regardless of query_type
        let base = all_on();
        let feat = MemFeatureFlags {
            test_map: false,
            ..MemFeatureFlags::default()
        };
        let masked = apply_feature_flags(base, &feat);
        assert!(
            !masked.l5_test_map,
            "L5 must be disabled when test_map=false"
        );
        assert!(masked.l2_type_graph, "L2 must be unaffected");
    }

    #[test]
    fn dep_manifest_false_disables_l4() {
        // E6.4: dep_manifest=false must zero L4 regardless of query_type
        let base = all_on();
        let feat = MemFeatureFlags {
            dep_manifest: false,
            ..MemFeatureFlags::default()
        };
        let masked = apply_feature_flags(base, &feat);
        assert!(
            !masked.l4_dep_manifest,
            "L4 must be disabled when dep_manifest=false"
        );
    }

    #[test]
    fn all_flags_off_zeros_all_three_optional_layers() {
        // E6.4: disabling all three optional layers simultaneously
        let base = all_on();
        let feat = MemFeatureFlags {
            type_graph: false,
            test_map: false,
            dep_manifest: false,
            ..MemFeatureFlags::default()
        };
        let masked = apply_feature_flags(base, &feat);
        assert!(!masked.l2_type_graph);
        assert!(!masked.l5_test_map);
        assert!(!masked.l4_dep_manifest);
        // L0, L1, L3, L6 must remain as set by query_type
        assert!(masked.l0_project_map);
        assert!(masked.l1_module_index);
        assert!(masked.l3_api_surface);
        assert!(masked.l6_change_digest);
    }

    #[test]
    fn feature_flags_only_mask_cannot_enable_what_query_type_disabled() {
        // E6.4: feature flags are AND-only — they cannot re-enable a layer that query_type disabled
        let bugfix = layers_for(super::super::QueryType::Bugfix); // L2 off for Bugfix
        assert!(!bugfix.l2_type_graph, "pre-condition: Bugfix disables L2");
        let feat = MemFeatureFlags::default(); // type_graph=true
        let masked = apply_feature_flags(bugfix, &feat);
        // L2 stays off because query_type already disabled it (apply_feature_flags is AND-only)
        assert!(
            !masked.l2_type_graph,
            "feature flags must not re-enable what query_type disabled"
        );
    }
}
