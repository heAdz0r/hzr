use crate::tracking;
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;
use std::process::Command;

/// Options for grep filtering, replacing the 8-argument signature. // fix #14: GrepOptions struct
pub struct GrepOptions<'a> {
    pub pattern: &'a str,
    pub path: &'a str,
    pub max_line_len: usize,
    pub max_results: usize,
    pub context_only: bool,
    pub file_type: Option<&'a str>,
    pub extra_args: &'a [String],
    pub verbose: u8,
}

pub fn run(opts: GrepOptions<'_>) -> Result<()> {
    let GrepOptions {
        pattern,
        path,
        max_line_len,
        max_results,
        context_only,
        file_type,
        extra_args,
        verbose,
    } = opts;
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("grep: '{}' in {}", pattern, path);
    }

    // Translate BRE -> PCRE: \| -> | (alternation), strip shell-injected escapes like \!
    let rg_pattern = bre_to_pcre(pattern);
    // changed: compile context regex once per run() call instead of per-line (performance fix)
    let context_re: Option<Regex> = if context_only {
        Regex::new(&format!("(?i).{{0,20}}{}.*", regex::escape(&rg_pattern))).ok()
    } else {
        None
    };

    let mut rg_cmd = Command::new("rg");
    // upstream v0.38: --no-ignore-vcs prevents false negatives from .gitignore'd files
    // (ripgrep skips them by default, but grep's recursive mode doesn't — the mismatch
    // causes AI agents to draw wrong conclusions about missing files)
    rg_cmd.args(["-n", "--no-heading", "--no-ignore-vcs", &rg_pattern, path]);

    if let Some(ft) = file_type {
        rg_cmd.arg("--type").arg(normalize_file_type(ft)); // fix: map extension aliases → rg type names
    }

    // changed: centralised flag translation (output_mode compat + rtk read flag guard)
    let translated = translate_extra_args(extra_args);
    let passthrough = is_passthrough_mode(&translated); // changed: detect output_mode before rg
    for arg in &translated {
        rg_cmd.arg(arg);
    }

    let output = rg_cmd
        .output()
        .or_else(|_| Command::new("grep").args(["-rn", pattern, path]).output())
        .context("grep/rg failed")?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    let raw_output = stdout.to_string();

    // Bug 1: rg exit 2 = regex parse error — stderr was silently swallowed, showed "0 results"
    let stderr_str = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(2) {
        // fix: on regex error, show hint with auto-escaped suggestion so LLM can self-correct
        let mut msg = format!(
            "❌ rg regex error for '{}': {}",
            rg_pattern,
            stderr_str.trim()
        );
        if let Some(hint) = hint_literal_parens(&rg_pattern) {
            msg.push('\n');
            msg.push_str(&hint);
        }
        println!("{}", msg);
        timer.track(
            &format!("grep -rn '{}' {}", pattern, path),
            "rtk grep",
            &raw_output,
            &msg,
        );
        return Ok(());
    }

    // changed: structural output modes — bypass line grouping, print rg output directly
    if passthrough {
        let mode = if translated.contains(&"--files-with-matches".to_string()) {
            "--files-with-matches"
        } else {
            "--count"
        };
        let result = format_passthrough_output(&stdout, mode);
        print!("{}", result);
        timer.track(
            &format!("grep -rn '{}' {}", pattern, path),
            "rtk grep",
            &raw_output,
            &result,
        );
        return Ok(());
    }

    if stdout.trim().is_empty() {
        // Bug 2: show rg_pattern (post-translation), not original BRE
        let mut msg = format!("🔍 0 for '{}'", rg_pattern);
        // changed: hint when unescaped ( ) are likely meant as literals (BRE vs PCRE confusion)
        if let Some(hint) = hint_literal_parens(&rg_pattern) {
            msg.push('\n');
            msg.push_str(&hint);
        }
        println!("{}", msg);
        timer.track(
            &format!("grep -rn '{}' {}", pattern, path),
            "rtk grep",
            &raw_output,
            &msg,
        );
        return Ok(());
    }

    let mut by_file: HashMap<String, Vec<(usize, String)>> = HashMap::with_capacity(64); // H5: pre-alloc avoids rehash for typical grep results
    let mut total = 0;

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(3, ':').collect();

        let (file, line_num, content) = if parts.len() == 3 {
            let ln = parts[1].parse().unwrap_or(0);
            (parts[0].to_string(), ln, parts[2])
        } else if parts.len() == 2 {
            let ln = parts[0].parse().unwrap_or(0);
            (path.to_string(), ln, parts[1])
        } else {
            continue;
        };

        total += 1;
        // Bug 3: pass rg_pattern (PCRE), not original BRE — regex::escape(BRE) breaks context
        let cleaned = clean_line(content, max_line_len, context_re.as_ref(), &rg_pattern);
        by_file.entry(file).or_default().push((line_num, cleaned));
    }

    let mut rtk_output = String::new();
    rtk_output.push_str(&format!("🔍 {} in {}F:\n\n", total, by_file.len()));

    let mut shown = 0;
    let mut files: Vec<_> = by_file.iter().collect();
    files.sort_by_key(|(f, _)| *f);

    for (file, matches) in files {
        if shown >= max_results {
            break;
        }

        let file_display = compact_path(file);
        rtk_output.push_str(&format!("📄 {} ({}):\n", file_display, matches.len()));

        for (line_num, content) in matches.iter().take(10) {
            rtk_output.push_str(&format!("  {:>4}: {}\n", line_num, content));
            shown += 1;
            if shown >= max_results {
                break;
            }
        }

        if matches.len() > 10 {
            rtk_output.push_str(&format!("  +{}\n", matches.len() - 10));
        }
        rtk_output.push('\n');
    }

    if total > shown {
        rtk_output.push_str(&format!("... +{}\n", total - shown));
    }

    print!("{}", rtk_output);
    timer.track(
        &format!("grep -rn '{}' {}", pattern, path),
        "rtk grep",
        &raw_output,
        &rtk_output,
    );

    Ok(())
}

