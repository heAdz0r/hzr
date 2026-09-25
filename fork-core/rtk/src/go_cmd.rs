use crate::tracking;
use crate::utils::truncate;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsString;
use std::process::Command;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GoTestEvent {
    #[serde(rename = "Time")]
    time: Option<String>,
    #[serde(rename = "Action")]
    action: String,
    #[serde(rename = "Package")]
    package: Option<String>,
    #[serde(rename = "Test")]
    test: Option<String>,
    #[serde(rename = "Output")]
    output: Option<String>,
    #[serde(rename = "Elapsed")]
    elapsed: Option<f64>,
    // 0.10.0: Go 1.24+ build events are keyed by ImportPath; the package fail names FailedBuild
    #[serde(rename = "ImportPath")]
    import_path: Option<String>,
    #[serde(rename = "FailedBuild")]
    failed_build: Option<String>,
}

#[derive(Debug, Default)]
struct PackageResult {
    pass: usize,
    fail: usize,
    skip: usize,
    build_failed: bool,        // 0.10.0: compile failure of this package
    build_errors: Vec<String>, // 0.10.0: compiler lines from build-output events
    failed_tests: Vec<(String, Vec<String>)>, // (test_name, output_lines)
    package_failed: bool,             // 0.10.0: timeout, signal or panic outside a test
    package_fail_output: Vec<String>, // 0.10.0: package-level output preceding that fail
}

pub fn run_test(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("go");
    cmd.arg("test");

    // Force JSON output if not already specified. 0.10.0: benchmarks keep their text
    // form (upstream v0.50.0): their ns/op lines are the result, not noise.
    let user_json = args.iter().any(|a| a == "-json");
    let inject_json = !user_json && !args.iter().any(|a| a.starts_with("-bench"));
    if inject_json {
        cmd.arg("-json");
    }

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: go test -json {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run go test. Is Go installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = crate::utils::make_raw(&stdout, &stderr); // fix #18: no double \n

    let exit_code = output // upstream sync: tee integration
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });
    // A run killed mid-stream (SIGKILL/OOM) leaves only passing events in the
    // NDJSON, so the parsed verdict is green while the process failed.
    let summary = if inject_json {
        filter_go_test_json(&stdout)
    } else {
        raw.clone()
    };
    let filtered = crate::guard::guard_exit(&raw, exit_code, "go test", &summary);

    let hint = crate::tee::tee_and_hint(&raw, "go_test", exit_code);
    // 0.10.0: NDJSON that rtk injected is not the caller's machine protocol, and its test
    // output words are not a failure signal (guard_exit owns the exit verdict). The plain
    // guard returned all of it — 5.4 MB on a red Go 1.26 module.
    let shown = if inject_json {
        crate::runner::emit_guarded_rendered(&filtered, hint.as_deref(), &raw)
    } else {
        crate::runner::emit_guarded(&filtered, hint.as_deref(), &raw)
    };

    // Include stderr if present (build errors, etc.)
    if !stderr.trim().is_empty() {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        &format!("go test {}", args.join(" ")),
        &format!("rtk go test {}", args.join(" ")),
        &raw,
        &shown,
    );

    // Preserve exit code for CI/CD
    if !output.status.success() {
        std::process::exit(exit_code); // upstream sync: use exit_code
    }

    Ok(())
}

pub fn run_build(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("go");
    cmd.arg("build");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: go build {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run go build. Is Go installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = crate::utils::make_raw(&stdout, &stderr); // fix #18: no double \n

    let exit_code = output // upstream sync: tee integration
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });
    let filtered = filter_go_build(&raw);

    let hint = crate::tee::tee_and_hint(&raw, "go_build", exit_code);
    let shown = crate::runner::emit_guarded(&filtered, hint.as_deref(), &raw);

    timer.track(
        &format!("go build {}", args.join(" ")),
        &format!("rtk go build {}", args.join(" ")),
        &raw,
        &shown,
    );

    // Preserve exit code for CI/CD
    if !output.status.success() {
        std::process::exit(exit_code); // upstream sync: use exit_code
    }

    Ok(())
}

