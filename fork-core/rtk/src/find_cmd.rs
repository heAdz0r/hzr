use crate::tracking;
use anyhow::{Context, Result};
use ignore::WalkBuilder;
use std::collections::HashSet; // 0.11.0 (US-004)
use std::path::{Path, PathBuf}; // 0.11.0 (US-004)

const PASSTHROUGH_MAX_LINES: usize = 200;
const PASSTHROUGH_MAX_BYTES: usize = 16 * 1024;

struct BoundedPassthrough {
    shown_lines: usize,
    shown_bytes: usize,
    omitted_lines: usize,
    recovery_command: String,
}

impl BoundedPassthrough {
    fn new(recovery_command: String) -> Self {
        Self {
            shown_lines: 0,
            shown_bytes: 0,
            omitted_lines: 0,
            recovery_command,
        }
    }
}

impl crate::stream::StreamFilter for BoundedPassthrough {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        let bytes = line.len().saturating_add(1);
        if self.shown_lines >= PASSTHROUGH_MAX_LINES
            || self.shown_bytes.saturating_add(bytes) > PASSTHROUGH_MAX_BYTES
        {
            self.omitted_lines = self.omitted_lines.saturating_add(1);
            return None;
        }
        self.shown_lines += 1;
        self.shown_bytes += bytes;
        Some(format!("{line}\n"))
    }

    fn flush(&mut self) -> String {
        if self.omitted_lines == 0 {
            String::new()
        } else {
            format!(
                "[{} line(s) omitted; recovery: HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=complete_log hzr exec run {}]\n",
                self.omitted_lines, self.recovery_command
            )
        }
    }
}

/// Match a filename against a glob pattern (supports `*` and `?`).
fn glob_match(pattern: &str, name: &str) -> bool {
    glob_match_inner(pattern.as_bytes(), name.as_bytes())
}

fn glob_match_inner(pat: &[u8], name: &[u8]) -> bool {
    match (pat.first(), name.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            // '*' matches zero or more characters
            glob_match_inner(&pat[1..], name)
                || (!name.is_empty() && glob_match_inner(pat, &name[1..]))
        }
        (Some(b'?'), Some(_)) => glob_match_inner(&pat[1..], &name[1..]),
        (Some(&p), Some(&n)) if p == n => glob_match_inner(&pat[1..], &name[1..]),
        _ => false,
    }
}

/// fix #211: parsed arguments from either native find or RTK find syntax.
#[derive(Debug)]
pub struct FindArgs {
    pub pattern: String,
    pub path: String,
    pub max_results: usize,
    pub max_depth: Option<usize>,
    pub file_type: String,
    pub case_insensitive: bool,
}

impl Default for FindArgs {
    fn default() -> Self {
        Self {
            pattern: "*".to_string(),
            path: ".".to_string(),
            max_results: 50,
            max_depth: None,
            file_type: "f".to_string(),
            case_insensitive: false,
        }
    }
}

// 0.11.0 (US-004, upstream 5d697b0/d3f31dd/f614c3c/989f453/988f2e3): dispatch on
// find's own grammar — `find [options] [paths...] [expression]`, paths ending at
// the first token that starts with `-`, `!` or a parenthesis. rtk walks the tree
// itself only for the subset it models exactly; every other invocation runs the
// real find. The previous flag-list parser read `find src` as a search for files
// *named* `src` and silently ignored predicates it did not know (`-path`,
// `-mindepth`, a second path), answering a different question with exit 0.

/// How an invocation is served.
enum FindPlan {
    /// rtk's own walk over the modelled subset.
    Walk(FindArgs),
    /// The real `find` with these arguments, output bounded.
    Native(Vec<String>),
}

fn is_expression_token(token: &str) -> bool {
    token.starts_with('-') || matches!(token, "!" | "(" | ")")
}

fn has_glob_meta(token: &str) -> bool {
    token.contains('*') || token.contains('?')
}

/// Leading find options (`-H`, `-L`, `-P`, `-D opts`, `-Olevel`) change how paths
/// are walked; rtk's walker does not model them.
fn has_leading_options(args: &[String]) -> bool {
    args.first().is_some_and(|first| {
        matches!(first.as_str(), "-H" | "-L" | "-P" | "-D")
            || (first.len() > 2 && (first.starts_with("-D") || first.starts_with("-O")))
    })
}

