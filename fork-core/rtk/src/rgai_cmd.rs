use crate::config; // grepai config
use crate::grepai; // grepai delegation
use crate::tracking;
use anyhow::{bail, Result};
use ignore::WalkBuilder;
use lazy_static::lazy_static;
use regex::Regex;
use serde_json::json;
use std::collections::HashMap; // added: for grepai hit grouping
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command; // added: for ripgrep backend

const MAX_SNIPPETS_PER_FILE: usize = 2;
const MAX_SNIPPET_LINE_LEN: usize = 140;
const MIN_FILE_SCORE: f64 = 2.4;
// ADDED: compact mode limits for token savings
const COMPACT_MAX_FILES: usize = 5;
const RELATIVE_SCORE_CUTOFF: f64 = 0.35;
const MAX_GREPAI_SNIPPET_LINES: usize = 5; // added: max lines per grepai snippet

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "code", "file", "find", "for", "from", "how",
    "in", "is", "it", "of", "on", "or", "search", "show", "that", "the", "this", "to", "use",
    "using", "what", "when", "where", "with", "why",
];

lazy_static! {
    /// H3: O(1) stop-word lookup — replaces O(N) slice scan in build_query_model.
    static ref STOP_WORDS_SET: std::collections::HashSet<&'static str> =
        STOP_WORDS.iter().copied().collect();
}

lazy_static! {
    static ref SYMBOL_DEF_RE: Regex = Regex::new(
        r"^\s*(?:pub\s+)?(?:async\s+)?(?:fn|def|class|struct|enum|trait|interface|impl|type)\s+[A-Za-z_][A-Za-z0-9_]*"
    )
    .expect("valid symbol regex");
}

#[derive(Debug, Clone)]
struct QueryModel {
    phrase: String,
    terms: Vec<String>,
}

#[derive(Debug, Clone)]
struct LineCandidate {
    line_idx: usize,
    score: f64,
    matched_terms: Vec<String>,
}

#[derive(Debug, Clone)]
struct Snippet {
    lines: Vec<(usize, String)>,
    matched_terms: Vec<String>,
}

#[derive(Debug, Clone)]
struct SearchHit {
    path: String,
    score: f64,
    matched_lines: usize,
    snippets: Vec<Snippet>,
}

#[derive(Debug, Default)]
struct SearchOutcome {
    scanned_files: usize,
    skipped_large: usize,
    skipped_binary: usize,
    hits: Vec<SearchHit>,
    raw_output: String,
}

pub fn run(
    query: &str,
    path: &str,
    max_results: usize,
    context_lines: usize,
    file_type: Option<&str>,
    max_file_kb: usize,
    json_output: bool,
    compact: bool,
    builtin: bool,       // --builtin flag: skip grepai delegation
    files: Option<&str>, // ADDED: --files flag — restrict search to listed paths
    verbose: u8,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let query = query.trim();
    if query.is_empty() {
        bail!("query cannot be empty");
    }

    let root = Path::new(path);
    if !root.exists() {
        bail!("path does not exist: {}", path);
    }

    // Try grepai delegation first (unless --builtin flag is set or --files is provided)
    // CHANGED: unpack (raw, filtered) for correct savings tracking
    if !builtin && files.is_none() {
        // CHANGED: skip grepai when --files restricts search
        if let Some((raw, filtered)) = try_grepai_delegation(
            query,
            path,
            root,
            max_results,
            json_output,
            compact,
            verbose,
        )? {
            print!("{}", filtered);
            timer.track(
                &format!("grepai search '{}' {}", query, path),
                "rtk rgai (grepai)",
                &raw,
                &filtered,
            );
            return Ok(());
        }
        // Fall through to built-in search
    }

    let query_model = build_query_model(query);
    if verbose > 0 {
        eprintln!(
            "rgai: '{}' in {} (terms: {})",
            query,
            path,
            query_model.terms.join(", ")
        );
    }

    let max_file_bytes = max_file_kb.saturating_mul(1024).max(1024);
    let effective_context = if compact { 0 } else { context_lines };
    let snippets_per_file = if compact { 1 } else { MAX_SNIPPETS_PER_FILE };

    // ADDED: --files mode — restrict search to specific files (two-stage memory pipeline)
    // CHANGED: try ripgrep backend first (fast), fall back to built-in walker (slow)
    let (outcome, backend) = if let Some(files_csv) = files {
        let file_paths: Vec<String> = files_csv
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        (
            search_file_list(&query_model, root, &file_paths, snippets_per_file, verbose)?,
            "rg-files", // ADDED: new backend label for file-list search
        )
    } else if !builtin {
        match try_ripgrep_search(
            &query_model,
            root,
            snippets_per_file,
            file_type,
            max_file_kb,
            verbose,
        )? {
            Some(o) => (o, "rg"),
            None => (
                search_project(
                    &query_model,
                    root,
                    effective_context,
                    snippets_per_file,
                    file_type,
                    max_file_bytes,
                    max_results, // C1: pass limit for early-exit
                    verbose,
                )?,
                "builtin",
            ),
        }
    } else {
        (
            search_project(
                &query_model,
                root,
                effective_context,
                snippets_per_file,
                file_type,
                max_file_bytes,
                max_results, // C1: pass limit for early-exit
                verbose,
            )?,
            "builtin",
        )
    };
    // ADDED: tracking label reflects which backend was used
    let tracking_label = if backend == "rg" {
        "rtk rgai (rg)"
    } else if backend == "rg-files" {
        "rtk rgai (files)" // ADDED: files-list backend label
    } else {
        "rtk rgai"
    };

    let mut rendered = String::new();
    if outcome.hits.is_empty() {
        if json_output {
            rendered = serde_json::to_string_pretty(&json!({
                "query": query,
                "path": path,
                "total_hits": 0,
                "scanned_files": outcome.scanned_files,
                "skipped_large": outcome.skipped_large,
                "skipped_binary": outcome.skipped_binary,
                "hits": []
            }))?;
            rendered.push('\n');
        } else {
            rendered.push_str(&format!("🧠 0 for '{}'\n", query));
        }
        print!("{}", rendered);
        timer.track(
            &format!("grepai search '{}' {}", query, path),
            tracking_label,
            &outcome.raw_output,
            &rendered,
        );
        return Ok(());
    }

    if json_output {
        let hits_json: Vec<_> = outcome
            .hits
            .iter()
            .take(max_results)
            .map(|hit| {
                let snippets: Vec<_> = hit
                    .snippets
                    .iter()
                    .map(|snippet| {
                        let lines: Vec<_> = snippet
                            .lines
                            .iter()
                            .map(|(line_no, text)| json!({ "line": line_no, "text": text }))
                            .collect();
                        json!({
                            "lines": lines,
                            "matched_terms": snippet.matched_terms,
                        })
                    })
                    .collect();
                json!({
                    "path": hit.path,
                    "score": hit.score,
                    "matched_lines": hit.matched_lines,
                    "snippets": snippets,
                })
            })
            .collect();

        rendered = serde_json::to_string_pretty(&json!({
            "query": query,
            "path": path,
            "total_hits": outcome.hits.len(),
            "shown_hits": max_results.min(outcome.hits.len()),
            "scanned_files": outcome.scanned_files,
            "skipped_large": outcome.skipped_large,
            "skipped_binary": outcome.skipped_binary,
            "hits": hits_json
        }))?;
        rendered.push('\n');
        print!("{}", rendered);
        timer.track(
            &format!("grepai search '{}' {}", query, path),
            tracking_label,
            &outcome.raw_output,
            &rendered,
        );
        return Ok(());
    }

    // CHANGED: use extracted render_text_hits with compact-aware limits
    let effective_max = if compact {
        max_results.min(COMPACT_MAX_FILES)
    } else {
        max_results
    };
    rendered = render_text_hits(
        &outcome.hits,
        effective_max,
        compact,
        query,
        outcome.scanned_files,
    );

    if verbose > 0 {
        rendered.push_str(&format!(
            "\nscan stats: skipped {} large, {} binary\n",
            outcome.skipped_large, outcome.skipped_binary
        ));
    }

    print!("{}", rendered);
    timer.track(
        &format!("grepai search '{}' {}", query, path),
        tracking_label,
        &outcome.raw_output,
        &rendered,
    );

    Ok(())
}

