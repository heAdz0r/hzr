use crate::tracking;
use crate::truncate::{CAP_ERRORS, CAP_WARNINGS};
use crate::utils::truncate;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsString;
use std::process::Command;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub enum CargoCommand {
    Build,
    Test,
    Clippy,
    Check,
    Install,
    Nextest,
}

pub fn run(cmd: CargoCommand, args: &[String], verbose: u8) -> Result<()> {
    match cmd {
        CargoCommand::Build => run_build(args, verbose),
        CargoCommand::Test => run_test(args, verbose),
        CargoCommand::Clippy => run_clippy(args, verbose),
        CargoCommand::Check => run_check(args, verbose),
        CargoCommand::Install => run_install(args, verbose),
        CargoCommand::Nextest => run_nextest(args, verbose),
    }
}

/// Generic cargo command runner with filtering
fn run_cargo_filtered<F>(subcommand: &str, args: &[String], verbose: u8, filter_fn: F) -> Result<()>
where
    F: Fn(&str) -> String,
{
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("cargo");
    cmd.arg(subcommand);
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: cargo {} {}", subcommand, args.join(" "));
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run cargo {}", subcommand))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = output // upstream sync: tee integration
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });
    let filtered = filter_fn(&raw);

    if let Some(hint) = crate::tee::tee_and_hint(&raw, &format!("cargo_{}", subcommand), exit_code)
    // upstream sync: tee
    {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("cargo {} {}", subcommand, args.join(" ")),
        &format!("rtk cargo {} {}", subcommand, args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code); // upstream sync: use exit_code
    }

    Ok(())
}

fn run_cargo_filtered_with_exit<F>(
    subcommand: &str,
    args: &[String],
    verbose: u8,
    filter_fn: F,
) -> Result<()>
where
    F: Fn(&str, i32) -> String,
{
    let timer = tracking::TimedExecution::start();
    let mut cmd = Command::new("cargo");
    cmd.arg(subcommand).args(args);

    if verbose > 0 {
        eprintln!("Running: cargo {} {}", subcommand, args.join(" "));
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run cargo {}", subcommand))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);
    let exit_code = crate::stream::status_to_exit_code(output.status);
    let filtered = filter_fn(&raw, exit_code);

    if let Some(hint) = crate::tee::tee_and_hint(&raw, &format!("cargo_{}", subcommand), exit_code)
    {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("cargo {} {}", subcommand, args.join(" ")),
        &format!("rtk cargo {} {}", subcommand, args.join(" ")),
        &raw,
        &filtered,
    );

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}

fn has_json_message_format(args: &[String]) -> bool {
    let mut json = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--message-format=") {
            json = value.contains("json");
        } else if arg == "--message-format" {
            if let Some(value) = iter.next() {
                json = value.contains("json");
            }
        }
    }
    json
}

fn run_build(args: &[String], verbose: u8) -> Result<()> {
    if has_json_message_format(args) {
        return run_cargo_filtered_with_exit("build", args, verbose, |output, exit_code| {
            filter_cargo_build_labeled(output, "build", exit_code)
        });
    }
    run_cargo_filtered("build", args, verbose, filter_cargo_build)
}

fn run_test(args: &[String], verbose: u8) -> Result<()> {
    run_cargo_filtered("test", args, verbose, |raw| {
        if verbose > 0 {
            filter_cargo_test_verbose(raw)
        } else {
            filter_cargo_test(raw)
        }
    })
}

fn run_clippy(args: &[String], verbose: u8) -> Result<()> {
    if has_json_message_format(args) {
        return run_cargo_filtered_with_exit("clippy", args, verbose, filter_cargo_clippy_json);
    }
    run_cargo_filtered("clippy", args, verbose, filter_cargo_clippy)
}

fn run_check(args: &[String], verbose: u8) -> Result<()> {
    if has_json_message_format(args) {
        return run_cargo_filtered_with_exit("check", args, verbose, |output, exit_code| {
            filter_cargo_build_labeled(output, "check", exit_code)
        });
    }
    run_cargo_filtered_with_exit("check", args, verbose, |output, exit_code| {
        filter_cargo_build_labeled(output, "check", exit_code)
    })
}

fn run_install(args: &[String], verbose: u8) -> Result<()> {
    run_cargo_filtered("install", args, verbose, filter_cargo_install)
}

fn run_nextest(args: &[String], verbose: u8) -> Result<()> {
    run_cargo_filtered("nextest", args, verbose, filter_cargo_nextest)
}

/// Format crate name + version into a display string
fn format_crate_info(name: &str, version: &str, fallback: &str) -> String {
    if name.is_empty() {
        fallback.to_string()
    } else if version.is_empty() {
        name.to_string()
    } else {
        format!("{} {}", name, version)
    }
}

/// Filter cargo install output - strip dep compilation, keep installed/replaced/errors
fn filter_cargo_install(output: &str) -> String {
    let mut errors: Vec<String> = Vec::new();
    let mut error_count = 0;
    let mut compiled = 0;
    let mut in_error = false;
    let mut current_error = Vec::new();
    let mut installed_crate = String::new();
    let mut installed_version = String::new();
    let mut replaced_lines: Vec<String> = Vec::new();
    let mut already_installed = false;
    let mut ignored_line = String::new();

    for line in output.lines() {
        let trimmed = line.trim_start();

        // Strip noise: dep compilation, downloading, locking, etc.
        if trimmed.starts_with("Compiling") {
            compiled += 1;
            continue;
        }
        if trimmed.starts_with("Downloading")
            || trimmed.starts_with("Downloaded")
            || trimmed.starts_with("Locking")
            || trimmed.starts_with("Updating")
            || trimmed.starts_with("Adding")
            || trimmed.starts_with("Finished")
            || trimmed.starts_with("Blocking waiting for file lock")
        {
            continue;
        }

        // Keep: Installing line (extract crate name + version)
        if trimmed.starts_with("Installing") {
            let rest = trimmed.strip_prefix("Installing").unwrap_or("").trim();
            if !rest.is_empty() && !rest.starts_with('/') {
                if let Some((name, version)) = rest.split_once(' ') {
                    installed_crate = name.to_string();
                    installed_version = version.to_string();
                } else {
                    installed_crate = rest.to_string();
                }
            }
            continue;
        }

        // Keep: Installed line (extract crate + version if not already set)
        if trimmed.starts_with("Installed") {
            let rest = trimmed.strip_prefix("Installed").unwrap_or("").trim();
            if !rest.is_empty() && installed_crate.is_empty() {
                let mut parts = rest.split_whitespace();
                if let (Some(name), Some(version)) = (parts.next(), parts.next()) {
                    installed_crate = name.to_string();
                    installed_version = version.to_string();
                }
            }
            continue;
        }

        // Keep: Replacing/Replaced lines
        if trimmed.starts_with("Replacing") || trimmed.starts_with("Replaced") {
            replaced_lines.push(trimmed.to_string());
            continue;
        }

        // Keep: "Ignored package" (already up to date)
        if trimmed.starts_with("Ignored package") {
            already_installed = true;
            ignored_line = trimmed.to_string();
            continue;
        }

        // Keep: actionable warnings (e.g., "be sure to add `/path` to your PATH")
        // Skip summary lines like "warning: `crate` generated N warnings"
        if line.starts_with("warning:") {
            if !(line.contains("generated") && line.contains("warning")) {
                replaced_lines.push(line.to_string());
            }
            continue;
        }

        // Detect error blocks
        if line.starts_with("error[") || line.starts_with("error:") {
            if line.contains("aborting due to") || line.contains("could not compile") {
                continue;
            }
            if in_error && !current_error.is_empty() {
                errors.push(current_error.join("\n"));
                current_error.clear();
            }
            error_count += 1;
            in_error = true;
            current_error.push(line.to_string());
        } else if in_error {
            if line.trim().is_empty() && current_error.len() > 3 {
                errors.push(current_error.join("\n"));
                current_error.clear();
                in_error = false;
            } else {
                current_error.push(line.to_string());
            }
        }
    }

    if !current_error.is_empty() {
        errors.push(current_error.join("\n"));
    }

    // Already installed / up to date
    if already_installed {
        let info = ignored_line.split('`').nth(1).unwrap_or(&ignored_line);
        return format!("✓ cargo install: {} already installed", info);
    }

    // Errors
    if error_count > 0 {
        let crate_info = format_crate_info(&installed_crate, &installed_version, "");
        let deps_info = if compiled > 0 {
            format!(", {} deps compiled", compiled)
        } else {
            String::new()
        };

        let mut result = String::new();
        if crate_info.is_empty() {
            result.push_str(&format!(
                "cargo install: {} error{}{}\n",
                error_count,
                if error_count > 1 { "s" } else { "" },
                deps_info
            ));
        } else {
            result.push_str(&format!(
                "cargo install: {} error{} ({}{})\n",
                error_count,
                if error_count > 1 { "s" } else { "" },
                crate_info,
                deps_info
            ));
        }
        result.push_str("═══════════════════════════════════════\n");

        for (i, err) in errors.iter().enumerate().take(15) {
            result.push_str(err);
            result.push('\n');
            if i < errors.len() - 1 {
                result.push('\n');
            }
        }

        if errors.len() > 15 {
            result.push_str(&format!("\n... +{} more issues\n", errors.len() - 15));
        }

        return result.trim().to_string();
    }

    // Success
    let crate_info = format_crate_info(&installed_crate, &installed_version, "package");

    let mut result = format!(
        "✓ cargo install ({}, {} deps compiled)",
        crate_info, compiled
    );

    for line in &replaced_lines {
        result.push_str(&format!("\n  {}", line));
    }

    result
}

