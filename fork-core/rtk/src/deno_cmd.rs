//! `rtk deno`: compact `deno lint`, `deno check` and `deno test`.
//!
//! 0.11.0 (US-009): ported in reduced form from upstream RTK v0.50.0
//! (856c345, 2416b8c, 65cd2f7, d23e158, 50bee30, 1173de0). Arguments go to deno
//! as an argv, never through a shell. Every other subcommand (`run`, `task`,
//! `fmt`, `compile`, …), watch mode and a reporter the caller chose run
//! unfiltered, so their output and exit code are deno's own.

use crate::stream::RelayedOutput;
use crate::tracking;
use crate::utils::{resolved_command, strip_ansi};
use anyhow::{Context, Result};

/// Dispatch `rtk deno <subcommand> [args…]`.
pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let (subcommand, rest) = match args.split_first() {
        Some((sub, rest)) => (sub.as_str(), rest),
        None => return passthrough(args, verbose),
    };
    let watch = rest.iter().any(|a| a == "--watch" || a.starts_with("--watch="));
    match subcommand {
        "lint" | "check" if !watch => run_filtered(subcommand, rest, verbose, filter_deno_output),
        "test" if !watch && !chose_output_format(rest) => {
            run_filtered("test", rest, verbose, filter_deno_test)
        }
        _ => passthrough(args, verbose),
    }
}

/// A format the caller named (`--reporter`, `--junit-path`) is the output they want.
fn chose_output_format(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "--reporter" || a.starts_with("--reporter=") || a.starts_with("--junit-path"))
}

fn run_filtered(
    subcommand: &str,
    args: &[String],
    verbose: u8,
    filter: fn(&str) -> String,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let mut cmd = resolved_command("deno");
    cmd.arg(subcommand).args(args);
    if verbose > 0 {
        eprintln!("Running: deno {} {}", subcommand, args.join(" "));
    }
    let output = cmd
        .output_relayed()
        .context("Failed to run deno. Is Deno installed?")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = crate::utils::make_raw(&stdout, &stderr);
    let exit_code = crate::stream::status_to_exit_code(output.status);

    let label = format!("deno {} {}", subcommand, args.join(" "));
    let filtered = crate::guard::guard_exit(&raw, exit_code, label.trim_end(), &filter(&raw));
    let hint = crate::tee::tee_and_hint(&raw, &format!("deno_{subcommand}"), exit_code);
    let shown = crate::runner::emit_guarded(&filtered, hint.as_deref(), &raw);
    timer.track(label.trim_end(), &format!("rtk {}", label.trim_end()), &raw, &shown);
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}

fn passthrough(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    if verbose > 0 {
        eprintln!("Running: deno {}", args.join(" "));
    }
    let status = resolved_command("deno")
        .args(args)
        .status()
        .context("Failed to run deno. Is Deno installed?")?;
    let label = format!("deno {}", args.join(" "));
    timer.track_passthrough(&label, &format!("rtk {label} (passthrough)"));
    let code = crate::stream::status_to_exit_code(status);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// Lines deno prints on a cold cache or before type-checking, with no diagnostic in them.
fn is_deno_noise(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || trimmed.starts_with("Download ")
}

/// `deno lint` / `deno check`: every diagnostic kept; colour, download lines and
/// blank lines dropped.
pub fn filter_deno_output(output: &str) -> String {
    let cleaned = strip_ansi(output);
    let kept: Vec<&str> = cleaned.lines().filter(|l| !is_deno_noise(l)).collect();
    if kept.is_empty() {
        "ok".to_string()
    } else {
        kept.join("\n")
    }
}

/// `deno test`: passing tests, per-file banners and `Check` lines go; failures,
/// their errors and the final tally stay.
pub fn filter_deno_test(output: &str) -> String {
    let cleaned = strip_ansi(output);
    let kept: Vec<&str> = cleaned
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            let file_banner = trimmed.starts_with("running ") && trimmed.contains(" test");
            let dropped = is_deno_noise(line)
                || trimmed.starts_with("Check file:")
                || file_banner
                || is_passing_test_line(trimmed);
            !dropped
        })
        .collect();
    if kept.is_empty() {
        "ok".to_string()
    } else {
        kept.join("\n")
    }
}

/// `name ... ok (3ms)` — also nested steps, which deno indents.
fn is_passing_test_line(trimmed: &str) -> bool {
    trimmed
        .rsplit_once(" ... ok")
        .is_some_and(|(name, tail)| !name.is_empty() && (tail.is_empty() || tail.starts_with(" (")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_FAILING: &str = "Check file:///p/main_test.ts\n\
running 3 tests from ./main_test.ts\n\
adds ... ok (1ms)\n\
nested ...\n  step one ... ok (0ms)\n\
nested ... ok (1ms)\n\
fails ... FAILED (2ms)\n\
\n ERRORS \n\n\
fails => ./main_test.ts:9:6\n\
error: AssertionError: Values are not equal.\n\
\n FAILURES \n\n\
fails => ./main_test.ts:9:6\n\
\nFAILED | 2 passed | 1 failed (6ms)\n\
\nerror: Test failed\n";

    #[test]
    fn test_filter_keeps_failures_and_tally() {
        let out = filter_deno_test(TEST_FAILING);
        assert!(out.contains("fails ... FAILED"), "{out}");
        assert!(out.contains("AssertionError"), "{out}");
        assert!(out.contains("FAILED | 2 passed | 1 failed"), "{out}");
        assert!(!out.contains("adds ... ok"), "{out}");
        assert!(!out.contains("step one"), "{out}");
        assert!(!out.contains("running 3 tests"), "{out}");
        assert!(out.len() < TEST_FAILING.len());
    }

    #[test]
    fn test_filter_passing_run_keeps_the_tally() {
        let raw = "Check file:///p/a_test.ts\nrunning 1 test from ./a_test.ts\nadds ... ok (1ms)\n\nok | 1 passed | 0 failed (2ms)\n";
        assert_eq!(filter_deno_test(raw), "ok | 1 passed | 0 failed (2ms)");
    }

    #[test]
    fn test_filter_deno_output_strips_noise_and_colour() {
        let raw = "\x1b[33mDownload https://deno.land/std/mod.ts\x1b[0m\n\n\x1b[31merror[no-var]\x1b[0m: `var` keyword is not allowed.\n    at /p/main.ts:1:1\n\nFound 1 problem\n";
        let out = filter_deno_output(raw);
        assert!(!out.contains("Download"), "{out}");
        assert!(!out.contains('\u{1b}'), "{out}");
        assert!(out.contains("error[no-var]"), "{out}");
        assert!(out.contains("Found 1 problem"), "{out}");
        assert_eq!(filter_deno_output("Download https://x\n\n"), "ok");
    }

    #[test]
    fn test_named_reporters_and_watch_are_not_filtered() {
        for a in ["--reporter=junit", "--reporter", "--junit-path=r.xml"] {
            assert!(chose_output_format(&[a.to_string()]), "{a}");
        }
        assert!(!chose_output_format(&["--allow-all".to_string()]));
    }
}
