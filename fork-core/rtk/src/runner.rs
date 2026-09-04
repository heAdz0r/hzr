use crate::tracking;
use anyhow::{bail, Context, Result};
use regex::Regex;
use std::process::{Command, Stdio};

pub fn emit_guarded(filtered: &str, hint: Option<&str>, raw: &str) -> String {
    let body = match hint {
        Some(hint) => format!("{}\n{}", filtered, hint),
        None => filtered.to_string(),
    };
    let shown = crate::guard::never_worse(raw, &body).to_string();
    println!("{}", shown);
    shown
}

/// Run a command and filter output to show only errors/warnings
pub fn run_err(command: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let display = command.join(" ");

    if verbose > 0 {
        eprintln!("Running: {}", display);
    }

    let Some((program, arguments)) = command.split_first() else {
        bail!("test/error command cannot be empty");
    };
    let output = Command::new(program)
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("Failed to execute command")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);
    let filtered = filter_errors(&raw);
    let mut rtk = String::new();

    if filtered.is_empty() {
        if output.status.success() {
            rtk.push_str("✅ Command completed successfully (no errors)");
        } else {
            rtk.push_str(&format!(
                "❌ Command failed (exit code: {:?})\n",
                output.status.code()
            ));
            let lines: Vec<&str> = raw.lines().collect();
            for line in lines.iter().rev().take(10).rev() {
                rtk.push_str(&format!("  {}\n", line));
            }
        }
    } else {
        rtk.push_str(&filtered);
    }

    let exit_code = output // upstream sync: tee integration
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });
    let hint = crate::tee::tee_and_hint(&raw, "err", exit_code);
    let shown = emit_guarded(&rtk, hint.as_deref(), &raw);
    timer.track(&display, "rtk run-err", &raw, &shown);
    if !output.status.success() {
        std::process::exit(exit_code);
    }
    Ok(())
}

/// Run tests and show only failures
pub fn run_test(command: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let display = command.join(" ");

    if verbose > 0 {
        eprintln!("Running tests: {}", display);
    }

    let Some((program, arguments)) = command.split_first() else {
        bail!("test/error command cannot be empty");
    };
    let output = Command::new(program)
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("Failed to execute test command")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = output // upstream sync: tee integration
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });
    let summary = if output.status.success() {
        extract_test_summary(&raw, &display)
    } else {
        extract_failure_summary(&raw, &display)
    };
    let hint = crate::tee::tee_and_hint(&raw, "test", exit_code);
    let shown = emit_guarded(&summary, hint.as_deref(), &raw);
    timer.track(&display, "rtk run-test", &raw, &shown);
    if !output.status.success() {
        std::process::exit(exit_code);
    }
    Ok(())
}