/// rtk's legacy form `find <glob> [path] [-m N] [-t f|d]`. Native find syntax
/// wins whenever it is plausible: the first token must be a glob that names no
/// existing path, and the command must not use `-name`/`-iname` itself.
fn is_legacy_syntax(args: &[String]) -> bool {
    let Some(first) = args.first() else {
        return false;
    };
    !is_expression_token(first)
        && has_glob_meta(first)
        && !Path::new(first).exists()
        && !args.iter().any(|arg| arg == "-name" || arg == "-iname")
}

fn legacy_to_find_syntax(args: &[String]) -> Vec<String> {
    let mut rebuilt = Vec::with_capacity(args.len() + 1);
    let mut rest = &args[1..];
    if let Some(path) = rest.first().filter(|p| !is_expression_token(p)) {
        rebuilt.push(path.clone());
        rest = &rest[1..];
    }
    rebuilt.push("-name".to_string());
    rebuilt.push(args[0].clone());
    rebuilt.extend(rest.iter().cloned());
    rebuilt
}

/// rtk's own `-m/--max N` and `-t/--file-type f|d`, recognised only at the very
/// end, where they cannot be a predicate value or an `-exec` argument.
fn peel_trailing_rtk_flags(args: &[String]) -> (Vec<String>, Option<usize>, Option<String>) {
    let mut end = args.len();
    let mut max = None;
    let mut file_type = None;
    while end >= 2 {
        let value = &args[end - 1];
        match args[end - 2].as_str() {
            "-m" | "--max" if max.is_none() => match value.parse::<usize>() {
                Ok(n) => max = Some(n),
                Err(_) => break,
            },
            "-t" | "--file-type" if file_type.is_none() && (value == "f" || value == "d") => {
                file_type = Some(value.clone())
            }
            _ => break,
        }
        end -= 2;
    }
    (args[..end].to_vec(), max, file_type)
}

/// The subset rtk walks itself: at most one path and, each at most once,
/// `-name`/`-iname` (`*` and `?` globs), `-type f|d`, `-maxdepth N`.
fn parse_subset(paths: &[String], expr: &[String]) -> Option<FindArgs> {
    if paths.len() > 1 {
        return None;
    }
    let mut parsed = FindArgs::default();
    if let Some(path) = paths.first() {
        parsed.path = path.clone();
    }
    let (mut seen_name, mut seen_type, mut seen_depth) = (false, false, false);
    let mut i = 0;
    while i < expr.len() {
        let value = expr.get(i + 1)?;
        match expr[i].as_str() {
            "-name" | "-iname" if !seen_name && !value.contains('[') => {
                parsed.pattern = value.clone();
                parsed.case_insensitive = expr[i] == "-iname";
                seen_name = true;
            }
            "-type" if !seen_type && (value == "f" || value == "d") => {
                parsed.file_type = value.clone();
                seen_type = true;
            }
            "-maxdepth" if !seen_depth => {
                parsed.max_depth = Some(value.parse().ok()?);
                seen_depth = true;
            }
            _ => return None,
        }
        i += 2;
    }
    Some(parsed)
}

fn plan(original: &[String]) -> FindPlan {
    let (args, max, file_type) = peel_trailing_rtk_flags(original);
    let args = if is_legacy_syntax(&args) {
        legacy_to_find_syntax(&args)
    } else {
        args
    };
    let native = || {
        let mut native = args.clone();
        if let Some(t) = &file_type {
            native.extend(["-type".to_string(), t.clone()]);
        }
        FindPlan::Native(native)
    };
    if has_leading_options(&args) {
        return native();
    }
    let split = args
        .iter()
        .position(|t| is_expression_token(t))
        .unwrap_or(args.len());
    match parse_subset(&args[..split], &args[split..]) {
        Some(mut parsed) => {
            if let Some(n) = max {
                parsed.max_results = n;
            }
            if let Some(t) = file_type {
                parsed.file_type = t;
            }
            FindPlan::Walk(parsed)
        }
        None => native(),
    }
}

/// fix #211: parse arguments supporting both native find and RTK syntax.
/// Returns Err when the invocation is outside the subset rtk walks itself.
#[cfg(test)] // 0.11.0 (US-004): production dispatch goes through plan()
pub fn parse_find_args(args: &[String]) -> anyhow::Result<FindArgs> {
    match plan(args) {
        FindPlan::Walk(parsed) => Ok(parsed), // 0.11.0 (US-004)
        FindPlan::Native(_) => anyhow::bail!(
            "rtk find walks only -name/-iname, -type f|d and -maxdepth over one path; \
             this invocation runs the real find"
        ),
    }
}

