use crate::stream::RelayedOutput; // 0.11.0 (US-007): relay signals while capturing
use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;

use crate::parser::{
    emit_degradation_warning, emit_passthrough_warning, extract_json_object, truncate_output,
    FormatMode, OutputParser, ParseResult, TestFailure, TestResult, TokenFormatter,
};
use crate::tracking;
use crate::utils::{package_manager_exec, strip_ansi};

/// Vitest JSON output structures (tool-specific format)
#[derive(Debug, Deserialize)]
struct VitestJsonOutput {
    #[serde(rename = "testResults")]
    test_results: Vec<VitestTestFile>,
    #[serde(rename = "numTotalTests")]
    num_total_tests: usize,
    #[serde(rename = "numPassedTests")]
    num_passed_tests: usize,
    #[serde(rename = "numFailedTests")]
    num_failed_tests: usize,
    #[serde(rename = "numPendingTests", default)]
    num_pending_tests: usize,
    #[serde(rename = "startTime")]
    start_time: Option<u64>,
    #[serde(rename = "endTime")]
    end_time: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct VitestTestFile {
    name: String,
    #[serde(rename = "assertionResults")]
    assertion_results: Vec<VitestTest>,
    // 0.11.0 (upstream PR #4184): a suite that fails to load has no assertions;
    // its status and message are the whole report.
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VitestTest {
    #[serde(rename = "fullName")]
    full_name: String,
    status: String,
    #[serde(rename = "failureMessages")]
    failure_messages: Vec<String>,
}

/// Parser for Vitest JSON output
pub struct VitestParser;

impl OutputParser for VitestParser {
    type Output = TestResult;

    fn parse(input: &str) -> ParseResult<TestResult> {
        // Tier 1: Try JSON parsing (with extraction fallback for pnpm/dotenv prefixes)
        let json_result = serde_json::from_str::<VitestJsonOutput>(input).or_else(|first_err| {
            // Fallback: Try extracting JSON object from prefixed output
            if let Some(extracted) = extract_json_object(input) {
                serde_json::from_str::<VitestJsonOutput>(extracted)
            } else {
                Err(first_err)
            }
        });

        match json_result {
            Ok(json) => {
                let failures = extract_failures_from_json(&json);
                let duration_ms = match (json.start_time, json.end_time) {
                    (Some(start), Some(end)) => Some(end.saturating_sub(start)),
                    _ => None,
                };

                // 0.11.0 (upstream PR #4184): suites that failed to load count as
                // failures, so the summary is never "PASS (0) FAIL (0)" on exit 1.
                let suite_failures = failures
                    .iter()
                    .filter(|f| f.test_name.ends_with(SUITE_LOAD_FAILURE))
                    .count();
                let result = TestResult {
                    total: json.num_total_tests,
                    passed: json.num_passed_tests,
                    failed: json.num_failed_tests + suite_failures,
                    skipped: json.num_pending_tests,
                    duration_ms,
                    failures,
                };

                ParseResult::Full(result)
            }
            Err(e) => {
                // Tier 2: Try regex extraction (only fires if user overrides --reporter flag)
                match extract_stats_regex(input) {
                    Some(result) => {
                        ParseResult::Degraded(result, vec![format!("JSON parse failed: {}", e)])
                    }
                    None => {
                        // Tier 3: Passthrough
                        ParseResult::Passthrough(truncate_output(input, 500))
                    }
                }
            }
        }
    }
}

/// Name given to a test file that failed before any test ran. (0.11.0, upstream PR #4184)
const SUITE_LOAD_FAILURE: &str = "(suite failed to load)";

/// Extract failures from JSON structure
fn extract_failures_from_json(json: &VitestJsonOutput) -> Vec<TestFailure> {
    let mut failures = Vec::new();

    for file in &json.test_results {
        let file_failed = file.status.as_deref() == Some("failed");
        let has_failed_test = file.assertion_results.iter().any(|t| t.status == "failed");
        if file_failed && !has_failed_test {
            // 0.11.0 (upstream PR #4184)
            failures.push(TestFailure {
                test_name: format!("{} {SUITE_LOAD_FAILURE}", file.name),
                file_path: file.name.clone(),
                error_message: file.message.clone().unwrap_or_default(),
                stack_trace: None,
            });
        }
        for test in &file.assertion_results {
            if test.status == "failed" {
                let error_message = test.failure_messages.join("\n");
                failures.push(TestFailure {
                    test_name: test.full_name.clone(),
                    file_path: file.name.clone(),
                    error_message,
                    stack_trace: None,
                });
            }
        }
    }

    failures
}

/// Tier 2: Extract test statistics using regex (degraded mode)
fn extract_stats_regex(output: &str) -> Option<TestResult> {
    lazy_static::lazy_static! {
        static ref TEST_FILES_RE: Regex = Regex::new(
            r"Test Files\s+(?:(\d+)\s+failed\s+\|\s+)?(\d+)\s+passed"
        ).unwrap();
        static ref TESTS_RE: Regex = Regex::new(
            r"Tests\s+(?:(\d+)\s+failed\s+\|\s+)?(\d+)\s+passed"
        ).unwrap();
        static ref DURATION_RE: Regex = Regex::new(
            r"Duration\s+([\d.]+)(ms|s)"
        ).unwrap();
    }

    let clean_output = strip_ansi(output);

    let mut passed = 0;
    let mut failed = 0;
    let mut total = 0;

    // Parse test counts
    if let Some(caps) = TESTS_RE.captures(&clean_output) {
        if let Some(fail_str) = caps.get(1) {
            failed = fail_str.as_str().parse().unwrap_or(0);
        }
        if let Some(pass_str) = caps.get(2) {
            passed = pass_str.as_str().parse().unwrap_or(0);
        }
        total = passed + failed;
    }

    // Parse duration
    let duration_ms = DURATION_RE.captures(&clean_output).and_then(|caps| {
        let value: f64 = caps[1].parse().ok()?;
        let unit = &caps[2];
        Some(if unit == "ms" {
            value as u64
        } else {
            (value * 1000.0) as u64
        })
    });

    // Only return if we found valid data
    if total > 0 {
        Some(TestResult {
            total,
            passed,
            failed,
            skipped: 0,
            duration_ms,
            failures: extract_failures_regex(&clean_output),
        })
    } else {
        None
    }
}

/// Extract failures using regex
fn extract_failures_regex(output: &str) -> Vec<TestFailure> {
    let mut failures = Vec::new();
    let lines: Vec<&str> = output.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if line.contains('✗') || line.contains("FAIL") {
            let mut error_lines = vec![line.to_string()];
            i += 1;

            // Collect subsequent indented lines
            while i < lines.len() && lines[i].starts_with("  ") {
                error_lines.push(lines[i].trim().to_string());
                i += 1;
            }

            if !error_lines.is_empty() {
                failures.push(TestFailure {
                    test_name: error_lines[0].clone(),
                    file_path: String::new(),
                    error_message: error_lines[1..].join("\n"),
                    stack_trace: None,
                });
            }
        } else {
            i += 1;
        }
    }

    failures
}

#[derive(Debug, Clone)]
pub enum VitestCommand {
    Run,
}

pub fn run(cmd: VitestCommand, args: &[String], verbose: u8) -> Result<()> {
    match cmd {
        VitestCommand::Run => run_vitest(args, verbose),
    }
}

fn run_vitest(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = package_manager_exec("vitest");
    let effective_args = build_vitest_effective_args(args);
    cmd.args(&effective_args.args);
    // 0.11.0 (upstream PR #4264): vitest 5 writes the JSON reporter's report to a file
    // and prints only "JSON report written to <path>", so every parser tier failed on
    // stdout. Send the report to a temp file (vitest 4 honours the same option) unless
    // the caller chose an output file, and read it from there.
    let report_path = (!effective_args.passthrough && !has_explicit_output_file(args)).then(|| {
        std::env::temp_dir().join(format!(
            "rtk-vitest-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ))
    });
    if let Some(path) = &report_path {
        cmd.arg(format!("--outputFile.json={}", path.display()));
    }

    let output = cmd.output_relayed().context("Failed to run vitest")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = report_path.as_ref().and_then(|path| {
        let text = std::fs::read_to_string(path).ok();
        let _ = std::fs::remove_file(path);
        text.filter(|t| !t.trim().is_empty())
    });
    // The report counts as raw output: the guard, the recovery tee and the savings
    // baseline all measure against everything vitest produced.
    let combined = match &report {
        Some(report) => format!("{report}\n{stdout}{stderr}"),
        None => format!("{}{}", stdout, stderr),
    };
    let parse_source = report.as_deref().unwrap_or(&stdout);

    let filtered = format_test_output(parse_source, &combined, effective_args.passthrough, verbose);

    let exit_code = output.status.code().unwrap_or(1); // upstream sync: tee integration
                                                       // A failed suite that the JSON parser could not attribute must not surface
                                                       // as a green run just because no individual test was marked failing.
    let filtered = FormattedTestOutput {
        text: crate::guard::guard_exit(&combined, exit_code, "vitest", &filtered.text),
        ..filtered
    };
    let rendered = render_test_output(&filtered, &combined, "vitest_run", exit_code);
    // 0.11.0: the JSON report is rtk's own injected format, not the caller's protocol.
    // `never_worse` treated an all-JSON output as a machine protocol and returned the
    // raw report whenever vitest printed nothing else — the filter never ran on a clean
    // vitest 4 run. The exit verdict is enforced by guard_exit above.
    let shown = if effective_args.passthrough {
        crate::guard::never_worse(&combined, &rendered)
    } else {
        crate::guard::never_worse_rendered(&combined, &rendered)
    };
    if shown.ends_with('\n') {
        print!("{}", shown);
    } else {
        println!("{}", shown);
    }

    timer.track("vitest run", "rtk vitest run", &combined, shown);

    // Propagate original exit code
    std::process::exit(exit_code) // upstream sync: use exit_code
}

struct EffectiveVitestArgs {
    args: Vec<String>,
    passthrough: bool,
}

/// vitest's own subcommands keep the leading position; `run` in front of them made
/// the word a test-name filter. (0.11.0, upstream PR #3680)
const VITEST_SUBCOMMANDS: &[&str] = &["related", "bench", "list", "typecheck", "init"];

fn build_vitest_effective_args(args: &[String]) -> EffectiveVitestArgs {
    if args.first().is_some_and(|a| VITEST_SUBCOMMANDS.contains(&a.as_str())) {
        // 0.11.0 (upstream PR #3680): unfiltered — their output is not a test report.
        return EffectiveVitestArgs {
            args: args.to_vec(),
            passthrough: true,
        };
    }
    let passthrough = has_explicit_vitest_reporter(args);
    let mut effective = vec!["run".to_string()];

    if !passthrough {
        effective.push("--reporter=json".to_string());
    }

    effective.extend(
        args.iter()
            .filter(|arg| !should_skip_vitest_arg(arg))
            .cloned(),
    );

    EffectiveVitestArgs {
        args: effective,
        passthrough,
    }
}

/// The caller named where reports go: `--outputFile`, `--outputFile=…`, `--outputFile.json…`.
fn has_explicit_output_file(args: &[String]) -> bool {
    args.iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "--outputFile" || a.starts_with("--outputFile=") || a.starts_with("--outputFile."))
}

fn has_explicit_vitest_reporter(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--reporter" || arg.starts_with("--reporter="))
}

fn should_skip_vitest_arg(arg: &str) -> bool {
    arg == "run" || arg.starts_with("--json") || arg.starts_with("--watch")
}

struct FormattedTestOutput {
    text: String,
    truncated: bool,
}

fn format_test_output(
    stdout: &str,
    combined: &str,
    passthrough_requested: bool,
    verbose: u8,
) -> FormattedTestOutput {
    if passthrough_requested {
        return format_passthrough_output(combined, 500);
    }

    let mode = FormatMode::from_verbosity(verbose);
    match VitestParser::parse(stdout) {
        ParseResult::Full(data) => {
            if verbose > 0 {
                eprintln!("vitest run (Tier 1: Full JSON parse)");
            }
            FormattedTestOutput {
                text: data.format(mode),
                truncated: false,
            }
        }
        ParseResult::Degraded(data, warnings) => {
            if verbose > 0 {
                emit_degradation_warning("vitest", &warnings.join(", "));
            }
            FormattedTestOutput {
                text: data.format(mode),
                truncated: false,
            }
        }
        ParseResult::Passthrough(_) => {
            emit_passthrough_warning("vitest", "All parsing tiers failed");
            format_passthrough_output(stdout, 500)
        }
    }
}

fn format_passthrough_output(raw: &str, max_chars: usize) -> FormattedTestOutput {
    FormattedTestOutput {
        text: truncate_output(raw, max_chars),
        truncated: raw.chars().count() > max_chars,
    }
}

fn render_test_output(
    filtered: &FormattedTestOutput,
    raw: &str,
    tee_label: &str,
    exit_code: i32,
) -> String {
    let hint = if filtered.truncated {
        crate::tee::force_tee_hint(raw, tee_label)
    } else {
        crate::tee::tee_and_hint(raw, tee_label, exit_code)
    };

    match hint {
        Some(hint) => format!("{}\n{}", filtered.text, hint),
        None => filtered.text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 0.11.0 (upstream PR #4184): a suite that failed to load is a failure.
    #[test]
    fn test_vitest_suite_load_failure_is_reported() {
        let json = r#"{"numTotalTestSuites":1,"numFailedTestSuites":1,"numTotalTests":0,"numPassedTests":0,"numFailedTests":0,"numPendingTests":0,"testResults":[{"name":"/p/broken.test.ts","status":"failed","message":"Failed to load url ./missing","assertionResults":[]}]}"#;
        match VitestParser::parse(json) {
            ParseResult::Full(result) => {
                assert_eq!(result.failed, 1);
                assert_eq!(result.failures[0].file_path, "/p/broken.test.ts");
                assert!(result.failures[0].error_message.contains("Failed to load url"));
            }
            _ => panic!("expected a full parse"),
        }
    }

    // 0.11.0 (upstream PR #3680): vitest subcommands are not buried under `run`.
    #[test]
    fn test_vitest_subcommands_keep_their_position() {
        for sub in ["list", "related", "bench", "typecheck", "init"] {
            let built = build_vitest_effective_args(&[sub.to_string(), "src/a.ts".to_string()]);
            assert_eq!(built.args, vec![sub.to_string(), "src/a.ts".to_string()]);
            assert!(built.passthrough, "{sub}");
        }
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn test_vitest_parser_json() {
        let json = r#"{
            "numTotalTests": 13,
            "numPassedTests": 13,
            "numFailedTests": 0,
            "numPendingTests": 0,
            "testResults": [],
            "startTime": 1000,
            "endTime": 1450
        }"#;

        let result = VitestParser::parse(json);
        assert_eq!(result.tier(), 1);
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.total, 13);
        assert_eq!(data.passed, 13);
        assert_eq!(data.failed, 0);
        assert_eq!(data.duration_ms, Some(450));
    }

    #[test]
    fn test_vitest_parser_regex_fallback() {
        let text = r#"
 Test Files  2 passed (2)
      Tests  13 passed (13)
   Duration  450ms
        "#;

        let result = VitestParser::parse(text);
        assert_eq!(result.tier(), 2); // Degraded
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.passed, 13);
        assert_eq!(data.failed, 0);
    }

    #[test]
    fn test_vitest_parser_passthrough() {
        let invalid = "random output with no structure";
        let result = VitestParser::parse(invalid);
        assert_eq!(result.tier(), 3); // Passthrough
        assert!(!result.is_ok());
    }

    #[test]
    fn test_strip_ansi() {
        let input = "\x1b[32m✓\x1b[0m test passed";
        let output = strip_ansi(input);
        assert_eq!(output, "✓ test passed");
        assert!(!output.contains("\x1b"));
    }

    #[test]
    fn test_vitest_parser_with_pnpm_prefix() {
        let input = r#"
Scope: all 6 workspace projects
 WARN  deprecated inflight@1.0.6: This module is not supported

{"numTotalTests": 13, "numPassedTests": 13, "numFailedTests": 0, "numPendingTests": 0, "testResults": [], "startTime": 1000, "endTime": 1450}
"#;
        let result = VitestParser::parse(input);
        assert_eq!(result.tier(), 1, "Should succeed with Tier 1 (full parse)");
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.total, 13);
        assert_eq!(data.passed, 13);
        assert_eq!(data.failed, 0);
    }

    #[test]
    fn test_vitest_parser_with_dotenv_prefix() {
        let input = r#"[dotenv] Loading environment variables from .env
[dotenv] Injected 5 variables

{"numTotalTests": 5, "numPassedTests": 4, "numFailedTests": 1, "numPendingTests": 0, "testResults": [], "startTime": 2000, "endTime": 2300}
"#;
        let result = VitestParser::parse(input);
        assert_eq!(result.tier(), 1, "Should succeed with Tier 1 (full parse)");
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.total, 5);
        assert_eq!(data.passed, 4);
        assert_eq!(data.failed, 1);
        assert_eq!(data.duration_ms, Some(300));
    }