pub fn run_vet(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("go");
    cmd.arg("vet");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: go vet {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run go vet. Is Go installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = crate::utils::make_raw(&stdout, &stderr); // fix #18: no double \n

    let exit_code = output // upstream sync: tee integration
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });
    let filtered = crate::guard::guard_exit(&raw, exit_code, "go vet", &filter_go_vet(&raw));

    let hint = crate::tee::tee_and_hint(&raw, "go_vet", exit_code);
    let shown = crate::runner::emit_guarded(&filtered, hint.as_deref(), &raw);

    timer.track(
        &format!("go vet {}", args.join(" ")),
        &format!("rtk go vet {}", args.join(" ")),
        &raw,
        &shown,
    );

    // Preserve exit code for CI/CD
    if !output.status.success() {
        std::process::exit(exit_code); // upstream sync: use exit_code
    }

    Ok(())
}

pub fn run_run(args: &[String], verbose: u8) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("go run: requires a package path or file");
    }

    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("go");
    cmd.arg("run");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: go run {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run go run. Is Go installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = crate::utils::make_raw(&stdout, &stderr); // fix #18: no double \n

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    let filtered = if output.status.success() {
        // On success: suppress noisy startup lines, show meaningful output
        let lines: Vec<&str> = stdout
            .lines()
            .filter(|l| !l.trim().is_empty())
            .take(10)
            .collect();
        if lines.is_empty() {
            "✓ go run: ok".to_string()
        } else {
            lines.join("\n")
        }
    } else {
        // On failure: show build errors (reuse go build filter logic)
        let errors = filter_go_build(&raw);
        if errors.contains("✓") {
            // filter_go_build returned success despite non-zero exit — show raw stderr
            stderr.trim().to_string()
        } else {
            errors
        }
    };

    let hint = crate::tee::tee_and_hint(&raw, "go_run", exit_code);
    let shown = crate::runner::emit_guarded(&filtered, hint.as_deref(), &raw);

    timer.track(
        &format!("go run {}", args.join(" ")),
        &format!("rtk go run {}", args.join(" ")),
        &raw,
        &shown,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

pub fn run_other(args: &[OsString], verbose: u8) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("go: no subcommand specified");
    }

    let timer = tracking::TimedExecution::start();

    let subcommand = args[0].to_string_lossy();
    let mut cmd = Command::new("go");
    cmd.arg(&*subcommand);

    for arg in &args[1..] {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: go {} ...", subcommand);
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run go {}", subcommand))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = crate::utils::make_raw(&stdout, &stderr); // fix #18: no double \n

    print!("{}", stdout);
    eprint!("{}", stderr);

    timer.track(
        &format!("go {}", subcommand),
        &format!("rtk go {}", subcommand),
        &raw,
        &raw, // No filtering for unsupported commands
    );

    // Preserve exit code
    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

/// Parse go test -json output (NDJSON format)
///
/// 0.10.0: ported from upstream RTK v0.50.0. Go 1.24+ reports compile failures as
/// `build-output`/`build-fail` events keyed by `ImportPath` (no `Package`), followed by a
/// package `fail` carrying `FailedBuild`; the 0.44 parser dropped all of them, so a red
/// build rendered as a partial or empty summary. Package-level failures without a failing
/// test (timeout, panic in init, signal) are counted too.
pub(crate) fn filter_go_test_json(output: &str) -> String {
    let mut packages: HashMap<String, PackageResult> = HashMap::new();
    let mut current_test_output: HashMap<(String, String), Vec<String>> = HashMap::new(); // (package, test) -> outputs
    let mut build_output: HashMap<String, Vec<String>> = HashMap::new(); // import_path -> error lines

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let event: GoTestEvent = match serde_json::from_str(trimmed) {
            Ok(e) => e,
            Err(_) => continue, // Skip non-JSON lines
        };

        match event.action.as_str() {
            "build-output" => {
                if let (Some(import_path), Some(output_text)) = (&event.import_path, &event.output)
                {
                    let text = output_text.trim_end().to_string();
                    if !text.is_empty() {
                        build_output
                            .entry(import_path.clone())
                            .or_default()
                            .push(text);
                    }
                }
                continue;
            }
            // The package-level `fail` with `FailedBuild` that follows carries the verdict.
            "build-fail" => continue,
            _ => {}
        }

        let package = event.package.unwrap_or_else(|| "unknown".to_string());
        let pkg_result = packages.entry(package.clone()).or_default();

        match event.action.as_str() {
            "pass" if event.test.is_some() => {
                pkg_result.pass += 1;
            }
            "fail" => {
                if let Some(test) = &event.test {
                    pkg_result.fail += 1;
                    let key = (package.clone(), test.clone());
                    let outputs = current_test_output.remove(&key).unwrap_or_default();
                    pkg_result.failed_tests.push((test.clone(), outputs));
                } else if let Some(import_path) = &event.failed_build {
                    pkg_result.build_failed = true;
                    if let Some(errors) = build_output.remove(import_path) {
                        pkg_result.build_errors = errors;
                    }
                } else {
                    pkg_result.package_failed = true;
                }
            }
            "skip" if event.test.is_some() => {
                pkg_result.skip += 1;
            }
            "output" => {
                if let Some(output_text) = &event.output {
                    if let Some(test) = &event.test {
                        let key = (package.clone(), test.clone());
                        current_test_output
                            .entry(key)
                            .or_default()
                            .push(output_text.trim_end().to_string());
                    } else {
                        let text = output_text.trim();
                        if !text.is_empty() {
                            pkg_result.package_fail_output.push(text.to_string());
                        }
                    }
                }
            }
            _ => {} // run, pause, cont, start, etc.
        }
    }

    // A build error reported for an import path whose package never got a verdict
    // (older toolchains) still belongs in the summary.
    for (import_path, errors) in build_output {
        let pkg_result = packages.entry(import_path).or_default();
        if !pkg_result.build_failed {
            pkg_result.build_failed = true;
            pkg_result.build_errors = errors;
        }
    }

    let total_packages = packages.len();
    let total_pass: usize = packages.values().map(|p| p.pass).sum();
    let total_fail: usize = packages.values().map(|p| p.fail).sum();
    let total_skip: usize = packages.values().map(|p| p.skip).sum();
    let total_build_fail = packages.values().filter(|p| p.build_failed).count();
    // go test -json emits a package-level `fail` after any failing test as a cascade; only a
    // package with no failing test and no build error failed on its own.
    let total_pkg_fail = packages
        .values()
        .filter(|p| p.package_failed && p.fail == 0 && !p.build_failed)
        .count();
    let has_failures = total_fail > 0 || total_build_fail > 0 || total_pkg_fail > 0;

    if !has_failures && total_pass == 0 {
        return "Go test: No tests found".to_string();
    }

    if !has_failures {
        return format!(
            "✓ Go test: {} passed in {} packages",
            total_pass, total_packages
        );
    }

    let mut result = format!(
        "Go test: {} passed, {} failed",
        total_pass,
        total_fail + total_build_fail + total_pkg_fail
    );
    if total_skip > 0 {
        result.push_str(&format!(", {} skipped", total_skip));
    }
    result.push_str(&format!(" in {} packages\n", total_packages));

    let mut pkg_list: Vec<_> = packages.iter().collect(); // fix #6: sort for deterministic output
    pkg_list.sort_by_key(|(name, _)| *name);

    for (package, pkg_result) in &pkg_list {
        if !pkg_result.package_failed || pkg_result.fail > 0 || pkg_result.build_failed {
            continue;
        }
        result.push_str(&format!("\n{} [FAIL]\n", compact_package_name(package)));
        for line in pkg_result.package_fail_output.iter().rev().take(5).rev() {
            result.push_str(&format!("  {}\n", truncate(line, 120)));
        }
    }

    for (package, pkg_result) in &pkg_list {
        if !pkg_result.build_failed {
            continue;
        }
        result.push_str(&format!(
            "\n{} [build failed]\n",
            compact_package_name(package)
        ));
        for line in &pkg_result.build_errors {
            let trimmed = line.trim();
            if !trimmed.starts_with('#') && !trimmed.is_empty() {
                result.push_str(&format!("  {}\n", truncate(trimmed, 160)));
            }
        }
    }

    for (package, pkg_result) in &pkg_list {
        if pkg_result.fail == 0 {
            continue;
        }
        result.push_str(&format!(
            "\n{} ({} passed, {} failed)\n",
            compact_package_name(package),
            pkg_result.pass,
            pkg_result.fail
        ));
        for (test, outputs) in &pkg_result.failed_tests {
            result.push_str(&format!("  [FAIL] {}\n", test));
            for line in select_go_test_failure_lines(outputs) {
                result.push_str(&format!("     {}\n", truncate(&line, 160)));
            }
        }
    }

    result.trim().to_string()
}

/// 0.10.0 (upstream v0.50.0): keep a failure's location line and the line after it, plus
/// assertion vocabulary; fall back to the first meaningful line so a failure is never bare.
fn select_go_test_failure_lines(outputs: &[String]) -> Vec<String> {
    let mut relevant = Vec::new();
    let mut keep_next_context_line = false;

    for line in outputs {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("=== RUN")
            || trimmed.starts_with("--- FAIL")
            || trimmed.starts_with("--- PASS")
        {
            keep_next_context_line = false;
            continue;
        }

        let is_location = is_go_test_location_line(trimmed);
        if is_location || is_go_test_failure_line(trimmed) || keep_next_context_line {
            relevant.push(trimmed.to_string());
            keep_next_context_line = is_location;
        } else {
            keep_next_context_line = false;
        }

        if relevant.len() >= 5 {
            break;
        }
    }

    if relevant.is_empty() {
        if let Some(line) = outputs.iter().map(|line| line.trim()).find(|line| {
            !line.is_empty()
                && !line.starts_with("=== RUN")
                && !line.starts_with("--- FAIL")
                && !line.starts_with("--- PASS")
        }) {
            relevant.push(line.to_string());
        }
    }

    relevant
}

fn is_go_test_location_line(line: &str) -> bool {
    line.split_once(".go:")
        .and_then(|(_, rest)| rest.chars().next())
        .is_some_and(|c| c.is_ascii_digit())
}

fn is_go_test_failure_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.starts_with("panic:")
        || lower.starts_with("error:")
        || lower.contains(" error:")
        || lower.contains("expected")
        || lower.contains("got")
        || lower.contains("want")
        || lower.contains("actual")
        || lower.contains("assert")
        || lower.contains("mismatch")
        || lower.contains("unexpected")
        || lower.contains("fatal")
        || line.starts_with("at ")
}