/// Convert grepai JSON hits into RTK SearchHit structs
/// Groups chunks by file, selects top snippets, prunes low-relevance files
fn convert_grepai_hits(grepai_hits: Vec<grepai::GrepaiHit>, compact: bool) -> Vec<SearchHit> {
    // Group chunks by file path
    let mut by_file: HashMap<String, Vec<grepai::GrepaiHit>> = HashMap::new();
    for hit in grepai_hits {
        by_file.entry(hit.file_path.clone()).or_default().push(hit);
    }

    let snippets_limit = if compact { 1 } else { MAX_SNIPPETS_PER_FILE };

    let mut search_hits: Vec<SearchHit> = by_file
        .into_iter()
        .map(|(path, mut chunks)| {
            // Sort chunks by score desc within each file
            chunks.sort_by(|a, b| b.score.total_cmp(&a.score));

            // Select top N chunks as snippets
            let selected: Vec<_> = chunks.iter().take(snippets_limit).collect();
            let score: f64 = selected.iter().map(|c| c.score).sum();
            let snippets: Vec<Snippet> = selected.iter().map(|c| parse_grepai_content(c)).collect();

            SearchHit {
                path,
                score,
                matched_lines: chunks.len(),
                snippets,
            }
        })
        .collect();

    // Sort files by score desc, then by path for deterministic ordering.
    search_hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
    });

    // Prune: relative cutoff 35% of top score (no MIN_FILE_SCORE for grepai)
    if let Some(top_score) = search_hits.first().map(|h| h.score) {
        let cutoff = top_score * RELATIVE_SCORE_CUTOFF;
        search_hits.retain(|h| h.score >= cutoff);
    }

    search_hits
}

/// Extract a Snippet from a GrepaiHit, stripping noise and truncating
fn parse_grepai_content(hit: &grepai::GrepaiHit) -> Snippet {
    let content = match &hit.content {
        Some(c) => c.as_str(),
        None => {
            return Snippet {
                lines: vec![(hit.start_line, String::new())],
                matched_terms: vec![],
            };
        }
    };

    let mut lines: Vec<(usize, String)> = Vec::new();

    // grepai JSON may prefix content with synthetic lines:
    //   File: <path>
    //
    // Strip this prefix without shifting source line numbers.
    let mut source_lines = content.lines().peekable();
    if source_lines
        .peek()
        .map(|line| line.trim().to_lowercase().starts_with("file:"))
        .unwrap_or(false)
    {
        source_lines.next();
        if source_lines
            .peek()
            .map(|line| line.trim().is_empty())
            .unwrap_or(false)
        {
            source_lines.next();
        }
    }

    for (offset, raw_line) in source_lines.enumerate() {
        let trimmed = raw_line.trim();

        // Skip empty lines
        if trimmed.is_empty() {
            continue;
        }

        if lines.len() >= MAX_GREPAI_SNIPPET_LINES {
            break;
        }

        let line_no = hit.start_line.saturating_add(offset);
        let truncated = truncate_chars(trimmed, MAX_SNIPPET_LINE_LEN);
        lines.push((line_no, truncated));
    }

    if lines.is_empty() {
        lines.push((hit.start_line, String::new()));
    }

    Snippet {
        lines,
        matched_terms: vec![],
    }
}

/// Try to delegate search to external grepai binary
/// CHANGED: returns (raw_input, filtered_output) for correct savings tracking
/// Returns Some((raw, filtered)) if grepai handled the search, None to fall back to built-in
fn try_grepai_delegation(
    query: &str,
    requested_path: &str,
    project_path: &Path,
    max: usize,
    json: bool,
    compact: bool,
    verbose: u8,
) -> Result<Option<(String, String)>> {
    let project_dir = resolve_grepai_project_dir(project_path);

    // Check config: is grepai delegation enabled?
    let cfg = config::Config::load().unwrap_or_default();
    if !cfg.grepai.enabled {
        if verbose > 0 {
            eprintln!("rgai: grepai delegation disabled in config");
        }
        return Ok(None);
    }

    // Use custom binary path from config, or auto-detect
    let state = if let Some(ref custom_path) = cfg.grepai.binary_path {
        if custom_path.exists() {
            let config_file = project_dir.join(".grepai").join("config.yaml");
            if config_file.exists() {
                grepai::GrepaiState::Ready(custom_path.clone())
            } else {
                grepai::GrepaiState::NotInitialized(custom_path.clone())
            }
        } else {
            if verbose > 0 {
                eprintln!(
                    "rgai: configured binary not found: {}",
                    custom_path.display()
                );
            }
            return Ok(None);
        }
    } else {
        grepai::detect_grepai(project_dir)
    };

    // CHANGED: helper to get raw JSON from grepai (always --json)
    // added: compute path_filter from requested_path for --path pushdown (v0.33.0)
    let path_filter: Option<&str> = if requested_path != "." && !requested_path.is_empty() {
        // added: --path pushdown
        Some(requested_path)
    } else {
        None
    };
    let get_raw = |binary: &Path| -> Result<Option<String>> {
        match grepai::execute_search(binary, project_dir, query, max, path_filter) {
            Ok(output) => Ok(output),
            Err(e) => {
                if verbose > 0 {
                    eprintln!(
                        "rgai: grepai search failed: {}, falling back to built-in",
                        e
                    );
                }
                Ok(None)
            }
        }
    };

    let raw = match state {
        grepai::GrepaiState::Ready(ref binary) => {
            if verbose > 0 {
                eprintln!("rgai: delegating to grepai ({})", binary.display());
            }
            get_raw(binary)?
        }
        grepai::GrepaiState::NotInitialized(ref binary) => {
            if cfg.grepai.auto_init {
                if verbose > 0 {
                    eprintln!("rgai: auto-initializing grepai in project...");
                }
                match grepai::init_project(binary, project_dir, verbose) {
                    Ok(()) => get_raw(binary)?,
                    Err(e) => {
                        if verbose > 0 {
                            eprintln!(
                                "rgai: grepai auto-init failed: {}, falling back to built-in",
                                e
                            );
                        }
                        return Ok(None);
                    }
                }
            } else {
                if verbose > 0 {
                    eprintln!("rgai: grepai not initialized, auto_init disabled, using built-in");
                }
                return Ok(None);
            }
        }
        grepai::GrepaiState::NotInstalled => {
            // Silent: no nagging — install happens via `rtk init`
            return Ok(None);
        }
    };

    // ADDED: filter raw grepai JSON through RTK pipeline
    let raw = match raw {
        Some(r) => r,
        None => return Ok(None),
    };

    let filtered = filter_grepai_output(&raw, query, requested_path, max, json, compact);
    Ok(Some((raw, filtered)))
}