    #[test]
    fn test_vitest_parser_with_nested_json() {
        let input = r#"prefix text
{"numTotalTests": 2, "numPassedTests": 2, "numFailedTests": 0, "numPendingTests": 0, "testResults": [{"name": "test.js", "assertionResults": [{"fullName": "nested test", "status": "passed", "failureMessages": []}]}], "startTime": 1000, "endTime": 1100}
"#;
        let result = VitestParser::parse(input);
        assert_eq!(result.tier(), 1, "Should succeed with Tier 1 (full parse)");
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.total, 2);
        assert_eq!(data.passed, 2);
    }

    #[test]
    fn test_vitest_effective_args_inject_json_reporter_by_default() {
        let effective =
            build_vitest_effective_args(&args(&["run", "constants.test.ts", "--watch"]));

        assert!(!effective.passthrough);
        assert_eq!(
            effective.args,
            args(&["run", "--reporter=json", "constants.test.ts"])
        );
    }

    #[test]
    fn test_vitest_effective_args_preserve_explicit_reporter() {
        let effective =
            build_vitest_effective_args(&args(&["constants.test.ts", "--reporter=verbose"]));

        assert!(effective.passthrough);
        assert_eq!(
            effective.args,
            args(&["run", "constants.test.ts", "--reporter=verbose"])
        );
    }

    #[test]
    fn test_vitest_explicit_reporter_keeps_verbose_output() {
        let output = " ✓ publicPaths.test.ts > keeps docs path\n Tests  1 passed (1)\n";
        let filtered = format_test_output(output, output, true, 0);

        assert!(filtered.text.contains("keeps docs path"));
        assert!(!filtered.truncated);
    }

    #[test]
    fn test_vitest_truncated_passthrough_is_marked_for_recovery() {
        let output = "verbose test output\n".repeat(80);
        let filtered = format_passthrough_output(&output, 100);

        assert!(filtered.truncated);
        assert!(filtered.text.chars().count() < output.chars().count());
    }
}
