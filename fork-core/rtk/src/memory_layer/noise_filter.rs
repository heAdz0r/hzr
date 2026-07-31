//! C2: Unified noise filter for plan-context candidate filtering.
//!
//! Supersedes `is_low_signal_candidate` (mod.rs legacy pipeline) and
//! `is_noise` (planner_graph.rs graph-first pipeline). Both callers
//! delegate here with tier=None (legacy) or tier=Some(1..3) (graph-first).

use std::collections::HashSet;

/// C2: Unified noise filter for plan-context candidates.
/// Returns true if the file should be excluded from the candidate pool.
///
/// `tier`: None = legacy pipeline (no tier-aware filtering),
///         Some(1) = Tier A, Some(2) = Tier B, Some(3) = Tier C (relaxed).
pub(super) fn is_noise_candidate(
    rel_path: &str,
    language: Option<&str>,
    line_count: Option<u32>,
    has_symbols: bool,
    has_imports: bool,
    query_tags: &[String],
    tier: Option<u8>,
) -> bool {
    let path = rel_path.replace('\\', "/").to_ascii_lowercase();

    // Always exclude .rtk-lock files
    if path.ends_with(".rtk-lock") {
        return true;
    }

    // Generated review/issue reports are noise for task planning
    if path.contains("/review/") || (path.contains("/issues/") && path.ends_with(".md")) {
        return true;
    }

    let lines = line_count.unwrap_or(0);
    let is_source = is_source_like_language(language);
    let is_doc = path.ends_with(".md") || path.ends_with(".rst") || path.ends_with(".txt");
    let is_config = matches!(language, Some("toml" | "yaml" | "json"));
    let is_text_blob = path.ends_with(".txt")
        || path.ends_with(".log")
        || path.ends_with(".out")
        || path.ends_with(".csv");
    let has_semantic_signals = has_imports || has_symbols;
    let overlap = path_query_overlap_hits(rel_path, query_tags);
    let is_test = path.contains("/test") || path.contains("_test") || path.contains("spec");

    // Tiny source stubs (empty __init__, barrel files) rarely help planning.
    if is_source && !has_symbols && !has_imports && lines <= 5 {
        return true;
    }

    // Text/report blobs without symbols/imports are almost always noise (legacy path).
    if tier.is_none() && is_text_blob && !has_semantic_signals {
        return true;
    }

    // Config/docs matter only when they match query terms.
    // Graph-first: only apply in Tier A/B (tier <= 2).
    let apply_overlap_filter = match tier {
        None => true,      // legacy: always apply
        Some(t) => t <= 2, // graph-first: Tier A/B only
    };
    if apply_overlap_filter {
        if (is_doc || is_config) && !has_semantic_signals && overlap == 0 {
            return true;
        }
        // Test files without overlap and no symbols (graph-first Tier A/B)
        if tier.is_some() && is_test && overlap == 0 && !has_symbols {
            return true;
        }
    }

    // Legacy-only: unknown file types with no structure are low-value.
    if tier.is_none() && !is_source && !is_doc && !is_config && !has_semantic_signals && lines <= 80
    {
        return true;
    }

    false
}

/// C2: Unified source-language classifier (superset of both pipelines).
pub(super) fn is_source_like_language(language: Option<&str>) -> bool {
    matches!(
        language,
        Some(
            "rust"
                | "typescript"
                | "javascript"
                | "python"
                | "go"
                | "java"
                | "kotlin"
                | "swift"
                | "ruby"
                | "php"
                | "scala"
                | "c"
                | "cpp"
                | "csharp"
        )
    )
}

