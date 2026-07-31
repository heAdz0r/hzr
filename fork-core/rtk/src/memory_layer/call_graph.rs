//! Symbol call graph: regex-based static analysis of Rust/TS/Python call sites.
//!
//! Determines which files CALL which symbols. Used as `f_call_graph_score`:
//! if a query mentions "store_artifact" and file A calls store_artifact(),
//! file A gets boosted in ranking regardless of its import structure.
//!
//! Approach: regex matching of `symbol(` and `symbol::` patterns.
//! Pure-data: build_from_content() enables unit testing without disk I/O.
//! Disk-IO path: build() reads file content via std::fs.

use aho_corasick::AhoCorasick; // M2: multi-pattern matching
use std::collections::{HashMap, HashSet};
use std::path::Path;

// ── Public types ──────────────────────────────────────────────────────────────

/// Inverted call graph: symbol_name → files that call it.
pub struct CallGraph {
    /// symbol → set of rel_paths that contain a call site for that symbol.
    caller_index: HashMap<String, Vec<String>>,
}

impl CallGraph {
    /// Build call graph from actual files on disk.
    /// `files` is a list of (rel_path, pub_symbol_names) pairs from the artifact.
    /// Reads each source file and scans for call sites.
    pub fn build(
        all_symbols: &[(String, Vec<String>)], // (rel_path, symbol_names)
        project_root: &Path,
    ) -> Self {
        let mut content_map: HashMap<String, String> = HashMap::new();
        for (rel_path, _) in all_symbols {
            let abs = project_root.join(rel_path);
            if let Ok(s) = std::fs::read_to_string(&abs) {
                content_map.insert(rel_path.clone(), s);
            }
        }
        Self::build_from_content(all_symbols, &content_map)
    }

    /// Build from pre-loaded content map (path → source text).
    /// M2: Uses Aho-Corasick automaton for O(F*L + S) instead of O(F*S) scanning.
    pub fn build_from_content(
        all_symbols: &[(String, Vec<String>)],
        content_map: &HashMap<String, String>,
    ) -> Self {
        // Collect all unique symbol names (>= 3 chars) across all files
        let all_known: Vec<String> = all_symbols
            .iter()
            .flat_map(|(_, syms)| syms.iter().cloned())
            .filter(|s| s.len() >= 3)
            .collect::<HashSet<String>>()
            .into_iter()
            .collect();

        if all_known.is_empty() || content_map.is_empty() {
            return Self {
                caller_index: HashMap::new(),
            };
        }

        // M2: Build AC patterns: "symbol(" and "symbol::" for each known symbol
        let mut patterns: Vec<String> = Vec::with_capacity(all_known.len() * 2);
        let mut pattern_to_symbol: Vec<usize> = Vec::with_capacity(all_known.len() * 2);
        for (idx, sym) in all_known.iter().enumerate() {
            patterns.push(format!("{sym}("));
            pattern_to_symbol.push(idx);
            patterns.push(format!("{sym}::"));
            pattern_to_symbol.push(idx);
        }

        let ac = match AhoCorasick::new(&patterns) {
            Ok(ac) => ac,
            Err(_) => {
                // Fallback: empty graph if AC build fails (shouldn't happen)
                return Self {
                    caller_index: HashMap::new(),
                };
            }
        };

        let mut caller_index: HashMap<String, Vec<String>> = HashMap::new();

        for (caller_path, content) in content_map {
            // M2: Single-pass scan — collect which symbols have matches in this file
            let mut matched_symbols: HashSet<usize> = HashSet::new();
            for mat in ac.find_iter(content) {
                matched_symbols.insert(pattern_to_symbol[mat.pattern().as_usize()]);
            }

            // Only run is_only_definition check for matched (file, symbol) pairs
            for sym_idx in matched_symbols {
                let symbol = &all_known[sym_idx];
                if !is_only_definition(content, symbol) {
                    caller_index
                        .entry(symbol.clone())
                        .or_default()
                        .push(caller_path.clone());
                }
            }
        }

        Self { caller_index }
    }