/// fix #211: entry point for trailing_var_arg dispatch from main.rs
pub fn run_from_args(args: &[String], verbose: u8) -> Result<()> {
    // 0.11.0 (US-004): one grammar-driven plan instead of two flag-list parsers.
    // Everything outside the modelled subset runs the caller's find verbatim, with
    // its exit code and bounded visible output — erroring was a dead end once the
    // hook had rewritten the command into `rtk find`.
    match plan(args) {
        FindPlan::Walk(parsed) => run_with_opts(
            &parsed.pattern,
            &parsed.path,
            parsed.max_results,
            &parsed.file_type,
            parsed.case_insensitive,
            parsed.max_depth,
            verbose,
        ),
        FindPlan::Native(native) => run_find_passthrough(&native, verbose),
    }
}

/// Extended run with case-insensitive and max_depth support
fn run_with_opts(
    pattern: &str,
    path: &str,
    max_results: usize,
    file_type: &str,
    case_insensitive: bool,
    max_depth: Option<usize>,
    verbose: u8,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let effective_pattern = if pattern == "." { "*" } else { pattern };

    if verbose > 0 {
        eprintln!(
            "find: {} in {} (icase={})",
            effective_pattern, path, case_insensitive
        );
    }

    // 0.11.0 (US-004, upstream 765b270): a missing root is find's error and exit 1,
    // not an empty success an agent reads as "no such files".
    if std::fs::symlink_metadata(path).is_err() {
        eprintln!("find: {}: No such file or directory", path);
        timer.track(
            &format!("find {} -name '{}'", path, effective_pattern),
            "rtk find",
            "",
            "",
        );
        std::process::exit(1);
    }

    let (files, skipped) = walk_matches(
        effective_pattern,
        path,
        file_type,
        case_insensitive,
        max_depth,
    );
    let recovery = native_find_command(
        path,
        effective_pattern,
        file_type,
        case_insensitive,
        max_depth,
    );
    let note = skipped.note(&recovery); // 0.11.0 (US-004)

    // Reuse the same display logic as run()
    run_display(
        &files,
        max_results,
        effective_pattern,
        path,
        note.as_deref(),
        &timer,
    )
}

/// Entries the walker pruned — hidden or ignored — and therefore never searched.
/// (0.11.0, US-004, upstream 9cf048a/68dc719)
#[derive(Debug, Default, PartialEq)]
struct Skipped {
    /// Pruned directories; their contents were not searched.
    dirs: usize,
    /// Pruned files whose name matches the pattern.
    files: usize,
}

impl Skipped {
    fn note(&self, recovery: &str) -> Option<String> {
        if self.dirs == 0 && self.files == 0 {
            return None;
        }
        let plural = |n: usize, one: &str, many: &str| {
            format!("{n} {}", if n == 1 { one } else { many })
        };
        let mut parts = Vec::new();
        if self.files > 0 {
            parts.push(plural(self.files, "matching file", "matching files"));
        }
        if self.dirs > 0 {
            parts.push(plural(self.dirs, "directory", "directories"));
        }
        Some(format!(
            "[hidden/gitignored, not searched: {}; everything: HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=complete_log hzr exec run {}]",
            parts.join(", "),
            shell_quote(recovery)
        ))
    }
}

/// The plain find command that answers the same question without rtk's filters.
fn native_find_command(
    path: &str,
    pattern: &str,
    file_type: &str,
    case_insensitive: bool,
    max_depth: Option<usize>,
) -> String {
    let mut parts = vec!["find".to_string(), inner_quote(path)];
    if let Some(depth) = max_depth {
        parts.push(format!("-maxdepth {depth}"));
    }
    let name_flag = if case_insensitive { "-iname" } else { "-name" };
    parts.push(format!("{name_flag} {}", inner_quote(pattern)));
    parts.push(format!("-type {}", inner_quote(file_type)));
    parts.join(" ")
}

/// Quote a word for a command that will itself be single-quoted: bare when it is
/// plainly safe, double quotes when nothing inside them expands, otherwise the
/// escaped single-quote form. Keeps the recovery line readable. (0.11.0, US-004)
fn inner_quote(word: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "/._-+=:,@%".contains(c);
    if !word.is_empty() && word.chars().all(plain) {
        word.to_string()
    } else if !word.chars().any(|c| "\"$`\\!'".contains(c)) {
        format!("\"{word}\"")
    } else {
        shell_quote(word)
    }
}

