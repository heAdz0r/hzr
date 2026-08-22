//! Never-worse output guard: RTK never emits more tokens than the raw command.

use crate::tracking::estimate_tokens;
use regex::Regex;
use std::sync::LazyLock;

/// "1 failed" / "44 passed, 1 failed" — a NON-ZERO failed count. "0 failed" is a
/// green verdict and never matches; the digit boundary keeps "10 failed" and
/// "20 failed" (counts that merely end in a zero) reading as failures.
static NONZERO_FAILED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^0-9])[1-9]\d* failed").unwrap());

/// "3 errors" / "10 errors" — but NOT "0 errors" ("No errors found" is green).
static NONZERO_ERRORS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^0-9])[1-9]\d* errors").unwrap());

/// "2 issues" / "golangci-lint: 5 issues in 3 files" — but NOT "No issues found".
static NONZERO_ISSUES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^0-9])[1-9]\d* issues").unwrap());

static NONZERO_WARNINGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^0-9])[1-9]\d* warnings").unwrap());

static NONZERO_PROBLEMS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^0-9])[1-9]\d* problems").unwrap());

/// Bare "FAIL" — nextest `FAIL [...]`, pytest `[FAIL]`, vitest `FAIL (2)`.
/// Suppressed when the text only carries the green `FAIL (0)` verdict.
static FAILED_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bfail\b").unwrap());

/// "FAIL (0)" — the green formatter verdict; suppresses [`FAILED_WORD`].
static FAIL_ZERO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bfail\s*\(\s*0\s*\)").unwrap());

/// "Failures:" / "1 failure". Suppressed by a green "Failures: 0" count.
static FAILURES_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bfailures?\b").unwrap());

/// "Failures: 0" / "failures = 0" — green counts that suppress [`FAILURES_WORD`].
static FAILURES_ZERO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bfailures?\s*[:=]\s*0(?:\D|$)").unwrap());

/// Compiler error lines: "error TS2322", "Error:", "error[E0308]". The word
/// boundary never matches the plural "errors", so "No errors found" stays green.
static BARE_ERROR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\berror\b").unwrap());

/// Panics, exceptions, crashes.
static EXCEPTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:panic(?:ked)?|exception|assertionerror|crashed)\b").unwrap()
});

/// `ruff format --check`: files that need reformatting.
static NEED_FORMATTING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"needs? formatting|would (be )?reformat").unwrap());

/// True when `text` contains "failed" that is not part of a "0 failed" verdict.
/// The optional leading count is captured so "0 failed" stays green while
/// "10 failed" and "20 failed" read as failures.
fn has_failed_word(text: &str) -> bool {
    static FAILED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?:(\d+)\s+)?failed\b").unwrap());
    FAILED
        .captures_iter(text)
        .any(|caps| caps.get(1).is_none_or(|m| m.as_str() != "0"))
}

/// Token estimate for one rendered body, so callers can compare two candidate
/// renderings before handing the winner to [`never_worse`].
pub fn estimate_body_tokens(body: &str) -> usize {
    estimate_tokens(body)
}

/// Returns `filtered`, or `raw` when `filtered` would emit more tokens.
pub fn never_worse<'a>(raw: &'a str, filtered: &'a str) -> &'a str {
    if estimate_tokens(filtered) > estimate_tokens(raw) {
        raw
    } else {
        filtered
    }
}

/// Fallback rendering for an unparsed non-zero exit: `"<tool>: failed (exit N)"`
/// followed by a capped raw tail, so the agent still sees why.
pub fn failure_fallback(tool: &str, exit_code: i32, output: &str) -> String {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    if lines.is_empty() {
        return format!("{}: failed (exit {})", tool, exit_code);
    }

    let mut result = format!("{}: failed (exit {})", tool, exit_code);
    result.push('\n');

    const MAX_FAILURE_LINES: usize = crate::truncate::CAP_ERRORS;
    for (index, line) in lines.iter().take(MAX_FAILURE_LINES).enumerate() {
        result.push_str(&format!(
            "{}. {}\n",
            index + 1,
            crate::utils::truncate(line, 120)
        ));
    }

    if lines.len() > MAX_FAILURE_LINES {
        result.push_str(&format!(
            "\n… +{} more output lines\n",
            lines.len() - MAX_FAILURE_LINES
        ));
    }

    result.trim().to_string()
}

