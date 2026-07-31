use crate::tracking;
use crate::utils::{package_manager_exec, strip_ansi};
use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;

use crate::parser::{
    emit_degradation_warning, emit_passthrough_warning, truncate_output, FormatMode, OutputParser,
    ParseResult, TestFailure, TestResult, TokenFormatter,
};

/// Playwright JSON output structures — fix #193: corrected to match real Playwright output format
#[derive(Debug, Deserialize)]
struct PlaywrightJsonOutput {
    #[serde(rename = "stats")]
    stats: PlaywrightStats,
    #[serde(rename = "suites")]
    suites: Vec<PlaywrightSuite>,
}

#[derive(Debug, Deserialize)]
struct PlaywrightStats {
    #[serde(rename = "expected")]
    expected: usize,
    #[serde(rename = "unexpected")]
    unexpected: usize,
    #[serde(rename = "skipped")]
    skipped: usize,
    #[serde(rename = "duration", default)]
    duration: f64, // fix #193: Playwright emits float (e.g. 3519.703), not u64
}

#[derive(Debug, Deserialize)]
struct PlaywrightSuite {
    title: String,
    #[serde(rename = "specs", default)]
    specs: Vec<PlaywrightSpec>, // fix #193: real format uses "specs", not "tests"
    #[serde(rename = "suites", default)]
    suites: Vec<PlaywrightSuite>,
}

// fix #193: spec has ok: bool + tests (per-browser executions)
#[derive(Debug, Deserialize)]
struct PlaywrightSpec {
    title: String,
    #[serde(rename = "ok")]
    ok: bool,
    #[serde(rename = "tests", default)]
    tests: Vec<PlaywrightExecution>,
}

// fix #193: per-browser execution with results array
#[derive(Debug, Deserialize)]
struct PlaywrightExecution {
    #[serde(rename = "status")]
    status: String,
    #[serde(rename = "results", default)]
    results: Vec<PlaywrightAttempt>,
}

#[derive(Debug, Deserialize)]
struct PlaywrightAttempt {
    #[serde(rename = "status")]
    status: String,
    #[serde(rename = "errors", default)]
    errors: Vec<PlaywrightError>, // fix #193: array, not Option<single>
    #[serde(rename = "duration", default)]
    duration: f64,
}

#[derive(Debug, Deserialize)]
struct PlaywrightError {
    message: String,
}

/// Parser for Playwright JSON output
pub struct PlaywrightParser;

impl OutputParser for PlaywrightParser {
    type Output = TestResult;

