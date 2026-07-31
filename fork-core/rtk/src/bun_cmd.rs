use crate::tracking;
use anyhow::{Context, Result};
use regex::Regex;
use std::process::Command;
use std::sync::OnceLock;

pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("bun requires arguments (e.g., `bun run <script>`)");
    }

    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("bun");
    for arg in args {
        cmd.arg(arg);
    }

    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }

    if verbose > 0 {
        eprintln!("Running: bun {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run bun. Is Bun installed?")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = crate::utils::make_raw(&stdout, &stderr); // fix #18: no double \n

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });
    let rendered = render_bun_output(args, &raw, verbose);

    if let Some(hint) = crate::tee::tee_and_hint(&raw, "bun", exit_code) {
        println!("{}\n{}", rendered, hint);
    } else {
        println!("{}", rendered);
    }

    timer.track(
        &format!("bun {}", args.join(" ")),
        &format!("rtk bun {}", args.join(" ")),
        &raw,
        &rendered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

fn render_bun_output(args: &[String], output: &str, verbose: u8) -> String {
    // Version/short commands should stay as-is.
    if matches!(args.first().map(|s| s.as_str()), Some("--version" | "-v")) {
        return filter_bun_output(output);
    }
    if verbose > 0 {
        return filter_bun_output(output);
    }
    summarize_bun_output(args, output)
}

fn filter_bun_output(output: &str) -> String {
    let mut result = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Common Bun script-run boilerplate, not useful for diagnostics.
        if trimmed.starts_with("bun run v") || trimmed.starts_with("bun test v") {
            continue;
        }
        if trimmed.starts_with("$ ") {
            continue;
        }
        if trimmed.starts_with("Done in ") {
            continue;
        }

        result.push(line.to_string());
    }

    if result.is_empty() {
        "ok ✓".to_string()
    } else {
        result.join("\n")
    }
}

fn summarize_bun_output(args: &[String], output: &str) -> String {
    static TEST_FILES_RE: OnceLock<Regex> = OnceLock::new();
    static TESTS_RE: OnceLock<Regex> = OnceLock::new();
    static MODULES_RE: OnceLock<Regex> = OnceLock::new();
    static BUILT_RE: OnceLock<Regex> = OnceLock::new();

    let test_files_re = TEST_FILES_RE.get_or_init(|| {
        Regex::new(r"^\s*Test Files\s+(\d+)\s+passed(?:\s+\((\d+)\))?")
            .expect("invalid TEST_FILES_RE")
    });
    let tests_re = TESTS_RE.get_or_init(|| {
        Regex::new(r"^\s*Tests\s+(\d+)\s+passed(?:\s+\((\d+)\))?").expect("invalid TESTS_RE")
    });
    let modules_re = MODULES_RE
        .get_or_init(|| Regex::new(r"^\s*✓\s+(\d+)\s+modules transformed\.").expect("invalid"));
    let built_re =
        BUILT_RE.get_or_init(|| Regex::new(r"^\s*✓\s+built in ([0-9.]+s)").expect("invalid"));

    let mut test_files: Option<(usize, usize)> = None;
    let mut tests: Option<(usize, usize)> = None;
    let mut modules: Option<usize> = None;
    let mut build_duration: Option<String> = None;

    for line in output.lines() {
        if let Some(caps) = test_files_re.captures(line) {
            let passed = caps
                .get(1)
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(0);
            let total = caps
                .get(2)
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(passed);
            test_files = Some((passed, total));
            continue;
        }
        if let Some(caps) = tests_re.captures(line) {
            let passed = caps
                .get(1)
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(0);
            let total = caps
                .get(2)
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(passed);
            tests = Some((passed, total));
            continue;
        }
        if let Some(caps) = modules_re.captures(line) {
            modules = caps.get(1).and_then(|m| m.as_str().parse::<usize>().ok());
            continue;
        }
        if let Some(caps) = built_re.captures(line) {
            build_duration = caps.get(1).map(|m| m.as_str().to_string());
            continue;
        }
    }

    let mut parts = Vec::new();
    if let Some((passed, total)) = tests {
        parts.push(format!("tests: {passed}/{total}"));
    }
    if let Some((passed, total)) = test_files {
        parts.push(format!("files: {passed}/{total}"));
    }
    if let Some(m) = modules {
        parts.push(format!("modules: {m}"));
    }
    if let Some(d) = build_duration {
        parts.push(format!("build: {d}"));
    }

    let headline = if parts.is_empty() {
        format!("✓ bun {}", args.join(" "))
    } else {
        format!("✓ bun {} ({})", args.join(" "), parts.join(", "))
    };

    let diag = crate::diag_summary::analyze_output(output); // changed: show error snippets
    let mut out = format!(
        "{}\n{}\n{}",
        headline,
        diag.warnings_line(),
        diag.errors_line()
    );
    if diag.errors > 0 {
        // changed: append file:line error hints when errors > 0
        let snippets = extract_bun_errors(output, 5);
        if !snippets.is_empty() {
            out.push('\n');
            out.push_str(&snippets.join("\n"));
        }
    }
    out
}