/// Filter go build output - show only errors
pub(crate) fn filter_go_build(output: &str) -> String {
    let mut errors: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();

        // Skip package markers (# package/name lines without errors)
        if trimmed.starts_with('#') && !lower.contains("error") {
            continue;
        }

        // Collect error lines (file:line:col format or error keywords)
        if !trimmed.is_empty()
            && (lower.contains("error")
                || trimmed.contains(".go:")
                || lower.contains("undefined")
                || lower.contains("cannot"))
        {
            errors.push(trimmed.to_string());
        }
    }

    if errors.is_empty() {
        return "✓ Go build: Success".to_string();
    }

    let mut result = String::new();
    result.push_str(&format!("Go build: {} errors\n", errors.len()));
    // 0.10.0: no decorative rule line (120 bytes of box drawing per summary)

    for (i, error) in errors.iter().take(20).enumerate() {
        result.push_str(&format!("{}. {}\n", i + 1, truncate(error, 120)));
    }

    if errors.len() > 20 {
        result.push_str(&format!("\n... +{} more errors\n", errors.len() - 20));
    }

    result.trim().to_string()
}

/// Filter go vet output - show issues
fn filter_go_vet(output: &str) -> String {
    let mut issues: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Collect issue lines (vet reports issues with file:line:col format)
        if !trimmed.is_empty() && !trimmed.starts_with('#') && trimmed.contains(".go:") {
            issues.push(trimmed.to_string());
        }
    }

    if issues.is_empty() {
        return "✓ Go vet: No issues found".to_string();
    }

    let mut result = String::new();
    result.push_str(&format!("Go vet: {} issues\n", issues.len()));
    // 0.10.0: no decorative rule line (120 bytes of box drawing per summary)

    for (i, issue) in issues.iter().take(20).enumerate() {
        result.push_str(&format!("{}. {}\n", i + 1, truncate(issue, 160))); // 0.10.0: keep the diagnosis
    }

    if issues.len() > 20 {
        result.push_str(&format!("\n... +{} more issues\n", issues.len() - 20));
    }

    result.trim().to_string()
}

