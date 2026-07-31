//! PRD R1: Graph-first Tier A/B/C candidate builder.
//!
//! Pipeline:
//!   Tier A — direct task/path/symbol match seeds
//!   Tier B — 1-hop import/call-graph neighbors of Tier A
//!   Tier C — limited fallback pool for recall
//!
//! Hard cap: cfg.plan_candidate_cap (default 60).
//! Noise filter applied before returning candidates.

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

use super::budget;
use super::call_graph::CallGraph;
use super::git_churn::{self, ChurnCache};
use super::indexer;
use super::intent;
use super::ranker::{self, Candidate};
use super::semantic_stage;
use crate::config::MemConfig;

// ── Noise filter ───────────────────────────────────────────────────────────────

/// C2: Delegates to unified noise_filter module (graph-first pipeline).
fn is_noise(
    rel_path: &str,
    language: Option<&str>,
    line_count: Option<u32>,
    has_symbols: bool,
    has_imports: bool,
    query_tags: &[String],
    tier: u8,
) -> bool {
    super::noise_filter::is_noise_candidate(
        rel_path,
        language,
        line_count,
        has_symbols,
        has_imports,
        query_tags,
        Some(tier), // graph-first pipeline — tier-aware filtering
    )
}

/// C2: Delegates to unified noise_filter::is_source_like_language.
fn is_source_lang(language: Option<&str>) -> bool {
    super::noise_filter::is_source_like_language(language)
}

/// C2: Delegates to unified noise_filter::path_query_overlap_hits (HashSet tokenization).
fn path_overlap(rel_path: &str, query_tags: &[String]) -> usize {
    super::noise_filter::path_query_overlap_hits(rel_path, query_tags)
}

// ── Tier building ──────────────────────────────────────────────────────────────

/// Score a file for Tier A (direct seed) membership.
/// Returns >0.0 if file is a direct seed for the task.
fn tier_a_score(
    rel_path: &str,
    language: Option<&str>,
    has_symbols: bool,
    _has_imports: bool, // reserved for future import-based seed scoring
    query_tags: &[String],
) -> f32 {
    let path = rel_path.replace('\\', "/").to_ascii_lowercase();
    let overlap = path_overlap(rel_path, query_tags);
    let mut score: f32 = 0.0;

    // Direct path/name token match with query
    if overlap > 0 {
        score += 0.5 + (overlap as f32 * 0.15).min(0.35);
    }

    // Source files with symbols are higher priority seeds
    if is_source_lang(language) && has_symbols {
        score += 0.1;
    }

    // Penalize docs/config unless they match the query
    let is_doc = path.ends_with(".md") || path.ends_with(".txt");
    let is_config = path.ends_with(".toml") || path.ends_with(".json") || path.ends_with(".yaml");
    if (is_doc || is_config) && overlap == 0 {
        score = 0.0;
    }

    score
}

// ── Main entry ─────────────────────────────────────────────────────────────────