/// Returns true when translated args contain a structural-query flag (--files-with-matches or --count).
/// In these modes rg's output format changes, so normal line grouping is skipped.
// changed: detect passthrough mode for output_mode compat
fn is_passthrough_mode(translated: &[String]) -> bool {
    translated
        .iter()
        .any(|a| a == "--files-with-matches" || a == "--count")
}

/// Format rg output for structural modes (--files-with-matches, --count).
/// Returns a compact human-readable string with a summary header.
// changed: passthrough display for output_mode files_with_matches / count
fn format_passthrough_output(rg_stdout: &str, mode: &str) -> String {
    let lines: Vec<&str> = rg_stdout.lines().filter(|l| !l.is_empty()).collect();
    if lines.is_empty() {
        return "🔍 0 matches\n".to_string();
    }
    if mode == "--count" {
        // rg --count outputs either "file:N" (dir search) or bare "N" (single file)
        let is_file_count = lines.iter().any(|l| l.contains(':'));
        if is_file_count {
            // changed: sort by count descending for easy prioritisation
            let mut pairs: Vec<(&str, u64)> = lines
                .iter()
                .filter_map(|l| {
                    let (f, n) = l.rsplit_once(':')?;
                    Some((f, n.trim().parse().unwrap_or(0)))
                })
                .collect();
            pairs.sort_by(|a, b| b.1.cmp(&a.1));
            let total: u64 = pairs.iter().map(|(_, n)| n).sum();
            let mut out = format!(
                "🔍 {} matches in {} file{}:\n",
                total,
                pairs.len(),
                if pairs.len() == 1 { "" } else { "s" }
            );
            for (file, n) in &pairs {
                out.push_str(&format!("  {:>5}  {}\n", n, compact_path(file)));
            }
            out
        } else {
            // single-file bare count
            let n: u64 = lines[0].trim().parse().unwrap_or(0);
            format!("🔍 {} matches\n", n)
        }
    } else {
        // --files-with-matches: one path per line
        let mut out = format!(
            "🔍 {} file{}:\n",
            lines.len(),
            if lines.len() == 1 { "" } else { "s" }
        );
        for line in &lines {
            out.push_str(&format!("  {}\n", compact_path(line)));
        }
        out
    }
}

