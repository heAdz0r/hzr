//! M1: Helper functions and constants extracted from mod.rs.

use super::*;

/// Return the runtime memory-layer config, falling back to defaults.
pub(super) fn mem_config() -> crate::config::MemConfig {
    crate::config::Config::load().unwrap_or_default().mem
}

pub(super) const EXCLUDED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".cache",
    ".turbo",
    ".venv",
    "venv",
    "__pycache__",
    "coverage",
];

pub(super) const ENTRY_POINT_HINTS: &[&str] = &[
    "README.md",
    "Cargo.toml",
    "src/main.rs",
    "src/lib.rs",
    "package.json",
    "tsconfig.json",
    "pyproject.toml",
    "go.mod",
    "main.py",
    "src/index.ts",
    "src/main.ts",
    "src/index.js",
    "src/main.js",
];

/// E6.3: Compute token savings by comparing raw file bytes to rendered context size.
/// Pure function — no I/O, fully testable.
pub(super) fn compute_gain_stats(artifact: &ProjectArtifact, detail: DetailLevel) -> GainStats {
    let raw_bytes = artifact.total_bytes;

    if artifact.files.is_empty() {
        return GainStats {
            raw_bytes: 0,
            context_bytes: 0,
            savings_pct: 0.0,
            files_indexed: 0,
        };
    }

    let empty_delta = DeltaSummary {
        added: 0,
        modified: 0,
        removed: 0,
        changes: vec![],
    };
    let limits = limits_for_detail(detail);
    let gain_layers = apply_feature_flags(
        layers_for(QueryType::General),
        &crate::config::MemFeatureFlags::default(),
    );
    let context = build_context_slice(artifact, &empty_delta, limits, gain_layers);

    let graph = summarize_graph(artifact);
    let response = MemoryResponse {
        command: "gain".to_string(),
        project_root: artifact.project_root.clone(),
        project_id: artifact.project_id.clone(),
        artifact_version: artifact.version,
        detail,
        cache_status: CacheStatus::Hit,
        cache_hit: true,
        freshness: "fresh",
        stats: ProjectStats {
            file_count: artifact.file_count,
            total_bytes: artifact.total_bytes,
            reused_entries: artifact.file_count,
            rehashed_entries: 0,
            scanned_files: artifact.file_count,
        },
        delta: Some(DeltaPayload {
            added: 0,
            modified: 0,
            removed: 0,
            files: vec![],
        }),
        context,
        graph,
    };
    let rendered = render_text(&response);
    let context_bytes = rendered.len() as u64;

    let savings_pct = if raw_bytes > 0 {
        (1.0 - (context_bytes as f64 / raw_bytes as f64)) * 100.0
    } else {
        0.0
    };

    GainStats {
        raw_bytes,
        context_bytes,
        savings_pct: savings_pct.max(0.0),
        files_indexed: artifact.files.len(),
    }
}

/// C2: Delegates to unified noise_filter module (legacy pipeline, tier=None).
pub(super) fn is_low_signal_candidate(fa: &FileArtifact, query_tags: &[String]) -> bool {
    noise_filter::is_noise_candidate(
        &fa.rel_path,
        fa.language.as_deref(),
        fa.line_count,
        !fa.pub_symbols.is_empty(),
        !fa.imports.is_empty(),
        query_tags,
        None,
    )
}

/// C2: Delegates to unified noise_filter::is_source_like_language.
pub(super) fn is_source_like_language(language: Option<&str>) -> bool {
    noise_filter::is_source_like_language(language)
}

/// C2: Delegates to unified noise_filter::path_query_overlap_hits.
pub(super) fn path_query_overlap_hits(rel_path: &str, query_tags: &[String]) -> usize {
    noise_filter::path_query_overlap_hits(rel_path, query_tags)
}

pub(super) fn structural_relevance_for_plan(
    language: Option<&str>,
    has_pub_symbols: bool,
    has_imports: bool,
) -> f32 {
    if has_pub_symbols {
        return 0.80;
    }

    if is_source_like_language(language) {
        if has_imports {
            0.65
        } else {
            0.42
        }
    } else if matches!(language, Some("toml" | "yaml" | "json")) {
        0.24
    } else {
        0.08
    }
}

pub(super) fn path_query_overlap_bonus(rel_path: &str, query_tags: &[String]) -> f32 {
    let hits = path_query_overlap_hits(rel_path, query_tags);
    (hits as f32 * 0.18).min(0.54)
}

pub(super) fn should_use_recency_signal(
    rel_path: &str,
    language: Option<&str>,
    query_tags: &[String],
) -> bool {
    if is_source_like_language(language) {
        return true;
    }

    let lower = rel_path.replace('\\', "/").to_ascii_lowercase();
    let is_doc_or_config = lower.ends_with(".md")
        || lower.ends_with(".toml")
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
        || lower.ends_with(".json");

    is_doc_or_config && path_query_overlap_hits(rel_path, query_tags) > 0
}

/// E3.2: Extract import edges from artifact and store in artifact_edges table.
pub(super) fn store_import_edges(artifact: &ProjectArtifact) {
    let mut edges: Vec<(String, String)> = Vec::new();
    for file in &artifact.files {
        for import in &file.imports {
            if import.starts_with("self:") {
                continue;
            }
            edges.push((file.rel_path.clone(), import.clone()));
        }
    }
    let _ = store_artifact_edges(&artifact.project_id, &edges);
}

pub(super) fn freshness_label(freshness: ArtifactFreshness) -> &'static str {
    match freshness {
        ArtifactFreshness::Fresh => "fresh",
        ArtifactFreshness::Stale => "stale",
        ArtifactFreshness::Dirty => "dirty",
    }
}

/// E1.4: Derive cache event label from build state for cache_stats recording.
pub(super) fn cache_status_event_label(state: &BuildState, refresh: bool) -> &'static str {
    if refresh {
        "refreshed"
    } else if state.stale_previous {
        "stale_rebuild"
    } else if state.cache_hit {
        "hit"
    } else if state.previous_exists && !state.delta.changes.is_empty() {
        "dirty_rebuild"
    } else {
        "miss"
    }
}