/// PRD R1+R2+R3: full graph-first pipeline.
/// Returns AssemblyResult (same contract as plan_context_legacy).
pub fn run_graph_first_pipeline(
    project: &Path,
    task: &str,
    token_budget: u32,
    cfg: &MemConfig,
) -> Result<budget::AssemblyResult> {
    use std::collections::HashSet;

    let token_budget = if token_budget == 0 {
        12_000
    } else {
        token_budget
    };
    let candidate_cap = cfg.plan_candidate_cap; // PRD R1: hard cap (default 60)
    let semantic_cap = cfg.plan_semantic_cap; // PRD R2: semantic stage cap (default 30)
    let min_final_score = cfg.plan_min_final_score; // PRD R3: threshold (default 0.12)

    // ── Index ──────────────────────────────────────────────────────────────────
    let project_root = super::cache::canonical_project_root(project)?;
    let state = indexer::build_state(&project_root, false, cfg.features.cascade_invalidation, 0)?;
    if !state.cache_hit {
        super::store_artifact(&state.artifact)?;
        super::store_import_edges(&state.artifact);
    }

    let churn = git_churn::load_churn(&project_root).unwrap_or_else(|_| ChurnCache {
        head_sha: "unknown".to_string(),
        freq_map: std::collections::HashMap::new(),
        max_count: 0,
    });

    let parsed_intent = intent::parse_intent(task, &state.project_id);
    let query_tags = parsed_intent.extracted_tags.clone();
    let recent_paths: HashSet<String> =
        state.delta.changes.iter().map(|d| d.path.clone()).collect();

    let all_symbols: Vec<(String, Vec<String>)> = state
        .artifact
        .files
        .iter()
        .map(|fa| {
            let syms: Vec<String> = fa
                .pub_symbols
                .iter()
                .filter(|s| s.kind == "fn")
                .map(|s| s.name.clone())
                .collect();
            (fa.rel_path.clone(), syms)
        })
        .collect();
    let cg = CallGraph::build(&all_symbols, &project_root);

    // ── Tier A: direct seeds ───────────────────────────────────────────────────
    let mut tier_a: Vec<(String, f32)> = Vec::new(); // (rel_path, tier_a_score)
    let mut all_paths: HashSet<String> = HashSet::new();

    for fa in &state.artifact.files {
        let has_symbols = !fa.pub_symbols.is_empty();
        let has_imports = !fa.imports.is_empty();
        if is_noise(
            &fa.rel_path,
            fa.language.as_deref(),
            fa.line_count,
            has_symbols,
            has_imports,
            &query_tags,
            1, // Tier A
        ) {
            continue;
        }
        let score = tier_a_score(
            &fa.rel_path,
            fa.language.as_deref(),
            has_symbols,
            has_imports,
            &query_tags,
        );
        if score > 0.0 {
            tier_a.push((fa.rel_path.clone(), score));
            all_paths.insert(fa.rel_path.clone());
        }
    }

    // Sort Tier A descending by score
    tier_a.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // ── Tier B: 1-hop neighbors via import edges ────────────────────────────────
    // Build import-reverse-index: file → files that import it
    let mut importers_of: HashMap<String, Vec<String>> = HashMap::new();
    for fa in &state.artifact.files {
        for imp in &fa.imports {
            importers_of
                .entry(imp.clone())
                .or_default()
                .push(fa.rel_path.clone());
        }
    }

    // Tier A seed paths (take top seeds for neighbor expansion)
    let seed_paths: HashSet<String> = tier_a
        .iter()
        .take(20) // expand neighbors from top-20 seeds only
        .map(|(p, _)| p.clone())
        .collect();

    // CHANGED: Use HashMap accumulator to avoid dedup_by-after-sort bug (only removes consecutive duplicates)
    let mut tier_b_map: HashMap<String, f32> = HashMap::new();
    // Build a lookup map for file artifacts
    let fa_map: HashMap<String, &super::FileArtifact> = state
        .artifact
        .files
        .iter()
        .map(|fa| (fa.rel_path.clone(), fa))
        .collect();

    for seed in &seed_paths {
        // Files that import this seed via module string matching
        // Note: imports are module strings (e.g. "super::budget"), not file paths.
        // We match by checking if the seed's stem appears in any import string.
        let seed_stem = std::path::Path::new(seed)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(seed.as_str());
        for fa in &state.artifact.files {
            if all_paths.contains(&fa.rel_path) {
                continue;
            }
            let imports_seed = fa.imports.iter().any(|imp| {
                let imp_lower = imp.to_ascii_lowercase();
                imp_lower.contains(seed_stem) || imp == "super::*"
            });
            if imports_seed {
                let has_symbols = !fa.pub_symbols.is_empty();
                let has_imports = !fa.imports.is_empty();
                if !is_noise(
                    &fa.rel_path,
                    fa.language.as_deref(),
                    fa.line_count,
                    has_symbols,
                    has_imports,
                    &query_tags,
                    2,
                ) {
                    // CHANGED: accumulate max score per path (dedup-safe)
                    let e = tier_b_map.entry(fa.rel_path.clone()).or_insert(0.0);
                    *e = e.max(0.3); // import-neighbor base score
                }
            }
        }
        // Call-graph neighbors (callers of seed symbols, query-tag aware)
        let cg_score = cg.caller_score(seed, &query_tags);
        if cg_score > 0.1 {
            for fa in &state.artifact.files {
                if all_paths.contains(&fa.rel_path) {
                    continue;
                }
                let cs = cg.caller_score(&fa.rel_path, &query_tags);
                if cs > 0.1 {
                    let has_symbols = !fa.pub_symbols.is_empty();
                    let has_imports = !fa.imports.is_empty();
                    if !is_noise(
                        &fa.rel_path,
                        fa.language.as_deref(),
                        fa.line_count,
                        has_symbols,
                        has_imports,
                        &query_tags,
                        2,
                    ) {
                        let e = tier_b_map.entry(fa.rel_path.clone()).or_insert(0.0);
                        *e = e.max(0.2 + cs * 0.3); // call-graph score
                    }
                }
            }
        }
    }
    // Drain map into sorted vec and update all_paths
    let mut tier_b: Vec<(String, f32)> = tier_b_map.into_iter().collect();
    tier_b.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for (path, _) in &tier_b {
        all_paths.insert(path.clone());
    }

    // ── Tier C: fallback recall pool ───────────────────────────────────────────
    // Fill remaining budget from source files with high churn or recency
    let tier_c_budget = candidate_cap.saturating_sub(tier_a.len() + tier_b.len());
    let mut tier_c: Vec<(String, f32)> = Vec::new();
    if tier_c_budget > 0 {
        let mut c_pool: Vec<(&super::FileArtifact, f32)> = state
            .artifact
            .files
            .iter()
            .filter(|fa| !all_paths.contains(&fa.rel_path))
            .filter(|fa| {
                !is_noise(
                    &fa.rel_path,
                    fa.language.as_deref(),
                    fa.line_count,
                    !fa.pub_symbols.is_empty(),
                    !fa.imports.is_empty(),
                    &query_tags,
                    3, // Tier C — relaxed filter
                )
            })
            .map(|fa| {
                let churn = git_churn::churn_score(&churn, &fa.rel_path);
                let recency = if recent_paths.contains(&fa.rel_path) {
                    1.0_f32
                } else {
                    0.0
                };
                let score = 0.15_f32 + churn * 0.1 + recency * 0.05;
                (fa, score)
            })
            .collect();
        c_pool.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (fa, score) in c_pool.into_iter().take(tier_c_budget) {
            tier_c.push((fa.rel_path.clone(), score));
            all_paths.insert(fa.rel_path.clone());
        }
    }

    // ── Build Candidate vec (hard-capped) ──────────────────────────────────────
    let pool: Vec<(String, f32, &str)> = tier_a
        .iter()
        .map(|(p, s)| (p.clone(), *s, "tier_a"))
        .chain(tier_b.iter().map(|(p, s)| (p.clone(), *s, "tier_b")))
        .chain(tier_c.iter().map(|(p, s)| (p.clone(), *s, "tier_c")))
        .take(candidate_cap) // PRD R1 hard cap
        .collect();

    let mut candidates: Vec<Candidate> = pool
        .iter()
        .filter_map(|(rel_path, graph_score, tier)| {
            let fa = fa_map.get(rel_path.as_str())?;
            let mut c = Candidate::new(rel_path);
            // Graph score is the combined tier signal
            c.features.f_structural_relevance = graph_score.min(1.0);
            c.features.f_churn_score = git_churn::churn_score(&churn, rel_path);
            c.features.f_recency_score = if recent_paths.contains(rel_path.as_str()) {
                1.0
            } else {
                0.0
            };
            c.features.f_risk_score = ranker::path_risk_score(rel_path);
            c.features.f_test_proximity = if ranker::is_test_file(rel_path) {
                0.8
            } else {
                0.0
            };
            c.features.f_call_graph_score = cg.caller_score(rel_path, &query_tags);
            let raw_cost = budget::estimate_tokens_for_path(rel_path, fa.line_count);
            c.estimated_tokens = raw_cost.max(180); // CHANGED: floor only, no ceiling — budget assembler handles overflow + min-1 guarantee
            c.features.f_token_cost = (raw_cost as f32 / 1000.0).min(1.0);
            c.sources.push(tier.to_string());
            Some(c)
        })
        .collect();

    // ── Stage-1 ranking ────────────────────────────────────────────────────────
    candidates = ranker::rank_stage1(candidates, &parsed_intent);

    // ── PRD R2: Semantic stage on top-semantic_cap candidates ──────────────────
    let sem_inputs: Vec<&Candidate> = candidates.iter().take(semantic_cap).collect();
    let (evidence_map, backend_used) =
        semantic_stage::run_semantic_stage(task, &sem_inputs, &project_root)?;

    // Record telemetry
    let _ = super::record_cache_event(&state.project_id, "plan_graph_first"); // CHANGED: use actual project_id for telemetry

    // ── PRD R3: Fusion scoring ──────────────────────────────────────────────────
    for c in &mut candidates {
        let graph_score = c.score; // Stage-1 score
        if let Some(ev) = evidence_map.get(&c.rel_path) {
            // Fusion: 0.65 * graph + 0.35 * semantic (PRD R3)
            c.score = 0.65 * graph_score + 0.35 * ev.semantic_score;
            c.sources
                .push(format!("semantic:{}", ev.matched_terms.join(",")));
        }
        // else: score stays as graph_score (fail-open)
    }
    // Re-sort after fusion
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // ── PRD R3: Pre-budget threshold filter ────────────────────────────────────
    // CHANGED: use higher threshold for infra/test files without semantic evidence
    // to prevent cheap low-relevance files displacing large high-relevance source files
    // in the budget utility calculation.
    candidates.retain(|c| {
        let has_semantic = c.sources.iter().any(|s| s.starts_with("semantic:"));
        let path = c.rel_path.to_ascii_lowercase();
        let is_infra = path.ends_with(".md")
            || path.contains("/test")
            || path.contains("__init__")
            || path.ends_with(".yaml")
            || path.ends_with(".toml")
            || path.ends_with(".json");
        if is_infra && !has_semantic {
            // Infra files without semantic evidence need higher threshold (CHANGED: was min_final_score)
            c.score >= 0.22
        } else {
            c.score >= min_final_score || has_semantic
        }
    });

    // ── PRD R3: Cap test/docs/config at 20% of final set ──────────────────────
    // Apply cap AFTER threshold filter to bound infra files in final budget pool.
    // (only if intent does not explicitly mention test/docs)
    let explicit_test_docs = parsed_intent
        .extracted_tags
        .iter()
        .any(|t| matches!(t.as_str(), "test" | "tests" | "doc" | "docs" | "readme"));
    if !explicit_test_docs {
        // CHANGED: cap at 2 absolute infra files (not percentage) to prevent cheap infra dominating
        let max_noise: usize = 2;
        let mut noise_count = 0usize;
        candidates.retain(|c| {
            let path = c.rel_path.to_ascii_lowercase();
            let is_infra = path.ends_with(".md")
                || path.ends_with(".txt")
                || path.ends_with(".yml")
                || path.ends_with(".yaml")
                || path.contains("/test")
                || path.contains("__init__")
                || path.contains("/.github/"); // CHANGED: extended infra check
            if is_infra {
                noise_count += 1;
                noise_count <= max_noise
            } else {
                true
            }
        });
    }

    let graph_candidate_count = pool.len(); // PRD R4: track before threshold filter
    let semantic_hit_count = evidence_map.len(); // PRD R4: how many got semantic evidence

    let mut result = budget::assemble(candidates, token_budget);
    // ADDED: PRD R4 additive trace fields
    result.pipeline_version = Some("graph_first_v1".to_string());
    result.semantic_backend_used = Some(backend_used);
    result.graph_candidate_count = Some(graph_candidate_count);
    result.semantic_hit_count = Some(semantic_hit_count);
    Ok(result)
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_source_lang ─────────────────────────────────────────────────────

    #[test]
    fn test_is_source_lang_rust() {
        assert!(is_source_lang(Some("rust")));
    }

    #[test]
    fn test_is_source_lang_python() {
        assert!(is_source_lang(Some("python")));
    }

    #[test]
    fn test_is_source_lang_none() {
        assert!(!is_source_lang(None));
    }

    #[test]
    fn test_is_source_lang_toml_is_not_source() {
        assert!(!is_source_lang(Some("toml")));
    }

    // ── path_overlap ───────────────────────────────────────────────────────

    #[test]
    fn test_path_overlap_single_match() {
        let tags = vec!["memory".to_string(), "budget".to_string()];
        assert_eq!(path_overlap("src/memory_layer/budget.rs", &tags), 2);
    }

    #[test]
    fn test_path_overlap_no_match() {
        let tags = vec!["auth".to_string()];
        assert_eq!(path_overlap("src/memory_layer/budget.rs", &tags), 0);
    }

    #[test]
    fn test_path_overlap_empty_tags() {
        assert_eq!(path_overlap("src/main.rs", &[]), 0);
    }

    #[test]
    fn test_path_overlap_case_insensitive() {
        let tags = vec!["memory".to_string()];
        // path_overlap lowercases the path
        assert_eq!(path_overlap("src/Memory_Layer/mod.rs", &tags), 1);
    }

    // ── is_noise ───────────────────────────────────────────────────────────

    #[test]
    fn test_noise_rtk_lock_always_excluded() {
        assert!(is_noise(
            "src/main.rs.rtk-lock",
            Some("rust"),
            Some(100),
            true,
            true,
            &[],
            1
        ));
    }

    #[test]
    fn test_noise_review_dir_excluded() {
        assert!(is_noise(
            "docs/review/20260218_report.md",
            Some("markdown"),
            Some(50),
            false,
            false,
            &[],
            1
        ));
    }

    #[test]
    fn test_noise_issue_dir_md_excluded() {
        assert!(is_noise(
            "docs/issues/20260218_perf.md",
            Some("markdown"),
            Some(50),
            false,
            false,
            &[],
            1
        ));
    }

    #[test]
    fn test_noise_tiny_source_no_symbols() {
        // Source file with <=5 lines, no symbols, no imports → noise
        assert!(is_noise(
            "src/__init__.py",
            Some("python"),
            Some(1),
            false,
            false,
            &[],
            1
        ));
    }

    #[test]
    fn test_noise_tiny_source_with_symbols_kept() {
        // Source file with <=5 lines but HAS symbols → kept
        assert!(!is_noise(
            "src/lib.rs",
            Some("rust"),
            Some(3),
            true,
            false,
            &[],
            1
        ));
    }

    #[test]
    fn test_noise_doc_without_overlap_tier_a() {
        // Docs without query overlap in Tier A → noise
        assert!(is_noise("README.md", None, Some(50), false, false, &[], 1));
    }

    #[test]
    fn test_noise_doc_with_overlap_tier_a_kept() {
        // Docs WITH query overlap → kept
        let tags = vec!["readme".to_string()];
        assert!(!is_noise(
            "README.md",
            None,
            Some(50),
            false,
            false,
            &tags,
            1
        ));
    }

    #[test]
    fn test_noise_config_without_overlap_tier_a() {
        // Config file without query overlap in Tier A → noise
        assert!(is_noise(
            "config.toml",
            Some("toml"),
            Some(20),
            false,
            false,
            &[],
            1
        ));
    }

    #[test]
    fn test_noise_normal_source_file_kept() {
        // Normal source file → not noise
        assert!(!is_noise(
            "src/main.rs",
            Some("rust"),
            Some(200),
            true,
            true,
            &[],
            1
        ));
    }

    #[test]
    fn test_noise_tier_c_relaxed() {
        // Tier C: docs/config without overlap still pass (tier > 2)
        assert!(!is_noise("README.md", None, Some(50), false, false, &[], 3));
    }

    #[test]
    fn test_noise_test_file_without_overlap_tier_a() {
        // Test file in Tier A without query overlap and no symbols → noise
        assert!(is_noise(
            "tests/test_auth.py",
            Some("python"),
            Some(100),
            false,
            false,
            &[],
            1
        ));
    }

    #[test]
    fn test_noise_test_file_with_overlap_kept() {
        let tags = vec!["auth".to_string()];
        assert!(!is_noise(
            "tests/test_auth.py",
            Some("python"),
            Some(100),
            false,
            false,
            &tags,
            1
        ));
    }

    // ── tier_a_score ───────────────────────────────────────────────────────

    #[test]
    fn test_tier_a_score_direct_match() {
        let tags = vec!["budget".to_string()];
        let score = tier_a_score(
            "src/memory_layer/budget.rs",
            Some("rust"),
            true,
            false,
            &tags,
        );
        assert!(
            score > 0.5,
            "direct path match should score > 0.5, got {score}"
        );
    }

    #[test]
    fn test_tier_a_score_no_match() {
        let tags = vec!["auth".to_string()];
        let score = tier_a_score(
            "src/memory_layer/budget.rs",
            Some("rust"),
            true,
            false,
            &tags,
        );
        // No path overlap with "auth", but it's a source file with symbols → 0.1
        assert!(
            (score - 0.1).abs() < 0.01,
            "expected 0.1 for source with symbols, got {score}"
        );
    }

    #[test]
    fn test_tier_a_score_doc_without_overlap_zero() {
        let tags = vec!["auth".to_string()];
        let score = tier_a_score("README.md", None, false, false, &tags);
        assert_eq!(score, 0.0, "doc without overlap should score 0");
    }

    #[test]
    fn test_tier_a_score_config_without_overlap_zero() {
        let tags = vec!["memory".to_string()];
        let score = tier_a_score("package.json", None, false, false, &tags);
        assert_eq!(score, 0.0, "config without overlap should score 0");
    }

    #[test]
    fn test_tier_a_score_multiple_tag_overlap() {
        let tags = vec!["memory".to_string(), "budget".to_string()];
        let score = tier_a_score(
            "src/memory_layer/budget.rs",
            Some("rust"),
            true,
            false,
            &tags,
        );
        // Two overlaps + source with symbols
        assert!(
            score > 0.7,
            "multiple tag overlap should boost score, got {score}"
        );
    }

    #[test]
    fn test_tier_a_score_source_without_symbols() {
        let tags = vec!["main".to_string()];
        let score_with = tier_a_score("src/main.rs", Some("rust"), true, false, &tags);
        let score_without = tier_a_score("src/main.rs", Some("rust"), false, false, &tags);
        assert!(score_with > score_without, "symbols should add 0.1 bonus");
    }
}