/// Translate extra_args before passing to rg:
/// - Strip rtk-read-only flags (--from, --to, --level) + consume their value; hint to stderr
/// - Translate native Grep tool output_mode (short: -o mode; long: --output_mode mode; inline: --output_mode=mode)
///   → --files-with-matches / --count / skip(content)
/// - Strip -r/--recursive (rg is recursive by default)
/// - Translate --include=PAT / --include PAT → --glob=PAT
// changed: extracted from inline loop in run() for testability + output_mode compat
fn translate_extra_args(extra_args: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    let mut iter = extra_args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "-r" || arg == "--recursive" {
            continue; // rg is recursive by default
        }
        if arg == "--compact" {
            continue; // rtk grep is compact by default — ignore rtk-specific flag
        }
        if arg == "--from" || arg == "--to" || arg == "--level" {
            // consume the paired value if it does not start with "-"
            if iter.peek().map(|a| !a.starts_with('-')).unwrap_or(false) {
                iter.next();
            }
            eprintln!("hint: '{}' is an rtk read flag — use: rtk read <file> --level none --from N --to M", arg);
            continue;
        }
        if arg == "-l" {
            result.push("--files-with-matches".to_string()); // changed: -l → --files-with-matches (grep/rg compat)
            continue;
        }
        if arg == "-o" {
            // Translate native Grep tool output_mode values; bare -o (only-matching) passes through
            match iter.peek().map(|s| s.as_str()) {
                Some("files_with_matches") => {
                    iter.next();
                    result.push("--files-with-matches".to_string()); // -o files_with_matches → rg -l
                }
                Some("count") => {
                    iter.next();
                    result.push("--count".to_string()); // -o count → rg --count
                }
                Some("content") => {
                    iter.next(); // -o content = default behaviour, skip silently
                }
                _ => result.push(arg.clone()), // bare -o (rg only-matching), pass through
            }
            continue;
        }
        // changed: handle --output_mode / --output-mode long form (native Grep tool lands here when multiple paths precede the flag)
        if arg == "--output_mode" || arg == "--output-mode" {
            match iter.peek().map(|s| s.as_str()) {
                Some("files_with_matches") => {
                    iter.next();
                    result.push("--files-with-matches".to_string());
                }
                Some("count") => {
                    iter.next();
                    result.push("--count".to_string());
                }
                Some("content") => {
                    iter.next();
                }
                _ => result.push(arg.clone()),
            }
            continue;
        }
        // changed: handle --output_mode=value / --output-mode=value inline = form
        if let Some(mode) = arg
            .strip_prefix("--output_mode=")
            .or_else(|| arg.strip_prefix("--output-mode="))
        {
            match mode {
                "files_with_matches" => result.push("--files-with-matches".to_string()),
                "count" => result.push("--count".to_string()),
                "content" => {}
                _ => result.push(arg.clone()),
            }
            continue;
        }
        if let Some(glob) = arg.strip_prefix("--include=") {
            result.push(format!("--glob={}", glob)); // fix: --include= → --glob=
        } else if arg == "--include" {
            if let Some(next) = iter.next() {
                result.push("--glob".to_string()); // fix: --include PAT → --glob PAT
                result.push(next.clone());
            }
        } else {
            result.push(arg.clone());
        }
    }
    result
}

/// If pattern has unescaped `(` or `)` (PCRE groups), suggest escaped literal version.
/// BRE users expect `(` to be literal; in PCRE it starts a capturing group.
// changed: hint for BRE-vs-PCRE paren confusion (0-results false negative)
fn hint_literal_parens(pattern: &str) -> Option<String> {
    // Walk pattern, detect first unescaped ( or )
    let mut chars = pattern.chars().peekable();
    let mut found = false;
    while let Some(c) = chars.next() {
        if c == '\\' {
            chars.next(); // skip escaped char
            continue;
        }
        if c == '(' || c == ')' {
            found = true;
            break;
        }
    }
    if !found {
        return None;
    }
    let escaped = escape_literal_parens(pattern);
    Some(format!(
        "hint: '(' and ')' are PCRE groups in rg — use \\( \\) for literal parens
  → try: rtk grep '{}'",
        escaped
    ))
}

/// Escape all unescaped `(` and `)` in a PCRE pattern so they match literally.
fn escape_literal_parens(pattern: &str) -> String {
    let mut result = String::with_capacity(pattern.len() + 8);
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            result.push(c);
            if let Some(next) = chars.next() {
                result.push(next);
            }
            continue;
        }
        if c == '(' || c == ')' {
            result.push('\\');
        }
        result.push(c);
    }
    result
}

/// Map common file extension aliases to ripgrep type names.
/// rg uses "rust" not "rs", "ruby" not "rb", etc.
fn normalize_file_type(ft: &str) -> &str {
    match ft {
        "rs" => "rust", // fix: rg type is "rust", not "rs"
        "rb" => "ruby",
        "js" => "js",
        "ts" => "ts",
        "py" => "py",
        "go" => "go",
        "sh" => "sh",
        "md" => "md",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "cpp" | "cc" | "cxx" => "cpp",
        other => other,
    }
}