/// Push a completed failure block (header + body) into the failures list, then clear the buffers.
fn flush_failure_block(header: &mut String, body: &mut Vec<String>, failures: &mut Vec<String>) {
    if header.is_empty() {
        return;
    }
    let mut block = header.clone();
    if !body.is_empty() {
        block.push('\n');
        block.push_str(&body.join("\n"));
    }
    failures.push(block);
    header.clear();
    body.clear();
}

/// Filter cargo nextest output - show failures + compact summary
fn filter_cargo_nextest(output: &str) -> String {
    static SUMMARY_RE: OnceLock<regex::Regex> = OnceLock::new();
    let summary_re = SUMMARY_RE.get_or_init(|| {
        regex::Regex::new(
            r"Summary \[\s*([\d.]+)s\]\s+(\d+) tests? run:\s+(\d+) passed(?:,\s+(\d+) failed)?(?:,\s+(\d+) skipped)?"
        ).expect("invalid nextest summary regex")
    });

    static STARTING_RE: OnceLock<regex::Regex> = OnceLock::new();
    let starting_re = STARTING_RE.get_or_init(|| {
        regex::Regex::new(r"Starting \d+ tests? across (\d+) binar(?:y|ies)")
            .expect("invalid nextest starting regex")
    });

    let mut failures: Vec<String> = Vec::new();
    let mut in_failure_block = false;
    let mut past_summary = false;
    let mut current_failure_header = String::new();
    let mut current_failure_body = Vec::new();
    let mut summary_line = String::new();
    let mut binaries: u32 = 0;
    let mut has_cancel_line = false;

    for line in output.lines() {
        let trimmed = line.trim();

        // Strip compilation noise
        if trimmed.starts_with("Compiling")
            || trimmed.starts_with("Downloading")
            || trimmed.starts_with("Downloaded")
            || trimmed.starts_with("Finished")
            || trimmed.starts_with("Locking")
            || trimmed.starts_with("Updating")
        {
            continue;
        }

        // Strip separator lines (────)
        if trimmed.starts_with("────") {
            continue;
        }

        // Skip post-summary recap lines (FAIL duplicates + "error: test run failed")
        if past_summary {
            continue;
        }

        // Parse binary count from Starting line
        if trimmed.starts_with("Starting") {
            if let Some(caps) = starting_re.captures(trimmed) {
                if let Some(m) = caps.get(1) {
                    binaries = m.as_str().parse().unwrap_or(0);
                }
            }
            continue;
        }

        // Strip PASS lines
        if trimmed.starts_with("PASS") {
            if in_failure_block {
                flush_failure_block(
                    &mut current_failure_header,
                    &mut current_failure_body,
                    &mut failures,
                );
                in_failure_block = false;
            }
            continue;
        }

        // Detect FAIL lines
        if trimmed.starts_with("FAIL") {
            // Close previous failure block if any
            if in_failure_block {
                flush_failure_block(
                    &mut current_failure_header,
                    &mut current_failure_body,
                    &mut failures,
                );
            }
            current_failure_header = trimmed.to_string();
            in_failure_block = true;
            continue;
        }

        // Cancellation notice
        if trimmed.starts_with("Cancelling") || trimmed.starts_with("Canceling") {
            has_cancel_line = true;
            continue;
        }

        // Nextest run ID line
        if trimmed.starts_with("Nextest run ID") {
            continue;
        }

        // Parse summary
        if trimmed.starts_with("Summary") {
            summary_line = trimmed.to_string();
            if in_failure_block {
                flush_failure_block(
                    &mut current_failure_header,
                    &mut current_failure_body,
                    &mut failures,
                );
                in_failure_block = false;
            }
            past_summary = true;
            continue;
        }

        // Collect failure body lines (stdout/stderr sections)
        if in_failure_block {
            current_failure_body.push(line.to_string());
        }
    }

    // Close last failure block
    if in_failure_block {
        flush_failure_block(
            &mut current_failure_header,
            &mut current_failure_body,
            &mut failures,
        );
    }

    // Parse summary with regex
    if let Some(caps) = summary_re.captures(&summary_line) {
        let duration = caps.get(1).map_or("?", |m| m.as_str());
        let passed: u32 = caps
            .get(3)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0);
        let failed: u32 = caps
            .get(4)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0);
        let skipped: u32 = caps
            .get(5)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0);

        let binary_text = if binaries == 1 {
            "1 binary".to_string()
        } else if binaries > 1 {
            format!("{} binaries", binaries)
        } else {
            String::new()
        };

        if failed == 0 {
            // All pass - compact single line
            let mut parts = vec![format!("{} passed", passed)];
            if skipped > 0 {
                parts.push(format!("{} skipped", skipped));
            }
            let meta = if binary_text.is_empty() {
                format!("{}s", duration)
            } else {
                format!("{}, {}s", binary_text, duration)
            };
            return format!("✓ cargo nextest: {} ({})", parts.join(", "), meta);
        }

        // With failures - show failure details then summary
        let mut result = String::new();

        for failure in &failures {
            result.push_str(failure);
            result.push('\n');
        }

        if has_cancel_line {
            result.push_str("Cancelling due to test failure\n");
        }

        let mut summary_parts = vec![format!("{} passed", passed)];
        if failed > 0 {
            summary_parts.push(format!("{} failed", failed));
        }
        if skipped > 0 {
            summary_parts.push(format!("{} skipped", skipped));
        }
        let meta = if binary_text.is_empty() {
            format!("{}s", duration)
        } else {
            format!("{}, {}s", binary_text, duration)
        };
        result.push_str(&format!(
            "cargo nextest: {} ({})",
            summary_parts.join(", "),
            meta
        ));

        return result.trim().to_string();
    }

    // Fallback: if summary regex didn't match, show what we have
    if !failures.is_empty() {
        let mut result = String::new();
        for failure in &failures {
            result.push_str(failure);
            result.push('\n');
        }
        if !summary_line.is_empty() {
            result.push_str(&summary_line);
        }
        return result.trim().to_string();
    }

    if !summary_line.is_empty() {
        return summary_line;
    }

    // Empty or unrecognized
    String::new()
}

#[derive(Default)]
struct JsonDiagnostics {
    errors: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Deserialize)]
struct CargoJsonLine {
    reason: String,
    message: Option<CargoDiagnostic>,
}

#[derive(Deserialize)]
struct CargoDiagnostic {
    level: String,
    #[serde(default)]
    message: String,
    rendered: Option<String>,
}