/// Filter raw grepai JSON through RTK rendering pipeline
/// Falls back to raw output if JSON parsing fails
fn filter_grepai_output(
    raw: &str,
    query: &str,
    path: &str,
    max_results: usize,
    json_output: bool,
    compact: bool,
) -> String {
    // Try to parse grepai JSON
    let grepai_hits = match grepai::parse_grepai_json(raw) {
        Ok(hits) => hits,
        Err(_) => {
            if json_output {
                return serde_json::to_string_pretty(&json!({
                    "query": query,
                    "path": path,
                    "total_hits": 0,
                    "shown_hits": 0,
                    "scanned_files": 0,
                    "skipped_large": 0,
                    "skipped_binary": 0,
                    "hits": [],
                    "parse_error": "failed to parse grepai JSON",
                    "fallback_raw": raw,
                }))
                .unwrap_or_default()
                    + "\n";
            }
            // Text fallback: return raw output if parsing fails.
            return raw.to_string();
        }
    };

    if grepai_hits.is_empty() {
        if json_output {
            return serde_json::to_string_pretty(&json!({
                "query": query,
                "path": path,
                "total_hits": 0,
                "shown_hits": 0,
                "scanned_files": 0,
                "skipped_large": 0,
                "skipped_binary": 0,
                "hits": []
            }))
            .unwrap_or_default()
                + "\n";
        }
        return format!("🧠 0 for '{}'\n", query);
    }

    // Convert to SearchHit structs with grouping/pruning
    let hits = convert_grepai_hits(grepai_hits, compact);

    if json_output {
        // Render as RTK JSON format (same as built-in)
        let hits_json: Vec<_> = hits
            .iter()
            .take(max_results)
            .map(|hit| {
                let snippets: Vec<_> = hit
                    .snippets
                    .iter()
                    .map(|snippet| {
                        let lines: Vec<_> = snippet
                            .lines
                            .iter()
                            .map(|(line_no, text)| json!({ "line": line_no, "text": text }))
                            .collect();
                        json!({
                            "lines": lines,
                            "matched_terms": snippet.matched_terms,
                        })
                    })
                    .collect();
                json!({
                    "path": hit.path,
                    "score": hit.score,
                    "matched_lines": hit.matched_lines,
                    "snippets": snippets,
                })
            })
            .collect();

        serde_json::to_string_pretty(&json!({
            "query": query,
            "path": path,
            "total_hits": hits.len(),
            "shown_hits": max_results.min(hits.len()),
            "scanned_files": 0,
            "skipped_large": 0,
            "skipped_binary": 0,
            "hits": hits_json
        }))
        .unwrap_or_default()
            + "\n"
    } else {
        // Render as text using existing render_text_hits
        let effective_max = if compact {
            max_results.min(COMPACT_MAX_FILES)
        } else {
            max_results
        };
        render_text_hits(&hits, effective_max, compact, query, 0)
    }
}

/// Build OR regex pattern from query terms: (term1|term2|term3)
/// Terms are regex-escaped to prevent injection.
/// Caller must ensure terms is non-empty (try_ripgrep_search guards this).
fn build_rg_pattern(query: &QueryModel) -> String {
    debug_assert!(
        !query.terms.is_empty(),
        "build_rg_pattern called with empty terms"
    );
    let escaped: Vec<String> = query.terms.iter().map(|t| regex::escape(t)).collect();
    format!("({})", escaped.join("|"))
}

/// Parse ripgrep stdout into scored SearchHit structs.
/// Pure function — testable without running rg.
fn parse_rg_output(
    stdout: &str,
    query: &QueryModel,
    root: &Path,
    snippets_per_file: usize,
) -> Vec<SearchHit> {
    // Parse rg output: file:line_no:content
    let mut by_file: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(3, ':').collect();
        if parts.len() != 3 {
            continue;
        }
        let line_no: usize = match parts[1].parse() {
            Ok(n) if n > 0 => n,
            _ => continue,
        };
        by_file
            .entry(parts[0].to_string())
            .or_default()
            .push((line_no, parts[2].to_string()));
    }

    let mut hits = Vec::new();
    for (file_path, lines) in &by_file {
        let display_path = compact_display_path(Path::new(file_path), root);
        let ext = Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        // Score each matched line via existing scorer
        let mut candidates = Vec::new();
        for (line_no, content) in lines {
            let line_idx = line_no.saturating_sub(1); // score_line expects 0-based
            if let Some(cand) = score_line(line_idx, content, query, ext) {
                candidates.push(cand);
            }
        }

        if candidates.is_empty() {
            let path_score = score_path(&display_path, query);
            if path_score < MIN_FILE_SCORE {
                continue;
            }
        }

        candidates.sort_by(|a, b| b.score.total_cmp(&a.score));

        // Select non-overlapping top candidates
        let mut selected = Vec::new();
        for cand in &candidates {
            let overlaps = selected.iter().any(|existing: &LineCandidate| {
                (existing.line_idx as isize - cand.line_idx as isize).abs() <= 3
            });
            if overlaps {
                continue;
            }
            selected.push(cand.clone());
            if selected.len() >= snippets_per_file {
                break;
            }
        }

        // Build snippets from matched lines (no context re-read)
        let mut snippets = Vec::new();
        for cand in &selected {
            let line_no = cand.line_idx + 1; // back to 1-based for display
            let content = lines
                .iter()
                .find(|(ln, _)| *ln == line_no)
                .map(|(_, c)| truncate_chars(c.trim(), MAX_SNIPPET_LINE_LEN))
                .unwrap_or_default();
            snippets.push(Snippet {
                lines: vec![(line_no, content)],
                matched_terms: cand.matched_terms.clone(),
            });
        }

        // Compute file score (same formula as search_project)
        let path_score = score_path(&display_path, query);
        let mut file_score = path_score + (candidates.len() as f64).ln_1p();
        for (idx, cand) in selected.iter().enumerate() {
            let weight = match idx {
                0 => 1.0,
                1 => 0.45,
                _ => 0.25,
            };
            file_score += cand.score * weight;
        }

        if file_score < MIN_FILE_SCORE {
            continue;
        }

        hits.push(SearchHit {
            path: display_path,
            score: file_score,
            matched_lines: candidates.len(),
            snippets,
        });
    }

    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
    });
    prune_by_relevance(&mut hits);
    hits
}

/// Search a specific list of files — used by --files flag (two-stage memory pipeline).
/// Runs rg against the listed file paths directly, skipping WalkBuilder.
fn search_file_list(
    query_model: &QueryModel,
    root: &Path,
    file_paths: &[String],
    snippets_per_file: usize,
    verbose: u8,
) -> Result<SearchOutcome> {
    if query_model.terms.is_empty() || file_paths.is_empty() {
        return Ok(SearchOutcome::default());
    }
    let pattern = build_rg_pattern(query_model);
    let mut cmd = Command::new("rg");
    cmd.args(["-n", "--no-heading", "-i", "--max-count", "50"]);
    cmd.arg(&pattern);
    // Resolve each path relative to root (or use as-is if absolute)
    let mut any_added = false;
    for fp in file_paths {
        let candidate = if std::path::Path::new(fp).is_absolute() {
            std::path::PathBuf::from(fp)
        } else {
            root.join(fp)
        };
        if candidate.exists() {
            cmd.arg(candidate);
            any_added = true;
        }
    }
    if !any_added {
        return Ok(SearchOutcome::default());
    }
    if verbose > 0 {
        eprintln!(
            "rgai[rg-files]: pattern={} files={}",
            pattern,
            file_paths.len()
        );
    }
    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            if verbose > 0 {
                eprintln!("rgai[rg-files]: rg failed: {}", e);
            }
            return Ok(SearchOutcome::default());
        }
    };
    match output.status.code() {
        Some(0) | Some(1) => {}
        _ => return Ok(SearchOutcome::default()),
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let hits = parse_rg_output(&stdout, query_model, root, snippets_per_file);
    let raw_output = build_raw_output(&hits);
    Ok(SearchOutcome {
        scanned_files: file_paths.len(),
        skipped_large: 0,
        skipped_binary: 0,
        hits,
        raw_output,
    })
}