    fn parse(input: &str) -> ParseResult<TestResult> {
        // Tier 1: Try JSON parsing
        match serde_json::from_str::<PlaywrightJsonOutput>(input) {
            Ok(json) => {
                let mut failures = Vec::new();
                let mut total = 0;
                collect_test_results(&json.suites, &mut total, &mut failures);

                let result = TestResult {
                    total,
                    passed: json.stats.expected,
                    failed: json.stats.unexpected,
                    skipped: json.stats.skipped,
                    duration_ms: Some(json.stats.duration as u64), // fix #193: f64 → u64
                    failures,
                };

                ParseResult::Full(result)
            }
            Err(e) => {
                // Tier 2: Try regex extraction
                match extract_playwright_regex(input) {
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

/// Recursively collect test results from suites — fix #193: uses specs/ok/errors structure
fn collect_test_results(
    suites: &[PlaywrightSuite],
    total: &mut usize,
    failures: &mut Vec<TestFailure>,
) {
    for suite in suites {
        // fix #193: iterate specs (not tests); each spec = one test case
        for spec in &suite.specs {
            *total += 1;

            if !spec.ok {
                // Collect error message from first failed attempt in any browser run
                let error_msg = spec
                    .tests
                    .iter()
                    .flat_map(|exec| exec.results.iter())
                    .flat_map(|attempt| attempt.errors.iter()) // fix #193: errors array
                    .next()
                    .map(|e| e.message.clone())
                    .unwrap_or_else(|| "Unknown error".to_string());

                failures.push(TestFailure {
                    test_name: spec.title.clone(),
                    file_path: suite.title.clone(),
                    error_message: error_msg,
                    stack_trace: None,
                });
            }
        }

        // Recurse into nested suites
        collect_test_results(&suite.suites, total, failures);
    }
}

/// Tier 2: Extract test statistics using regex (degraded mode)
fn extract_playwright_regex(output: &str) -> Option<TestResult> {
    lazy_static::lazy_static! {
        static ref SUMMARY_RE: Regex = Regex::new(
            r"(\d+)\s+(passed|failed|flaky|skipped)"
        ).unwrap();
        static ref DURATION_RE: Regex = Regex::new(
            r"\((\d+(?:\.\d+)?)(ms|s|m)\)"
        ).unwrap();
    }

    let clean_output = strip_ansi(output);

    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;

    // Parse summary counts
    for caps in SUMMARY_RE.captures_iter(&clean_output) {
        let count: usize = caps[1].parse().unwrap_or(0);
        match &caps[2] {
            "passed" => passed = count,
            "failed" => failed = count,
            "skipped" => skipped = count,
            _ => {}
        }
    }

    // Parse duration
    let duration_ms = DURATION_RE.captures(&clean_output).and_then(|caps| {
        let value: f64 = caps[1].parse().ok()?;
        let unit = &caps[2];
        Some(match unit {
            "ms" => value as u64,
            "s" => (value * 1000.0) as u64,
            "m" => (value * 60000.0) as u64,
            _ => value as u64,
        })
    });

    // Only return if we found valid data
    let total = passed + failed + skipped;
    if total > 0 {
        Some(TestResult {
            total,
            passed,
            failed,
            skipped,
            duration_ms,
            failures: extract_failures_regex(&clean_output),
        })
    } else {
        None
    }
}

/// Extract failures using regex
fn extract_failures_regex(output: &str) -> Vec<TestFailure> {
    lazy_static::lazy_static! {
        static ref TEST_PATTERN: Regex = Regex::new(
            r"[×✗]\s+.*?›\s+([^›]+\.spec\.[tj]sx?)"
        ).unwrap();
    }

    let mut failures = Vec::new();

    for caps in TEST_PATTERN.captures_iter(output) {
        if let Some(spec) = caps.get(1) {
            failures.push(TestFailure {
                test_name: caps[0].to_string(),
                file_path: spec.as_str().to_string(),
                error_message: String::new(),
                stack_trace: None,
            });
        }
    }

    failures
}

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = package_manager_exec("playwright");

    // fix #193: --reporter=json must come AFTER the subcommand (e.g. "playwright test --reporter=json")
    // Playwright rejects flags before the subcommand.
    let mut inserted_reporter = false;
    for (i, arg) in args.iter().enumerate() {
        cmd.arg(arg);
        if i == 0 && !arg.starts_with("-") && !inserted_reporter {
            // First positional arg is the subcommand — insert reporter flag right after
            cmd.arg("--reporter=json");
            inserted_reporter = true;
        }
    }
    if !inserted_reporter {
        // No subcommand provided, append at end
        cmd.arg("--reporter=json");
    }

    if verbose > 0 {
        eprintln!("Running: playwright {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run playwright (try: npm install -g playwright)")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    // Parse output using PlaywrightParser
    let parse_result = PlaywrightParser::parse(&stdout);
    let mode = FormatMode::from_verbosity(verbose);

    let filtered = match parse_result {
        ParseResult::Full(data) => {
            if verbose > 0 {
                eprintln!("playwright test (Tier 1: Full JSON parse)");
            }
            data.format(mode)
        }
        ParseResult::Degraded(data, warnings) => {
            if verbose > 0 {
                emit_degradation_warning("playwright", &warnings.join(", "));
            }
            data.format(mode)
        }
        ParseResult::Passthrough(raw) => {
            emit_passthrough_warning("playwright", "All parsing tiers failed");
            raw
        }
    };

    let shown = crate::guard::never_worse(&raw, &filtered);
    println!("{}", shown);

    timer.track(
        &format!("playwright {}", args.join(" ")),
        &format!("rtk playwright {}", args.join(" ")),
        &raw,
        shown,
    );

    // Preserve exit code for CI/CD
    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // fix #193: updated to use real Playwright JSON format (specs, ok, errors array, f64 duration)
    #[test]
    fn test_playwright_parser_json_real_format() {
        let json = r#"{
            "stats": {
                "expected": 1,
                "unexpected": 0,
                "skipped": 0,
                "duration": 3519.703
            },
            "suites": [
                {
                    "title": "auth/login.spec.ts",
                    "specs": [
                        {
                            "title": "should login",
                            "ok": true,
                            "tests": [
                                {
                                    "status": "passed",
                                    "results": [{"status": "passed", "errors": [], "duration": 2300.0}]
                                }
                            ]
                        }
                    ],
                    "suites": []
                }
            ]
        }"#;

        let result = PlaywrightParser::parse(json);
        assert_eq!(result.tier(), 1, "Should be Full JSON parse");
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.passed, 1);
        assert_eq!(data.failed, 0);
        assert_eq!(data.total, 1);
        assert_eq!(data.duration_ms, Some(3519)); // f64 3519.703 cast to u64
    }

    // fix #193: verify f64 duration does not cause deserialization failure
    #[test]
    fn test_playwright_float_duration_parsed() {
        let json = r#"{
            "stats": {"expected": 0, "unexpected": 1, "skipped": 0, "duration": 12345.678},
            "suites": [{
                "title": "suite",
                "specs": [{
                    "title": "failing test",
                    "ok": false,
                    "tests": [{
                        "status": "failed",
                        "results": [{
                            "status": "failed",
                            "errors": [{"message": "Expected true to be false"}],
                            "duration": 500.0
                        }]
                    }]
                }],
                "suites": []
            }]
        }"#;

        let result = PlaywrightParser::parse(json);
        assert_eq!(result.tier(), 1);
        let data = result.unwrap();
        assert_eq!(data.failed, 1);
        assert_eq!(data.failures.len(), 1);
        assert_eq!(data.failures[0].error_message, "Expected true to be false");
        assert_eq!(data.duration_ms, Some(12345));
    }

    #[test]
    fn test_playwright_parser_regex_fallback() {
        let text = "3 passed (7.3s)";
        let result = PlaywrightParser::parse(text);
        assert_eq!(result.tier(), 2); // Degraded
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.passed, 3);
        assert_eq!(data.failed, 0);
    }

    #[test]
    fn test_playwright_parser_passthrough() {
        let invalid = "random output";
        let result = PlaywrightParser::parse(invalid);
        assert_eq!(result.tier(), 3); // Passthrough
        assert!(!result.is_ok());
    }
}