fn extract_json_diagnostics(raw: &str) -> JsonDiagnostics {
    let mut diagnostics = JsonDiagnostics::default();
    for line in raw.lines() {
        let line = line.trim_start();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<CargoJsonLine>(line) else {
            continue;
        };
        if entry.reason != "compiler-message" {
            continue;
        }
        let Some(message) = entry.message else {
            continue;
        };
        let bucket = match message.level.as_str() {
            "error" | "error: internal compiler error" => &mut diagnostics.errors,
            "warning" => &mut diagnostics.warnings,
            _ => continue,
        };
        if message.message.starts_with("aborting due to")
            || message.message.starts_with("could not compile")
            || (message.message.contains("warning") && message.message.contains("generated"))
        {
            continue;
        }
        if let Some(rendered) = message.rendered {
            bucket.push(crate::utils::strip_ansi(&rendered).trim_end().to_string());
        } else if !message.message.is_empty() {
            bucket.push(message.message);
        }
    }
    diagnostics
}

/// Filter cargo build/check output - strip "Compiling"/"Checking" lines, keep errors + summary
fn filter_cargo_build(output: &str) -> String {
    filter_cargo_build_labeled(output, "build", 0)
}

fn filter_cargo_build_labeled(output: &str, label: &str, exit_code: i32) -> String {
    let mut errors: Vec<String> = Vec::new();
    let mut compiled = 0;
    let mut in_error = false;
    let mut current_error = Vec::new();

    for line in output.lines() {
        if line.trim_start().starts_with("Compiling") || line.trim_start().starts_with("Checking") {
            compiled += 1;
            continue;
        }
        if line.trim_start().starts_with("Downloading")
            || line.trim_start().starts_with("Downloaded")
        {
            continue;
        }
        if line.trim_start().starts_with("Finished") {
            continue;
        }

        // Detect error/warning blocks
        if line.starts_with("error[") || line.starts_with("error:") {
            // Skip "error: aborting due to" summary lines
            if line.contains("aborting due to") || line.contains("could not compile") {
                continue;
            }
            if in_error && !current_error.is_empty() {
                errors.push(current_error.join("\n"));
                current_error.clear();
            }
            in_error = true;
            current_error.push(line.to_string());
        } else if line.starts_with("warning:")
            && line.contains("generated")
            && line.contains("warning")
        {
            // "warning: `crate` generated N warnings" summary line
            continue;
        } else if line.starts_with("warning:") || line.starts_with("warning[") {
            if in_error && !current_error.is_empty() {
                errors.push(current_error.join("\n"));
                current_error.clear();
            }
            in_error = true;
            current_error.push(line.to_string());
        } else if in_error {
            if line.trim().is_empty() && current_error.len() > 3 {
                errors.push(current_error.join("\n"));
                current_error.clear();
                in_error = false;
            } else {
                current_error.push(line.to_string());
            }
        }
    }

    if !current_error.is_empty() {
        errors.push(current_error.join("\n"));
    }

    // Normalize: trim trailing blank lines/whitespace before dedup.
    // Blocks may differ in trailing \n depending on how they were flushed
    // (blank-line flush vs. next-error flush), causing identical errors to
    // not match in the HashSet. Trim_end makes dedup reliable.
    for block in &mut errors {
        *block = block.trim_end().to_string();
    }
    // Dedup: cargo reports same error for each target (lib/bin), remove exact duplicates
    let mut seen_blocks = std::collections::HashSet::new();
    errors.retain(|e| seen_blocks.insert(e.clone()));
    // Count from deduplicated blocks (not from the parse loop, which may double-count)
    let json = extract_json_diagnostics(output);
    let error_count = errors
        .iter()
        .filter(|e| e.starts_with("error"))
        .count()
        .max(json.errors.len());
    let warnings = errors
        .iter()
        .filter(|e| e.starts_with("warning"))
        .count()
        .max(json.warnings.len());

    if error_count == 0 && warnings == 0 && exit_code == 0 {
        return format!("✓ cargo {} ({} crates compiled)", label, compiled);
    }

    let mut result = String::new();
    if error_count == 0 && warnings == 0 {
        result.push_str(&format!("cargo {}: failed (exit {})\n", label, exit_code));
    } else {
        result.push_str(&format!(
            "cargo {}: {} errors, {} warnings ({} crates)\n",
            label, error_count, warnings, compiled
        ));
    }
    result.push_str("═══════════════════════════════════════\n");

    for (i, err) in errors.iter().enumerate().take(CAP_ERRORS) {
        result.push_str(err);
        result.push('\n');
        if i < errors.len() - 1 {
            result.push('\n');
        }
    }

    for diagnostic in json.errors.iter().take(CAP_ERRORS) {
        result.push_str(diagnostic);
        result.push_str("\n\n");
    }
    for diagnostic in json.warnings.iter().take(CAP_WARNINGS) {
        result.push_str(diagnostic);
        result.push_str("\n\n");
    }

    let hidden_errors =
        errors.len().saturating_sub(CAP_ERRORS) + json.errors.len().saturating_sub(CAP_ERRORS);
    let hidden_warnings = json.warnings.len().saturating_sub(CAP_WARNINGS);
    if hidden_errors + hidden_warnings > 0 {
        result.push_str(&format!(
            "... +{} more issues\n",
            hidden_errors + hidden_warnings
        ));
        let full = json
            .errors
            .iter()
            .chain(json.warnings.iter())
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n\n");
        if let Some(hint) = crate::tee::force_tee_hint(&full, "cargo-json-issues") {
            result.push_str(&format!("  {}\n", hint));
        }
    }

    result.trim().to_string()
}

/// Aggregated test results for compact display
#[derive(Debug, Default, Clone)]
struct AggregatedTestResult {
    passed: usize,
    failed: usize,
    ignored: usize,
    measured: usize,
    filtered_out: usize,
    suites: usize,
    duration_secs: f64,
    has_duration: bool,
}

impl AggregatedTestResult {
    /// Parse a test result summary line
    /// Format: "test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s"
    fn parse_line(line: &str) -> Option<Self> {
        static RE: OnceLock<regex::Regex> = OnceLock::new();
        let re = RE.get_or_init(|| {
            regex::Regex::new(
                r"test result: (\w+)\.\s+(\d+) passed;\s+(\d+) failed;\s+(\d+) ignored;\s+(\d+) measured;\s+(\d+) filtered out(?:;\s+finished in ([\d.]+)s)?"
            ).unwrap()
        });

        let caps = re.captures(line)?;
        let passed = caps.get(2)?.as_str().parse().ok()?;
        let failed = caps.get(3)?.as_str().parse().ok()?;
        let ignored = caps.get(4)?.as_str().parse().ok()?;
        let measured = caps.get(5)?.as_str().parse().ok()?;
        let filtered_out = caps.get(6)?.as_str().parse().ok()?;

        let (duration_secs, has_duration) = if let Some(duration_match) = caps.get(7) {
            (duration_match.as_str().parse().unwrap_or(0.0), true)
        } else {
            (0.0, false)
        };

        Some(Self {
            passed,
            failed,
            ignored,
            measured,
            filtered_out,
            suites: 1,
            duration_secs,
            has_duration,
        })
    }

    /// Merge another test result into this one
    fn merge(&mut self, other: &Self) {
        self.passed += other.passed;
        self.failed += other.failed;
        self.ignored += other.ignored;
        self.measured += other.measured;
        self.filtered_out += other.filtered_out;
        self.suites += other.suites;
        self.duration_secs += other.duration_secs;
        self.has_duration = self.has_duration && other.has_duration;
    }

    /// Format as compact single line
    fn format_compact(&self) -> String {
        let mut parts = vec![format!("{} passed", self.passed)];

        if self.failed > 0 {
            parts.push(format!("{} failed", self.failed));
        }
        if self.ignored > 0 {
            parts.push(format!("{} ignored", self.ignored));
        }
        if self.filtered_out > 0 {
            parts.push(format!("{} filtered out", self.filtered_out));
        }

        let counts = parts.join(", ");

        let suite_text = if self.suites == 1 {
            "1 suite".to_string()
        } else {
            format!("{} suites", self.suites)
        };

        let mark = if self.failed > 0 { "✗" } else { "✓" };
        if self.has_duration {
            format!(
                "{mark} cargo test: {} ({}, {:.2}s)",
                counts, suite_text, self.duration_secs
            )
        } else {
            format!("{mark} cargo test: {} ({})", counts, suite_text)
        }
    }
}