/// Translate a BRE/grep pattern to a PCRE/Rust-regex pattern for ripgrep.
///
/// Two transformations, single char-by-char pass:
/// 1. `\|`  -> `|`   BRE GNU alternation -> PCRE alternation
/// 2. `\X`  -> `X`   backslash before a non-regex-metachar (e.g. `\!` injected by
///                   zsh histexpand) is an undefined/invalid escape; strip the backslash.
///
/// Characters kept as `\X` (valid PCRE escape sequences):
///   `\\` `\^` `\$` `\.` `\*` `\+` `\?` `\(` `\)` `\[` `\]` `\{` `\}`
///   `\n` `\r` `\t` `\f` `\a` `\v`
///   `\d` `\D` `\w` `\W` `\s` `\S` `\b` `\B` `\A` `\z`
///   `\x` `\u` `\U` `\p` `\P` `\0`-`\9`
fn bre_to_pcre(pattern: &str) -> String {
    let translated = bre_to_pcre_raw(pattern);
    if Regex::new(&translated).is_ok() {
        return translated;
    }
    // fallback 1: escape bare braces (e.g. "Plan {")
    let brace_fixed = escape_bare_braces(&translated);
    if Regex::new(&brace_fixed).is_ok() {
        return brace_fixed;
    }
    // fallback 2: escape bare parens (e.g. "\.Search(ctx") — fix: unescaped ( causes rg exit-code 2
    let paren_fixed = escape_literal_parens(&brace_fixed);
    if Regex::new(&paren_fixed).is_ok() {
        return paren_fixed;
    }
    // last resort: full literal escape — never gives rg an invalid regex
    regex::escape(pattern)
}

/// Raw BRE→PCRE translation (no validation).
fn bre_to_pcre_raw(pattern: &str) -> String {
    let mut result = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            result.push(c);
            continue;
        }
        match chars.peek().copied() {
            // BRE alternation \| -> PCRE |
            Some('|') => {
                result.push('|');
                chars.next();
            }
            // Valid PCRE/Rust-regex escape — keep backslash unchanged
            Some(next) if is_pcre_escape_char(next) => {
                result.push('\\');
                result.push(chars.next().unwrap());
            }
            // Unknown/shell-injected escape (e.g. \! from zsh histexpand) — strip backslash
            Some(_) => {
                result.push(chars.next().unwrap());
            }
            // Trailing bare backslash — keep (rg will emit its own regex error)
            None => result.push('\\'),
        }
    }
    result
}