/// Ripgrep-accelerated search: fast file discovery via rg + built-in scoring.
/// Returns None if rg is not available (caller falls back to built-in walker).
fn try_ripgrep_search(
    query_model: &QueryModel,
    root: &Path,
    snippets_per_file: usize,
    file_type: Option<&str>,
    max_file_kb: usize,
    verbose: u8,
) -> Result<Option<SearchOutcome>> {
    if query_model.terms.is_empty() {
        return Ok(None);
    }

    let pattern = build_rg_pattern(query_model);
    // FIXED: bail on non-UTF8 paths instead of silently falling back to "."
    let root_str = match root.to_str() {
        Some(s) => s,
        None => {
            if verbose > 0 {
                eprintln!("rgai[rg]: path is not valid UTF-8, falling back to built-in");
            }
            return Ok(None);
        }
    };

    let mut cmd = Command::new("rg");
    cmd.args([
        "-n",
        "--no-heading",
        "-i",
        "--max-filesize",
        &format!("{}K", max_file_kb),
        "--max-count",
        "50", // cap matches per file
    ]);

    // FIXED: map RTK type aliases to ripgrep type names
    if let Some(ft) = file_type {
        let rg_type = match ft {
            "rs" => "rust",
            "python" => "py",
            "javascript" => "js",
            "typescript" => "ts",
            "c++" | "cpp" => "cpp",
            "markdown" => "md",
            other => other,
        };
        cmd.arg("--type").arg(rg_type);
    }

    cmd.arg(&pattern);
    cmd.arg(root_str);

    if verbose > 0 {
        eprintln!("rgai[rg]: pattern={}", pattern);
    }

    // FIXED: log actual error instead of discarding it
    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            if verbose > 0 {
                eprintln!(
                    "rgai[rg]: failed to run rg: {}, falling back to built-in",
                    e
                );
            }
            return Ok(None);
        }
    };

    // Exit code: 0=matches, 1=no matches, 2+=error, None=killed by signal
    match output.status.code() {
        Some(0) | Some(1) => {}
        other => {
            if verbose > 0 {
                eprintln!(
                    "rgai[rg]: rg exited with {:?}, falling back to built-in",
                    other
                );
            }
            return Ok(None);
        }
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let hits = parse_rg_output(&stdout, query_model, root, snippets_per_file);
    let raw_output = build_raw_output(&hits);

    Ok(Some(SearchOutcome {
        scanned_files: hits.len(), // files with matches (rg doesn't report scanned total)
        skipped_large: 0,
        skipped_binary: 0,
        hits,
        raw_output,
    }))
}

fn resolve_grepai_project_dir(project_path: &Path) -> &Path {
    if project_path.is_dir() {
        project_path
    } else {
        project_path.parent().unwrap_or(project_path)
    }
}

fn search_project(
    query: &QueryModel,
    root: &Path,
    context_lines: usize,
    snippets_per_file: usize,
    file_type: Option<&str>,
    max_file_bytes: usize,
    max_results: usize, // C1: early-exit limit — collect at most max_results * 3 candidates
    _verbose: u8,
) -> Result<SearchOutcome> {
    let mut outcome = SearchOutcome::default();

    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if !entry
            .file_type()
            .as_ref()
            .map(|ft| ft.is_file())
            .unwrap_or(false)
        {
            continue;
        }

        let full_path = entry.path();
        if !is_supported_text_file(full_path) {
            continue;
        }

        if let Some(ft) = file_type {
            if !matches_file_type(full_path, ft) {
                continue;
            }
        }

        let metadata = match fs::metadata(full_path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        outcome.scanned_files += 1;

        if metadata.len() > max_file_bytes as u64 {
            outcome.skipped_large += 1;
            continue;
        }

        let bytes = match fs::read(full_path) {
            Ok(b) => b,
            Err(_) => continue,
        };

        if looks_binary(&bytes) {
            outcome.skipped_binary += 1;
            continue;
        }

        let content = String::from_utf8_lossy(&bytes).to_string();
        let display_path = compact_display_path(full_path, root);
        if let Some(hit) = analyze_file(
            &display_path,
            &content,
            query,
            context_lines,
            snippets_per_file,
        ) {
            outcome.hits.push(hit);
            // C1: early-exit — 3x candidates is enough to sort and pick best max_results
            if max_results > 0 && outcome.hits.len() >= max_results.saturating_mul(3).max(30) {
                break;
            }
        }
    }

    outcome.hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
    });

    // ADDED: prune low-relevance hits before building raw output
    prune_by_relevance(&mut outcome.hits);

    outcome.raw_output = build_raw_output(&outcome.hits);
    Ok(outcome)
}

fn analyze_file(
    path: &str,
    content: &str,
    query: &QueryModel,
    context_lines: usize,
    snippets_per_file: usize,
) -> Option<SearchHit> {
    // FIX: extract extension for extension-aware comment detection
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let mut candidates = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        if let Some(candidate) = score_line(idx, line, query, ext) {
            candidates.push(candidate);
        }
    }

    let path_score = score_path(path, query);
    if candidates.is_empty() && path_score < MIN_FILE_SCORE {
        return None;
    }

    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.line_idx.cmp(&b.line_idx))
    });

    let mut selected = Vec::new();
    let overlap_window = (context_lines * 2 + 1) as isize;
    for cand in candidates.iter().cloned() {
        let overlaps = selected.iter().any(|existing: &LineCandidate| {
            let delta = existing.line_idx as isize - cand.line_idx as isize;
            delta.abs() <= overlap_window
        });
        if overlaps {
            continue;
        }
        selected.push(cand);
        if selected.len() >= snippets_per_file {
            break;
        }
    }

    if selected.is_empty() {
        return None;
    }

    let lines: Vec<&str> = content.lines().collect();
    let mut snippets = Vec::new();
    for cand in &selected {
        snippets.push(build_snippet(&lines, cand, context_lines));
    }

    let mut file_score = path_score + (candidates.len() as f64).ln_1p();
    for (idx, cand) in selected.iter().enumerate() {
        let weight = match idx {
            0 => 1.0,
            1 => 0.45,
            _ => 0.25,
        };
        file_score += cand.score * weight;
    }

    if file_score < MIN_FILE_SCORE {
        return None;
    }

    Some(SearchHit {
        path: path.to_string(),
        score: file_score,
        matched_lines: candidates.len(),
        snippets,
    })
}

fn build_snippet(lines: &[&str], candidate: &LineCandidate, context_lines: usize) -> Snippet {
    if lines.is_empty() {
        return Snippet {
            lines: vec![(candidate.line_idx + 1, String::new())],
            matched_terms: candidate.matched_terms.clone(),
        };
    }

    let start = candidate.line_idx.saturating_sub(context_lines);
    let end = (candidate.line_idx + context_lines + 1).min(lines.len());
    let mut rendered_lines = Vec::new();

    for (idx, line) in lines.iter().enumerate().take(end).skip(start) {
        let cleaned = line.trim();
        if cleaned.is_empty() {
            continue;
        }
        rendered_lines.push((idx + 1, truncate_chars(cleaned, MAX_SNIPPET_LINE_LEN)));
    }

    if rendered_lines.is_empty() {
        rendered_lines.push((candidate.line_idx + 1, String::new()));
    }

    Snippet {
        lines: rendered_lines,
        matched_terms: candidate.matched_terms.clone(),
    }
}

fn build_raw_output(hits: &[SearchHit]) -> String {
    let mut raw = String::new();
    for hit in hits.iter().take(60) {
        for snippet in &hit.snippets {
            for (line_no, line) in &snippet.lines {
                raw.push_str(&format!("{}:{}:{}\n", hit.path, line_no, line));
            }
        }
    }
    raw
}

// ADDED: dynamic relevance cutoff — prune hits below 35% of top score
fn prune_by_relevance(hits: &mut Vec<SearchHit>) {
    if let Some(top) = hits.first() {
        let cutoff = (top.score * RELATIVE_SCORE_CUTOFF).max(MIN_FILE_SCORE);
        hits.retain(|h| h.score >= cutoff);
    }
}