/// Bookkeeping bound for the disclosure pass on very large trees.
const DISCLOSURE_ENTRY_CAP: usize = 20_000;
/// Never worth announcing: every repository has one.
const UNREPORTED_DIR: &str = ".git";

/// Walk `path` and return every match exactly as plain `find` would print it:
/// each result keeps the search root as it was given, relative or absolute.
///
/// Results used to be rewritten relative to the search root. That silently
/// corrupted machine-consumed output — `find /a/b -name x` answered `c/x`
/// instead of `/a/b/c/x`, with no marker that anything was rewritten — and a
/// consumer resolving the answer against its own cwd derived a wrong location
/// (heAdz0r/hzr#7).
#[cfg(test)]
fn collect_matches(
    effective_pattern: &str,
    path: &str,
    file_type: &str,
    case_insensitive: bool,
    max_depth: Option<usize>,
) -> Vec<String> {
    walk_matches(effective_pattern, path, file_type, case_insensitive, max_depth).0
}

/// Walk and also count what the walker pruned. (0.11.0, US-004)
fn walk_matches(
    effective_pattern: &str,
    path: &str,
    file_type: &str,
    case_insensitive: bool,
    max_depth: Option<usize>,
) -> (Vec<String>, Skipped) {
    let want_dirs = file_type == "d";
    // A pattern aimed at dotfiles (`-name '.env*'`) must walk hidden entries.
    let search_hidden = effective_pattern.starts_with('.'); // 0.11.0 (US-004)

    let mut builder = WalkBuilder::new(path);
    builder
        .hidden(!search_hidden) // 0.11.0 (US-004)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true);
    // `[filters].ignore_dirs` / `ignore_files` from config.toml were declared but
    // never read by any command. Applied here as exclude globs on top of the
    // gitignore rules, so a configured entry actually removes matches.
    if let Some(overrides) = configured_overrides(path) {
        builder.overrides(overrides);
    }
    if let Some(depth) = max_depth {
        builder.max_depth(Some(depth));
    }
    let walker = builder.build();

    let mut files: Vec<String> = Vec::new();
    // 0.11.0 (US-004): remember what the walk visited so the pruned children of
    // each visited directory can be counted afterwards.
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut visited_dirs: Vec<(PathBuf, usize)> = Vec::new();
    let mut disclose = true;

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let ft = entry.file_type();
        let is_dir = ft.as_ref().is_some_and(|t| t.is_dir());
        if disclose {
            if visited.len() >= DISCLOSURE_ENTRY_CAP {
                disclose = false;
                visited.clear();
                visited_dirs.clear();
            } else {
                visited.insert(entry.path().to_path_buf());
                // The root may be a symlink to a directory.
                let dir_like = is_dir
                    || (entry.depth() == 0
                        && std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()));
                if dir_like {
                    visited_dirs.push((entry.path().to_path_buf(), entry.depth()));
                }
            }
        }

        if want_dirs && !is_dir {
            continue;
        }
        if !want_dirs && is_dir {
            continue;
        }

        let entry_path = entry.path();
        let name = match entry_path.file_name() {
            Some(n) => n.to_string_lossy().to_string(),
            None => continue,
        };

        let matched = if case_insensitive {
            glob_match(&effective_pattern.to_lowercase(), &name.to_lowercase())
        } else {
            glob_match(effective_pattern, &name)
        };
        if !matched {
            continue;
        }

        let display_path = entry_path.to_string_lossy().to_string();
        if !display_path.is_empty() {
            files.push(display_path);
        }
    }

    let skipped = if disclose {
        count_pruned(
            &visited_dirs,
            &visited,
            effective_pattern,
            want_dirs,
            case_insensitive,
            max_depth,
        )
    } else {
        Skipped::default()
    };
    (files, skipped)
}

/// Count the children of visited directories that the walk never yielded.
/// A pruned directory counts once (its contents were not searched); a pruned
/// file counts only if its name matches. (0.11.0, US-004, upstream 68dc719)
fn count_pruned(
    visited_dirs: &[(PathBuf, usize)],
    visited: &HashSet<PathBuf>,
    pattern: &str,
    want_dirs: bool,
    case_insensitive: bool,
    max_depth: Option<usize>,
) -> Skipped {
    let mut skipped = Skipped::default();
    for (dir, depth) in visited_dirs {
        if max_depth.is_some_and(|max| *depth >= max) {
            continue;
        }
        let Ok(children) = std::fs::read_dir(dir) else {
            continue;
        };
        for child in children.flatten() {
            let child_path = child.path();
            if visited.contains(&child_path) {
                continue;
            }
            let name = child.file_name().to_string_lossy().into_owned();
            if child.file_type().is_ok_and(|t| t.is_dir()) {
                if name != UNREPORTED_DIR {
                    skipped.dirs += 1;
                }
            } else if !want_dirs {
                let matched = if case_insensitive {
                    glob_match(&pattern.to_lowercase(), &name.to_lowercase())
                } else {
                    glob_match(pattern, &name)
                };
                if matched {
                    skipped.files += 1;
                }
            }
        }
    }
    skipped
}