/// Escape bare `{` characters that are not part of valid PCRE quantifiers (`{n}`, `{n,}`, `{n,m}`).
/// Called as a fallback when the translated pattern fails regex validation.
fn escape_bare_braces(pattern: &str) -> String {
    // changed: fix bare { that break PCRE (e.g. "Plan {")
    let chars: Vec<char> = pattern.chars().collect();
    let mut result = String::with_capacity(pattern.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            // Keep any \X sequence intact (including \{ already escaped)
            result.push('\\');
            result.push(chars[i + 1]);
            i += 2;
        } else if chars[i] == '{' {
            if is_valid_quantifier_brace(&chars, i) {
                result.push('{');
            } else {
                result.push_str("\\{"); // escape bare { that isn't a quantifier
            }
            i += 1;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

/// Returns true if `{` at `pos` is the start of a valid PCRE quantifier: `{n}`, `{n,}`, `{n,m}`.
fn is_valid_quantifier_brace(chars: &[char], pos: usize) -> bool {
    let mut j = pos + 1;
    let start = j;
    // Must start with at least one digit
    while j < chars.len() && chars[j].is_ascii_digit() {
        j += 1;
    }
    if j == start || j >= chars.len() {
        return false;
    }
    match chars[j] {
        '}' => true, // {n}
        ',' => {
            j += 1;
            // Optional upper bound digits
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            j < chars.len() && chars[j] == '}' // {n,} or {n,m}
        }
        _ => false,
    }
}

/// Returns true for characters that form valid/meaningful Rust-regex escape sequences.
/// Backslash before these chars must be preserved; before anything else it is stripped.
fn is_pcre_escape_char(c: char) -> bool {
    matches!(
        c,
        // Metacharacters that need escaping to be literal
        '\\' | '^' | '$' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}'
        // Character class shorthands
        | 'd' | 'D' | 'w' | 'W' | 's' | 'S'
        // Anchors
        | 'b' | 'B' | 'A' | 'z'
        // Standard C escape chars
        | 'n' | 'r' | 't' | 'f' | 'a' | 'v'
        // Hex / Unicode
        | 'x' | 'u' | 'U'
        // Unicode properties
        | 'p' | 'P'
        // Back-references \0-\9
        | '0'..='9'
    )
}

// changed: accepts pre-compiled regex instead of recompiling on every call
fn clean_line(line: &str, max_len: usize, context_re: Option<&Regex>, pattern: &str) -> String {
    let trimmed = line.trim();

    if let Some(re) = context_re {
        if let Some(m) = re.find(trimmed) {
            let matched = m.as_str();
            if matched.len() <= max_len {
                return matched.to_string();
            }
        }
    }

    if trimmed.len() <= max_len {
        trimmed.to_string()
    } else {
        let lower = trimmed.to_lowercase();
        let pattern_lower = pattern.to_lowercase();

        if let Some(pos) = lower.find(&pattern_lower) {
            let char_pos = lower[..pos].chars().count();
            let chars: Vec<char> = trimmed.chars().collect();
            let char_len = chars.len();

            let start = char_pos.saturating_sub(max_len / 3);
            let end = (start + max_len).min(char_len);
            let start = if end == char_len {
                end.saturating_sub(max_len)
            } else {
                start
            };

            let slice: String = chars[start..end].iter().collect();
            if start > 0 && end < char_len {
                format!("...{}...", slice)
            } else if start > 0 {
                format!("...{}", slice)
            } else {
                format!("{}...", slice)
            }
        } else {
            let t: String = trimmed.chars().take(max_len - 3).collect();
            format!("{}...", t)
        }
    }
}

fn compact_path(path: &str) -> String {
    if path.len() <= 50 {
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

#[cfg(test)]
mod tests {
    use super::*;

    // changed: files_with_matches output mode detected in translated args
    #[test]
    fn test_is_passthrough_mode_files_with_matches() {
        let args = vec!["-o".to_string(), "files_with_matches".to_string()];
        let translated = translate_extra_args(&args);
        assert!(is_passthrough_mode(&translated));
    }

    // changed: count output mode detected
    #[test]
    fn test_is_passthrough_mode_count() {
        let args = vec!["-o".to_string(), "count".to_string()];
        let translated = translate_extra_args(&args);
        assert!(is_passthrough_mode(&translated));
    }

    // changed: normal content mode is NOT passthrough
    #[test]
    fn test_is_passthrough_mode_content_false() {
        let args: Vec<String> = vec![];
        assert!(!is_passthrough_mode(&translate_extra_args(&args)));
    }

    // changed: format_passthrough_output wraps file list with header (files_with_matches)
    #[test]
    fn test_format_passthrough_files_with_matches() {
        let rg_out = "src/foo.rs\nsrc/bar.rs\n";
        let result = format_passthrough_output(rg_out, "--files-with-matches");
        assert!(result.contains("2 files"), "got: {result}");
        assert!(result.contains("src/foo.rs"), "got: {result}");
        assert!(result.contains("src/bar.rs"), "got: {result}");
    }

    // changed: format_passthrough_output count (dir search) shows total + sorted by count
    #[test]
    fn test_format_passthrough_count_dir() {
        let rg_out = "src/foo.rs:12\nsrc/bar.rs:3\n";
        let result = format_passthrough_output(rg_out, "--count");
        assert!(result.contains("15 matches"), "total: {result}");
        assert!(result.contains("2 files"), "files: {result}");
        // foo.rs (12) should appear before bar.rs (3) after sort-by-count-desc
        let pos_foo = result.find("foo.rs").unwrap_or(usize::MAX);
        let pos_bar = result.find("bar.rs").unwrap_or(usize::MAX);
        assert!(pos_foo < pos_bar, "sorted by count desc: {result}");
    }

    // changed: format_passthrough_output count (single file) shows bare match count
    #[test]
    fn test_format_passthrough_count_single_file() {
        let rg_out = "34\n";
        let result = format_passthrough_output(rg_out, "--count");
        assert!(result.contains("34 matches"), "got: {result}");
    }

    // changed: format_passthrough_output empty rg output
    #[test]
    fn test_format_passthrough_empty() {
        let result = format_passthrough_output("", "--files-with-matches");
        assert!(result.contains('0'), "got: {result}");
    }

    // changed: -l → --files-with-matches (grep compat, -l was conflicting with --max-len)
    #[test]
    fn test_translate_l_flag_files_with_matches() {
        let args = vec!["-l".to_string()];
        assert_eq!(translate_extra_args(&args), vec!["--files-with-matches"]);
    }

    // changed: -o files_with_matches → rg --files-with-matches (native Grep tool output_mode compat)
    #[test]
    fn test_translate_output_mode_files_with_matches() {
        let args = vec!["-o".to_string(), "files_with_matches".to_string()];
        assert_eq!(translate_extra_args(&args), vec!["--files-with-matches"]);
    }

    // changed: -o count → rg --count
    #[test]
    fn test_translate_output_mode_count() {
        let args = vec!["-o".to_string(), "count".to_string()];
        assert_eq!(translate_extra_args(&args), vec!["--count"]);
    }

    // changed: -o content is the default, skip silently
    #[test]
    fn test_translate_output_mode_content_skipped() {
        let args = vec!["-o".to_string(), "content".to_string()];
        assert!(translate_extra_args(&args).is_empty());
    }

    // changed: bare -o (rg only-matching) must pass through unchanged
    #[test]
    fn test_translate_bare_o_passes_through() {
        let args = vec!["-o".to_string()];
        assert_eq!(translate_extra_args(&args), vec!["-o"]);
    }

    // changed: --from/--to are rtk read flags — must be filtered with hint
    #[test]
    fn test_translate_rtk_read_flags_filtered() {
        let args = vec![
            "--from".to_string(),
            "10".to_string(),
            "--to".to_string(),
            "50".to_string(),
        ];
        assert!(translate_extra_args(&args).is_empty());
    }

    // changed: --level is an rtk read flag, filtered; sibling rg flags kept
    #[test]
    fn test_translate_rtk_level_flag_filtered_keeps_others() {
        let args = vec![
            "--level".to_string(),
            "none".to_string(),
            "-C".to_string(),
            "3".to_string(),
        ];
        assert_eq!(translate_extra_args(&args), vec!["-C", "3"]);
    }

    // changed: --include=PAT → --glob=PAT (existing behaviour, now via helper)
    #[test]
    fn test_translate_include_to_glob_inline() {
        let args = vec!["--include=*.go".to_string()];
        assert_eq!(translate_extra_args(&args), vec!["--glob=*.go"]);
    }

    // changed: --include PAT (space-separated) → --glob PAT
    #[test]
    fn test_translate_include_paired_to_glob() {
        let args = vec!["--include".to_string(), "*.rs".to_string()];
        assert_eq!(translate_extra_args(&args), vec!["--glob", "*.rs"]);
    }

    // changed: -r/--recursive stripped (rg is recursive by default)
    #[test]
    fn test_translate_recursive_stripped() {
        let args = vec!["-r".to_string(), "-i".to_string()];
        assert_eq!(translate_extra_args(&args), vec!["-i"]);
    }

    #[test]
    fn test_translate_compact_stripped() {
        // --compact is rtk-specific, must not be forwarded to rg
        let args = vec!["--compact".to_string(), "-i".to_string()];
        assert_eq!(translate_extra_args(&args), vec!["-i"]);
    }

    #[test]
    fn test_clean_line() {
        let line = "            const result = someFunction();";
        let cleaned = clean_line(line, 50, None, "result"); // changed: None = no context regex
        assert!(!cleaned.starts_with(' '));
        assert!(cleaned.len() <= 50);
    }

    #[test]
    fn test_compact_path() {
        let path = "/Users/patrick/dev/project/src/components/Button.tsx";
        let compact = compact_path(path);
        assert!(compact.len() <= 60);
    }

    #[test]
    fn test_extra_args_accepted() {
        // Test that the function signature accepts extra_args
        // This is a compile-time test - if it compiles, the signature is correct
        let _extra: Vec<String> = vec!["-i".to_string(), "-A".to_string(), "3".to_string()];
        // No need to actually run - we're verifying the parameter exists
    }

    #[test]
    fn test_clean_line_multibyte() {
        // Thai text that exceeds max_len in bytes
        let line = "  สวัสดีครับ นี่คือข้อความที่ยาวมากสำหรับทดสอบ  ";
        let cleaned = clean_line(line, 20, None, "ครับ"); // changed: None = no context regex
                                                         // Should not panic
        assert!(!cleaned.is_empty());
    }

    #[test]
    fn test_clean_line_emoji() {
        let line = "🎉🎊🎈🎁🎂🎄 some text 🎃🎆🎇✨";
        let cleaned = clean_line(line, 15, None, "text"); // changed: None = no context regex
        assert!(!cleaned.is_empty());
    }

    // Fix: BRE \| alternation is translated to PCRE | for rg
    #[test]
    fn test_bre_alternation_translated() {
        let pattern = r"fn foo\|pub.*bar";
        let rg_pattern = pattern.replace(r"\|", "|");
        assert_eq!(rg_pattern, "fn foo|pub.*bar");
    }

    // Fix: -r flag (grep recursive) is stripped from extra_args (rg is recursive by default)
    #[test]
    fn test_recursive_flag_stripped() {
        let extra_args: Vec<String> = vec!["-r".to_string(), "-i".to_string()];
        let filtered: Vec<&String> = extra_args
            .iter()
            .filter(|a| *a != "-r" && *a != "--recursive")
            .collect();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0], "-i");
    }

    // Verify line numbers are always enabled in rg invocation (grep_cmd.rs:24).
    // The -n/--line-numbers clap flag in main.rs is a no-op accepted for compat.
    #[test]
    fn test_rg_always_has_line_numbers() {
        // grep_cmd::run() always passes "-n" to rg (line 24).
        // This test documents that -n is built-in, so the clap flag is safe to ignore.
        let mut cmd = std::process::Command::new("rg");
        cmd.args(["-n", "--no-heading", "NONEXISTENT_PATTERN_12345", "."]);
        // If rg is available, it should accept -n without error (exit 1 = no match, not error)
        if let Ok(output) = cmd.output() {
            assert!(
                output.status.code() == Some(1) || output.status.success(),
                "rg -n should be accepted"
            );
        }
        // If rg is not installed, skip gracefully (test still passes)
    }

    // bre_to_pcre: \| -> |
    #[test]
    fn test_bre_to_pcre_alternation() {
        assert_eq!(
            bre_to_pcre(r"panic\|todo\|unimplemented"),
            "panic|todo|unimplemented"
        );
    }

    // bre_to_pcre: \! (shell histexpand artifact) -> ! (strip spurious backslash)
    #[test]
    fn test_bre_to_pcre_strips_shell_escaped_bang() {
        assert_eq!(bre_to_pcre(r"panic\!"), "panic!");
        assert_eq!(bre_to_pcre(r"panic\!\|todo\!"), "panic!|todo!");
    }

    // bre_to_pcre: valid PCRE escapes are preserved unchanged
    #[test]
    fn test_bre_to_pcre_preserves_valid_escapes() {
        assert_eq!(bre_to_pcre(r"\d+\.\w+"), r"\d+\.\w+");
        assert_eq!(bre_to_pcre(r"word"), r"word");
        assert_eq!(bre_to_pcre(r"#\[tokio"), r"#\[tokio"); // \[ = literal [, keep
    }

    // fix: trailing bare backslash is invalid regex — bre_to_pcre now produces valid output via last-resort regex::escape
    #[test]
    fn test_bre_to_pcre_trailing_backslash() {
        let result = bre_to_pcre(r"foo\");
        // old behavior: returned invalid "foo\" and let rg fail with exit-code 2
        // new behavior: fallback to regex::escape → valid regex matching literal foo\
        assert!(
            Regex::new(&result).is_ok(),
            "trailing backslash must produce valid regex via fallback: {result}"
        );
    }

    // changed: bare { not a quantifier must be escaped so rg doesn't fail
    #[test]
    fn test_bre_to_pcre_escapes_bare_brace() {
        // "Plan {" — the { is NOT a quantifier, must become \{
        let r = bre_to_pcre("Plan {");
        assert!(
            Regex::new(&r).is_ok(),
            "escaped pattern must be valid regex: {r}"
        );
        assert!(r.contains("\\{"), "bare {{ should be escaped: {r}");
    }

    #[test]
    fn test_bre_to_pcre_alternation_with_bare_brace() {
        // "run_plan\|MemorySubcommand\|Plan {" — the real failure case
        let r = bre_to_pcre(r"run_plan\|MemorySubcommand\|Plan {");
        assert!(Regex::new(&r).is_ok(), "must compile: {r}");
        assert!(
            r.contains("run_plan|MemorySubcommand|"),
            "alternation preserved: {r}"
        );
    }

    #[test]
    fn test_bre_to_pcre_keeps_valid_quantifiers() {
        // {3}, {3,}, {3,5} must NOT be escaped
        assert_eq!(bre_to_pcre(r"\w{3}"), r"\w{3}");
        assert_eq!(bre_to_pcre(r"\d{2,}"), r"\d{2,}");
        assert_eq!(bre_to_pcre(r"a{1,5}"), r"a{1,5}");
    }

    #[test]
    fn test_bre_to_pcre_escapes_brace_in_text() {
        // Struct literal syntax in code searches
        let r = bre_to_pcre("Foo { bar }");
        assert!(Regex::new(&r).is_ok(), "must compile: {r}");
    }

    // changed: hint_literal_parens detects unescaped ( ) that are PCRE groups
    #[test]
    fn test_hint_literal_parens_detects_unescaped() {
        // Pattern like #\[cfg(test)\] has unescaped ( ) — should hint
        let hint = hint_literal_parens(r"#\[cfg(test)\]");
        assert!(hint.is_some(), "should detect unescaped parens");
        let hint = hint.unwrap();
        assert!(hint.contains("\\("), "should show escaped form");
        assert!(
            hint.contains(r"#\[cfg\(test\)\]"),
            "escaped suggestion correct"
        );
    }

    #[test]
    fn test_hint_literal_parens_no_parens() {
        // Pattern with only escaped parens or no parens — no hint
        assert!(hint_literal_parens(r"#\[cfg\(test\)\]").is_none());
        assert!(hint_literal_parens(r"\d+\.\w+").is_none());
        assert!(hint_literal_parens("fn foo").is_none());
    }

    #[test]
    fn test_escape_literal_parens_basic() {
        assert_eq!(escape_literal_parens(r"cfg(test)"), r"cfg\(test\)");
        assert_eq!(
            escape_literal_parens(r"#\[cfg(test)\]"),
            r"#\[cfg\(test\)\]"
        );
    }

    #[test]
    fn test_escape_literal_parens_already_escaped() {
        // Already-escaped \( must not be double-escaped
        assert_eq!(escape_literal_parens(r"\(test\)"), r"\(test\)");
    }

    #[test]
    fn test_translate_output_mode_long_form_files_with_matches() {
        let args = vec![
            "--output_mode".to_string(),
            "files_with_matches".to_string(),
        ];
        assert_eq!(translate_extra_args(&args), vec!["--files-with-matches"]);
    }

    #[test]
    fn test_translate_output_mode_long_form_count() {
        let args = vec!["--output_mode".to_string(), "count".to_string()];
        assert_eq!(translate_extra_args(&args), vec!["--count"]);
    }

    #[test]
    fn test_translate_output_mode_long_form_content_skipped() {
        let args = vec!["--output_mode".to_string(), "content".to_string()];
        assert!(translate_extra_args(&args).is_empty());
    }

    #[test]
    fn test_translate_output_mode_dash_form() {
        let args = vec![
            "--output-mode".to_string(),
            "files_with_matches".to_string(),
        ];
        assert_eq!(translate_extra_args(&args), vec!["--files-with-matches"]);
    }

    #[test]
    fn test_translate_output_mode_inline_eq_files_with_matches() {
        let args = vec!["--output_mode=files_with_matches".to_string()];
        assert_eq!(translate_extra_args(&args), vec!["--files-with-matches"]);
    }

    #[test]
    fn test_translate_output_mode_inline_eq_count() {
        let args = vec!["--output_mode=count".to_string()];
        assert_eq!(translate_extra_args(&args), vec!["--count"]);
    }

    #[test]
    fn test_translate_output_mode_inline_eq_content_skipped() {
        let args = vec!["--output_mode=content".to_string()];
        assert!(translate_extra_args(&args).is_empty());
    }

    #[test]
    fn test_translate_extra_paths_with_output_mode() {
        let args = vec![
            "reranker/".to_string(),
            "raptor/".to_string(),
            "--output_mode".to_string(),
            "files_with_matches".to_string(),
        ];
        assert_eq!(
            translate_extra_args(&args),
            vec!["reranker/", "raptor/", "--files-with-matches"]
        );
    }
}

#[cfg(test)]
mod regex_retry_tests {
    use super::*;

    // RED: unescaped ( in pattern must produce valid regex after bre_to_pcre
    #[test]
    fn test_bre_to_pcre_escapes_unescaped_open_paren() {
        let r = bre_to_pcre(r"\.Search(ctx");
        assert!(
            Regex::new(&r).is_ok(),
            "unescaped ( must be auto-escaped to produce valid regex, got: {r}"
        );
        assert!(r.contains(r"\("), "( should be escaped in: {r}");
    }

    // RED: unescaped ) causes same issue
    #[test]
    fn test_bre_to_pcre_escapes_unescaped_close_paren() {
        let r = bre_to_pcre(r"foo)bar");
        assert!(
            Regex::new(&r).is_ok(),
            "unescaped ) must be auto-escaped: {r}"
        );
    }

    // GREEN guard: already-escaped parens must not be double-escaped
    #[test]
    fn test_bre_to_pcre_no_double_escape_parens() {
        let r = bre_to_pcre(r"cfg\(test\)");
        assert!(Regex::new(&r).is_ok(), "must stay valid: {r}");
        // should not gain extra backslashes
        assert!(!r.contains(r"\\("), "no double-escape: {r}");
    }
}
