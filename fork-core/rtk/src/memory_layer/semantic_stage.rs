//! PRD R2: Targeted semantic search stage (rgai ladder).
//!
//! Backend ladder (PRD R2):
//!   1. grepai global result → intersect with candidate set  [skipped — --files mode used]
//!   2. rg (candidate-scoped via subprocess rtk rgai --files)
//!   3. builtin scorer (candidate-scoped, fallback)
//!
//! Returns: map of rel_path → SemanticEvidence + backend_used label.

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use super::ranker::Candidate;
use super::SemanticEvidence;

/// PRD R2: Run semantic stage on candidate list, returning evidence per file.
/// `candidates` is a slice of refs to the top-N candidates (by semantic_cap).
/// Returns (evidence_map, backend_used).
pub fn run_semantic_stage(
    task: &str,
    candidates: &[&Candidate],
    project_root: &Path,
) -> Result<(HashMap<String, SemanticEvidence>, String)> {
    if candidates.is_empty() || task.trim().is_empty() {
        return Ok((HashMap::new(), "none".to_string()));
    }

    // Build CSV of candidate paths for --files flag
    let files_csv: String = candidates
        .iter()
        .map(|c| c.rel_path.as_str())
        .collect::<Vec<_>>()
        .join(",");

    // ── Backend 1: rg candidate-scoped via rtk rgai --files --json ────────────
    if let Ok((map, backend)) = rg_files_backend(task, &files_csv, project_root, candidates) {
        if !map.is_empty() {
            return Ok((map, backend));
        }
    }

    // ── Backend 2: builtin term scorer (candidate-scoped, no subprocess) ───────
    let (map, backend) = builtin_scorer(task, candidates, project_root);
    Ok((map, backend))
}

// ── Backend 1: rg-files via subprocess ────────────────────────────────────────

fn rg_files_backend(
    task: &str,
    files_csv: &str,
    project_root: &Path,
    candidates: &[&Candidate],
) -> Result<(HashMap<String, SemanticEvidence>, String)> {
    // Use rtk rgai with --files and --json for structured output
    let rtk_bin = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("rtk"));

    let output = Command::new(&rtk_bin)
        .args([
            "rgai",
            task,
            "--path",
            project_root.to_str().unwrap_or("."),
            "--files",
            files_csv,
            "--json",
            "--builtin", // use rg-files path, skip grepai
            "--compact",
        ])
        .output();

    let output = match output {
        Ok(o) => o,
        Err(_) => return Ok((HashMap::new(), "rg-files-err".to_string())),
    };

    if !output.status.success() && output.status.code() != Some(1) {
        return Ok((HashMap::new(), "rg-files-exit".to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return Ok((HashMap::new(), "rg-files-empty".to_string()));
    }

    // Parse JSON output from rtk rgai --json
    let evidence_map = parse_json_output(&stdout, candidates);
    if evidence_map.is_empty() {
        return Ok((HashMap::new(), "rg-files-nomatch".to_string()));
    }

    Ok((evidence_map, "rg-files".to_string()))
}

/// Parse rtk rgai --json output into SemanticEvidence map.
/// FIXED: rtk rgai --json emits a single envelope: {"hits": [{path, score, snippets:[{lines, matched_terms}]}]}
fn parse_json_output(
    // CHANGED: fixed JSON schema to match actual rtk rgai --json format
    stdout: &str,
    candidates: &[&Candidate],
) -> HashMap<String, SemanticEvidence> {
    let mut map: HashMap<String, SemanticEvidence> = HashMap::new();
    let candidate_paths: std::collections::HashSet<&str> =
        candidates.iter().map(|c| c.rel_path.as_str()).collect();

    // Parse the top-level envelope {"hits": [...]}
    let envelope = match serde_json::from_str::<serde_json::Value>(stdout.trim()) {
        Ok(v) => v,
        Err(_) => return map,
    };

    // Navigate envelope["hits"] -> array of hit objects
    let hits = match envelope.get("hits").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return map,
    };

    for hit in hits {
        extract_rgai_hit(hit, &candidate_paths, &mut map);
    }

    map
}