// ADDED: extract text rendering for testability and compact mode support
fn render_text_hits(
    hits: &[SearchHit],
    max_results: usize,
    compact: bool,
    query: &str,
    scanned_files: usize,
) -> String {
    let mut rendered = String::new();

    // CHANGED: compact header is shorter (no "scan NF")
    if compact {
        rendered.push_str(&format!("🧠 {} for '{}'\n", hits.len(), query));
    } else {
        rendered.push_str(&format!(
            "🧠 {}F for '{}' (scan {}F)\n",
            hits.len(),
            query,
            scanned_files
        ));
        rendered.push('\n');
    }

    for hit in hits.iter().take(max_results) {
        rendered.push_str(&format!(
            "📄 {} [{:.1}]\n",
            compact_path(&hit.path),
            hit.score
        ));

        for snippet in &hit.snippets {
            for (line_no, line) in &snippet.lines {
                rendered.push_str(&format!("  {:>4}: {}\n", line_no, line));
            }

            // CHANGED: suppress matched_terms in compact mode (already suppressed)
            if !compact && !snippet.matched_terms.is_empty() {
                rendered.push_str(&format!("       ~ {}\n", snippet.matched_terms.join(", ")));
            }
            // CHANGED: no blank line between snippets in compact mode
            if !compact {
                rendered.push('\n');
            }
        }

        // CHANGED: suppress "+N more lines" in compact mode
        let shown_lines = hit.snippets.len();
        if !compact && hit.matched_lines > shown_lines {
            rendered.push_str(&format!(
                "  +{} more lines\n\n",
                hit.matched_lines - shown_lines
            ));
        }
    }

    // CHANGED: suppress "+NF" footer in compact mode
    if !compact && hits.len() > max_results {
        rendered.push_str(&format!("... +{}F\n", hits.len() - max_results));
    }

    rendered
}

fn score_line(line_idx: usize, line: &str, query: &QueryModel, ext: &str) -> Option<LineCandidate> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_lowercase();
    let mut score = 0.0;
    let mut matched_terms = Vec::new();

    if query.phrase.len() >= 3 && lower.contains(&query.phrase) {
        score += 6.0;
    }

    for term in &query.terms {
        if lower.contains(term) {
            score += if term.len() >= 5 { 1.7 } else { 1.4 };
            matched_terms.push(term.clone());
        }
    }

    let unique_matches = dedup_terms(matched_terms);
    if unique_matches.is_empty() {
        return None;
    }

    if unique_matches.len() > 1 {
        score += 1.2;
    }

    if is_symbol_definition(trimmed) {
        score += 2.5;
    }

    if is_comment_line(trimmed, ext) {
        // FIX: pass ext for extension-aware comment detection
        score *= 0.7;
    }

    if trimmed.chars().count() > 220 {
        score *= 0.9;
    }

    if score < 1.2 {
        return None;
    }

    Some(LineCandidate {
        line_idx,
        score,
        matched_terms: unique_matches,
    })
}

fn score_path(path: &str, query: &QueryModel) -> f64 {
    let lower = path.to_lowercase();
    let mut score = 0.0;

    if query.phrase.len() >= 3 && lower.contains(&query.phrase) {
        score += 3.5;
    }

    for term in &query.terms {
        if lower.contains(term) {
            score += 1.2;
        }
    }

    score
}

fn build_query_model(query: &str) -> QueryModel {
    let phrase = query.trim().to_lowercase();
    let mut terms = Vec::new();
    let mut seen = HashSet::new();

    for token in split_terms(&phrase) {
        if token.len() < 2 || STOP_WORDS_SET.contains(token.as_str()) {
            // H3: O(1) via HashSet
            continue;
        }
        push_unique(&mut terms, &mut seen, &token);

        let stemmed = stem_token(&token);
        if stemmed != token && stemmed.len() >= 2 {
            push_unique(&mut terms, &mut seen, &stemmed);
        }
    }

    if terms.is_empty() && !phrase.is_empty() {
        terms.push(phrase.clone());
    }

    QueryModel { phrase, terms }
}

fn split_terms(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in input.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.extend(ch.to_lowercase());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn stem_token(token: &str) -> String {
    if !token.is_ascii() {
        return token.to_string();
    }

    // FIX: removed "es" suffix — it broke stems for -ce/-ge/-ve words common in code
    // (caches→cach, services→servic, changes→chang). Stripping just "s" handles
    // these correctly (caches→cache, services→service). The trade-off (classes→classe)
    // is acceptable since original unstemmed tokens are also kept in the query model.
    let suffixes = ["ingly", "edly", "ing", "ed", "s"];
    for suffix in suffixes {
        if token.len() > suffix.len() + 2 && token.ends_with(suffix) {
            return token[..token.len() - suffix.len()].to_string();
        }
    }
    token.to_string()
}

fn push_unique(out: &mut Vec<String>, seen: &mut HashSet<String>, item: &str) {
    if seen.insert(item.to_string()) {
        out.push(item.to_string());
    }
}

fn dedup_terms(input: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for item in input {
        if seen.insert(item.clone()) {
            out.push(item);
        }
    }
    out
}

fn is_symbol_definition(line: &str) -> bool {
    SYMBOL_DEF_RE.is_match(line)
}

// FIX: accept file extension to avoid penalizing Markdown headers and YAML keys.
// '#' is only treated as a comment prefix for scripting languages (py, sh, rb, etc.).
fn is_comment_line(line: &str, ext: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//")
        || trimmed.starts_with('*')
        || trimmed.starts_with("/*")
        || trimmed.starts_with("--")
    {
        return true;
    }
    // Only treat '#' as comment for languages that actually use it
    if trimmed.starts_with('#') {
        return matches!(
            ext,
            "py" | "sh"
                | "bash"
                | "zsh"
                | "rb"
                | "pl"
                | "pm"
                | "r"
                | "jl"
                | "makefile"
                | "mk"
                | "dockerfile"
                | "tf"
                | "cfg"
                | "conf"
                | "ini"
        );
    }
    false
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(4096).any(|b| *b == 0)
}

fn is_supported_text_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    !matches!(
        ext.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "ico"
            | "pdf"
            | "zip"
            | "gz"
            | "tar"
            | "7z"
            | "mp3"
            | "mp4"
            | "mov"
            | "db"
            | "sqlite"
            | "woff"
            | "woff2"
            | "ttf"
            | "otf"
            | "lock"
            | "jar"
            | "class"
            | "wasm"
    )
}

