use crate::stream::RelayedOutput; // 0.11.0 (US-007): relay signals while capturing
use crate::tracking;
// 0.11.0 (US-016): messages are no longer truncated
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;
// 0.11.0 (US-009): PathBuf no longer needed (command_for_tsc removed)
use std::process::Command;

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    // 0.11.0 (US-009): the runner the caller named, PATH, node_modules/.bin, then
    // lockfile detection (bun included) — not an unconditional `npx`.
    let (mut cmd, tool) = crate::utils::js_tool_command("tsc");

    for arg in args {
        cmd.arg(arg);
    }

    let tool = tool.as_str();
    run_tsc_like(
        cmd,
        tool,
        args,
        verbose,
        "Failed to run tsc (try: npm install -g typescript)",
        &format!("{tool} {}", args.join(" ")),
        &format!("rtk tsc {}", args.join(" ")),
    )
}

pub fn run_vue_tsc(args: &[String], verbose: u8, skip_env: bool) -> Result<()> {
    let (mut cmd, _) = crate::utils::js_tool_command("vue-tsc"); // 0.11.0 (US-009)
    for arg in args {
        cmd.arg(arg);
    }
    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }
    run_tsc_like(
        cmd,
        "npx vue-tsc",
        args,
        verbose,
        "Failed to run npx vue-tsc (try: npm install -D vue-tsc)",
        &format!("npx vue-tsc {}", args.join(" ")),
        &format!("rtk npx vue-tsc {}", args.join(" ")),
    )
}