/// Extract compact error snippets from bun/vite/tsc build output.
/// Returns up to `max` one-line descriptions: "file:line  code: short-message"
fn extract_bun_errors(output: &str, max: usize) -> Vec<String> {
    static TS_ERR_RE: OnceLock<Regex> = OnceLock::new(); // fix #13: use imported aliases
    static VITE_ERR_RE: OnceLock<Regex> = OnceLock::new();
    static VITE_LOC_RE: OnceLock<Regex> = OnceLock::new();
    static GENERIC_ERR_RE: OnceLock<Regex> = OnceLock::new();

    // TypeScript: "src/App.tsx:12:3 - error TS7006: Parameter 'x'..."
    let ts_err = TS_ERR_RE.get_or_init(|| {
        regex::Regex::new(r"^([A-Za-z0-9_./@-]+\.[a-z]+):(\d+):\d+\s+-\s+error\s+(TS\d+):\s*(.+)$")
            .expect("TS_ERR_RE")
    });
    // Vite/esbuild: "✗ [ERROR] message" or "  [ERROR] message" or "error: message"
    let vite_err = VITE_ERR_RE.get_or_init(|| {
        regex::Regex::new(r"(?:✗\s*)?\[ERROR\]\s+(.+)$|^error:\s+(.+)$").expect("VITE_ERR_RE")
    });
    // Vite file location following an error: "    src/App.tsx:5:10:"
    let vite_loc = VITE_LOC_RE.get_or_init(|| {
        regex::Regex::new(r"^\s{2,}([A-Za-z0-9_./@-]+\.[a-z]+):(\d+):\d+:?\s*$")
            .expect("VITE_LOC_RE")
    });
    // Generic bare file:line error: "src/App.tsx:12:3: ERROR reason"
    let generic_err = GENERIC_ERR_RE.get_or_init(|| {
        regex::Regex::new(r"^([A-Za-z0-9_./@-]+\.[a-z]+):(\d+):\d+:\s*(?:ERROR|error):\s*(.+)$")
            .expect("GENERIC_ERR_RE")
    });

    let mut snippets: Vec<String> = Vec::new();
    let mut pending_msg: Option<String> = None; // message waiting for a file:line

    for line in output.lines() {
        let trimmed = line.trim();

        // TypeScript tsc/vue-tsc style
        if let Some(caps) = ts_err.captures(trimmed) {
            let file = caps.get(1).map_or("?", |m| m.as_str());
            let lineno = caps.get(2).map_or("?", |m| m.as_str());
            let code = caps.get(3).map_or("", |m| m.as_str());
            let msg = caps.get(4).map_or("", |m| m.as_str());
            let short = truncate_str(msg, 70);
            snippets.push(format!("  {file}:{lineno}  {code}: {short}"));
            pending_msg = None;
            if snippets.len() >= max {
                break;
            }
            continue;
        }

        // Generic "file:line:col: ERROR msg"
        if let Some(caps) = generic_err.captures(trimmed) {
            let file = caps.get(1).map_or("?", |m| m.as_str());
            let lineno = caps.get(2).map_or("?", |m| m.as_str());
            let msg = caps.get(3).map_or("", |m| m.as_str());
            let short = truncate_str(msg, 70);
            snippets.push(format!("  {file}:{lineno}  {short}"));
            pending_msg = None;
            if snippets.len() >= max {
                break;
            }
            continue;
        }

        // Vite/esbuild "[ERROR] msg" — store for next file:line line
        if let Some(caps) = vite_err.captures(trimmed) {
            let msg = caps
                .get(1)
                .or_else(|| caps.get(2))
                .map_or("", |m| m.as_str());
            pending_msg = Some(truncate_str(msg, 70));
            continue;
        }

        // Vite file location line (indented "  src/App.tsx:5:10:")
        if let Some(caps) = vite_loc.captures(line) {
            let file = caps.get(1).map_or("?", |m| m.as_str());
            let lineno = caps.get(2).map_or("?", |m| m.as_str());
            if let Some(msg) = pending_msg.take() {
                snippets.push(format!("  {file}:{lineno}  {msg}"));
                if snippets.len() >= max {
                    break;
                }
            }
            continue;
        }

        // Non-empty non-matching line clears pending
        if !trimmed.is_empty() && !trimmed.starts_with('│') {
            pending_msg = None;
        }
    }

    snippets
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_bun_output_strips_boilerplate() {
        let output = r#"
bun run v1.1.42
$ vue-tsc --noEmit
src/main.ts:12:3 - error TS7006: Parameter 'x' implicitly has an 'any' type.
Done in 0.27s
"#;

        let result = filter_bun_output(output);
        assert!(!result.contains("bun run v"));
        assert!(!result.contains("$ vue-tsc --noEmit"));
        assert!(!result.contains("Done in"));
        assert!(result.contains("TS7006"));
    }

    #[test]
    fn test_filter_bun_output_keeps_version_output() {
        let output = "1.2.0\n";
        let result = filter_bun_output(output);
        assert_eq!(result, "1.2.0");
    }

    #[test]
    fn test_summarize_bun_build_compact() {
        let output = r#"
bun run v1.2.0
$ vitest run && vite build
Test Files  1 passed (1)
Tests  10 passed (10)
✓ 2748 modules transformed.
✓ built in 4.88s
dist/assets/index.js 147.72 kB
"#;
        let args = vec!["run".to_string(), "build".to_string()];
        let result = summarize_bun_output(&args, output);
        assert!(result.contains("✓ bun run build"));
        assert!(result.contains("tests: 10/10"));
        assert!(result.contains("files: 1/1"));
        assert!(result.contains("modules: 2748"));
        assert!(result.contains("build: 4.88s"));
        assert!(result.contains("warnings: 0"));
        assert!(result.contains("errors: 0"));
        assert!(!result.contains("dist/assets/index.js"));
    }

    #[test]
    fn test_extract_bun_errors_typescript() {
        // changed: test TS error extraction
        let output = "
bun run v1.2.0
$ vue-tsc --noEmit
src/App.tsx:45:10 - error TS2345: Argument of type 'string' is not assignable to parameter of type 'number'.
src/components/Card.tsx:12:3 - error TS7006: Parameter 'x' implicitly has an 'any' type.
Found 2 errors. Watching for file changes.
";
        let snippets = extract_bun_errors(output, 5);
        assert_eq!(snippets.len(), 2, "should extract 2 TS errors");
        assert!(
            snippets[0].contains("src/App.tsx:45"),
            "should include file:line"
        );
        assert!(snippets[0].contains("TS2345"), "should include TS code");
        assert!(
            snippets[0].contains("Argument of type"),
            "should include message"
        );
        assert!(snippets[1].contains("src/components/Card.tsx:12"));
    }

    #[test]
    fn test_extract_bun_errors_vite() {
        // changed: test vite/esbuild error extraction
        let output = "
vite build
[ERROR] Cannot find name 'useState'

  src/hooks/useData.ts:8:10:
    8 │ const [x, setX] = useState();

Build failed with 1 error:
";
        let snippets = extract_bun_errors(output, 5);
        assert_eq!(snippets.len(), 1, "should extract 1 vite error");
        assert!(
            snippets[0].contains("src/hooks/useData.ts:8"),
            "should include file:line"
        );
        assert!(
            snippets[0].contains("Cannot find name"),
            "should include message"
        );
    }

    #[test]
    fn test_extract_bun_errors_max_limit() {
        // changed: test max limit on snippets
        let output = "
src/a.tsx:1:1 - error TS1001: err1
src/b.tsx:2:1 - error TS1002: err2
src/c.tsx:3:1 - error TS1003: err3
src/d.tsx:4:1 - error TS1004: err4
src/e.tsx:5:1 - error TS1005: err5
src/f.tsx:6:1 - error TS1006: err6
";
        let snippets = extract_bun_errors(output, 3);
        assert_eq!(snippets.len(), 3, "should respect max limit of 3");
    }

    #[test]
    fn test_summarize_bun_build_with_ts_error() {
        // changed: end-to-end test with errors
        let output = "
bun run v1.2.0
$ tsc --noEmit && vite build
src/App.tsx:45:10 - error TS2345: Argument of type 'string' is not assignable.
Found 1 error.
";
        let args = vec!["run".to_string(), "build".to_string()];
        let result = summarize_bun_output(&args, output);
        assert!(result.contains("errors: 1"), "should show error count");
        assert!(result.contains("src/App.tsx:45"), "should show file:line");
        assert!(result.contains("TS2345"), "should show TS code");
    }
}