/// Shared display logic extracted from run()
fn run_display(
    files: &[String],
    max_results: usize,
    pattern: &str,
    path: &str,
    note: Option<&str>, // 0.11.0 (US-004): disclosure of pruned entries
    timer: &tracking::TimedExecution,
) -> Result<()> {
    let raw_output = files.join("\n");

    if files.is_empty() {
        // 0.11.0 (US-004): an empty answer still says what was never searched.
        let shown = note.map(|n| format!("{n}\n")).unwrap_or_default();
        print!("{}", shown);
        timer.track(
            &format!("find {} -name '{}'", path, pattern),
            "rtk find",
            &raw_output,
            &shown,
        );
        return Ok(());
    }

    let body = format_grouped_results(files, max_results);
    // Compare against the plain listing *at the same cap*, not against the full
    // match list: on a small capped run the grouped header cost more tokens than
    // the paths it replaced, and comparing against every match hid that.
    let capped_plain = files
        .iter()
        .take(max_results)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    let shown = if crate::guard::estimate_body_tokens(&body)
        > crate::guard::estimate_body_tokens(&capped_plain)
    {
        capped_plain.clone()
    } else {
        body.clone()
    };
    let mut shown = crate::guard::never_worse_content(&raw_output, &shown).to_string(); // 0.10.0: content guard

    // rtk imposed this cap on its own initiative, so the hidden matches must stay
    // recoverable without re-running the search.
    if files.len() > max_results {
        if let Some(hint) = crate::tee::force_tee_tail_hint(&raw_output, "find", max_results + 1) {
            shown.push('\n');
            shown.push_str(&hint);
        }
    }
    // 0.11.0 (US-004): disclosure after the guard, and output ends with a newline.
    if let Some(note) = note {
        if !shown.ends_with('\n') {
            shown.push('\n');
        }
        shown.push_str(note);
    }
    if !shown.ends_with('\n') {
        shown.push('\n');
    }

    print!("{}", shown);
    timer.track(
        &format!("find {} -name '{}'", path, pattern),
        "rtk find",
        &raw_output,
        &shown,
    );
    Ok(())
}