fn filter_errors(output: &str) -> String {
    lazy_static::lazy_static! {
        static ref ERROR_PATTERNS: Vec<Regex> = vec![
            // Generic errors
            Regex::new(r"(?i)^.*error[\s:\[].*$").unwrap(),
            Regex::new(r"(?i)^.*\berr\b.*$").unwrap(),
            Regex::new(r"(?i)^.*warning[\s:\[].*$").unwrap(),
            Regex::new(r"(?i)^.*\bwarn\b.*$").unwrap(),
            Regex::new(r"(?i)^.*failed.*$").unwrap(),
            Regex::new(r"(?i)^.*failure.*$").unwrap(),
            Regex::new(r"(?i)^.*exception.*$").unwrap(),
            Regex::new(r"(?i)^.*panic.*$").unwrap(),
            // Rust specific
            Regex::new(r"^error\[E\d+\]:.*$").unwrap(),
            Regex::new(r"^\s*--> .*:\d+:\d+$").unwrap(),
            // Python
            Regex::new(r"^Traceback.*$").unwrap(),
            Regex::new(r#"^\s*File ".*", line \d+.*$"#).unwrap(),
            // JavaScript/TypeScript
            Regex::new(r"^\s*at .*:\d+:\d+.*$").unwrap(),
            // Go
            Regex::new(r"^.*\.go:\d+:.*$").unwrap(),
        ];
    }

    let mut result = Vec::new();
    let mut in_error_block = false;
    let mut blank_count = 0;

    for line in output.lines() {
        let is_error_line = ERROR_PATTERNS.iter().any(|p| p.is_match(line));

        if is_error_line {
            in_error_block = true;
            blank_count = 0;
            result.push(line.to_string());
        } else if in_error_block {
            if line.trim().is_empty() {
                blank_count += 1;
                if blank_count >= 2 {
                    in_error_block = false;
                } else {
                    result.push(line.to_string());
                }
            } else if line.starts_with(' ') || line.starts_with('\t') {
                // Continuation of error
                result.push(line.to_string());
                blank_count = 0;
            } else {
                in_error_block = false;
            }
        }
    }

    result.join("\n")
}

fn extract_failure_summary(output: &str, command: &str) -> String {
    let mut blocks = Vec::new();
    let mut in_block = false;
    for line in output.lines() {
        if line.starts_with("---- ")
            && (line.ends_with(" stdout ----") || line.ends_with(" stderr ----"))
        {
            in_block = true;
        } else if line == "failures:" || line.starts_with("test result:") {
            in_block = false;
        }
        if in_block {
            blocks.push(line);
        }
    }
    if blocks.is_empty() {
        // An unknown failure format must retain its cause. A last-lines summary can
        // discard the only actionable diagnostic, costing another execution.
        return output.to_owned();
    }
    format!(
        "{}\n\n{}",
        extract_test_summary(output, command),
        blocks.join("\n").trim_end()
    )
}

fn extract_test_summary(output: &str, command: &str) -> String {
    let mut result = Vec::new();
    let lines: Vec<&str> = output.lines().collect();

    // Detect test framework
    let is_cargo = command.contains("cargo test")
        || output.lines().any(|line| line.starts_with("test result:"));
    let is_pytest = command.contains("pytest");
    let is_jest =
        command.contains("jest") || command.contains("npm test") || command.contains("yarn test");
    let is_go = command.contains("go test");

    // Collect failures
    let mut failures = Vec::new();
    let mut in_failure = false;
    let mut failure_lines = Vec::new();

    for line in lines.iter() {
        // Cargo test
        if is_cargo {
            if line.contains("test result:") {
                result.push(line.to_string());
            }
            if line.contains("FAILED") && !line.contains("test result") {
                failures.push(line.to_string());
            }
            if line.starts_with("failures:") {
                in_failure = true;
            }
            if in_failure && line.starts_with("    ") {
                failure_lines.push(line.to_string());
            }
        }

        // Pytest
        if is_pytest {
            if line.contains(" passed") || line.contains(" failed") || line.contains(" error") {
                result.push(line.to_string());
            }
            if line.contains("FAILED") {
                failures.push(line.to_string());
            }
        }

        // Jest
        if is_jest {
            if line.contains("Tests:") || line.contains("Test Suites:") {
                result.push(line.to_string());
            }
            if line.contains("✕") || line.contains("FAIL") {
                failures.push(line.to_string());
            }
        }

        // Go test
        if is_go {
            if line.starts_with("ok") || line.starts_with("FAIL") || line.starts_with("---") {
                result.push(line.to_string());
            }
            if line.contains("FAIL") {
                failures.push(line.to_string());
            }
        }
    }

    let warnings: Vec<_> = lines
        .iter()
        .filter(|line| line.trim_start().starts_with("warning:"))
        .copied()
        .collect();

    // Build output
    let mut output = String::new();
    if !warnings.is_empty() {
        output.push_str("Compiler warning summaries (details omitted):\n");
        for warning in warnings {
            output.push_str(warning);
            output.push('\n');
        }
        output.push('\n');
    }

    if !failures.is_empty() {
        output.push_str("❌ FAILURES:\n");
        for f in failures.iter().take(10) {
            output.push_str(&format!("  {}\n", f));
        }
        if failures.len() > 10 {
            output.push_str(&format!("  ... +{} more failures\n", failures.len() - 10));
        }
        output.push('\n');
    }

    if !result.is_empty() {
        output.push_str("📊 SUMMARY:\n");
        for r in &result {
            output.push_str(&format!("  {}\n", r));
        }
    } else {
        // Fallback: show last few lines
        output.push_str("📊 OUTPUT (last 5 lines):\n");
        let start = lines.len().saturating_sub(5);
        for line in &lines[start..] {
            if !line.trim().is_empty() {
                output.push_str(&format!("  {}\n", line));
            }
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_failure_retains_unindented_error_and_assertion() {
        let output = "running 1 test\ntest owns_receipt ... FAILED\n\nfailures:\n\n---- owns_receipt stdout ----\nError: no such table: commands\nthread 'owns_receipt' panicked at tests/example.rs:46:10:\nassertion left == right failed\n  left: 0\n right: 1\n\nfailures:\n    owns_receipt\n\ntest result: FAILED. 0 passed; 1 failed\nwarning: noisy generated code\nwarning: more noise\n";
        let shown = extract_failure_summary(output, "scripts/complete-gate.sh --source");
        assert!(shown.contains("no such table: commands"));
        assert!(shown.contains("tests/example.rs:46:10"));
        assert!(shown.contains("left: 0"));
        assert!(shown.contains("right: 1"));
        assert!(shown.contains("0 passed; 1 failed"));
        assert!(shown.contains("warning: noisy generated code"));
    }

    #[test]
    fn successful_test_summary_keeps_warning_signal_without_triggering_raw_fallback() {
        let raw = format!("running 1 test\ntest example ... ok\ntest result: ok. 1 passed; 0 failed\nwarning: unused field\n{}\nwarning: crate generated 1 warning\n",
            "   | compiler detail and source lines\n".repeat(30));
        let summary = extract_test_summary(&raw, "cargo test");
        assert!(summary.contains("1 passed; 0 failed"));
        assert!(summary.contains("warning: unused field"));
        assert!(summary.contains("details omitted"));
        assert_eq!(crate::guard::never_worse(&raw, &summary), summary);
        assert!(summary.len() < raw.len() / 2);
    }

    #[test]
    fn unknown_failure_is_exact_instead_of_hiding_cause_in_tail() {
        let output =
            "unknown-tool important diagnostic\n".to_owned() + &"cleanup noise\n".repeat(20);
        assert_eq!(
            extract_failure_summary(&output, "custom-test-runner"),
            output
        );
    }

    #[test]
    fn test_filter_errors() {
        let output = "info: compiling\nerror: something failed\n  at line 10\ninfo: done";
        let filtered = filter_errors(output);
        assert!(filtered.contains("error"));
        assert!(!filtered.contains("info"));
    }
}