fn flush_cargo_test_failure(current: &mut Vec<String>, failures: &mut Vec<String>) {
    while current.last().is_some_and(|line| line.trim().is_empty()) {
        current.pop();
    }
    if !current.is_empty() {
        failures.push(current.join("\n"));
        current.clear();
    }
}

fn cargo_test_failures(output: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let mut current = Vec::new();
    let mut in_failure_details = false;
    let mut in_failure_names = false;

    for line in output.lines() {
        if line == "failures:" {
            if in_failure_details {
                flush_cargo_test_failure(&mut current, &mut failures);
                in_failure_details = false;
                in_failure_names = true;
            } else if !in_failure_names {
                in_failure_details = true;
            }
            continue;
        }

        if line.trim_start().starts_with("test result:") {
            if in_failure_details {
                flush_cargo_test_failure(&mut current, &mut failures);
            }
            in_failure_details = false;
            in_failure_names = false;
            continue;
        }

        if !in_failure_details {
            continue;
        }

        if line.starts_with("---- ") {
            flush_cargo_test_failure(&mut current, &mut failures);
            current.push(line.to_string());
        } else if !current.is_empty() {
            current.push(line.to_string());
        }
    }

    flush_cargo_test_failure(&mut current, &mut failures);
    failures
}

fn format_cargo_test_failures(failures: &[String]) -> String {
    let mut result = format!("FAILURES ({}):\n", failures.len());
    for (index, failure) in failures.iter().take(CAP_WARNINGS).enumerate() {
        let mut compact_lines = Vec::new();
        for line in failure.lines().filter(|line| !line.trim().is_empty()) {
            if line.starts_with("note: run with `RUST_BACKTRACE") {
                continue;
            }
            if let Some((_, panic)) = line.split_once(" panicked at ") {
                compact_lines.push(format!("panicked at {panic}"));
            } else {
                compact_lines.push(line.to_string());
            }
        }
        result.push_str(&format!(
            "{}. {}\n",
            index + 1,
            truncate(&compact_lines.join("\n"), 200)
        ));
    }
    if failures.len() > CAP_WARNINGS {
        result.push_str(&format!(
            "… +{} more failures\n",
            failures.len() - CAP_WARNINGS
        ));
    }
    result
}

/// Filter cargo test output - compact summary with bounded failure details.
pub(crate) fn filter_cargo_test(output: &str) -> String {
    let diag = crate::diag_summary::analyze_output(output);
    let json = extract_json_diagnostics(output);
    let failures = cargo_test_failures(output);

    fn with_diag(
        base: String,
        failures: &[String],
        diag: &crate::diag_summary::DiagnosticSummary,
    ) -> String {
        let details = if failures.is_empty() {
            String::new()
        } else {
            format_cargo_test_failures(failures)
        };
        format!(
            "{}{}\n{}\n{}",
            details,
            base,
            diag.warnings_line(),
            diag.errors_line()
        )
    }
    let summary_lines: Vec<&str> = output
        .lines()
        .filter(|l| l.trim_start().starts_with("test result:"))
        .collect();

    // Compilation failed: no test result lines → show actual error messages
    if summary_lines.is_empty() && (diag.errors > 0 || !json.errors.is_empty()) {
        return filter_cargo_build_labeled(output, "test", 1);
    }

    let mut aggregated: Option<AggregatedTestResult> = None;
    let mut parsed_any = false;
    let mut parsed_all = true;

    for line in &summary_lines {
        if let Some(parsed) = AggregatedTestResult::parse_line(line.trim()) {
            if let Some(ref mut agg) = aggregated {
                agg.merge(&parsed);
            } else {
                aggregated = Some(parsed);
            }
            parsed_any = true;
        } else {
            parsed_all = false;
        }
    }

    if parsed_any && parsed_all {
        if let Some(agg) = aggregated {
            if agg.suites > 0 {
                return with_diag(agg.format_compact(), &failures, &diag);
            }
        }
    }

    if !summary_lines.is_empty() {
        return with_diag(
            format!("cargo test: {}", summary_lines.last().unwrap().trim()),
            &failures,
            &diag,
        );
    }

    with_diag("cargo test: completed".to_string(), &failures, &diag)
}

/// Verbose cargo test filter with failure details (for `-v`).
fn filter_cargo_test_verbose(output: &str) -> String {
    let mut failures: Vec<String> = Vec::new();
    let mut summary_lines: Vec<String> = Vec::new();
    let mut in_failure_section = false;
    let mut current_failure = Vec::new();

    for line in output.lines() {
        if line.trim_start().starts_with("Compiling")
            || line.trim_start().starts_with("Downloading")
            || line.trim_start().starts_with("Downloaded")
            || line.trim_start().starts_with("Finished")
        {
            continue;
        }

        if line.starts_with("running ") || (line.starts_with("test ") && line.ends_with("... ok")) {
            continue;
        }

        if line == "failures:" {
            in_failure_section = true;
            continue;
        }

        if in_failure_section {
            if line.starts_with("test result:") {
                in_failure_section = false;
                summary_lines.push(line.to_string());
            } else if line.starts_with("    ") || line.starts_with("---- ") {
                current_failure.push(line.to_string());
            } else if line.trim().is_empty() && !current_failure.is_empty() {
                failures.push(current_failure.join("\n"));
                current_failure.clear();
            } else if !line.trim().is_empty() {
                current_failure.push(line.to_string());
            }
        }

        if !in_failure_section && line.starts_with("test result:") {
            summary_lines.push(line.to_string());
        }
    }

    if !current_failure.is_empty() {
        failures.push(current_failure.join("\n"));
    }

    let mut result = String::new();
    if !failures.is_empty() {
        result.push_str(&format!("FAILURES ({}):\n", failures.len()));
        result.push_str("═══════════════════════════════════════\n");
        for (i, failure) in failures.iter().enumerate().take(10) {
            result.push_str(&format!("{}. {}\n", i + 1, truncate(failure, 200)));
        }
        if failures.len() > 10 {
            result.push_str(&format!("\n... +{} more failures\n", failures.len() - 10));
        }
        result.push('\n');
    }
    for line in &summary_lines {
        result.push_str(&format!("{}\n", line));
    }
    if result.trim().is_empty() {
        let meaningful: Vec<&str> = output
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with("Compiling"))
            .collect();
        for line in meaningful.iter().rev().take(5).rev() {
            result.push_str(&format!("{}\n", line));
        }
    }
    result.trim().to_string()
}