/// Extract SemanticEvidence from a single rtk rgai hit object.
/// Hit shape: { "path": "src/foo.rs", "score": 8.5, "matched_lines": 3,
///              "snippets": [{ "lines": [{"line": N, "text": "..."}], "matched_terms": [...] }] }
fn extract_rgai_hit(
    // CHANGED: rewritten for actual rgai JSON schema
    hit: &serde_json::Value,
    candidate_paths: &std::collections::HashSet<&str>,
    map: &mut HashMap<String, SemanticEvidence>,
) {
    let path = match hit.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return,
    };
    if !candidate_paths.contains(path) {
        return;
    }
    // Score is raw float from rg (higher = more matches). Normalize via tanh to [0..1].
    let raw_score = hit.get("score").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
    let semantic_score = (raw_score / 20.0).tanh().clamp(0.0, 1.0); // CHANGED: normalize raw rg score

    // Extract from first snippet block
    let first_snippet = hit
        .get("snippets")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first());

    let matched_terms: Vec<String> = first_snippet
        .and_then(|s| s.get("matched_terms"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.as_str())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    let snippet: String = first_snippet
        .and_then(|s| s.get("lines"))
        .and_then(|v| v.as_array())
        .and_then(|lines| lines.first())
        .and_then(|l| l.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .take(200) // token-safe short evidence (PRD R2)
        .collect();

    map.insert(
        path.to_string(),
        SemanticEvidence {
            semantic_score,
            matched_terms,
            snippet,
        },
    );
}

// ── Backend 2: builtin term scorer ─────────────────────────────────────────────

/// Simple term-overlap scorer: reads each candidate file and scores by term frequency.
/// Language-agnostic (PRD R1 language-agnostic path).
fn builtin_scorer(
    task: &str,
    candidates: &[&Candidate],
    project_root: &Path,
) -> (HashMap<String, SemanticEvidence>, String) {
    let terms: Vec<String> = task
        .split_whitespace()
        .filter(|w| w.len() >= 3)
        .map(|w| w.to_lowercase())
        .collect();

    if terms.is_empty() {
        return (HashMap::new(), "builtin-no-terms".to_string());
    }

    let mut map: HashMap<String, SemanticEvidence> = HashMap::new();

    for c in candidates {
        let abs = project_root.join(&c.rel_path);
        let content = match std::fs::read_to_string(&abs) {
            Ok(s) => s.to_lowercase(),
            Err(_) => continue,
        };

        let mut hit_terms: Vec<String> = Vec::new();
        let mut total_hits = 0usize;
        let mut snippet_line: Option<String> = String::new().into();

        for term in &terms {
            let count = content.matches(term.as_str()).count();
            if count > 0 {
                hit_terms.push(term.clone());
                total_hits += count;
                // Grab first line containing the term for snippet
                if snippet_line.as_deref().unwrap_or("").is_empty() {
                    snippet_line = content
                        .lines()
                        .find(|line| line.contains(term.as_str()))
                        .map(|l| l.chars().take(120).collect());
                }
            }
        }

        if !hit_terms.is_empty() {
            let score = (hit_terms.len() as f32 / terms.len() as f32 * 0.5
                + (total_hits.min(10) as f32 * 0.05))
                .clamp(0.0, 1.0);
            map.insert(
                c.rel_path.clone(),
                SemanticEvidence {
                    semantic_score: score,
                    matched_terms: hit_terms,
                    snippet: snippet_line.unwrap_or_default(),
                },
            );
        }
    }

    (map, "builtin".to_string())
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_layer::ranker::Candidate;

    fn make_candidate(path: &str) -> Candidate {
        Candidate::new(path)
    }

    // ── parse_json_output ──────────────────────────────────────────────────

    #[test]
    fn test_parse_json_output_valid_envelope() {
        let json = r#"{"hits": [
            {"path": "src/main.rs", "score": 10.0, "matched_lines": 3,
             "snippets": [{"lines": [{"line": 42, "text": "fn main()"}], "matched_terms": ["main", "fn"]}]}
        ]}"#;
        let candidates = [make_candidate("src/main.rs")];
        let refs: Vec<&Candidate> = candidates.iter().collect();
        let map = parse_json_output(json, &refs);
        assert_eq!(map.len(), 1);
        let ev = map.get("src/main.rs").unwrap();
        assert!(ev.semantic_score > 0.0 && ev.semantic_score <= 1.0);
        assert_eq!(ev.matched_terms, vec!["main", "fn"]);
        assert!(ev.snippet.contains("fn main()"));
    }

    #[test]
    fn test_parse_json_output_filters_non_candidates() {
        let json = r#"{"hits": [
            {"path": "src/other.rs", "score": 5.0, "snippets": [{"matched_terms": ["x"]}]}
        ]}"#;
        let candidates = [make_candidate("src/main.rs")]; // different path
        let refs: Vec<&Candidate> = candidates.iter().collect();
        let map = parse_json_output(json, &refs);
        assert!(map.is_empty(), "non-candidate paths should be filtered out");
    }

    #[test]
    fn test_parse_json_output_empty_hits() {
        let json = r#"{"hits": []}"#;
        let candidates = [make_candidate("src/main.rs")];
        let refs: Vec<&Candidate> = candidates.iter().collect();
        let map = parse_json_output(json, &refs);
        assert!(map.is_empty());
    }

    #[test]
    fn test_parse_json_output_invalid_json() {
        let json = "not valid json {";
        let candidates = [make_candidate("src/main.rs")];
        let refs: Vec<&Candidate> = candidates.iter().collect();
        let map = parse_json_output(json, &refs);
        assert!(map.is_empty(), "invalid JSON should return empty map");
    }

    #[test]
    fn test_parse_json_output_missing_hits_key() {
        let json = r#"{"results": []}"#;
        let candidates = [make_candidate("src/main.rs")];
        let refs: Vec<&Candidate> = candidates.iter().collect();
        let map = parse_json_output(json, &refs);
        assert!(map.is_empty(), "missing 'hits' key should return empty map");
    }

    #[test]
    fn test_parse_json_output_multiple_hits() {
        let json = r#"{"hits": [
            {"path": "src/a.rs", "score": 15.0, "snippets": [{"matched_terms": ["auth"]}]},
            {"path": "src/b.rs", "score": 3.0, "snippets": [{"matched_terms": ["token"]}]}
        ]}"#;
        let candidates = [make_candidate("src/a.rs"), make_candidate("src/b.rs")];
        let refs: Vec<&Candidate> = candidates.iter().collect();
        let map = parse_json_output(json, &refs);
        assert_eq!(map.len(), 2);
        // Higher raw score → higher normalized score
        assert!(map["src/a.rs"].semantic_score > map["src/b.rs"].semantic_score);
    }

    // ── extract_rgai_hit ───────────────────────────────────────────────────

    #[test]
    fn test_extract_rgai_hit_score_normalization() {
        // tanh(0.5/20) ≈ 0.025, tanh(100/20) ≈ 1.0
        let low_hit: serde_json::Value = serde_json::json!({
            "path": "a.rs", "score": 0.5,
            "snippets": [{"matched_terms": ["x"]}]
        });
        let high_hit: serde_json::Value = serde_json::json!({
            "path": "b.rs", "score": 100.0,
            "snippets": [{"matched_terms": ["y"]}]
        });
        let paths: std::collections::HashSet<&str> = ["a.rs", "b.rs"].into_iter().collect();
        let mut map = HashMap::new();
        extract_rgai_hit(&low_hit, &paths, &mut map);
        extract_rgai_hit(&high_hit, &paths, &mut map);
        assert!(
            map["a.rs"].semantic_score < 0.1,
            "low raw score → low normalized"
        );
        assert!(
            map["b.rs"].semantic_score > 0.9,
            "high raw score → near 1.0"
        );
    }

    #[test]
    fn test_extract_rgai_hit_missing_path() {
        let hit: serde_json::Value = serde_json::json!({
            "score": 5.0, "snippets": [{"matched_terms": ["x"]}]
        });
        let paths: std::collections::HashSet<&str> = ["a.rs"].into_iter().collect();
        let mut map = HashMap::new();
        extract_rgai_hit(&hit, &paths, &mut map);
        assert!(map.is_empty(), "hit without path should be skipped");
    }

    #[test]
    fn test_extract_rgai_hit_snippet_truncated() {
        let long_text = "x".repeat(300);
        let hit: serde_json::Value = serde_json::json!({
            "path": "a.rs", "score": 5.0,
            "snippets": [{"lines": [{"line": 1, "text": long_text}], "matched_terms": ["x"]}]
        });
        let paths: std::collections::HashSet<&str> = ["a.rs"].into_iter().collect();
        let mut map = HashMap::new();
        extract_rgai_hit(&hit, &paths, &mut map);
        assert!(
            map["a.rs"].snippet.len() <= 200,
            "snippet should be truncated to 200 chars"
        );
    }

    // ── builtin_scorer ─────────────────────────────────────────────────────

    #[test]
    fn test_builtin_scorer_finds_terms() {
        // Create a temp file with known content
        let dir = std::env::temp_dir().join("rtk_test_semantic");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            dir.join("auth.rs"),
            "fn authenticate_user(token: &str) { verify(token) }",
        )
        .unwrap();

        let c = make_candidate("auth.rs");
        let refs = vec![&c];
        let (map, backend) = builtin_scorer("authenticate token", &refs, &dir);
        assert_eq!(backend, "builtin");
        assert!(map.contains_key("auth.rs"), "should find terms in file");
        let ev = &map["auth.rs"];
        assert!(ev.semantic_score > 0.0);
        assert!(
            ev.matched_terms.contains(&"authenticate".to_string())
                || ev.matched_terms.contains(&"token".to_string())
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_builtin_scorer_no_match() {
        let dir = std::env::temp_dir().join("rtk_test_semantic_nomatch");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            dir.join("math.rs"),
            "fn add(a: i32, b: i32) -> i32 { a + b }",
        )
        .unwrap();

        let c = make_candidate("math.rs");
        let refs = vec![&c];
        let (map, _) = builtin_scorer("authentication jwt refresh", &refs, &dir);
        assert!(map.is_empty(), "no matching terms → empty map");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_builtin_scorer_short_terms_filtered() {
        let dir = std::env::temp_dir().join("rtk_test_semantic_short");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("a.rs"), "fn a() { b() }").unwrap();

        let c = make_candidate("a.rs");
        let refs = vec![&c];
        let (map, backend) = builtin_scorer("a b", &refs, &dir); // both terms < 3 chars
        assert_eq!(backend, "builtin-no-terms");
        assert!(map.is_empty(), "short terms should be filtered");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_builtin_scorer_score_clamped() {
        let dir = std::env::temp_dir().join("rtk_test_semantic_clamp");
        let _ = std::fs::create_dir_all(&dir);
        // File with many occurrences of the term
        let content = "auth ".repeat(100);
        std::fs::write(dir.join("hot.rs"), &content).unwrap();

        let c = make_candidate("hot.rs");
        let refs = vec![&c];
        let (map, _) = builtin_scorer("auth", &refs, &dir);
        assert!(
            map["hot.rs"].semantic_score <= 1.0,
            "score must be clamped to 1.0"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── run_semantic_stage edge cases ──────────────────────────────────────

    #[test]
    fn test_run_semantic_stage_empty_candidates() {
        let dir = std::env::temp_dir();
        let (map, backend) = run_semantic_stage("fix bug", &[], &dir).unwrap();
        assert!(map.is_empty());
        assert_eq!(backend, "none");
    }

    #[test]
    fn test_run_semantic_stage_empty_task() {
        let c = make_candidate("a.rs");
        let refs = vec![&c];
        let dir = std::env::temp_dir();
        let (map, backend) = run_semantic_stage("", &refs, &dir).unwrap();
        assert!(map.is_empty());
        assert_eq!(backend, "none");
    }
}
