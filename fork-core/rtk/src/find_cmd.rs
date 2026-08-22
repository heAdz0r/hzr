use crate::tracking;
use anyhow::{Context, Result};
use ignore::WalkBuilder;

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

/// Consume the next argument from `args` at position `i`, advancing the index.
fn next_arg(args: &[String], i: &mut usize) -> Option<String> {
    *i += 1;
    args.get(*i).cloned()
}

/// Check if args contain native find flags (-name, -type, -maxdepth, -iname).
fn has_native_find_flags(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "-name" || a == "-type" || a == "-maxdepth" || a == "-iname")
}

/// Native find flags that RTK cannot handle correctly.
const UNSUPPORTED_FIND_FLAGS: &[&str] = &[
    "-not", "!", "-or", "-o", "-and", "-a", "-exec", "-execdir", "-delete", "-print0", "-newer",
    "-perm", "-size", "-mtime", "-mmin", "-atime", "-amin", "-ctime", "-cmin", "-empty", "-link",
    "-regex", "-iregex",
];

fn has_unsupported_find_flags(args: &[String]) -> bool {
    args.iter()
        .any(|a| UNSUPPORTED_FIND_FLAGS.contains(&a.as_str()))
}

/// fix #211: parse arguments supporting both native find and RTK syntax.
/// Returns Err for unsupported compound predicates (-not, -exec, etc.).
pub fn parse_find_args(args: &[String]) -> anyhow::Result<FindArgs> {
    if args.is_empty() {
        return Ok(FindArgs::default());
    }

    if has_unsupported_find_flags(args) {
        anyhow::bail!(
            "rtk find does not support compound predicates or actions (e.g. -not, -exec). Use `find` directly."
        );
    }

    if has_native_find_flags(args) {
        parse_native_find_args(args)
    } else {
        Ok(parse_rtk_find_args(args))
    }
}

/// Parse native find syntax: `find [path] -name "*.rs" -type f -maxdepth 3`
fn parse_native_find_args(args: &[String]) -> anyhow::Result<FindArgs> {
    let mut parsed = FindArgs::default();
    let mut i = 0;

    // First non-flag argument is the path (standard find behavior)
    if !args[0].starts_with('-') {
        parsed.path = args[0].clone();
        i = 1;
    }

    while i < args.len() {
        match args[i].as_str() {
            "-name" => {
                if let Some(val) = next_arg(args, &mut i) {
                    parsed.pattern = val;
                }
            }
            "-iname" => {
                if let Some(val) = next_arg(args, &mut i) {
                    parsed.pattern = val;
                    parsed.case_insensitive = true;
                }
            }
            "-type" => {
                if let Some(val) = next_arg(args, &mut i) {
                    parsed.file_type = val;
                }
            }
            "-maxdepth" => {
                if let Some(val) = next_arg(args, &mut i) {
                    parsed.max_depth = Some(val.parse().context("invalid -maxdepth value")?);
                }
            }
            flag if flag.starts_with('-') => {
                eprintln!("rtk find: unknown flag '{}', ignored", flag);
            }
            _ => {}
        }
        i += 1;
    }

    Ok(parsed)
}

/// Parse RTK syntax: `find *.rs [path] [-m max] [-t type]`
fn parse_rtk_find_args(args: &[String]) -> FindArgs {
    let mut parsed = FindArgs::default();
    let mut positional = 0usize;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-m" | "--max" => {
                if i + 1 < args.len() {
                    parsed.max_results = args[i + 1].parse().unwrap_or(50);
                    i += 2;
                    continue;
                }
            }
            "-t" | "--type" => {
                if i + 1 < args.len() {
                    parsed.file_type = args[i + 1].clone();
                    i += 2;
                    continue;
                }
            }
            a if !a.starts_with('-') => {
                if positional == 0 {
                    parsed.pattern = a.to_string();
                } else if positional == 1 {
                    parsed.path = a.to_string();
                }
                positional += 1;
            }
            _ => {}
        }
        i += 1;
    }

    parsed
}

/// fix #211: entry point for trailing_var_arg dispatch from main.rs
pub fn run_from_args(args: &[String], verbose: u8) -> Result<()> {
    if args.is_empty() {
        return run("*", ".", 50, "f", verbose);
    }
    // Compound predicates and actions have no filtered equivalent. Erroring here
    // was a dead end: once the hook has rewritten `find … -exec …` into
    // `rtk find …`, the agent cannot reach the real binary any more. Run the
    // caller's command verbatim and keep its output and exit code untouched.
    if has_unsupported_find_flags(args) {
        return run_find_passthrough(args, verbose);
    }
    let parsed = parse_find_args(args)?;
    run_with_opts(
        &parsed.pattern,
        &parsed.path,
        parsed.max_results,
        &parsed.file_type,
        parsed.case_insensitive,
        parsed.max_depth,
        verbose,
    )
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

    let want_dirs = file_type == "d";

    let mut builder = WalkBuilder::new(path);
    builder
        .hidden(true)
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

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let ft = entry.file_type();
        let is_dir = ft.as_ref().is_some_and(|t| t.is_dir());

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

        let display_path = entry_path
            .strip_prefix(path)
            .unwrap_or(entry_path)
            .to_string_lossy()
            .to_string();

        if !display_path.is_empty() {
            files.push(display_path);
        }
    }

    // Reuse the same display logic as run()
    run_display(&files, max_results, effective_pattern, path, &timer)
}

/// Shared display logic extracted from run()
fn run_display(
    files: &[String],
    max_results: usize,
    pattern: &str,
    path: &str,
    timer: &tracking::TimedExecution,
) -> Result<()> {
    let raw_output = files.join("\n");

    if files.is_empty() {
        timer.track(
            &format!("find {} -name '{}'", path, pattern),
            "rtk find",
            &raw_output,
            "",
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
    let mut shown = crate::guard::never_worse(&raw_output, &shown).to_string();

    // rtk imposed this cap on its own initiative, so the hidden matches must stay
    // recoverable without re-running the search.
    if files.len() > max_results {
        if let Some(hint) = crate::tee::force_tee_tail_hint(&raw_output, "find", max_results + 1) {
            shown.push('\n');
            shown.push_str(&hint);
        }
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

/// Run the caller's `find` verbatim, preserving stdout, stderr and exit code.
fn run_find_passthrough(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    if verbose > 0 {
        eprintln!("find passthrough: {}", args.join(" "));
    }
    let mut cmd = std::process::Command::new("find");
    cmd.args(args);
    let result = crate::stream::run_streaming(
        &mut cmd,
        crate::stream::StdinMode::Inherit,
        crate::stream::FilterMode::Passthrough,
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
        let dir_display = if dir.len() > 50 {
            format!("...{}", &dir[dir.len() - 47..])
        } else {
            dir.clone()
        };
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
}