/// Filter cargo clippy output - group warnings by lint rule
fn filter_cargo_clippy(output: &str) -> String {
    // (is_error, locations): track type per rule for accurate deduped counts
    let mut by_rule: HashMap<String, (bool, Vec<String>)> = HashMap::new();

    // Parse clippy output lines
    // Format: "warning: description\n  --> file:line:col\n  |\n  | code\n"
    let mut current_rule = String::new();
    let mut current_is_error = false;

    for line in output.lines() {
        // Skip compilation lines
        if line.trim_start().starts_with("Compiling")
            || line.trim_start().starts_with("Checking")
            || line.trim_start().starts_with("Downloading")
            || line.trim_start().starts_with("Downloaded")
            || line.trim_start().starts_with("Finished")
        {
            continue;
        }

        // "warning: unused variable [unused_variables]" or "warning: description [clippy::rule_name]"
        if (line.starts_with("warning:") || line.starts_with("warning["))
            || (line.starts_with("error:") || line.starts_with("error["))
        {
            // Skip summary lines: "warning: `rtk` (bin) generated 5 warnings"
            if line.contains("generated") && line.contains("warning") {
                continue;
            }
            // Skip "error: aborting" / "error: could not compile"
            if line.contains("aborting due to") || line.contains("could not compile") {
                continue;
            }

            current_is_error = line.starts_with("error");

            // Extract rule name from brackets
            current_rule = if let Some(bracket_start) = line.rfind('[') {
                if let Some(bracket_end) = line.rfind(']') {
                    line[bracket_start + 1..bracket_end].to_string()
                } else {
                    line.to_string()
                }
            } else {
                // No bracket: use the message itself as the rule
                let prefix = if current_is_error {
                    "error: "
                } else {
                    "warning: "
                };
                line.strip_prefix(prefix).unwrap_or(line).to_string()
            };
        } else if line.trim_start().starts_with("--> ") {
            let location = line.trim_start().trim_start_matches("--> ").to_string();
            if !current_rule.is_empty() {
                let entry = by_rule
                    .entry(current_rule.clone())
                    .or_insert((current_is_error, Vec::new()));
                // Dedup: same location may appear for each target (lib/bin)
                if !entry.1.contains(&location) {
                    entry.1.push(location);
                }
            }
        }
    }

    // Recount from deduplicated by_rule (cargo may double-count for lib+bin targets)
    let error_count: usize = by_rule
        .values()
        .filter(|(is_err, _)| *is_err)
        .map(|(_, locs)| locs.len())
        .sum();
    let warning_count: usize = by_rule
        .values()
        .filter(|(is_err, _)| !*is_err)
        .map(|(_, locs)| locs.len())
        .sum();

    if error_count == 0 && warning_count == 0 {
        return "✓ cargo clippy: No issues found".to_string();
    }

    let mut result = String::new();
    result.push_str(&format!(
        "cargo clippy: {} errors, {} warnings\n",
        error_count, warning_count,
    ));
    result.push_str("═══════════════════════════════════════\n");

    // Sort rules by frequency (by_rule value is (is_error, locations))
    let mut rule_counts: Vec<_> = by_rule.iter().collect();
    rule_counts.sort_by(|a, b| b.1 .1.len().cmp(&a.1 .1.len()));

    for (rule, (_is_error, locations)) in rule_counts.iter().take(15) {
        result.push_str(&format!("  {} ({}x)\n", rule, locations.len()));
        for loc in locations.iter().take(3) {
            result.push_str(&format!("    {}\n", loc));
        }
        if locations.len() > 3 {
            result.push_str(&format!("    ... +{} more\n", locations.len() - 3));
        }
    }

    if by_rule.len() > 15 {
        result.push_str(&format!("\n... +{} more rules\n", by_rule.len() - 15));
    }

    result.trim().to_string()
}

fn filter_cargo_clippy_json(output: &str, exit_code: i32) -> String {
    let json = extract_json_diagnostics(output);
    if json.errors.is_empty() && json.warnings.is_empty() && exit_code == 0 {
        return "cargo clippy: No issues found".to_string();
    }
    filter_cargo_build_labeled(output, "clippy", exit_code)
}