/// C2: Precise path-query overlap using HashSet tokenization.
/// Splits path into alphanumeric tokens (>=3 chars) and counts tag matches.
pub(super) fn path_query_overlap_hits(rel_path: &str, query_tags: &[String]) -> usize {
    let path_tokens: HashSet<String> = rel_path
        .to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(|t| t.to_string())
        .collect();

    query_tags
        .iter()
        .map(|s| s.as_str())
        .filter(|tag| tag.len() >= 3)
        .filter(|tag| path_tokens.contains(*tag))
        .count()
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_noise_candidate ─────────────────────────────────────────────────

    #[test]
    fn test_rtk_lock_always_excluded() {
        assert!(is_noise_candidate(
            "src/main.rs.rtk-lock",
            Some("rust"),
            Some(100),
            true,
            true,
            &[],
            None
        ));
        assert!(is_noise_candidate(
            "src/main.rs.rtk-lock",
            Some("rust"),
            Some(100),
            true,
            true,
            &[],
            Some(1)
        ));
    }

    #[test]
    fn test_review_dir_excluded() {
        assert!(is_noise_candidate(
            "docs/review/report.md",
            None,
            Some(50),
            false,
            false,
            &[],
            None
        ));
    }

    #[test]
    fn test_tiny_source_no_symbols() {
        assert!(is_noise_candidate(
            "src/__init__.py",
            Some("python"),
            Some(1),
            false,
            false,
            &[],
            None
        ));
        assert!(is_noise_candidate(
            "src/__init__.py",
            Some("python"),
            Some(1),
            false,
            false,
            &[],
            Some(1)
        ));
    }

    #[test]
    fn test_tiny_source_with_symbols_kept() {
        assert!(!is_noise_candidate(
            "src/lib.rs",
            Some("rust"),
            Some(3),
            true,
            false,
            &[],
            None
        ));
    }

    #[test]
    fn test_doc_without_overlap_excluded() {
        assert!(is_noise_candidate(
            "README.md",
            None,
            Some(50),
            false,
            false,
            &[],
            None
        ));
        assert!(is_noise_candidate(
            "README.md",
            None,
            Some(50),
            false,
            false,
            &[],
            Some(1)
        ));
    }

    #[test]
    fn test_doc_with_overlap_kept() {
        let tags = vec!["readme".to_string()];
        assert!(!is_noise_candidate(
            "README.md",
            None,
            Some(50),
            false,
            false,
            &tags,
            None
        ));
    }

    #[test]
    fn test_tier_c_relaxed() {
        // Tier C: docs without overlap pass (tier > 2)
        assert!(!is_noise_candidate(
            "README.md",
            None,
            Some(50),
            false,
            false,
            &[],
            Some(3)
        ));
    }

    #[test]
    fn test_normal_source_kept() {
        assert!(!is_noise_candidate(
            "src/main.rs",
            Some("rust"),
            Some(200),
            true,
            true,
            &[],
            None
        ));
    }

    #[test]
    fn test_text_blob_legacy_excluded() {
        assert!(is_noise_candidate(
            "data/output.log",
            None,
            Some(1000),
            false,
            false,
            &[],
            None
        ));
    }

    #[test]
    fn test_test_file_without_overlap_tier_a() {
        assert!(is_noise_candidate(
            "tests/test_auth.py",
            Some("python"),
            Some(100),
            false,
            false,
            &[],
            Some(1)
        ));
    }

    #[test]
    fn test_test_file_with_overlap_kept() {
        let tags = vec!["auth".to_string()];
        assert!(!is_noise_candidate(
            "tests/test_auth.py",
            Some("python"),
            Some(100),
            false,
            false,
            &tags,
            Some(1)
        ));
    }

    // ── path_query_overlap_hits ────────────────────────────────────────────

    #[test]
    fn test_overlap_multiple_tokens() {
        let tags = vec!["memory".to_string(), "budget".to_string()];
        assert_eq!(
            path_query_overlap_hits("src/memory_layer/budget.rs", &tags),
            2
        );
    }

    #[test]
    fn test_overlap_no_match() {
        let tags = vec!["auth".to_string()];
        assert_eq!(
            path_query_overlap_hits("src/memory_layer/budget.rs", &tags),
            0
        );
    }

    #[test]
    fn test_overlap_empty_tags() {
        assert_eq!(path_query_overlap_hits("src/main.rs", &[]), 0);
    }

    #[test]
    fn test_overlap_short_tokens_ignored() {
        let tags = vec!["rs".to_string()]; // < 3 chars
        assert_eq!(path_query_overlap_hits("src/main.rs", &tags), 0);
    }

    // ── is_source_like_language ────────────────────────────────────────────

    #[test]
    fn test_source_lang_rust() {
        assert!(is_source_like_language(Some("rust")));
    }

    #[test]
    fn test_source_lang_csharp_added() {
        assert!(is_source_like_language(Some("csharp")));
    }

    #[test]
    fn test_source_lang_none() {
        assert!(!is_source_like_language(None));
    }

    #[test]
    fn test_source_lang_toml_not_source() {
        assert!(!is_source_like_language(Some("toml")));
    }
}