fn run_tsc_like(
    mut cmd: Command,
    tool_name: &str,
    args: &[String],
    verbose: u8,
    error_context: &str,
    track_input_cmd: &str,
    track_rtk_cmd: &str,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Running: {} {}", tool_name, args.join(" "));
    }

    let output = cmd.output_relayed().with_context(|| error_context.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_tsc_output(&raw);

    let exit_code = output.status.code().unwrap_or(1); // upstream sync: tee integration
                                                       // tsc reports position-less global errors (bad tsconfig, missing lib) that
                                                       // the per-file grouping cannot see; exit code is the only signal left.
    let filtered = crate::guard::guard_exit(&raw, exit_code, "tsc", &filtered);
    if let Some(hint) = crate::tee::tee_and_hint(&raw, "tsc", exit_code) {
        // upstream sync: tee
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(track_input_cmd, track_rtk_cmd, &raw, &filtered);

    // Preserve tsc exit code for CI/CD compatibility
    std::process::exit(exit_code); // upstream sync: use exit_code
}

/// Filter TypeScript compiler output - group errors by file, show every error
pub(crate) fn filter_tsc_output(output: &str) -> String {
    lazy_static::lazy_static! {
        // Pattern: src/file.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'.
        static ref TSC_ERROR: Regex = Regex::new(
            r"^(.+?)\((\d+),(\d+)\):\s+(error|warning)\s+(TS\d+):\s+(.+)$"
        ).unwrap();
        // 0.11.0 (US-016, upstream 9d1c60a): `--pretty` / `"pretty": true`:
        // src/file.ts:12:5 - error TS2322: Type 'string' is not assignable …
        static ref TSC_PRETTY: Regex = Regex::new(
            r"^(.+?):(\d+):(\d+) - (error|warning) (TS\d+): (.+)$"
        ).unwrap();
        // 0.11.0 (US-016, upstream 6232c37): position-less global diagnostics
        // (bad tsconfig option, missing lib): error TS5023: Unknown compiler option …
        static ref TSC_GLOBAL: Regex = Regex::new(
            r"^(error|warning) (TS\d+): (.+)$"
        ).unwrap();
        // Pretty code frame: `12 const a = 1;` and the `~~~` marker line.
        static ref PRETTY_FRAME: Regex = Regex::new(r"^\s*(\d+ |\s*~+\s*$)").unwrap();
    }
    // 0.11.0 (US-016): colour is stripped before parsing; pretty output is coloured.
    let cleaned = crate::utils::strip_ansi(output);
    let output = cleaned.as_str();

    struct TsError {
        file: String,
        line: usize,
        code: String,
        message: String,
        context_lines: Vec<String>,
    }

    let mut errors: Vec<TsError> = Vec::new();
    let lines: Vec<&str> = output.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let parsed = TSC_ERROR
            .captures(line)
            .or_else(|| TSC_PRETTY.captures(line))
            .map(|caps| (caps[1].to_string(), caps[2].parse().unwrap_or(0), caps[5].to_string(), caps[6].to_string()))
            .or_else(|| {
                TSC_GLOBAL
                    .captures(line)
                    .map(|caps| ("(global)".to_string(), 0, caps[2].to_string(), caps[3].to_string()))
            });
        if let Some((file, line_no, code, message)) = parsed {
            let mut err = TsError {
                file,
                line: line_no,
                code,
                message,
                context_lines: Vec::new(),
            };

            // Capture continuation lines (indented context from tsc); a pretty
            // code frame is presentation and is skipped. (0.11.0, US-016)
            i += 1;
            while i < lines.len() {
                let next = lines[i];
                let starts_diag = TSC_ERROR.is_match(next) || TSC_PRETTY.is_match(next);
                if next.trim().is_empty() || starts_diag {
                    if next.trim().is_empty() && lines.get(i + 1).is_some_and(|l| PRETTY_FRAME.is_match(l)) {
                        i += 1;
                        continue;
                    }
                    break;
                }
                if PRETTY_FRAME.is_match(next) {
                    i += 1;
                } else if next.starts_with("  ") || next.starts_with('\t') {
                    err.context_lines.push(next.trim().to_string());
                    i += 1;
                } else {
                    break;
                }
            }

            errors.push(err);
        } else {
            i += 1;
        }
    }

    if errors.is_empty() {
        if output.contains("Found 0 errors") {
            return "✓ TypeScript: No errors found".to_string();
        }
        // 0.11.0: no diagnostics is not no output — `tsc --version`, `--showConfig`
        // and `--listFiles` print what was asked for, and it replaced all of them.
        if !output.trim().is_empty() {
            return output.trim_end().to_string();
        }
        return "TypeScript compilation completed".to_string();
    }

    // Group by file
    let mut by_file: HashMap<String, Vec<&TsError>> = HashMap::new();
    for err in &errors {
        by_file.entry(err.file.clone()).or_default().push(err);
    }

    // Count by error code for summary
    let mut by_code: HashMap<String, usize> = HashMap::new();
    for err in &errors {
        *by_code.entry(err.code.clone()).or_insert(0) += 1;
    }

    let mut result = String::new();
    result.push_str(&format!(
        "TypeScript: {} errors in {} files\n",
        errors.len(),
        by_file.len()
    ));
    result.push_str("═══════════════════════════════════════\n");

    // Top error codes summary (compact, one line)
    let mut code_counts: Vec<_> = by_code.iter().collect();
    code_counts.sort_by(|a, b| b.1.cmp(a.1));

    if code_counts.len() > 1 {
        let codes_str: Vec<String> = code_counts
            .iter()
            .take(5)
            .map(|(code, count)| format!("{} ({}x)", code, count))
            .collect();
        result.push_str(&format!("Top codes: {}\n\n", codes_str.join(", ")));
    }

    // Files sorted by error count (most errors first)
    let mut files_sorted: Vec<_> = by_file.iter().collect();
    files_sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    // Show every error per file — no limits
    for (file, file_errors) in &files_sorted {
        result.push_str(&format!("{} ({} errors)\n", file, file_errors.len()));

        for err in *file_errors {
            // 0.11.0 (US-016): the message is the payload — never cut. A cut at
            // 120 characters dropped the list of allowed values from TS6046.
            result.push_str(&format!("  L{}: {} {}\n", err.line, err.code, err.message));
            for ctx in &err.context_lines {
                result.push_str(&format!("    {}\n", ctx));
            }
        }
        result.push('\n');
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 0.11.0 (US-016): real tsc 5 `--pretty` output and a position-less diagnostic.
    #[test]
    fn test_pretty_and_global_diagnostics_are_grouped() {
        let pretty = "\x1b[96ma.ts\x1b[0m:\x1b[93m1\x1b[0m:\x1b[93m7\x1b[0m - \x1b[91merror\x1b[0m\x1b[90m TS2322: \x1b[0mType 'string' is not assignable to type 'number'.\n\n\
\x1b[7m1\x1b[0m const a: number = \"x\";\n\x1b[7m \x1b[0m \x1b[91m      ~\x1b[0m\n\n\
error TS5023: Unknown compiler option 'bogusOpt'.\n\n\
Found 2 errors in the same file, starting at: a.ts\x1b[90m:1\x1b[0m\n";
        let out = filter_tsc_output(pretty);
        assert!(out.starts_with("TypeScript: 2 errors in 2 files"), "{out}");
        assert!(out.contains("L1: TS2322 Type 'string' is not assignable"), "{out}");
        assert!(out.contains("(global) (1 errors)"), "{out}");
        assert!(out.contains("TS5023 Unknown compiler option 'bogusOpt'."), "{out}");
        assert!(!out.contains("const a"), "code frame leaked: {out}");
        assert!(!out.contains('\u{1b}'), "{out}");
    }

    #[test]
    fn test_filter_tsc_output() {
        let output = r#"
src/server/api/auth.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'.
src/server/api/auth.ts(15,10): error TS2345: Argument of type 'number' is not assignable to parameter of type 'string'.
src/components/Button.tsx(8,3): error TS2339: Property 'onClick' does not exist on type 'ButtonProps'.
src/components/Button.tsx(10,5): error TS2322: Type 'string' is not assignable to type 'number'.

Found 4 errors in 2 files.
"#;
        let result = filter_tsc_output(output);
        assert!(result.contains("TypeScript: 4 errors in 2 files"));
        assert!(result.contains("auth.ts (2 errors)"));
        assert!(result.contains("Button.tsx (2 errors)"));
        assert!(result.contains("TS2322"));
        assert!(!result.contains("Found 4 errors")); // Summary line should be replaced
    }

    #[test]
    fn test_every_error_message_shown() {
        let output = "\
src/api.ts(10,5): error TS2322: Type 'string' is not assignable to type 'number'.
src/api.ts(20,5): error TS2322: Type 'boolean' is not assignable to type 'string'.
src/api.ts(30,5): error TS2322: Type 'null' is not assignable to type 'object'.
";
        let result = filter_tsc_output(output);
        // Each error message must be individually visible, not collapsed
        assert!(result.contains("Type 'string' is not assignable to type 'number'"));
        assert!(result.contains("Type 'boolean' is not assignable to type 'string'"));
        assert!(result.contains("Type 'null' is not assignable to type 'object'"));
        assert!(result.contains("L10:"));
        assert!(result.contains("L20:"));
        assert!(result.contains("L30:"));
    }

    #[test]
    fn test_continuation_lines_preserved() {
        let output = "\
src/app.tsx(10,3): error TS2322: Type '{ children: Element; }' is not assignable to type 'Props'.
  Property 'children' does not exist on type 'Props'.
src/app.tsx(20,5): error TS2345: Argument of type 'number' is not assignable to parameter of type 'string'.
";
        let result = filter_tsc_output(output);
        assert!(result.contains("Property 'children' does not exist on type 'Props'"));
        assert!(result.contains("L10:"));
        assert!(result.contains("L20:"));
    }

    #[test]
    fn test_no_file_limit() {
        // 15 files with errors — all must appear
        let mut output = String::new();
        for i in 1..=15 {
            output.push_str(&format!(
                "src/file{}.ts({},1): error TS2322: Error in file {}.\n",
                i, i, i
            ));
        }
        let result = filter_tsc_output(&output);
        assert!(result.contains("15 errors in 15 files"));
        for i in 1..=15 {
            assert!(
                result.contains(&format!("file{}.ts", i)),
                "file{}.ts missing from output",
                i
            );
        }
    }

    #[test]
    fn test_filter_no_errors() {
        let output = "Found 0 errors. Watching for file changes.";
        let result = filter_tsc_output(output);
        assert!(result.contains("No errors found"));
    }

    #[test]
    fn test_filter_vue_tsc_output() {
        let output = r#"
src/components/LetterBalloonsVisualizer.vue(43,26): error TS2367: This comparison appears to be unintentional because the types '"pairs" | "basic" | "similar"' and '"words"' have no overlap.
src/components/LetterBalloonsVisualizer.vue(43,52): error TS2367: This comparison appears to be unintentional because the types '"pairs" | "basic" | "similar"' and '"words-sh"' have no overlap.
src/components/LetterCountVisualizer.vue(239,26): error TS7053: Element implicitly has an 'any' type because expression of type 'number' can't be used to index type 'Map<number, Set<number>>'.

Found 3 errors in 2 files.
"#;
        let result = filter_tsc_output(output);
        assert!(result.contains("TypeScript: 3 errors in 2 files"));
        assert!(result.contains("LetterBalloonsVisualizer.vue (2 errors)"));
        assert!(result.contains("LetterCountVisualizer.vue (1 errors)"));
        assert!(result.contains("TS2367"));
        assert!(result.contains("TS7053"));
    }
}