/// Runs an unsupported cargo subcommand by passing it through directly
pub fn run_passthrough(args: &[OsString], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("cargo passthrough: {:?}", args);
    }
    let status = Command::new("cargo")
        .args(args)
        .status()
        .context("Failed to run cargo")?;

    let args_str = tracking::args_display(args);
    timer.track_passthrough(
        &format!("cargo {}", args_str),
        &format!("rtk cargo {} (passthrough)", args_str),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_message_format_detection_honors_last_value() {
        assert!(has_json_message_format(&[
            "--message-format".into(),
            "json-diagnostic-rendered-ansi".into(),
        ]));
        assert!(has_json_message_format(&[
            "--release".into(),
            "--message-format=json-render-diagnostics".into(),
        ]));
        assert!(!has_json_message_format(&[
            "--message-format=json".into(),
            "--message-format=human".into(),
        ]));
        assert!(!has_json_message_format(&["--message-format".into()]));
    }

    #[test]
    fn cargo_json_error_is_not_reported_as_success() {
        let input = r#"{"reason":"compiler-message","message":{"level":"error","message":"mismatched types","rendered":"error[E0308]: mismatched types\n --> src/main.rs:1:1"}}"#;
        let result = filter_cargo_build_labeled(input, "build", 101);
        assert!(result.contains("cargo build: 1 errors"), "{result}");
        assert!(result.contains("error[E0308]"), "{result}");
        assert!(!result.contains("✓ cargo build"), "{result}");
    }

    #[test]
    fn cargo_json_clippy_warning_is_rendered() {
        let input = r#"{"reason":"compiler-message","message":{"level":"warning","message":"needless borrow","rendered":"warning: needless borrow\n --> src/lib.rs:3:1"}}"#;
        let result = filter_cargo_clippy_json(input, 0);
        assert!(
            result.contains("cargo clippy: 0 errors, 1 warnings"),
            "{result}"
        );
        assert!(result.contains("needless borrow"), "{result}");
    }

    #[test]
    fn cargo_test_json_compile_error_is_not_silent() {
        let input = r#"{"reason":"compiler-message","message":{"level":"error","message":"cannot find value","rendered":"error[E0425]: cannot find value `x`\n --> src/lib.rs:2:1"}}"#;
        let result = filter_cargo_test(input);
        assert!(!result.trim().is_empty());
        assert!(result.contains("cargo test: 1 errors"), "{result}");
        assert!(result.contains("E0425"), "{result}");
    }

    #[test]
    fn test_filter_cargo_build_success() {
        let output = r#"   Compiling libc v0.2.153
   Compiling cfg-if v1.0.0
   Compiling rtk v0.5.0
    Finished dev [unoptimized + debuginfo] target(s) in 15.23s
"#;
        let result = filter_cargo_build(output);
        assert!(result.contains("✓ cargo build"));
        assert!(result.contains("3 crates compiled"));
    }

    #[test]
    fn test_filter_cargo_build_errors() {
        let output = r#"   Compiling rtk v0.5.0
error[E0308]: mismatched types
 --> src/main.rs:10:5
  |
10|     "hello"
  |     ^^^^^^^ expected `i32`, found `&str`

error: aborting due to 1 previous error
"#;
        let result = filter_cargo_build(output);
        assert!(result.contains("1 errors"));
        assert!(result.contains("E0308"));
        assert!(result.contains("mismatched types"));
    }

    #[test]
    fn test_filter_cargo_test_all_pass() {
        let output = r#"   Compiling rtk v0.5.0
    Finished test [unoptimized + debuginfo] target(s) in 2.53s
     Running target/debug/deps/rtk-abc123

running 15 tests
test utils::tests::test_truncate_short_string ... ok
test utils::tests::test_truncate_long_string ... ok
test utils::tests::test_strip_ansi_simple ... ok

test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
"#;
        let result = filter_cargo_test(output);
        assert!(
            result.contains("✓ cargo test: 15 passed (1 suite, 0.01s)"),
            "Expected compact format, got: {}",
            result
        );
        assert!(result.contains("warnings: 0"));
        assert!(result.contains("errors: 0"));
        assert!(!result.contains("Compiling"));
        assert!(!result.contains("test utils"));
    }

    #[test]
    fn test_filter_cargo_test_compact_warning_summary_with_files() {
        let output = r#"warning: unused variable: `start`
   --> src/init.rs:561:17
warning: constant `BILLION` is never used
  --> src/cc_economics.rs:17:7
warning: `rtk` (bin "rtk" test) generated 17 warnings

running 2 tests
test foo::a ... ok
test foo::b ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
"#;
        let result = filter_cargo_test(output);
        assert!(result.contains("✓ cargo test: 2 passed (1 suite, 0.01s)"));
        assert!(result.contains("warnings: 17"));
        assert!(result.contains("src/init.rs"));
        assert!(result.contains("src/cc_economics.rs"));
        assert!(result.contains("errors: 0"));
    }

    #[test]
    fn test_filter_cargo_test_failures() {
        let output = r#"running 5 tests
test foo::test_a ... ok
test foo::test_b ... FAILED
test foo::test_c ... ok

failures:

---- foo::test_b stdout ----
thread 'foo::test_b' panicked at 'assert_eq!(1, 2)'

failures:
    foo::test_b

        test result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
"#;
        let result = filter_cargo_test(output);
        assert!(result.contains("✗ cargo test: 4 passed, 1 failed (1 suite)"));
        assert!(result.contains("warnings: 0"));
        assert!(result.contains("errors: 0"));
        assert!(result.contains("FAILURES (1)"));
        assert!(result.contains("foo::test_b"));
        assert!(result.contains("assert_eq!(1, 2)"));
        assert_eq!(result.matches("foo::test_b").count(), 1, "got: {result}");
    }

    #[test]
    fn test_filter_cargo_test_multi_suite_all_pass() {
        let output = r#"   Compiling rtk v0.5.0
    Finished test [unoptimized + debuginfo] target(s) in 2.53s
     Running unittests src/lib.rs (target/debug/deps/rtk-abc123)

running 50 tests
test result: ok. 50 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.45s

     Running unittests src/main.rs (target/debug/deps/rtk-def456)

running 30 tests
test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.30s

     Running tests/integration.rs (target/debug/deps/integration-ghi789)

running 25 tests
test result: ok. 25 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.25s

   Doc-tests rtk

running 32 tests
test result: ok. 32 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.45s
"#;
        let result = filter_cargo_test(output);
        assert!(
            result.contains("✓ cargo test: 137 passed (4 suites, 1.45s)"),
            "Expected aggregated format, got: {}",
            result
        );
        assert!(!result.contains("running"));
    }

    #[test]
    fn test_filter_cargo_test_multi_suite_with_failures() {
        let output = r#"     Running unittests src/lib.rs

running 20 tests
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s

     Running unittests src/main.rs

running 15 tests
test foo::test_bad ... FAILED

failures:

---- foo::test_bad stdout ----
thread panicked at 'assertion failed'

test result: FAILED. 14 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s

     Running tests/integration.rs

running 10 tests
        test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
"#;
        let result = filter_cargo_test(output);
        assert!(
            result.contains("✗ cargo test: 44 passed, 1 failed (3 suites, 0.17s)"),
            "got: {}",
            result
        );
        assert!(result.contains("warnings: 0"), "got: {}", result);
        assert!(result.contains("errors: 0"), "got: {}", result);
        assert!(result.contains("FAILURES (1)"), "got: {}", result);
        assert!(result.contains("test_bad"), "got: {}", result);
        assert!(result.contains("assertion failed"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_test_caps_failure_details() {
        let mut output = String::from("failures:\n\n");
        for index in 0..=CAP_WARNINGS {
            output.push_str(&format!(
                "---- suite::test_{index} stdout ----\nthread panicked at 'failure {index}'\n\n"
            ));
        }
        output.push_str(
            "failures:\n    suite::test_0\n\ntest result: FAILED. 0 passed; 11 failed; 0 ignored; 0 measured; 0 filtered out\n",
        );

        let result = filter_cargo_test(&output);
        assert!(result.contains("FAILURES (11)"), "got: {result}");
        assert!(result.contains("suite::test_9"), "got: {result}");
        assert!(!result.contains("suite::test_10 stdout"), "got: {result}");
        assert!(result.contains("… +1 more failures"), "got: {result}");
    }

    #[test]
    fn test_filter_cargo_test_all_suites_zero_tests() {
        let output = r#"     Running unittests src/empty1.rs

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/empty2.rs

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/empty3.rs

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
"#;
        let result = filter_cargo_test(output);
        assert!(
            result.contains("✓ cargo test: 0 passed (3 suites, 0.00s)"),
            "Expected compact format for zero tests, got: {}",
            result
        );
    }

    #[test]
    fn test_filter_cargo_test_with_ignored_and_filtered() {
        let output = r#"     Running unittests src/lib.rs

running 50 tests
test result: ok. 45 passed; 0 failed; 3 ignored; 0 measured; 2 filtered out; finished in 0.50s

     Running tests/integration.rs

running 20 tests
test result: ok. 18 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.20s
"#;
        let result = filter_cargo_test(output);
        assert!(
            result.contains("✓ cargo test: 63 passed, 5 ignored, 2 filtered out (2 suites, 0.70s)"),
            "Expected compact format with ignored and filtered, got: {}",
            result
        );
    }

    #[test]
    fn test_filter_cargo_test_single_suite_compact() {
        let output = r#"     Running unittests src/main.rs

running 15 tests
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
"#;
        let result = filter_cargo_test(output);
        assert!(
            result.contains("✓ cargo test: 15 passed (1 suite, 0.01s)"),
            "Expected singular 'suite', got: {}",
            result
        );
    }

    #[test]
    fn test_filter_cargo_test_regex_fallback() {
        let output = r#"     Running unittests src/main.rs

running 15 tests
test result: MALFORMED LINE WITHOUT PROPER FORMAT
"#;
        let result = filter_cargo_test(output);
        assert!(
            result.contains("cargo test: test result: MALFORMED LINE WITHOUT PROPER FORMAT"),
            "Expected fallback format, got: {}",
            result
        );
    }

    #[test]
    fn test_filter_cargo_test_compile_error_shows_details() {
        // When cargo test fails to compile, show actual error[E...] messages
        let output = r#"   Compiling rtk v0.5.0
error[E0425]: cannot find value `mem_context` in this scope
 --> src/init.rs:567:13
  |
567|     let x = mem_context;
  |             ^^^^^^^^^^^ not found in this scope

error[E0061]: this function takes 6 arguments but 7 were supplied
 --> src/init.rs:123:5

error: aborting due to 2 previous errors
"#;
        let result = filter_cargo_test(output);
        // Must show error codes, not just a count
        assert!(
            result.contains("E0425"),
            "Expected error code E0425, got: {}",
            result
        );
        assert!(
            result.contains("E0061"),
            "Expected error code E0061, got: {}",
            result
        );
        assert!(
            result.contains("cargo test:"),
            "Expected cargo test: prefix, got: {}",
            result
        );
        // Must NOT reduce to just a count summary
        assert!(
            !result.contains("cargo test: completed"),
            "Should not show 'completed' on compile failure"
        );
    }

    #[test]
    fn test_filter_cargo_test_verbose_keeps_failure_details() {
        let output = r#"running 3 tests
test foo::a ... ok
test foo::b ... FAILED

failures:
---- foo::b stdout ----
thread 'foo::b' panicked

test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
"#;
        let result = filter_cargo_test_verbose(output);
        assert!(result.contains("FAILURES"));
        assert!(result.contains("foo::b"));
        assert!(result.contains("test result: FAILED"));
    }

    #[test]
    fn test_filter_cargo_clippy_clean() {
        let output = r#"    Checking rtk v0.5.0
    Finished dev [unoptimized + debuginfo] target(s) in 1.53s
"#;
        let result = filter_cargo_clippy(output);
        assert!(result.contains("✓ cargo clippy: No issues found"));
    }

    #[test]
    fn test_filter_cargo_clippy_warnings() {
        let output = r#"    Checking rtk v0.5.0
warning: unused variable: `x` [unused_variables]
 --> src/main.rs:10:9
  |
10|     let x = 5;
  |         ^ help: if this is intentional, prefix it with an underscore: `_x`

warning: this function has too many arguments [clippy::too_many_arguments]
 --> src/git.rs:16:1
  |
16| pub fn run(a: i32, b: i32, c: i32, d: i32, e: i32, f: i32, g: i32, h: i32) {}
  |

warning: `rtk` (bin) generated 2 warnings
    Finished dev [unoptimized + debuginfo] target(s) in 1.53s
"#;
        let result = filter_cargo_clippy(output);
        assert!(result.contains("0 errors, 2 warnings"));
        assert!(result.contains("unused_variables"));
        assert!(result.contains("clippy::too_many_arguments"));
    }

    #[test]
    fn test_filter_cargo_install_success() {
        let output = r#"  Installing rtk v0.11.0
  Downloading crates ...
  Downloaded anyhow v1.0.80
  Downloaded clap v4.5.0
   Compiling libc v0.2.153
   Compiling cfg-if v1.0.0
   Compiling anyhow v1.0.80
   Compiling clap v4.5.0
   Compiling rtk v0.11.0
    Finished `release` profile [optimized] target(s) in 45.23s
  Replacing /Users/user/.cargo/bin/rtk
   Replaced package `rtk v0.9.4` with `rtk v0.11.0` (/Users/user/.cargo/bin/rtk)
"#;
        let result = filter_cargo_install(output);
        assert!(result.contains("✓ cargo install"), "got: {}", result);
        assert!(result.contains("rtk v0.11.0"), "got: {}", result);
        assert!(result.contains("5 deps compiled"), "got: {}", result);
        assert!(result.contains("Replaced"), "got: {}", result);
        assert!(!result.contains("Compiling"), "got: {}", result);
        assert!(!result.contains("Downloading"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_replace() {
        let output = r#"  Installing rtk v0.11.0
   Compiling rtk v0.11.0
    Finished `release` profile [optimized] target(s) in 10.0s
  Replacing /Users/user/.cargo/bin/rtk
   Replaced package `rtk v0.9.4` with `rtk v0.11.0` (/Users/user/.cargo/bin/rtk)
"#;
        let result = filter_cargo_install(output);
        assert!(result.contains("✓ cargo install"), "got: {}", result);
        assert!(result.contains("Replacing"), "got: {}", result);
        assert!(result.contains("Replaced"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_error() {
        let output = r#"  Installing rtk v0.11.0
   Compiling rtk v0.11.0
error[E0308]: mismatched types
 --> src/main.rs:10:5
  |
10|     "hello"
  |     ^^^^^^^ expected `i32`, found `&str`

error: aborting due to 1 previous error
"#;
        let result = filter_cargo_install(output);
        assert!(result.contains("cargo install: 1 error"), "got: {}", result);
        assert!(result.contains("E0308"), "got: {}", result);
        assert!(result.contains("mismatched types"), "got: {}", result);
        assert!(!result.contains("aborting"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_already_installed() {
        let output = r#"  Ignored package `rtk v0.11.0`, is already installed
"#;
        let result = filter_cargo_install(output);
        assert!(result.contains("already installed"), "got: {}", result);
        assert!(result.contains("rtk v0.11.0"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_up_to_date() {
        let output = r#"  Ignored package `cargo-deb v2.1.0 (/Users/user/cargo-deb)`, is already installed
"#;
        let result = filter_cargo_install(output);
        assert!(result.contains("already installed"), "got: {}", result);
        assert!(result.contains("cargo-deb v2.1.0"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_empty_output() {
        let result = filter_cargo_install("");
        assert!(result.contains("✓ cargo install"), "got: {}", result);
        assert!(result.contains("0 deps compiled"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_path_warning() {
        let output = r#"  Installing rtk v0.11.0
   Compiling rtk v0.11.0
    Finished `release` profile [optimized] target(s) in 10.0s
  Replacing /Users/user/.cargo/bin/rtk
   Replaced package `rtk v0.9.4` with `rtk v0.11.0` (/Users/user/.cargo/bin/rtk)
warning: be sure to add `/Users/user/.cargo/bin` to your PATH
"#;
        let result = filter_cargo_install(output);
        assert!(result.contains("✓ cargo install"), "got: {}", result);
        assert!(
            result.contains("be sure to add"),
            "PATH warning should be kept: {}",
            result
        );
        assert!(result.contains("Replaced"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_multiple_errors() {
        let output = r#"  Installing rtk v0.11.0
   Compiling rtk v0.11.0
error[E0308]: mismatched types
 --> src/main.rs:10:5
  |
10|     "hello"
  |     ^^^^^^^ expected `i32`, found `&str`

error[E0425]: cannot find value `foo`
 --> src/lib.rs:20:9
  |
20|     foo
  |     ^^^ not found in this scope

error: aborting due to 2 previous errors
"#;
        let result = filter_cargo_install(output);
        assert!(
            result.contains("2 errors"),
            "should show 2 errors: {}",
            result
        );
        assert!(result.contains("E0308"), "got: {}", result);
        assert!(result.contains("E0425"), "got: {}", result);
        assert!(!result.contains("aborting"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_locking_and_blocking() {
        let output = r#"  Locking 45 packages to latest compatible versions
  Blocking waiting for file lock on package cache
  Downloading crates ...
  Downloaded serde v1.0.200
   Compiling serde v1.0.200
   Compiling rtk v0.11.0
    Finished `release` profile [optimized] target(s) in 30.0s
  Installing rtk v0.11.0
"#;
        let result = filter_cargo_install(output);
        assert!(result.contains("✓ cargo install"), "got: {}", result);
        assert!(!result.contains("Locking"), "got: {}", result);
        assert!(!result.contains("Blocking"), "got: {}", result);
        assert!(!result.contains("Downloading"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_from_path() {
        let output = r#"  Installing /Users/user/projects/rtk
   Compiling rtk v0.11.0
    Finished `release` profile [optimized] target(s) in 10.0s
"#;
        let result = filter_cargo_install(output);
        // Path-based install: crate info not extracted from path
        assert!(result.contains("✓ cargo install"), "got: {}", result);
        assert!(result.contains("1 deps compiled"), "got: {}", result);
    }

    #[test]
    fn test_format_crate_info() {
        assert_eq!(format_crate_info("rtk", "v0.11.0", ""), "rtk v0.11.0");
        assert_eq!(format_crate_info("rtk", "", ""), "rtk");
        assert_eq!(format_crate_info("", "", "package"), "package");
        assert_eq!(format_crate_info("", "v0.1.0", "fallback"), "fallback");
    }

    #[test]
    fn test_filter_cargo_nextest_all_pass() {
        let output = r#"   Compiling rtk v0.15.2
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.04s
────────────────────────────
    Starting 301 tests across 1 binary
        PASS [   0.009s] (1/301) rtk::bin/rtk cargo_cmd::tests::test_one
        PASS [   0.008s] (2/301) rtk::bin/rtk cargo_cmd::tests::test_two
        PASS [   0.007s] (301/301) rtk::bin/rtk cargo_cmd::tests::test_last
────────────────────────────
     Summary [   0.192s] 301 tests run: 301 passed, 0 skipped
"#;
        let result = filter_cargo_nextest(output);
        assert_eq!(
            result, "✓ cargo nextest: 301 passed (1 binary, 0.192s)",
            "got: {}",
            result
        );
    }

    #[test]
    fn test_filter_cargo_nextest_with_failures() {
        let output = r#"    Starting 4 tests across 1 binary (1 test skipped)
        PASS [   0.006s] (1/4) test-proj tests::passing_test
        FAIL [   0.006s] (2/4) test-proj tests::failing_test

  stderr ───

    thread 'tests::failing_test' panicked at src/lib.rs:15:9:
    assertion `left == right` failed
      left: 1
     right: 2

  Cancelling due to test failure: 2 tests still running
        PASS [   0.007s] (3/4) test-proj tests::another_passing
        FAIL [   0.006s] (4/4) test-proj tests::another_failing

  stderr ───

    thread 'tests::another_failing' panicked at src/lib.rs:20:9:
    something went wrong

────────────────────────────
     Summary [   0.007s] 4 tests run: 2 passed, 2 failed, 1 skipped
        FAIL [   0.006s] (2/4) test-proj tests::failing_test
        FAIL [   0.006s] (4/4) test-proj tests::another_failing
error: test run failed
"#;
        let result = filter_cargo_nextest(output);
        assert!(
            result.contains("tests::failing_test"),
            "should contain first failure: {}",
            result
        );
        assert!(
            result.contains("tests::another_failing"),
            "should contain second failure: {}",
            result
        );
        assert!(
            result.contains("panicked"),
            "should contain stderr detail: {}",
            result
        );
        assert!(
            result.contains("2 passed, 2 failed, 1 skipped"),
            "should contain summary: {}",
            result
        );
        assert!(
            !result.contains("PASS"),
            "should not contain PASS lines: {}",
            result
        );
        // Post-summary FAIL recaps must not create duplicate FAIL header entries
        // (test names may appear in both header and stderr body naturally)
        assert_eq!(
            result.matches("FAIL [").count(),
            2,
            "should have exactly 2 FAIL headers (no post-summary duplicates): {}",
            result
        );
        assert!(
            !result.contains("error: test run failed"),
            "should not contain post-summary error line: {}",
            result
        );
    }

    #[test]
    fn test_filter_cargo_nextest_with_skipped() {
        let output = r#"    Starting 50 tests across 2 binaries (3 tests skipped)
        PASS [   0.010s] (1/50) rtk::bin/rtk test_one
        PASS [   0.010s] (50/50) rtk::bin/rtk test_last
────────────────────────────
     Summary [   0.500s] 50 tests run: 50 passed, 3 skipped
"#;
        let result = filter_cargo_nextest(output);
        assert_eq!(
            result, "✓ cargo nextest: 50 passed, 3 skipped (2 binaries, 0.500s)",
            "got: {}",
            result
        );
    }

    #[test]
    fn test_filter_cargo_nextest_single_failure_detail() {
        let output = r#"    Starting 2 tests across 1 binary
        PASS [   0.005s] (1/2) proj tests::good
        FAIL [   0.005s] (2/2) proj tests::bad

  stderr ───

    thread 'tests::bad' panicked at src/lib.rs:5:9:
    assertion failed: false

────────────────────────────
     Summary [   0.010s] 2 tests run: 1 passed, 1 failed
        FAIL [   0.005s] (2/2) proj tests::bad
error: test run failed
"#;
        let result = filter_cargo_nextest(output);
        assert!(
            result.contains("assertion failed: false"),
            "should show panic message: {}",
            result
        );
        assert!(
            result.contains("1 passed, 1 failed"),
            "should show summary: {}",
            result
        );
        // Post-summary recap must not duplicate FAIL headers
        assert_eq!(
            result.matches("FAIL [").count(),
            1,
            "should have exactly 1 FAIL header (no post-summary duplicate): {}",
            result
        );
    }

    #[test]
    fn test_filter_cargo_nextest_multiple_binaries() {
        let output = r#"    Starting 100 tests across 5 binaries
        PASS [   0.010s] (100/100) test_last
────────────────────────────
     Summary [   1.234s] 100 tests run: 100 passed, 0 skipped
"#;
        let result = filter_cargo_nextest(output);
        assert_eq!(
            result, "✓ cargo nextest: 100 passed (5 binaries, 1.234s)",
            "got: {}",
            result
        );
    }

    #[test]
    fn test_filter_cargo_nextest_compilation_stripped() {
        let output = r#"   Compiling serde v1.0.200
   Compiling rtk v0.15.2
   Downloading crates ...
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.00s
────────────────────────────
    Starting 10 tests across 1 binary
        PASS [   0.010s] (10/10) test_last
────────────────────────────
     Summary [   0.050s] 10 tests run: 10 passed, 0 skipped
"#;
        let result = filter_cargo_nextest(output);
        assert!(
            !result.contains("Compiling"),
            "should strip Compiling: {}",
            result
        );
        assert!(
            !result.contains("Downloading"),
            "should strip Downloading: {}",
            result
        );
        assert!(
            !result.contains("Finished"),
            "should strip Finished: {}",
            result
        );
        assert!(
            result.contains("✓ cargo nextest: 10 passed"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_filter_cargo_nextest_empty() {
        let result = filter_cargo_nextest("");
        assert!(result.is_empty(), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_nextest_cancellation_notice() {
        let output = r#"    Starting 3 tests across 1 binary
        FAIL [   0.005s] (1/3) proj tests::bad

  stderr ───

    thread panicked at 'oops'

  Cancelling due to test failure: 2 tests still running
────────────────────────────
     Summary [   0.010s] 3 tests run: 2 passed, 1 failed
        FAIL [   0.005s] (1/3) proj tests::bad
error: test run failed
"#;
        let result = filter_cargo_nextest(output);
        assert!(
            result.contains("Cancelling due to test failure"),
            "should include cancel notice: {}",
            result
        );
        assert!(
            result.contains("1 failed"),
            "should show failure count: {}",
            result
        );
        // Post-summary recap must not duplicate FAIL headers
        assert_eq!(
            result.matches("FAIL [").count(),
            1,
            "should have exactly 1 FAIL header (no post-summary duplicate): {}",
            result
        );
    }

    #[test]
    fn test_filter_cargo_nextest_summary_regex_fallback() {
        let output = r#"    Starting 5 tests across 1 binary
        PASS [   0.005s] (5/5) test_last
────────────────────────────
     Summary MALFORMED LINE
"#;
        let result = filter_cargo_nextest(output);
        assert!(
            result.contains("Summary MALFORMED"),
            "should fall back to raw summary: {}",
            result
        );
    }

    // --- filter_cargo_build dedup tests ---

    #[test]
    fn test_filter_cargo_build_dedup_exact_duplicate_errors() {
        // Simulates cargo reporting the same error for lib + bin targets
        let dup_error = "error[E0277]: the trait bound `DetailLevel: Default` is not satisfied\n  --> src/config.rs:45:23\n  |\n  | ...";
        let output = format!("{}\n\n{}", dup_error, dup_error);
        let result = filter_cargo_build(&output);
        // Should appear exactly once after dedup
        assert_eq!(
            result.matches("error[E0277]").count(),
            1,
            "duplicate error must be deduped: {}",
            result
        );
        assert!(
            result.contains("1 errors"),
            "header should reflect 1 unique error: {}",
            result
        );
    }

    #[test]
    fn test_filter_cargo_build_distinct_errors_preserved() {
        let error_a = "error[E0277]: trait bound not satisfied\n  --> src/config.rs:10:5";
        let error_b = "error[E0308]: mismatched types\n  --> src/main.rs:20:9";
        let output = format!("{}\n\n{}", error_a, error_b);
        let result = filter_cargo_build(&output);
        assert_eq!(
            result.matches("error[E").count(),
            2,
            "distinct errors must both appear: {}",
            result
        );
        assert!(
            result.contains("2 errors"),
            "header should reflect 2 errors: {}",
            result
        );
    }

    // Regression: short error blocks (≤3 lines) include the trailing blank line
    // in their content (because the "> 3" flush condition is not met). This caused
    // one block to end with "\n" and the other not to, making HashSet dedup fail.
    // The trim_end normalization before dedup fixes this.
    #[test]
    fn test_filter_cargo_build_dedup_short_block_with_trailing_blank() {
        // 2-line error: exactly the pattern "error: unknown start of token: \"
        // that cargo emits for backslash-escape issues. With lib+bin targets,
        // cargo emits this twice; the first flush includes the blank separator.
        let short_err = "error: unknown start of token: \\\n --> src/main.rs:10:5";
        let output = format!("{}\n\n{}", short_err, short_err);
        let result = filter_cargo_build(&output);
        assert_eq!(
            result.matches("unknown start of token").count(),
            1,
            "short duplicate error must be deduped even with trailing blank: {}",
            result
        );
        assert!(
            result.contains("1 errors"),
            "header must reflect 1 unique error: {}",
            result
        );
    }

    // --- filter_cargo_clippy dedup tests ---

    #[test]
    fn test_filter_cargo_clippy_dedup_duplicate_locations() {
        // Simulates same warning reported for lib + bin targets (same file:line)
        let output = "\
warning: unused variable: `x` [unused_variables]\n  --> src/lib.rs:10:9\n  |\n10 |     let x = 1;\n\
warning: unused variable: `x` [unused_variables]\n  --> src/lib.rs:10:9\n  |\n10 |     let x = 1;\n\
";
        let result = filter_cargo_clippy(output);
        assert_eq!(
            result.matches("src/lib.rs:10:9").count(),
            1,
            "duplicate location must be deduped: {}",
            result
        );
        assert!(
            result.contains("1 warnings"),
            "header should reflect 1 unique warning: {}",
            result
        );
    }

    #[test]
    fn test_filter_cargo_clippy_distinct_locations_preserved() {
        let output = "\
warning: unused variable: `x` [unused_variables]\n  --> src/lib.rs:10:9\n\
warning: unused variable: `y` [unused_variables]\n  --> src/lib.rs:20:9\n\
";
        let result = filter_cargo_clippy(output);
        assert_eq!(
            result.matches("src/lib.rs").count(),
            2,
            "distinct locations must both appear: {}",
            result
        );
        assert!(
            result.contains("2 warnings"),
            "header should reflect 2 warnings: {}",
            result
        );
    }
}
