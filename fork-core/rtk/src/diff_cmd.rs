//! `rtk diff`: the native `diff`, bounded.
//!
//! 0.11.0 (US-001): rewritten. The inherited module compared line N to line N,
//! declared files "identical" when every difference was a modification, read
//! files through `read_to_string` (failing on non-UTF-8), truncated lines to 80
//! characters, never exited 1, and — because its never-worse guard measured
//! against a dump of both files — printed exactly that dump for ordinary edits.
//!
//! Upstream RTK v0.50.0 answers with its own aligner and a large family of
//! edge-case renderers, and measures its savings against the classic diff. For
//! HZR the classic diff *is* the answer: `diff(1)` already aligns minimally and
//! owns exit codes 0/1/2, line endings, missing final newlines, binary files and
//! every flag (`-u`, `-r`, `-q`, `-w`, …). RTK adds only what a model needs on
//! top: a bound on very large outputs with the exact recovery command.

use crate::tracking;
use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::process::{Command, Stdio};

/// Visible-output bounds, matching the bounded passthrough used by `find`.
const MAX_LINES: usize = 200;
const MAX_BYTES: usize = 16 * 1024;

/// `rtk diff [diff-args…]`: run the native `diff` with the caller's argv.
/// Returns diff's own exit code (0 identical, 1 different, 2 trouble).
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    if verbose > 0 {
        eprintln!("diff: {}", args.join(" "));
    }
    let output = Command::new("diff")
        .args(args)
        .stdin(Stdio::inherit()) // `diff - file` reads the caller's stdin
        .output()
        .context("Failed to run diff")?;

    let command = std::iter::once("diff".to_string())
        .chain(args.iter().map(|a| shell_quote_word(a)))
        .collect::<Vec<_>>()
        .join(" ");
    let (shown, omitted) = bound_output(&output.stdout);
    {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(shown).context("Failed to write diff output")?;
        if omitted > 0 {
            if !shown.ends_with(b"\n") {
                stdout.write_all(b"\n")?;
            }
            writeln!(stdout, "{}", recovery_notice(omitted, &command))?;
        }
        stdout.flush()?;
    }
    {
        let mut stderr = std::io::stderr().lock();
        stderr.write_all(&output.stderr)?;
        stderr.flush()?;
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let delivered = if omitted > 0 {
        format!(
            "{}\n{}",
            String::from_utf8_lossy(shown),
            recovery_notice(omitted, &command)
        )
    } else {
        raw.to_string()
    };
    timer.track(&command, "rtk diff", &raw, &delivered);
    Ok(crate::stream::status_to_exit_code(output.status))
}

/// The first lines of `bytes` that fit the bounds, sliced from the original so
/// CRLF and non-UTF-8 bytes survive, plus how many lines were left out.
fn bound_output(bytes: &[u8]) -> (&[u8], usize) {
    let total_lines = bytes.iter().filter(|b| **b == b'\n').count()
        + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
    if total_lines <= MAX_LINES && bytes.len() <= MAX_BYTES {
        return (bytes, 0);
    }
    let (mut end, mut kept) = (0usize, 0usize);
    for (i, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            if kept == MAX_LINES || i + 1 > MAX_BYTES {
                break;
            }
            kept += 1;
            end = i + 1;
        }
    }
    (&bytes[..end], total_lines - kept)
}

fn recovery_notice(omitted: usize, command: &str) -> String {
    format!(
        "[{omitted} line(s) omitted; full diff: HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=full_patch hzr exec run {}]",
        shell_quote_word(command)
    )
}

fn shell_quote_word(word: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "/._-+=:,@%".contains(c);
    if !word.is_empty() && word.chars().all(plain) {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// `… | rtk diff`: compact a piped unified diff.
///
/// A git-style diff (`diff --git` sections) goes through the same compaction as
/// `rtk git diff`, which keeps markers at column 0, full hunk headers, hunk
/// bodies bounded by their declared lengths (so `++x`/`--x` content lines are
/// content) and counted truncation with a recovery line. Anything else is
/// printed unchanged within the visible bound. (0.11.0, US-001)
pub fn run_stdin(_verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes)?;

    let input = String::from_utf8_lossy(&bytes);
    let git_shaped = std::str::from_utf8(&bytes).is_ok()
        && input.lines().any(|line| line.starts_with("diff --git "));
    let mut stdout = std::io::stdout().lock();
    if git_shaped {
        let condensed = crate::git::compact_diff(&input, 500);
        let shown = crate::guard::never_worse_content(&input, &condensed);
        stdout.write_all(shown.as_bytes())?;
        if !shown.ends_with('\n') {
            stdout.write_all(b"\n")?;
        }
        timer.track("diff (stdin)", "rtk diff (stdin)", &input, shown);
    } else {
        let (shown, omitted) = bound_output(&bytes);
        stdout.write_all(shown)?;
        let mut delivered = String::from_utf8_lossy(shown).into_owned();
        if omitted > 0 {
            let notice = format!(
                "[{omitted} line(s) omitted; the full diff is the piped command's own output]"
            );
            if !shown.ends_with(b"\n") {
                stdout.write_all(b"\n")?;
            }
            writeln!(stdout, "{notice}")?;
            delivered.push_str(&notice);
        }
        timer.track("diff (stdin)", "rtk diff (stdin)", &input, &delivered);
    }
    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_outputs_pass_through_byte_exact() {
        for bytes in [&b""[..], b"2c2\n< a\r\n---\n> b\r\n", b"\\ No newline", b"caf\xe9\n"] {
            assert_eq!(bound_output(bytes), (bytes, 0), "{bytes:?}");
        }
    }

    #[test]
    fn long_outputs_keep_whole_leading_lines_and_count_the_rest() {
        let bytes: Vec<u8> = (0..450).flat_map(|i| format!("> {i}\n").into_bytes()).collect();
        let (shown, omitted) = bound_output(&bytes);
        assert_eq!(shown.iter().filter(|b| **b == b'\n').count(), MAX_LINES);
        assert_eq!(omitted, 450 - MAX_LINES);
        assert!(shown.ends_with(b"\n"));
    }

    #[test]
    fn the_byte_bound_applies_to_long_lines() {
        let line = format!("> {}\n", "x".repeat(4000));
        let bytes = line.repeat(10).into_bytes();
        let (shown, omitted) = bound_output(&bytes);
        assert!(shown.len() <= MAX_BYTES, "{}", shown.len());
        assert_eq!(omitted, 10 - shown.iter().filter(|b| **b == b'\n').count());
    }

    #[test]
    fn recovery_names_the_exact_command() {
        let notice = recovery_notice(3, "diff -u 'a b' c");
        assert!(notice.contains("3 line(s) omitted"));
        assert!(notice.contains("hzr exec run 'diff -u '\\''a b'\\'' c'"), "{notice}");
    }
}