/// Run the caller's `find` predicates verbatim while bounding model-visible output.
fn run_find_passthrough(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    if verbose > 0 {
        eprintln!("find passthrough: {}", args.join(" "));
    }
    let mut cmd = std::process::Command::new("find");
    cmd.args(args);
    let original = std::iter::once("find".to_owned())
        .chain(args.iter().map(|argument| shell_quote(argument)))
        .collect::<Vec<_>>()
        .join(" ");
    let result = crate::stream::run_streaming(
        &mut cmd,
        crate::stream::StdinMode::Inherit,
        crate::stream::FilterMode::Streaming(Box::new(BoundedPassthrough::new(shell_quote(
            &original,
        )))),
    )
    .context("Failed to run find")?;

    timer.track_passthrough(
        &format!("find {}", args.join(" ")),
        &format!("rtk find {} (passthrough)", args.join(" ")),
    );

    if result.exit_code != 0 {
        std::process::exit(result.exit_code);
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn format_grouped_results(files: &[String], max_results: usize) -> String {
    let mut by_dir: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for file in files {
        let p = std::path::Path::new(file);
        let dir = p
            .parent()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());
        let dir = if dir.is_empty() { ".".to_string() } else { dir };
        let filename = p
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        by_dir.entry(dir).or_default().push(filename);
    }

    let mut dirs: Vec<_> = by_dir.keys().cloned().collect();
    dirs.sort();
    let dirs_count = dirs.len();
    let total_files = files.len();

    let mut body = format!("{}F {}D:\n\n", total_files, dirs_count);

    let mut displayed = 0;
    for dir in &dirs {
        if displayed >= max_results {
            break;
        }
        let files_in_dir = &by_dir[dir];
        let dir_display = dir;
        let remaining_budget = max_results - displayed;
        if files_in_dir.len() <= remaining_budget {
            body.push_str(&format!("{}/ {}\n", dir_display, files_in_dir.join(" ")));
            displayed += files_in_dir.len();
        } else {
            let partial: Vec<_> = files_in_dir
                .iter()
                .take(remaining_budget)
                .cloned()
                .collect();
            body.push_str(&format!("{}/ {}\n", dir_display, partial.join(" ")));
            displayed += partial.len();
            break;
        }
    }

    if displayed < total_files {
        body.push_str(&format!("+{} more\n", total_files - displayed));
    }

    body
}

#[cfg(test)] // 0.11.0 (US-004): run_from_args covers the empty invocation now
pub fn run(
    pattern: &str,
    path: &str,
    max_results: usize,
    file_type: &str,
    verbose: u8,
) -> Result<()> {
    run_with_opts(pattern, path, max_results, file_type, false, None, verbose)
}

/// Exclude globs built from the user's configured ignore lists. Returns `None`
/// when nothing is configured beyond the defaults, so the common path builds no
/// override matcher at all.
fn configured_overrides(root: &str) -> Option<ignore::overrides::Override> {
    let dirs = crate::config::Config::merged_ignore_dirs(&[]);
    let files = crate::config::Config::merged_ignore_files(&[]);
    if dirs.is_empty() && files.is_empty() {
        return None;
    }
    let mut builder = ignore::overrides::OverrideBuilder::new(root);
    for dir in &dirs {
        builder.add(&format!("!{}/**", dir)).ok()?;
        builder.add(&format!("!{}", dir)).ok()?;
    }
    for file in &files {
        builder.add(&format!("!{}", file)).ok()?;
    }
    builder.build().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- glob_match unit tests ---

    #[test]
    fn glob_match_star_rs() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.rs", "find_cmd.rs"));
        assert!(!glob_match("*.rs", "main.py"));
        assert!(!glob_match("*.rs", "rs"));
    }

    #[test]
    fn glob_match_star_all() {
        assert!(glob_match("*", "anything.txt"));
        assert!(glob_match("*", "a"));
        assert!(glob_match("*", ".hidden"));
    }

    #[test]
    fn glob_match_question_mark() {
        assert!(glob_match("?.rs", "a.rs"));
        assert!(!glob_match("?.rs", "ab.rs"));
    }

    #[test]
    fn glob_match_exact() {
        assert!(glob_match("Cargo.toml", "Cargo.toml"));
        assert!(!glob_match("Cargo.toml", "cargo.toml"));
    }

    #[test]
    fn glob_match_complex() {
        assert!(glob_match("test_*", "test_foo"));
        assert!(glob_match("test_*", "test_"));
        assert!(!glob_match("test_*", "test"));
    }

    // --- dot pattern treated as star ---

    #[test]
    fn dot_becomes_star() {
        // run() converts "." to "*" internally, test the logic
        let effective = if "." == "." { "*" } else { "." };
        assert_eq!(effective, "*");
    }

    // --- integration: run on this repo ---

    #[test]
    fn find_rs_files_in_src() {
        // Should find .rs files without error
        let result = run("*.rs", "src", 100, "f", 0);
        assert!(result.is_ok());
    }

    #[test]
    fn find_dot_pattern_works() {
        // "." pattern should not error (was broken before)
        let result = run(".", "src", 10, "f", 0);
        assert!(result.is_ok());
    }

    #[test]
    fn find_no_matches() {
        let result = run("*.xyz_nonexistent", "src", 50, "f", 0);
        assert!(result.is_ok());
    }

    #[test]
    fn find_respects_max() {
        // With max=2, should not error
        let result = run("*.rs", "src", 2, "f", 0);
        assert!(result.is_ok());
    }

    // --- search-root preservation (heAdz0r/hzr#7) ---

    #[test]
    fn absolute_search_root_is_preserved_in_results() {
        let directory = tempfile::tempdir().expect("tempdir");
        let nested = directory.path().join("owner/project-x");
        std::fs::create_dir_all(&nested).expect("mkdir");
        let root = directory.path().to_string_lossy().to_string();
        let matches = collect_matches("project-x", &root, "d", false, Some(3));
        assert_eq!(matches.len(), 1, "{matches:?}");
        assert!(
            std::path::Path::new(&matches[0]).is_absolute(),
            "absolute search root came back relative: {}",
            matches[0]
        );
        assert!(matches[0].starts_with(&root), "{}", matches[0]);
    }

    #[test]
    fn relative_search_root_is_preserved_in_results() {
        // `find src -name find_cmd.rs` prints `src/find_cmd.rs`; a consumer
        // resolves that against its own cwd. Stripping the root breaks it.
        let matches = collect_matches("find_cmd.rs", "src", "f", false, None);
        assert_eq!(matches, vec!["src/find_cmd.rs".to_string()]);
    }

    #[test]
    fn grouped_output_preserves_long_unicode_directories() {
        let root = format!("/{}", "я".repeat(40));
        let files = vec![format!("{root}/a.rs"), format!("{root}/b.rs")];
        let output = format_grouped_results(&files, 10);
        assert!(output.contains(&format!("{root}/ a.rs b.rs")), "{output}");
        assert!(!output.contains("..."));
    }

    #[test]
    fn grouped_output_uses_token_efficient_header() {
        let files = vec!["src/main.rs".to_string(), "tests/cli.rs".to_string()];
        let output = format_grouped_results(&files, 50);
        assert!(output.starts_with("2F 2D:\n\n"), "got: {output}");
        assert!(!output.contains('📁'), "got: {output}");
    }

    #[test]
    fn find_gitignored_excluded() {
        // target/ is in .gitignore — files inside should not appear
        let result = run("*", ".", 1000, "f", 0);
        assert!(result.is_ok());
        // We can't easily capture stdout in unit tests, but at least
        // verify it runs without error. The smoke tests verify content.
    }

    // fix #211: parse_find_args tests (parse_find_args returns Result)
    #[test]
    fn test_parse_find_native_name() {
        let args: Vec<String> = vec![".".into(), "-name".into(), "*.rs".into()];
        let parsed = parse_find_args(&args).unwrap();
        assert_eq!(parsed.pattern, "*.rs");
        assert_eq!(parsed.path, ".");
        assert!(!parsed.case_insensitive);
    }

    #[test]
    fn test_parse_find_native_name_type() {
        let args: Vec<String> = vec![
            ".".into(),
            "-name".into(),
            "*.rs".into(),
            "-type".into(),
            "f".into(),
        ];
        let parsed = parse_find_args(&args).unwrap();
        assert_eq!(parsed.pattern, "*.rs");
        assert_eq!(parsed.file_type, "f");
    }

    #[test]
    fn test_parse_find_native_iname() {
        let args: Vec<String> = vec!["-iname".into(), "*.RS".into()];
        let parsed = parse_find_args(&args).unwrap();
        assert_eq!(parsed.pattern, "*.RS");
        assert!(parsed.case_insensitive);
    }

    #[test]
    fn test_parse_find_native_maxdepth() {
        let args: Vec<String> = vec![
            ".".into(),
            "-name".into(),
            "*.toml".into(),
            "-maxdepth".into(),
            "2".into(),
        ];
        let parsed = parse_find_args(&args).unwrap();
        assert_eq!(parsed.max_depth, Some(2));
    }

    #[test]
    fn test_parse_find_unsupported_flags_error() {
        // -exec and -not should return Err, not silently pass
        let args: Vec<String> = vec![
            ".".into(),
            "-name".into(),
            "*.rs".into(),
            "-exec".into(),
            "echo".into(),
        ];
        assert!(parse_find_args(&args).is_err());
        let args2: Vec<String> = vec!["-not".into(), "-name".into(), "*.rs".into()];
        assert!(parse_find_args(&args2).is_err());
    }

    #[test]
    fn test_parse_find_rtk_syntax() {
        let args: Vec<String> = vec!["*.rs".into(), "src".into(), "-m".into(), "10".into()];
        let parsed = parse_find_args(&args).unwrap();
        assert_eq!(parsed.pattern, "*.rs");
        assert_eq!(parsed.path, "src");
        assert_eq!(parsed.max_results, 10);
    }

    #[test]
    fn test_parse_find_rtk_type_flag() {
        let args: Vec<String> = vec!["*.rs".into(), "-t".into(), "f".into()];
        let parsed = parse_find_args(&args).unwrap();
        assert_eq!(parsed.file_type, "f");
    }

    #[test]
    fn test_find_native_name_runs() {
        let args: Vec<String> = vec!["src".into(), "-name".into(), "*.rs".into()];
        let result = run_from_args(&args, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_find_rtk_compat_runs() {
        let args: Vec<String> = vec!["*.rs".into(), "src".into()];
        let result = run_from_args(&args, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn unsupported_find_output_is_bounded_and_names_exact_recovery() {
        use crate::stream::StreamFilter;

        let mut filter = BoundedPassthrough::new("'find . -newer marker'".into());
        let line = "x".repeat(100);
        let shown = (0..400)
            .filter_map(|_| filter.feed_line(&line))
            .collect::<String>();
        let notice = filter.flush();

        assert!(shown.len() <= PASSTHROUGH_MAX_BYTES);
        assert!(shown.lines().count() <= PASSTHROUGH_MAX_LINES);
        assert!(notice.contains("line(s) omitted"));
        assert!(notice.contains("HZR_RAW_FIDELITY_REASON=complete_log"));
        assert!(notice.contains("hzr exec run 'find . -newer marker'"));
    }

    // --- 0.11.0 (US-004): grammar-driven dispatch, missing roots, disclosure ---

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    fn walk_args(args: &[&str]) -> FindArgs {
        match plan(&strings(args)) {
            FindPlan::Walk(parsed) => parsed,
            FindPlan::Native(native) => panic!("{args:?} went native: {native:?}"),
        }
    }

    fn native_args(args: &[&str]) -> Vec<String> {
        match plan(&strings(args)) {
            FindPlan::Native(native) => native,
            FindPlan::Walk(parsed) => panic!("{args:?} was walked: {parsed:?}"),
        }
    }

    #[test]
    fn a_bare_directory_is_the_search_root_not_a_pattern() {
        let parsed = walk_args(&["src"]);
        assert_eq!((parsed.path.as_str(), parsed.pattern.as_str()), ("src", "*"));
    }

    #[test]
    fn unmodelled_predicates_and_shapes_run_the_real_find() {
        assert_eq!(native_args(&["src", "-path", "*a*"]), strings(&["src", "-path", "*a*"]));
        native_args(&["src", "tests", "-name", "*.rs"]);
        native_args(&["-L", ".", "-name", "*.rs"]);
        native_args(&[".", "-name", "*.rs", "-name", "*.py"]);
        native_args(&[".", "-name", "[ab].rs"]);
        native_args(&[".", "-mindepth", "2"]);
        native_args(&[".", "-type", "l"]);
    }

    #[test]
    fn a_missing_root_stays_a_root() {
        // Not rtk's legacy syntax: no glob, so find reports the missing path itself.
        let parsed = walk_args(&["no-such-dir-xyz", "-name", "*.rs"]);
        assert_eq!(parsed.path, "no-such-dir-xyz");
        assert_eq!(parsed.pattern, "*.rs");
    }

    #[test]
    fn legacy_syntax_needs_a_glob_that_names_nothing() {
        let parsed = walk_args(&["*.rs", "src", "-m", "10", "-t", "f"]);
        assert_eq!(
            (parsed.pattern.as_str(), parsed.path.as_str(), parsed.max_results),
            ("*.rs", "src", 10)
        );
        let native = native_args(&["*.rs", "src", "-exec", "echo", "{}", ";"]);
        assert_eq!(&native[..3], strings(&["src", "-name", "*.rs"]).as_slice());
    }

    #[test]
    fn pruned_entries_are_counted_and_named() {
        let root = std::env::temp_dir().join(format!("rtk-find-disclose-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join(".hid")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        for file in ["src/a.rs", "src/.dot.rs", ".hid/x.rs", "src/notes.txt"] {
            std::fs::write(root.join(file), "x").unwrap();
        }
        let root_str = root.to_string_lossy().to_string();
        let (files, skipped) = walk_matches("*.rs", &root_str, "f", false, None);
        assert_eq!(files.len(), 1, "{files:?}");
        // `.dot.rs` is a hidden match; `.hid` is a pruned directory; `.git` is never named.
        assert_eq!(skipped, Skipped { dirs: 1, files: 1 });
        let note = skipped.note("find x -name \"*.rs\" -type f").unwrap();
        assert!(note.contains("1 matching file, 1 directory"), "{note}");
        assert!(note.contains("hzr exec run 'find x -name \"*.rs\" -type f'"), "{note}");

        // A dotfile pattern walks hidden entries instead of pruning them.
        let (dot, _) = walk_matches(".dot*", &root_str, "f", false, None);
        assert_eq!(dot.len(), 1, "{dot:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recovery_command_quotes_only_what_needs_it() {
        assert_eq!(
            native_find_command("src", "*.rs", "f", false, Some(2)),
            "find src -maxdepth 2 -name \"*.rs\" -type f"
        );
        assert_eq!(inner_quote("it's"), "'it'\\''s'");
    }
}