/// Compact package name (remove long paths)
fn compact_package_name(package: &str) -> String {
    // Remove common module prefixes like github.com/user/repo/
    if let Some(pos) = package.rfind('/') {
        package[pos + 1..].to_string()
    } else {
        package.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 0.10.0: real `go test -json` from go1.25.1 — one package fails to compile, one test
    // fails, one package passes. The 0.44 parser ignored the build events entirely.
    #[test]
    fn go124_build_events_are_summarised_with_the_compiler_error() {
        let raw = include_str!("../tests/fixtures/go_test_go125_build_and_test_fail.json");
        let summary = filter_go_test_json(raw);
        assert!(summary.starts_with("Go test: 3 passed, 2 failed in 3 packages"), "{summary}");
        assert!(summary.contains("c [build failed]"), "{summary}");
        assert!(summary.contains("c/c.go:4:9: cannot use"), "{summary}");
        assert!(summary.contains("[FAIL] TestFail"), "{summary}");
        assert!(summary.contains("a_test.go:10: expected 1, got 2"), "{summary}");
        assert!(summary.len() < 600, "{} bytes", summary.len());
        assert_eq!(
            crate::guard::never_worse_rendered(raw, &summary),
            summary,
            "self-injected NDJSON must not win the guard"
        );
    }

    #[test]
    fn package_level_timeout_counts_as_a_failure() {
        let raw = r#"{"Action":"start","Package":"example.com/slow"}
{"Action":"output","Package":"example.com/slow","Output":"panic: test timed out after 1s\n"}
{"Action":"fail","Package":"example.com/slow","Elapsed":1.0}"#;
        let summary = filter_go_test_json(raw);
        assert!(summary.contains("1 failed"), "{summary}");
        assert!(summary.contains("slow [FAIL]"), "{summary}");
        assert!(summary.contains("timed out"), "{summary}");
    }

    #[test]
    fn test_filter_go_test_all_pass() {
        let output = r#"{"Time":"2024-01-01T10:00:00Z","Action":"run","Package":"example.com/foo","Test":"TestBar"}
{"Time":"2024-01-01T10:00:01Z","Action":"output","Package":"example.com/foo","Test":"TestBar","Output":"=== RUN   TestBar\n"}
{"Time":"2024-01-01T10:00:02Z","Action":"pass","Package":"example.com/foo","Test":"TestBar","Elapsed":0.5}
{"Time":"2024-01-01T10:00:02Z","Action":"pass","Package":"example.com/foo","Elapsed":0.5}"#;

        let result = filter_go_test_json(output);
        assert!(result.contains("✓ Go test"));
        assert!(result.contains("1 passed"));
        assert!(result.contains("1 packages"));
    }

    #[test]
    fn test_filter_go_test_with_failures() {
        let output = r#"{"Time":"2024-01-01T10:00:00Z","Action":"run","Package":"example.com/foo","Test":"TestFail"}
{"Time":"2024-01-01T10:00:01Z","Action":"output","Package":"example.com/foo","Test":"TestFail","Output":"=== RUN   TestFail\n"}
{"Time":"2024-01-01T10:00:02Z","Action":"output","Package":"example.com/foo","Test":"TestFail","Output":"    Error: expected 5, got 3\n"}
{"Time":"2024-01-01T10:00:03Z","Action":"fail","Package":"example.com/foo","Test":"TestFail","Elapsed":0.5}
{"Time":"2024-01-01T10:00:03Z","Action":"fail","Package":"example.com/foo","Elapsed":0.5}"#;

        let result = filter_go_test_json(output);
        assert!(result.contains("1 failed"));
        assert!(result.contains("TestFail"));
        assert!(result.contains("expected 5, got 3"));
    }

    #[test]
    fn test_filter_go_build_success() {
        let output = "";
        let result = filter_go_build(output);
        assert!(result.contains("✓ Go build"));
        assert!(result.contains("Success"));
    }

    #[test]
    fn test_filter_go_build_errors() {
        let output = r#"# example.com/foo
main.go:10:5: undefined: missingFunc
main.go:15:2: cannot use x (type int) as type string"#;

        let result = filter_go_build(output);
        assert!(result.contains("2 errors"));
        assert!(result.contains("undefined: missingFunc"));
        assert!(result.contains("cannot use x"));
    }

    #[test]
    fn test_filter_go_vet_no_issues() {
        let output = "";
        let result = filter_go_vet(output);
        assert!(result.contains("Go vet"));
        assert!(result.contains("No issues found"));
    }

    #[test]
    fn test_filter_go_vet_with_issues() {
        let output = r#"main.go:42:2: Printf format %d has arg x of wrong type string
utils.go:15:5: unreachable code"#;

        let result = filter_go_vet(output);
        assert!(result.contains("2 issues"));
        assert!(result.contains("Printf format"));
        assert!(result.contains("unreachable code"));
    }

    #[test]
    fn test_compact_package_name() {
        assert_eq!(compact_package_name("github.com/user/repo/pkg"), "pkg");
        assert_eq!(compact_package_name("example.com/foo"), "foo");
        assert_eq!(compact_package_name("simple"), "simple");
    }

    #[test]
    // renamed #10: was test_run_run_success_with_output
    fn test_filter_go_build_empty_is_success() {
        // Simulate successful go run output: first 10 non-empty lines shown
        let stdout = "server started on :8080\nlistening...\n";
        let stderr = "";
        let raw = crate::utils::make_raw(&stdout, &stderr); // fix #18: no double \n
                                                            // filter_go_build would return success for empty stderr+stdout
                                                            // Here we test the success branch logic via filter_go_build
        let result = filter_go_build(&raw);
        assert!(result.contains("✓"), "empty output should show success");
    }

    #[test]
    // renamed #10: was test_run_run_build_error_filter
    fn test_filter_go_build_errors_from_run_output() {
        // Simulate go run compilation failure
        let output = r#"# example.com/cmd/serve
cmd/serve/main.go:42:5: undefined: missingFunc
cmd/serve/main.go:50:2: cannot use x (type int) as type string"#;
        let result = filter_go_build(output);
        assert!(result.contains("2 errors"));
        assert!(result.contains("undefined: missingFunc"));
        assert!(result.contains("cannot use x"));
    }
}