    /// Return files that call the given symbol.
    pub fn callers_of(&self, symbol: &str) -> &[String] {
        self.caller_index
            .get(symbol)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Score a file by how many of the query tags map to symbols it calls.
    /// Returns value in [0.0, 1.0].
    pub fn caller_score(&self, rel_path: &str, query_tags: &[String]) -> f32 {
        if query_tags.is_empty() {
            return 0.0;
        }
        // Count query tags for which this file has at least one call site
        let hits = query_tags
            .iter()
            .filter(|tag| {
                // Fuzzy: tag matches if any known symbol contains the tag as substring
                self.caller_index.iter().any(|(sym, callers)| {
                    (sym.contains(tag.as_str()) || tag.contains(sym.as_str()))
                        && callers.iter().any(|p| p == rel_path)
                })
            })
            .count();
        (hits as f32 / query_tags.len() as f32).min(1.0)
    }

    /// True if the graph has any entries.
    pub fn is_empty(&self) -> bool {
        self.caller_index.is_empty()
    }

    /// Total number of (symbol, caller_file) edges.
    pub fn edge_count(&self) -> usize {
        self.caller_index.values().map(|v| v.len()).sum()
    }
}

// ── Call site detection ───────────────────────────────────────────────────────

/// Returns true if `content` contains a call site for `symbol`.
///
/// Patterns matched:
/// - `symbol(` — direct function call
/// - `symbol::` — module/static access
/// - `symbol` as standalone word (for method calls: `.symbol(`)
///
/// Filters out: definition sites (`fn symbol`, `pub fn symbol`, `def symbol`, etc.)
/// to avoid self-scoring (a file doesn't "call" its own definitions).
pub fn has_call_site(content: &str, symbol: &str) -> bool {
    if symbol.len() < 3 {
        return false; // skip very short names (too many false positives)
    }
    let call_pat = format!("{symbol}(");
    let mod_pat = format!("{symbol}::");
    // Check for call pattern
    (content.contains(&call_pat) || content.contains(&mod_pat))
        // Exclude if the ONLY occurrences are definition sites
        && !is_only_definition(content, symbol)
}

/// Returns true if `symbol` appears ONLY as a definition (fn/struct/enum/class/def/const).
/// If at least one non-definition occurrence exists, returns false.
///
/// Uses per-line analysis to avoid double-counting overlapping definition patterns
/// (e.g. "fn store_artifact(" and "pub fn store_artifact" are not independent counts).
fn is_only_definition(content: &str, symbol: &str) -> bool {
    let call_pat = format!("{symbol}(");
    let mod_pat = format!("{symbol}::");

    let mut non_def_calls: usize = 0;
    for line in content.lines() {
        let has_call = line.contains(&call_pat) || line.contains(&mod_pat);
        if !has_call {
            continue;
        }
        // Is this occurrence a definition site?
        let is_def = line.contains(&format!("fn {symbol}"))
            || line.contains(&format!("def {symbol}("))      // Python
            || line.contains(&format!("function {symbol}(")) // JS/TS
            || line.contains(&format!("const {symbol} ="))   // JS/TS arrow fn
            || line.contains(&format!("struct {symbol}"))
            || line.contains(&format!("enum {symbol}"))
            || line.contains(&format!("trait {symbol}"));
        if !is_def {
            non_def_calls += 1;
        }
    }
    non_def_calls == 0
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── has_call_site unit tests ──

    #[test]
    fn test_call_site_detected_direct_call() {
        assert!(has_call_site(
            "let x = store_artifact(data);",
            "store_artifact"
        ));
    }

    #[test]
    fn test_call_site_detected_module_access() {
        assert!(has_call_site(
            "cache::store_artifact(data)",
            "store_artifact"
        ));
    }

    #[test]
    fn test_call_site_not_detected_only_definition() {
        // A file that only defines the function, doesn't call it elsewhere
        let src = "pub fn store_artifact(data: &Data) -> Result<()> { Ok(()) }";
        assert!(!has_call_site(src, "store_artifact"));
    }

    #[test]
    fn test_call_site_detected_when_both_defined_and_called() {
        // File defines AND calls the function (recursive or tests)
        let src = "pub fn store_artifact(x: u32) {\n    store_artifact(x - 1);\n}";
        // 1 definition, 1 call → call_count(2) > def_count(1) → has call site
        assert!(has_call_site(src, "store_artifact"));
    }

    #[test]
    fn test_call_site_skips_short_symbols() {
        // Symbol < 3 chars — too many false positives
        assert!(!has_call_site("let x = fn(a, b);", "fn"));
    }

    #[test]
    fn test_call_site_not_detected_absent_symbol() {
        assert!(!has_call_site("let x = other_fn(data);", "store_artifact"));
    }

    // ── CallGraph unit tests ──

    fn make_content_map(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn make_symbols(entries: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        entries
            .iter()
            .map(|(path, syms)| {
                (
                    path.to_string(),
                    syms.iter().map(|s| s.to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn test_build_finds_callers() {
        let symbols = make_symbols(&[("src/cache.rs", &["store_artifact", "load_artifact"])]);
        let content = make_content_map(&[
            ("src/api.rs", "fn handle() { store_artifact(x); }"),
            ("src/main.rs", "fn run() { load_artifact(); }"),
            (
                "src/cache.rs",
                "pub fn store_artifact() {} pub fn load_artifact() {}",
            ),
        ]);
        let g = CallGraph::build_from_content(&symbols, &content);

        let callers_store = g.callers_of("store_artifact");
        assert!(
            callers_store.contains(&"src/api.rs".to_string()),
            "api.rs should call store_artifact"
        );
        assert!(
            !callers_store.contains(&"src/cache.rs".to_string()),
            "cache.rs defines store_artifact, should not be listed as caller"
        );
    }

    #[test]
    fn test_build_module_access_pattern() {
        let symbols = make_symbols(&[("src/cache.rs", &["store_artifact"])]);
        let content = make_content_map(&[("src/api.rs", "cache::store_artifact(data)")]);
        let g = CallGraph::build_from_content(&symbols, &content);
        assert!(g
            .callers_of("store_artifact")
            .contains(&"src/api.rs".to_string()));
    }

    #[test]
    fn test_caller_score_zero_no_match() {
        let symbols = make_symbols(&[("src/cache.rs", &["store_artifact"])]);
        let content = make_content_map(&[("src/api.rs", "fn handle() { other_fn(); }")]);
        let g = CallGraph::build_from_content(&symbols, &content);
        let score = g.caller_score("src/api.rs", &["store_artifact".to_string()]);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_caller_score_partial_match() {
        let symbols = make_symbols(&[("src/cache.rs", &["store_artifact", "load_artifact"])]);
        let content = make_content_map(&[("src/api.rs", "fn h() { store_artifact(x); other(); }")]);
        let g = CallGraph::build_from_content(&symbols, &content);
        // tags: ["store_artifact", "load_artifact"] — api.rs calls one of two
        let score = g.caller_score(
            "src/api.rs",
            &["store_artifact".to_string(), "load_artifact".to_string()],
        );
        assert!(score > 0.0 && score <= 1.0, "partial match: score={score}");
    }

    #[test]
    fn test_caller_score_empty_tags_returns_zero() {
        let g = CallGraph::build_from_content(&[], &HashMap::new());
        assert_eq!(g.caller_score("any.rs", &[]), 0.0);
    }

    #[test]
    fn test_empty_graph() {
        let g = CallGraph::build_from_content(&[], &HashMap::new());
        assert!(g.is_empty());
        assert_eq!(g.edge_count(), 0);
    }

    #[test]
    fn test_edge_count() {
        let symbols = make_symbols(&[("src/cache.rs", &["foo", "bar"])]);
        let content = make_content_map(&[
            ("src/a.rs", "fn x() { foo(1); bar(2); }"),
            ("src/b.rs", "fn y() { foo(3); }"),
        ]);
        let g = CallGraph::build_from_content(&symbols, &content);
        // foo: a.rs, b.rs (2 edges) + bar: a.rs (1 edge) = 3
        assert_eq!(
            g.edge_count(),
            3,
            "expected 3 edges, got {}",
            g.edge_count()
        );
    }
}