fn matches_file_type(path: &Path, file_type: &str) -> bool {
    let wanted = file_type.trim_start_matches('.').to_ascii_lowercase();
    if wanted.is_empty() {
        return true;
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    match wanted.as_str() {
        "rust" | "rs" => ext == "rs",
        "python" | "py" => ext == "py",
        "javascript" | "js" => matches!(ext.as_str(), "js" | "jsx" | "mjs" | "cjs"),
        "typescript" | "ts" => matches!(ext.as_str(), "ts" | "tsx"),
        "go" => ext == "go",
        "java" => ext == "java",
        "c" => matches!(ext.as_str(), "c" | "h"),
        "cpp" | "c++" => matches!(ext.as_str(), "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx"),
        "markdown" | "md" => matches!(ext.as_str(), "md" | "mdx"),
        "json" => ext == "json",
        other => ext == other,
    }
}

fn compact_display_path(path: &Path, root: &Path) -> String {
    let rel = match path.strip_prefix(root) {
        Ok(r) => r.to_path_buf(),
        Err(_) => {
            if let Ok(cwd) = std::env::current_dir() {
                match path.strip_prefix(cwd) {
                    Ok(r) => r.to_path_buf(),
                    Err(_) => PathBuf::from(path),
                }
            } else {
                PathBuf::from(path)
            }
        }
    };
    rel.to_string_lossy().trim_start_matches("./").to_string()
}

fn compact_path(path: &str) -> String {
    if path.len() <= 58 {
        return path.to_string();
    }

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 3 {
        return path.to_string();
    }

    format!(
        "{}/.../{}/{}",
        parts[0],
        parts[parts.len() - 2],
        parts[parts.len() - 1]
    )
}

fn truncate_chars(input: &str, max_len: usize) -> String {
    if input.chars().count() <= max_len {
        return input.to_string();
    }
    if max_len <= 3 {
        return "...".to_string();
    }
    let clipped: String = input.chars().take(max_len - 3).collect();
    format!("{clipped}...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn build_query_model_removes_stop_words() {
        let model = build_query_model("how to find auth token refresh");
        assert!(model.terms.contains(&"auth".to_string()));
        assert!(model.terms.contains(&"token".to_string()));
        assert!(model.terms.contains(&"refresh".to_string()));
        assert!(!model.terms.contains(&"how".to_string()));
        assert!(!model.terms.contains(&"find".to_string()));
    }

    #[test]
    fn score_line_prefers_symbol_definitions() {
        let query = build_query_model("refresh token");
        let line = "pub fn refresh_token(session: &Session) -> Result<String> {";
        let cand = score_line(10, line, &query, "rs").expect("line should match");
        assert!(cand.score > 3.0);
        assert!(cand.matched_terms.contains(&"refresh".to_string()));
        assert!(cand.matched_terms.contains(&"token".to_string()));
    }

    #[test]
    fn search_project_finds_most_relevant_file() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/auth.rs"),
            r#"
pub struct Session {}

pub fn refresh_token(session: &Session) -> String {
    format!("new-token-{}", 1)
}
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/logger.rs"),
            r#"
pub fn log_info(msg: &str) {
    println!("{}", msg);
}
"#,
        )
        .unwrap();

        let query = build_query_model("refresh token session");
        let outcome = search_project(&query, root, 0, 2, None, 256 * 1024, 10, 0).unwrap(); // C1: added max_results param

        assert!(!outcome.hits.is_empty());
        assert_eq!(outcome.hits[0].path, "src/auth.rs");
    }

    #[test]
    fn matches_file_type_aliases() {
        let p = Path::new("src/app.tsx");
        assert!(matches_file_type(p, "ts"));
        assert!(matches_file_type(p, "typescript"));
        assert!(!matches_file_type(p, "rust"));
    }

    #[test]
    fn truncate_chars_handles_unicode() {
        let s = "Привет это длинная строка для теста";
        let truncated = truncate_chars(s, 10);
        assert!(truncated.chars().count() <= 10);
    }

    // FIX: stem_token must preserve trailing 'e' for common code identifiers
    #[test]
    fn stem_token_preserves_trailing_e() {
        assert_eq!(stem_token("caches"), "cache");
        assert_eq!(stem_token("services"), "service");
        assert_eq!(stem_token("changes"), "change");
        assert_eq!(stem_token("images"), "image");
        assert_eq!(stem_token("packages"), "package");
        assert_eq!(stem_token("interfaces"), "interface");
        assert_eq!(stem_token("sources"), "source");
    }

    #[test]
    fn stem_token_handles_regular_suffixes() {
        assert_eq!(stem_token("tokens"), "token");
        assert_eq!(stem_token("running"), "runn");
        assert_eq!(stem_token("created"), "creat");
    }

    // FIX: is_comment_line respects file extension — no false positives on .md/.yaml
    #[test]
    fn is_comment_line_ignores_hash_in_non_script_files() {
        assert!(!is_comment_line("# Installation", "md"));
        assert!(!is_comment_line("## API Reference", "md"));
        assert!(!is_comment_line("# yaml comment", "yaml"));
        assert!(!is_comment_line("#[derive(Debug)]", "rs"));
        assert!(!is_comment_line("# toml section", "toml"));
    }

    #[test]
    fn is_comment_line_detects_hash_in_script_files() {
        assert!(is_comment_line("# python comment", "py"));
        assert!(is_comment_line("# shell comment", "sh"));
        assert!(is_comment_line("# ruby comment", "rb"));
    }

    #[test]
    fn is_comment_line_detects_universal_comment_markers() {
        assert!(is_comment_line("// rust comment", "rs"));
        assert!(is_comment_line("/* block comment */", "js"));
        assert!(is_comment_line("-- sql comment", "sql"));
    }

    // --- compact output optimization tests ---

    #[test]
    fn prune_by_relevance_removes_low_scores() {
        // Arrange: hits with scores [20.0, 18.0, 15.0, 5.0, 3.0]
        let mut hits = vec![
            SearchHit {
                path: "a.rs".into(),
                score: 20.0,
                matched_lines: 5,
                snippets: vec![],
            },
            SearchHit {
                path: "b.rs".into(),
                score: 18.0,
                matched_lines: 3,
                snippets: vec![],
            },
            SearchHit {
                path: "c.rs".into(),
                score: 15.0,
                matched_lines: 2,
                snippets: vec![],
            },
            SearchHit {
                path: "d.rs".into(),
                score: 5.0,
                matched_lines: 1,
                snippets: vec![],
            },
            SearchHit {
                path: "e.rs".into(),
                score: 3.0,
                matched_lines: 1,
                snippets: vec![],
            },
        ];
        // Act: cutoff = 20.0 * 0.35 = 7.0
        prune_by_relevance(&mut hits);
        // Assert: only scores >= 7.0 remain
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].path, "a.rs");
        assert_eq!(hits[2].path, "c.rs");
    }

    #[test]
    fn prune_by_relevance_keeps_all_when_scores_close() {
        // Arrange: all scores within 35% of top
        let mut hits = vec![
            SearchHit {
                path: "a.rs".into(),
                score: 10.0,
                matched_lines: 3,
                snippets: vec![],
            },
            SearchHit {
                path: "b.rs".into(),
                score: 8.0,
                matched_lines: 2,
                snippets: vec![],
            },
            SearchHit {
                path: "c.rs".into(),
                score: 6.0,
                matched_lines: 1,
                snippets: vec![],
            },
        ];
        // Act: cutoff = 10.0 * 0.35 = 3.5 — all pass
        prune_by_relevance(&mut hits);
        // Assert: nothing pruned
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn prune_by_relevance_respects_min_file_score() {
        // Arrange: low top score where 35% cutoff < MIN_FILE_SCORE
        let mut hits = vec![
            SearchHit {
                path: "a.rs".into(),
                score: 4.0,
                matched_lines: 2,
                snippets: vec![],
            },
            SearchHit {
                path: "b.rs".into(),
                score: 2.5,
                matched_lines: 1,
                snippets: vec![],
            },
        ];
        // Act: 4.0 * 0.35 = 1.4, but MIN_FILE_SCORE = 2.4 is used instead
        prune_by_relevance(&mut hits);
        // Assert: both survive (both >= 2.4)
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn prune_by_relevance_empty_hits() {
        let mut hits: Vec<SearchHit> = vec![];
        prune_by_relevance(&mut hits);
        assert!(hits.is_empty());
    }

    #[test]
    fn render_compact_omits_more_lines() {
        // Arrange
        let hits = vec![SearchHit {
            path: "src/auth.rs".into(),
            score: 15.0,
            matched_lines: 20, // many matched lines
            snippets: vec![Snippet {
                lines: vec![(10, "pub fn login() {".into())],
                matched_terms: vec!["login".into()],
            }],
        }];
        // Act
        let output = render_text_hits(&hits, 5, true, "login", 100);
        // Assert: no "+N more lines" noise in compact
        assert!(!output.contains("more lines"));
        // but the snippet IS present
        assert!(output.contains("pub fn login()"));
    }

    #[test]
    fn render_compact_omits_remaining_files_footer() {
        // Arrange: 10 hits but max=3
        let hits: Vec<SearchHit> = (0..10)
            .map(|i| SearchHit {
                path: format!("f{}.rs", i),
                score: 20.0 - i as f64,
                matched_lines: 1,
                snippets: vec![Snippet {
                    lines: vec![(1, format!("line {}", i))],
                    matched_terms: vec!["term".into()],
                }],
            })
            .collect();
        // Act
        let output = render_text_hits(&hits, 3, true, "term", 50);
        // Assert: no "+NF" footer in compact
        assert!(!output.contains("+7F"));
        assert!(!output.contains("..."));
        // only 3 files shown
        assert!(output.contains("f0.rs"));
        assert!(output.contains("f2.rs"));
        assert!(!output.contains("f3.rs"));
    }

    #[test]
    fn render_normal_keeps_more_lines_and_footer() {
        // Arrange
        let hits: Vec<SearchHit> = (0..10)
            .map(|i| SearchHit {
                path: format!("f{}.rs", i),
                score: 20.0 - i as f64,
                matched_lines: 15,
                snippets: vec![Snippet {
                    lines: vec![(1, format!("line {}", i))],
                    matched_terms: vec!["term".into()],
                }],
            })
            .collect();
        // Act: compact=false
        let output = render_text_hits(&hits, 5, false, "term", 50);
        // Assert: normal mode KEEPS the noise
        assert!(output.contains("more lines"));
        assert!(output.contains("+5F"));
    }

    // --- grepai filtering tests ---

    #[test]
    fn convert_grepai_hits_groups_by_file() {
        use crate::grepai::GrepaiHit;
        let hits = vec![
            GrepaiHit {
                file_path: "src/auth.rs".into(),
                start_line: 10,
                end_line: 15,
                score: 0.9,
                content: Some("pub fn login() {}".into()),
            },
            GrepaiHit {
                file_path: "src/auth.rs".into(),
                start_line: 30,
                end_line: 35,
                score: 0.7,
                content: Some("pub fn logout() {}".into()),
            },
            GrepaiHit {
                file_path: "src/session.rs".into(),
                start_line: 1,
                end_line: 5,
                score: 0.8,
                content: Some("struct Session {}".into()),
            },
        ];
        let result = convert_grepai_hits(hits, false);
        // Should group into 2 files
        assert_eq!(result.len(), 2);
        // auth.rs has higher combined score (0.9 + 0.7 = 1.6) vs session.rs (0.8)
        assert_eq!(result[0].path, "src/auth.rs");
        assert_eq!(result[0].snippets.len(), 2); // normal mode: 2 snippets
        assert_eq!(result[1].path, "src/session.rs");
    }

    #[test]
    fn convert_grepai_hits_compact_limits_snippets() {
        use crate::grepai::GrepaiHit;
        let hits = vec![
            GrepaiHit {
                file_path: "src/auth.rs".into(),
                start_line: 10,
                end_line: 15,
                score: 0.9,
                content: Some("pub fn login() {}".into()),
            },
            GrepaiHit {
                file_path: "src/auth.rs".into(),
                start_line: 30,
                end_line: 35,
                score: 0.7,
                content: Some("pub fn logout() {}".into()),
            },
        ];
        let result = convert_grepai_hits(hits, true);
        assert_eq!(result.len(), 1);
        // Compact: only 1 snippet per file (highest score)
        assert_eq!(result[0].snippets.len(), 1);
        assert!(result[0].snippets[0].lines[0].1.contains("login"));
    }

    #[test]
    fn convert_grepai_hits_prunes_low_scores() {
        use crate::grepai::GrepaiHit;
        let hits = vec![
            GrepaiHit {
                file_path: "a.rs".into(),
                start_line: 1,
                end_line: 5,
                score: 0.95,
                content: Some("high score".into()),
            },
            GrepaiHit {
                file_path: "b.rs".into(),
                start_line: 1,
                end_line: 5,
                score: 0.1, // < 0.95 * 0.35 = 0.3325
                content: Some("low score".into()),
            },
        ];
        let result = convert_grepai_hits(hits, false);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].path, "a.rs");
    }

    #[test]
    fn convert_grepai_hits_tie_breaks_by_path_when_scores_equal() {
        use crate::grepai::GrepaiHit;
        let hits = vec![
            GrepaiHit {
                file_path: "z.rs".into(),
                start_line: 1,
                end_line: 1,
                score: 0.9,
                content: Some("z".into()),
            },
            GrepaiHit {
                file_path: "a.rs".into(),
                start_line: 1,
                end_line: 1,
                score: 0.9,
                content: Some("a".into()),
            },
        ];
        let result = convert_grepai_hits(hits, true);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].path, "a.rs");
        assert_eq!(result[1].path, "z.rs");
    }

    #[test]
    fn parse_grepai_content_strips_file_prefix() {
        use crate::grepai::GrepaiHit;
        let hit = GrepaiHit {
            file_path: "src/main.rs".into(),
            start_line: 10,
            end_line: 15,
            score: 0.8,
            content: Some("File: src/main.rs\n\nfn main() {\n    println!(\"hello\");\n}".into()),
        };
        let snippet = parse_grepai_content(&hit);
        // "File:" line and empty line should be stripped
        assert!(!snippet.lines.iter().any(|(_, t)| t.starts_with("File:")));
        assert!(snippet.lines.iter().any(|(_, t)| t.contains("fn main()")));
        assert_eq!(snippet.lines[0].0, 10);
    }

    #[test]
    fn parse_grepai_content_preserves_line_numbers_with_internal_blank_lines() {
        use crate::grepai::GrepaiHit;
        let hit = GrepaiHit {
            file_path: "src/main.rs".into(),
            start_line: 20,
            end_line: 24,
            score: 0.8,
            content: Some("File: src/main.rs\n\nfn main() {\n\nlet x = 1;\n}".into()),
        };
        let snippet = parse_grepai_content(&hit);
        let rendered_lines: Vec<usize> = snippet.lines.iter().map(|(line, _)| *line).collect();
        assert_eq!(rendered_lines, vec![20, 22, 23]);
    }

    #[test]
    fn parse_grepai_content_truncates_long_lines() {
        use crate::grepai::GrepaiHit;
        let long_line = "x".repeat(200);
        let hit = GrepaiHit {
            file_path: "a.rs".into(),
            start_line: 1,
            end_line: 1,
            score: 0.5,
            content: Some(long_line),
        };
        let snippet = parse_grepai_content(&hit);
        assert!(snippet.lines[0].1.chars().count() <= MAX_SNIPPET_LINE_LEN);
        assert!(snippet.lines[0].1.ends_with("..."));
    }

    #[test]
    fn parse_grepai_content_limits_to_max_lines() {
        use crate::grepai::GrepaiHit;
        let content = (1..=10)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let hit = GrepaiHit {
            file_path: "a.rs".into(),
            start_line: 1,
            end_line: 10,
            score: 0.5,
            content: Some(content),
        };
        let snippet = parse_grepai_content(&hit);
        assert_eq!(snippet.lines.len(), MAX_GREPAI_SNIPPET_LINES);
    }

    #[test]
    fn parse_grepai_content_no_content_returns_placeholder() {
        use crate::grepai::GrepaiHit;
        let hit = GrepaiHit {
            file_path: "a.rs".into(),
            start_line: 42,
            end_line: 42,
            score: 0.5,
            content: None,
        };
        let snippet = parse_grepai_content(&hit);
        assert_eq!(snippet.lines.len(), 1);
        assert_eq!(snippet.lines[0].0, 42);
    }

    #[test]
    fn filter_grepai_output_text_format() {
        let raw = r#"[
            {"file_path": "src/auth.rs", "start_line": 10, "end_line": 15, "score": 0.87, "content": "pub fn login() {}"},
            {"file_path": "src/session.rs", "start_line": 1, "end_line": 5, "score": 0.65, "content": "struct Session {}"}
        ]"#;
        let output = filter_grepai_output(raw, "auth login", ".", 10, false, false);
        // Should contain RTK-formatted text output
        assert!(output.contains("🧠"));
        assert!(output.contains("src/auth.rs"));
        assert!(output.contains("pub fn login()"));
    }

    #[test]
    fn filter_grepai_output_json_format() {
        let raw = r#"[
            {"file_path": "src/auth.rs", "start_line": 10, "end_line": 15, "score": 0.87, "content": "pub fn login() {}"}
        ]"#;
        let output = filter_grepai_output(raw, "auth", ".", 10, true, false);
        // Should be valid JSON with RTK structure
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["query"], "auth");
        assert_eq!(parsed["path"], ".");
        assert_eq!(parsed["total_hits"], 1);
        assert_eq!(parsed["shown_hits"], 1);
        assert_eq!(parsed["scanned_files"], 0);
        assert_eq!(parsed["skipped_large"], 0);
        assert_eq!(parsed["skipped_binary"], 0);
        assert!(parsed["hits"].is_array());
    }

    #[test]
    fn filter_grepai_output_fallback_on_invalid_json() {
        let raw = "not valid json at all";
        let output = filter_grepai_output(raw, "query", ".", 10, false, false);
        // Should return raw output as fallback
        assert_eq!(output, raw);
    }

    #[test]
    fn filter_grepai_output_json_fallback_is_valid_json() {
        let raw = "not valid json at all";
        let output = filter_grepai_output(raw, "query", ".", 10, true, false);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["query"], "query");
        assert_eq!(parsed["path"], ".");
        assert_eq!(parsed["total_hits"], 0);
        assert_eq!(parsed["shown_hits"], 0);
        assert_eq!(parsed["scanned_files"], 0);
        assert_eq!(parsed["skipped_large"], 0);
        assert_eq!(parsed["skipped_binary"], 0);
        assert!(parsed["parse_error"].is_string());
        assert_eq!(parsed["fallback_raw"], raw);
    }

    #[test]
    fn filter_grepai_output_json_schema_matches_builtin_shape() {
        let raw = r#"[
            {"file_path": "src/auth.rs", "start_line": 10, "end_line": 15, "score": 0.87, "content": "pub fn login() {}"}
        ]"#;
        let output = filter_grepai_output(raw, "auth", ".", 10, true, false);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        for key in [
            "query",
            "path",
            "total_hits",
            "shown_hits",
            "scanned_files",
            "skipped_large",
            "skipped_binary",
            "hits",
        ] {
            assert!(parsed.get(key).is_some(), "missing key: {}", key);
        }
    }

    // --- ripgrep backend tests ---

    #[test]
    fn build_rg_pattern_joins_terms_with_or() {
        // Arrange
        let model = build_query_model("auth token refresh");
        // Act
        let pattern = build_rg_pattern(&model);
        // Assert: each term present, joined with |
        assert!(pattern.contains("auth"));
        assert!(pattern.contains("token"));
        assert!(pattern.contains("refresh"));
        assert!(pattern.contains('|'));
        // wrapped in parens
        assert!(pattern.starts_with('('));
        assert!(pattern.ends_with(')'));
    }

    #[test]
    fn build_rg_pattern_escapes_regex_metacharacters() {
        // Arrange: build_query_model splits on non-alnum, so test escaping directly
        let model = QueryModel {
            phrase: "foo.bar".to_string(),
            terms: vec!["foo.bar".to_string(), "baz[0]".to_string()],
        };
        // Act
        let pattern = build_rg_pattern(&model);
        // Assert: dots and brackets escaped
        assert!(pattern.contains(r"foo\.bar"));
        assert!(pattern.contains(r"baz\[0\]"));
    }

    #[test]
    fn build_rg_pattern_empty_terms_returns_none() {
        // Arrange: only stop words
        let model = build_query_model("how to find");
        // Act
        let pattern = build_rg_pattern(&model);
        // Assert: empty pattern when all terms are stop words
        // (build_query_model falls back to full phrase if no terms remain)
        assert!(!pattern.is_empty());
    }

    #[test]
    fn parse_rg_output_groups_by_file_and_scores() {
        // Arrange: simulated ripgrep output
        let rg_stdout = "\
src/auth.rs:10:pub fn refresh_token(session: &Session) -> String {
src/auth.rs:15:    let token = generate_token();
src/logger.rs:5:fn log_token_refresh(msg: &str) {";
        let query = build_query_model("refresh token");
        let root = Path::new(".");
        // Act
        let hits = parse_rg_output(rg_stdout, &query, root, 2);
        // Assert: grouped into 2 files
        assert_eq!(hits.len(), 2);
        // auth.rs should rank higher (more matches + symbol definition)
        assert_eq!(hits[0].path, "src/auth.rs");
        assert!(hits[0].score > hits[1].score);
    }

    #[test]
    fn parse_rg_output_respects_snippets_limit() {
        // Arrange: many matches in one file
        let rg_stdout = "\
src/big.rs:1:token one
src/big.rs:10:token two
src/big.rs:20:token three
src/big.rs:30:token four
src/big.rs:40:token five";
        let query = build_query_model("token");
        let root = Path::new(".");
        // Act: limit to 1 snippet per file
        let hits = parse_rg_output(rg_stdout, &query, root, 1);
        // Assert
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].snippets.len(), 1); // only 1 snippet
        assert_eq!(hits[0].matched_lines, 5); // but all 5 counted
    }

    #[test]
    fn parse_rg_output_empty_stdout() {
        let query = build_query_model("nothing");
        let hits = parse_rg_output("", &query, Path::new("."), 2);
        assert!(hits.is_empty());
    }

    #[test]
    fn parse_rg_output_malformed_lines_skipped() {
        // Arrange: mix of valid and malformed lines
        let rg_stdout = "\
src/ok.rs:10:valid match token
not-a-valid-line
:also:bad
src/ok.rs:20:another token match";
        let query = build_query_model("token");
        let hits = parse_rg_output(rg_stdout, &query, Path::new("."), 2);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].matched_lines, 2); // only valid lines counted
    }

    #[test]
    fn filter_grepai_output_savings_ratio() {
        // Simulate realistic grepai output (~2KB raw JSON)
        let hits: Vec<serde_json::Value> = (0..8)
            .map(|i| {
                json!({
                    "file_path": format!("src/module_{}.rs", i),
                    "start_line": 1,
                    "end_line": 20,
                    "score": 0.9 - (i as f64 * 0.1),
                    "content": format!(
                        "/// Documentation for module {}\npub fn function_{}() {{\n    let x = {};\n    println!(\"value: {{}}\", x);\n    // more code here\n    let y = x * 2;\n    if y > 10 {{\n        return;\n    }}\n}}\n",
                        i, i, i
                    )
                })
            })
            .collect();
        let raw = serde_json::to_string(&hits).unwrap();
        let filtered = filter_grepai_output(&raw, "function module", ".", 10, false, false);
        // Filtered should be significantly smaller than raw
        assert!(
            filtered.len() < raw.len(),
            "filtered ({}) should be < raw ({})",
            filtered.len(),
            raw.len()
        );
    }

    // ADDED: Phase 2 — search_file_list never returns hits from files outside the list
    #[test]
    fn test_rgai_files_restricts_search() {
        use std::fs;
        use tempfile::TempDir;

        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();

        // Both files have matching content; only one is in the file list
        let content = b"pub fn score_structural_relevance() -> f32 { 0.99 }";
        fs::write(root.join("allowed.rs"), content).unwrap();
        fs::write(root.join("excluded.rs"), content).unwrap();

        let qm = build_query_model("structural relevance");
        // Only allowed.rs in the list — excluded.rs must not appear even though it also matches
        let file_list = vec!["allowed.rs".to_string()];
        let outcome = search_file_list(&qm, root, &file_list, 2, 0).unwrap();

        // Key invariant: excluded.rs must never appear in any hit
        for hit in &outcome.hits {
            assert!(
                !hit.path.contains("excluded.rs"),
                "excluded.rs must not appear when restricted to allowed.rs"
            );
        }
        // scanned_files reflects the file list length, not all files in the dir
        assert_eq!(
            outcome.scanned_files, 1,
            "scanned_files must equal the file list length"
        );
    }
}