/// True when `text` already communicates failure.
///
/// This is a DENYLIST, deliberately. The exit guard fires unless a non-zero-exit
/// filter result reads as a failure, so a new filter or a reworded green summary
/// can never silently bypass it — the default is the failure fallback. Only
/// phrases that genuinely communicate failure belong here; green verdicts
/// ("N passed", "No issues found", "All files formatted correctly") must stay
/// marker-free so the guard catches them.
fn looks_failed(text: &str) -> bool {
    let text = text.to_lowercase();
    NONZERO_FAILED.is_match(&text)
        || NONZERO_ERRORS.is_match(&text)
        || NONZERO_ISSUES.is_match(&text)
        || NONZERO_WARNINGS.is_match(&text)
        || NONZERO_PROBLEMS.is_match(&text)
        || (FAILED_WORD.is_match(&text) && !FAIL_ZERO.is_match(&text))
        || (FAILURES_WORD.is_match(&text) && !FAILURES_ZERO.is_match(&text))
        || BARE_ERROR.is_match(&text)
        || EXCEPTION.is_match(&text)
        || NEED_FORMATTING.is_match(&text)
        || has_failed_word(&text)
}

/// Enforce the exit-code invariant: a filter must never render an all-green
/// summary when the child exited non-zero.
///
/// A compacted summary that says "No tests collected" next to exit 2, or
/// "All files formatted correctly" next to exit 1, is worse than no filtering at
/// all — it converts a hard failure into a confident pass.
pub fn guard_exit(raw: &str, exit_code: i32, tool: &str, filtered: &str) -> String {
    if exit_code == 0 || looks_failed(filtered) {
        return filtered.to_string();
    }
    failure_fallback(tool, exit_code, raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_filtered_when_smaller() {
        let raw = "a".repeat(400);
        assert_eq!(never_worse(&raw, "ok"), "ok");
    }

    #[test]
    fn falls_back_to_raw_when_filtered_bigger() {
        let raw = "{}";
        let filtered = "{\n  \"pretty\": true\n}";
        assert_eq!(never_worse(raw, filtered), raw);
    }

    #[test]
    fn tie_keeps_filtered() {
        assert_eq!(never_worse("abcd", "wxyz"), "wxyz");
    }

    #[test]
    fn token_boundary_follows_estimate_tokens() {
        assert_eq!(never_worse("abcd", "abcde"), "abcd");
        assert_eq!(never_worse("abcdefgh", "ijklmnop"), "ijklmnop");
    }

    #[test]
    fn empty_raw_returns_raw() {
        assert_eq!(never_worse("", "0 matches"), "");
    }

    #[test]
    fn empty_filtered_returns_filtered() {
        assert_eq!(never_worse("data", ""), "");
    }

    #[test]
    fn both_empty_returns_filtered() {
        assert_eq!(never_worse("", ""), "");
    }

    #[test]
    fn zero_exit_keeps_the_filtered_summary() {
        assert_eq!(
            guard_exit("raw", 0, "pytest", "12 passed"),
            "12 passed",
            "a passing run must not be rewritten"
        );
    }

    #[test]
    fn green_verdicts_are_replaced_on_a_non_zero_exit() {
        for green in [
            "No tests collected",
            "All files formatted correctly",
            "No issues found",
            "Go vet: No issues found",
            "12 passed",
            "0 failed",
            "Failures: 0",
            "FAIL (0)",
            "No errors found",
        ] {
            let guarded = guard_exit("raw tail line", 1, "tool", green);
            assert!(
                guarded.starts_with("tool: failed (exit 1)"),
                "{green:?} must not survive a non-zero exit, got: {guarded}"
            );
        }
    }

    #[test]
    fn results_that_already_read_as_failures_are_left_alone() {
        for failing in [
            "1 failed",
            "10 failed",
            "3 errors",
            "golangci-lint: 5 issues in 3 files",
            "error TS2322: bad type",
            "thread 'main' panicked at src/lib.rs:1",
            "2 files would be reformatted",
            "Failures: 2",
            "build failed",
        ] {
            assert_eq!(
                guard_exit("raw", 1, "tool", failing),
                failing,
                "{failing:?} already communicates failure"
            );
        }
    }

    #[test]
    fn failure_fallback_caps_the_raw_tail() {
        let raw = (1..=100)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let rendered = failure_fallback("tool", 137, &raw);
        assert!(rendered.starts_with("tool: failed (exit 137)"));
        assert!(rendered.contains("+80 more output lines"));
    }

    #[test]
    fn failure_fallback_without_output_still_names_the_exit() {
        assert_eq!(
            failure_fallback("tool", 2, "   \n \n"),
            "tool: failed (exit 2)"
        );
    }
}
