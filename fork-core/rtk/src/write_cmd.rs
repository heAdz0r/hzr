use crate::tracking;
use crate::write_core::{AtomicWriter, CasError, CasOptions, WriteOptions}; // changed: import CAS types for locked_write
use crate::write_lock::FileLockGuard; // changed: import flock guard for concurrent write safety

use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use memchr::memmem::Finder;
use regex::Regex;
use serde::Serialize;
use serde_json::ser::{PrettyFormatter, Serializer};
use serde_json::{Map, Value as JsonValue};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::io::Read as _; // changed: for expand_at_ref stdin support
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;
use toml::Value as TomlValue;
use toml_edit::{DocumentMut, Item, Table, Value as TomlEditValue};

/// Expand `@filename` references in argument values before processing. // changed: @filename expansion helper
/// - `@/path/to/file` or `@relative/path` -> reads file content (UTF-8)
/// - `@-` -> reads stdin to end
/// - Any other value -> passed through as-is (zero-copy)
pub fn expand_at_ref(val: &str) -> Result<std::borrow::Cow<'_, str>> {
    let Some(path) = val.strip_prefix('@') else {
        return Ok(std::borrow::Cow::Borrowed(val));
    };
    if path == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("@-: failed to read stdin")?;
        return Ok(std::borrow::Cow::Owned(buf));
    }
    let file_content =
        fs::read_to_string(path).with_context(|| format!("@file: failed to read {:?}", path))?;
    Ok(std::borrow::Cow::Owned(file_content))
}

/// Expand `@filename` references in BatchOp field values. // changed: @file support in batch JSON fields
/// Like expand_at_ref but disallows @- (stdin may already be consumed by --plan @-).
fn expand_batch_field(val: &str) -> Result<std::borrow::Cow<'_, str>> {
    let Some(path) = val.strip_prefix('@') else {
        return Ok(std::borrow::Cow::Borrowed(val));
    };
    if path == "-" {
        anyhow::bail!("@- is not supported in batch op fields; use @/path/to/file instead");
    }
    let file_content = fs::read_to_string(path)
        .with_context(|| format!("@file ref: failed to read {:?}", path))?;
    Ok(std::borrow::Cow::Owned(file_content))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ConfigFormat {
    Auto,
    Json,
    Toml,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ConfigValueType {
    Auto,
    String,
    Number,
    Bool,
    Null,
    Json,
}

/// Output mode for write commands — controls token cost and LLM-friendliness
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum OutputMode {
    /// Empty stdout on success, stderr on error
    Quiet,
    /// Short human-readable status (default)
    #[default]
    Concise,
    /// Machine-readable JSON (version 1 schema)
    Json,
}

/// Shared parameters for all write operations — fixes clippy::too_many_arguments
#[derive(Debug, Clone, Copy)]
pub struct WriteParams {
    pub dry_run: bool,
    pub fast: bool,
    pub verbose: u8,
    pub output: OutputMode,
    pub concurrency: ConcurrencyOpts, // changed: added for flock + CAS + retry
}

/// Concurrency options for write safety (flock + CAS + retry) // changed: new struct
#[derive(Debug, Clone, Copy, Default)]
pub struct ConcurrencyOpts {
    pub cas: bool,        // --cas: explicit CAS check
    pub max_retries: u32, // --retry N
}

/// Structured response — single renderer for all write operations
#[derive(Debug, Clone, Serialize)]
pub struct WriteResponse {
    pub version: u8,
    pub ok: bool,
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>, // changed: retry count when conflict resolved
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_resolved: Option<bool>, // changed: true when CAS conflict was resolved via retry
}

impl WriteResponse {
    fn success(op: &str, applied: usize) -> Self {
        Self {
            version: 1,
            ok: true,
            op: op.to_string(),
            applied: Some(applied),
            failed: None,
            dry_run: None,
            error: None,
            hint: None,
            detail: None,
            retries: None,           // changed: new field
            conflict_resolved: None, // changed: new field
        }
    }

    fn dry_run(op: &str, planned: usize) -> Self {
        Self {
            version: 1,
            ok: true,
            op: op.to_string(),
            applied: Some(planned),
            failed: None,
            dry_run: Some(true),
            error: None,
            hint: None,
            detail: None,
            retries: None,           // changed: new field
            conflict_resolved: None, // changed: new field
        }
    }

    fn noop(op: &str) -> Self {
        Self {
            version: 1,
            ok: true,
            op: op.to_string(),
            applied: Some(0),
            failed: None,
            dry_run: None,
            error: None,
            hint: Some("no-op".to_string()),
            detail: None,
            retries: None,           // changed: new field
            conflict_resolved: None, // changed: new field
        }
    }

    fn render(&self, mode: OutputMode, concise_msg: &str) {
        match mode {
            OutputMode::Quiet => {} // silent on success
            OutputMode::Concise => println!("{}", concise_msg),
            OutputMode::Json => {
                // unwrap safe: WriteResponse is always serializable
                println!("{}", serde_json::to_string(self).unwrap());
            }
        }
    }
}

/// Structured error with code — for predictable LLM parsing
fn write_error(code: &str, hint: &str, mode: OutputMode) -> anyhow::Error {
    if mode == OutputMode::Json {
        let resp = WriteResponse {
            version: 1,
            ok: false,
            op: String::new(),
            applied: None,
            failed: None,
            dry_run: None,
            error: Some(code.to_string()),
            hint: Some(hint.to_string()),
            detail: None,
            retries: None,           // changed: new field
            conflict_resolved: None, // changed: new field
        };
        // Print JSON error to stdout before returning the anyhow::Error
        println!("{}", serde_json::to_string(&resp).unwrap());
    }
    anyhow::anyhow!("ERR {} {}", code, hint)
}

/// Batch operation plan entry (deserialized from --plan JSON)
#[derive(Debug, Clone, serde::Deserialize)]
pub struct BatchOp {
    pub op: String,
    pub file: PathBuf,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub old: Option<String>,
    #[serde(default)]
    pub new: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub value_type: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub content: Option<String>, // changed: content for create op
}

/// Generate concise message from WriteResponse for rendering. // changed: extracted from run_* functions
fn format_response_msg(resp: &WriteResponse, op: &str) -> String {
    if resp.dry_run == Some(true) {
        return format!("dry-run: {} {} planned", op, resp.applied.unwrap_or(0));
    }
    if resp.hint.as_deref() == Some("no-op") {
        return format!("no-op: {} produces identical content", op);
    }
    let mut msg = format!("OK {} applied={}", op, resp.applied.unwrap_or(0));
    if let Some(retries) = resp.retries {
        msg.push_str(&format!(" retries={}", retries));
    }
    msg
}

/// Result of a compute function inside locked_write. // changed: new enum for retry logic
enum WriteAttempt {
    /// Computed new content to write (updated_content, applied_count)
    Success(String, usize),
    /// Retryable conflict (transient, can be retried with backoff)
    #[allow(dead_code)]
    RetryableConflict(String),
    /// Deterministic terminal error that must not be retried
    TerminalError { code: &'static str, hint: String },
    /// Content unchanged (no-op)
    Unchanged,
}

/// Core retry-with-lock function for concurrent write safety. // changed: new function
/// Acquires flock, reads file, runs compute_fn, writes with optional CAS verification.
/// Retries only retryable conflicts (CAS or explicit conflict) with exponential backoff.
fn locked_write<F>(
    file: &Path,
    params: WriteParams,
    op_name: &str,
    mut compute_fn: F,
) -> Result<(WriteResponse, String)>
where
    F: FnMut(&str) -> WriteAttempt,
{
    let max_retries = params.concurrency.max_retries;
    let use_cas = params.concurrency.cas || max_retries > 0; // auto-enable CAS when retries > 0
    let strict_hash = params.concurrency.cas; // hash only for explicit strict CAS mode

    for attempt in 0..=max_retries {
        // Always acquire flock (transparent, ~0.1ms overhead)
        let _guard = FileLockGuard::acquire(file)?;

        // changed: handle NotFound gracefully so create ops can write new files
        let content = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).with_context(|| format!("Failed to read {}", file.display())),
        };

        // Build CAS snapshot from in-memory content to avoid an extra file re-read.
        let snapshot = if use_cas {
            crate::write_core::snapshot_from_content(file, content.as_bytes(), strict_hash)?
        } else {
            None
        };

        match compute_fn(&content) {
            WriteAttempt::Success(updated, count) => {
                if updated == content {
                    return Ok((WriteResponse::noop(op_name), content));
                }

                if params.dry_run {
                    let resp = WriteResponse::dry_run(op_name, count);
                    return Ok((resp, content));
                }

                // Write with optional CAS verification
                let mut options = if params.fast {
                    WriteOptions::fast()
                } else {
                    WriteOptions::durable()
                };
                options.idempotent_skip = false;
                if let Some(ref snap) = snapshot {
                    options.cas = Some(CasOptions::from_snapshot(snap));
                }

                let writer = AtomicWriter::new(options);
                match writer.write_str(file, &updated) {
                    Ok(stats) => {
                        let mut resp = WriteResponse::success(op_name, count);
                        if attempt > 0 {
                            resp.retries = Some(attempt); // changed: report retry count
                            resp.conflict_resolved = Some(true); // changed: conflict was resolved
                        }
                        if params.verbose > 1 {
                            eprintln!(
                                "bytes_written={}, fsync={}, rename={}",
                                stats.bytes_written, stats.fsync_count, stats.rename_count
                            );
                        }
                        return Ok((resp, content));
                    }
                    Err(e) => {
                        // Check if this is a CAS conflict (retryable)
                        if e.downcast_ref::<CasError>().is_some() && attempt < max_retries {
                            drop(_guard); // release lock before sleeping
                            let backoff = Duration::from_millis(50 * 2u64.pow(attempt).min(8)); // cap at 400ms
                            thread::sleep(backoff);
                            continue;
                        }
                        return Err(e);
                    }
                }
            }
            WriteAttempt::RetryableConflict(msg) => {
                if attempt < max_retries {
                    drop(_guard); // release lock before sleeping
                    let backoff = Duration::from_millis(50 * 2u64.pow(attempt).min(8));
                    thread::sleep(backoff);
                    continue;
                }
                return Err(write_error("CONFLICT", &msg, params.output));
            }
            WriteAttempt::TerminalError { code, hint } => {
                return Err(write_error(code, &hint, params.output));
            }
            WriteAttempt::Unchanged => {
                return Ok((WriteResponse::noop(op_name), content));
            }
        }
    }

    // Exhausted retries (should not reach here, but safety net)
    Err(write_error(
        "RETRY_EXHAUSTED",
        &format!("failed after {} retries", max_retries),
        params.output,
    ))
}

/// Build tracking arguments for write operations.
/// Returns (neutral_baseline, rtk_output, normalized_cmd).
/// - neutral_baseline: compact output repeated because no comparable native response was observed
/// - rtk_output: compact message rtk actually printed
/// - normalized_cmd: always "rtk write" for gain grouping
fn write_tracking_args<'a>(
    op: &str,
    file_content: &str,
    rtk_msg: &'a str,
) -> (&'a str, &'a str, &'static str) {
    let _ = (op, file_content); // all write ops normalize to one savings-neutral command
    (rtk_msg, rtk_msg, "rtk write")
}

fn write_tracking_enabled() -> bool {
    static WRITE_TRACKING_ENABLED: OnceLock<bool> = OnceLock::new();
    *WRITE_TRACKING_ENABLED.get_or_init(|| match std::env::var("RTK_WRITE_TRACKING") {
        Ok(v) => {
            let normalized = v.trim().to_ascii_lowercase();
            !matches!(normalized.as_str(), "0" | "false" | "off" | "no")
        }
        Err(_) => true,
    })
}

/// Characters that are commonly shell-escaped by agents/wrappers and can break
/// exact text matching when passed through multiple layers.
fn is_shell_escaped_punct(ch: char) -> bool {
    matches!(
        ch,
        '!' | '$'
            | '&'
            | '('
            | ')'
            | '*'
            | ';'
            | '<'
            | '>'
            | '?'
            | '['
            | ']'
            | '{'
            | '}'
            | '|'
            | '#'
    )
}

/// Single-pass best-effort unescape for shell-escaped punctuation
/// (e.g. r"\!" -> "!"). Used only as a fallback when exact matching fails.
fn maybe_unescape_shell_escapes_once(input: &str) -> Option<String> {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut changed = false;

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(&next) = chars.peek() {
                if is_shell_escaped_punct(next) {
                    out.push(next);
                    chars.next();
                    changed = true;
                    continue;
                }
            }
        }
        out.push(ch);
    }

    changed.then_some(out)
}

/// Generate progressively unescaped variants (up to `max_steps`).
/// This handles cases where strings are escaped multiple times (e.g. r"\\\\!" -> r"\\!" -> "!").
fn shell_unescape_candidates(input: &str, max_steps: usize) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut current = input.to_string();

    for step in 1..=max_steps {
        let Some(next) = maybe_unescape_shell_escapes_once(current.as_str()) else {
            break;
        };
        if out.iter().any(|(_, seen)| seen == &next) {
            break;
        }
        out.push((step, next.clone()));
        current = next;
    }

    out
}

fn apply_shell_unescape_steps<'a>(input: &'a str, steps: usize) -> Cow<'a, str> {
    let mut current: Cow<'a, str> = Cow::Borrowed(input);
    for _ in 0..steps {
        let Some(next) = maybe_unescape_shell_escapes_once(current.as_ref()) else {
            break;
        };
        current = Cow::Owned(next);
    }
    current
}

fn has_shell_escape_like_pattern(pattern: &str) -> bool {
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek().copied().is_some_and(is_shell_escaped_punct) {
            return true;
        }
    }
    false
}

/// Bigram containment: fraction of `needle` character bigrams present in `hay`
/// (whitespace runs collapsed). Asymmetric on purpose: a pattern mangled by the
/// shell is a *subset* of the original line, so containment beats symmetric Dice.
fn bigram_containment(needle: &str, hay: &str) -> f64 {
    fn bigrams(s: &str) -> Vec<(char, char)> {
        let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
        let chars: Vec<char> = collapsed.chars().take(400).collect();
        chars.windows(2).map(|w| (w[0], w[1])).collect()
    }
    let n = bigrams(needle);
    if n.is_empty() {
        return 0.0;
    }
    let mut pool = bigrams(hay);
    let mut hits = 0usize;
    for g in &n {
        if let Some(pos) = pool.iter().position(|x| x == g) {
            pool.swap_remove(pos);
            hits += 1;
        }
    }
    hits as f64 / n.len() as f64
}

/// Find the file line most similar to the pattern's longest line.
/// Returns (1-based line number, raw line) when similarity is convincing.
fn closest_line_candidate<'a>(content: &'a str, pattern: &str) -> Option<(usize, &'a str)> {
    let anchor = pattern
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .max_by_key(|l| l.chars().count())?;
    if anchor.chars().count() < 8 {
        return None; // too short to discriminate
    }
    let mut best: Option<(usize, &'a str, f64)> = None;
    for (i, line) in content.lines().enumerate().take(20_000) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let score = bigram_containment(anchor, trimmed);
        if score > best.map_or(0.0, |(_, _, s)| s) {
            best = Some((i + 1, line, score));
        }
    }
    best.filter(|&(_, _, s)| s >= 0.5).map(|(n, l, _)| (n, l))
}

/// Markers that the shell evaluates inside double quotes (command/parameter substitution).
fn has_shell_substitution_markers(s: &str) -> bool {
    s.contains('`') || s.contains("${") || s.contains("$(")
}

fn smart_no_match_hint(base: &str, flag: &str, pattern: &str, content: &str) -> String {
    let mut hints: Vec<String> = Vec::new();

    if let Some((line_no, line)) = closest_line_candidate(content, pattern) {
        let display: String = line.trim().chars().take(160).collect();
        hints.push(format!(
            "closest candidate at line {}: {}",
            line_no, display
        ));
        if has_shell_substitution_markers(line) && !has_shell_substitution_markers(pattern) {
            hints.push(format!(
                "that line contains ` or ${{ }} which are missing from {flag} — \
the shell likely performed command/parameter substitution on the inline argument \
before rtk saw it; pass the block via @file: \
rtk write file /tmp/old.txt --content @- <<'EOF' ... EOF, then {flag} @/tmp/old.txt"
            ));
        }
    }

    if has_shell_escape_like_pattern(pattern) {
        hints.push(format!(
            r"pattern for {} contains shell-style escapes (for example, '\\\\!'); pass literal text or single-quote the argument",
            flag
        ));
    }

    if pattern.contains("\\n") && !pattern.contains('\n') {
        hints.push(format!(
            "pattern for {} contains literal '\\\\n'; for multi-line hunks pass real newline characters",
            flag
        ));
    }

    if pattern.contains('\n') {
        hints.push(format!(
            // changed: accurate hint — flex already tried; @file does not fix content mismatch
            "pattern for {} is multi-line; flex-whitespace fallback was already tried; \
verify exact bytes with `rtk read <file>` and copy the block verbatim — \
@file helps with shell-escaping but not content differences",
            flag
        ));
    }

    if hints.is_empty() {
        base.to_string()
    } else {
        format!("{}; {}", base, hints.join("; "))
    }
}

/// Try to match a pattern that contains embedded newlines (e.g. from a multiline bash arg)
/// by treating each line-boundary as flexible whitespace (`\s+`) in the actual file content.
/// Returns the exact original content span that matches, so it can be used as the replacement target.
fn try_multiline_flex_match(content: &str, pattern: &str) -> Option<String> {
    // changed: new flex-whitespace fallback for embedded-newline patterns
    if !pattern.contains('\n') {
        return None;
    }
    // Keep leading whitespace of the first part (preserves indentation for the match anchor),
    // but trim both ends of continuation parts (bash multiline args add arbitrary indentation).
    let parts: Vec<&str> = pattern
        .split('\n')
        .enumerate()
        .map(|(i, line)| if i == 0 { line.trim_end() } else { line.trim() })
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() < 2 {
        return None;
    }
    // Build a regex where parts are literal and line boundaries match any whitespace run
    let escaped: Vec<String> = parts.iter().map(|p| regex::escape(p)).collect();
    let re_str = escaped.join(r"\s+");
    let re = Regex::new(&re_str).ok()?;
    let m = re.find(content)?;
    Some(content[m.start()..m.end()].to_string())
}

/// Resolve exact or shell-unescaped match pair.
fn resolve_match_pair<'a>(
    content: &str,
    pattern: &'a str,
    replacement: &'a str,
) -> Option<(Cow<'a, str>, Cow<'a, str>)> {
    if content.contains(pattern) {
        return Some((Cow::Borrowed(pattern), Cow::Borrowed(replacement)));
    }

    for (steps, candidate) in shell_unescape_candidates(pattern, 4) {
        if content.contains(candidate.as_str()) {
            let replacement_candidate = apply_shell_unescape_steps(replacement, steps);
            return Some((Cow::Owned(candidate), replacement_candidate));
        }
    }

    // changed: flex-whitespace fallback for patterns with embedded newlines (multiline bash args)
    if let Some(actual_match) = try_multiline_flex_match(content, pattern) {
        return Some((Cow::Owned(actual_match), Cow::Borrowed(replacement)));
    }

    None
}

pub fn run_replace(
    file: &Path,
    from: &str,
    to: &str,
    all: bool,
    params: WriteParams,
) -> Result<()> {
    let timer = write_tracking_enabled().then(tracking::TimedExecution::start);
    if from.is_empty() {
        return Err(write_error(
            "EMPTY_PATTERN",
            "--from must be non-empty",
            params.output,
        ));
    }

    // changed: delegate to locked_write for flock + CAS + retry
    let from_owned = from.to_string();
    let to_owned = to.to_string();
    let (resp, content) = locked_write(file, params, "replace", |content| {
        let Some((from_match, to_match)) =
            resolve_match_pair(content, from_owned.as_str(), to_owned.as_str())
        else {
            return WriteAttempt::TerminalError {
                code: "NO_MATCH",
                hint: smart_no_match_hint(
                    "no matches for --from",
                    "--from",
                    from_owned.as_str(),
                    content,
                ),
            };
        };

        let (updated, count) = if all {
            replace_all_counted(content, from_match.as_ref(), to_match.as_ref())
        } else {
            (
                content.replacen(from_match.as_ref(), to_match.as_ref(), 1),
                1,
            )
        };
        if updated == content {
            return WriteAttempt::Unchanged;
        }
        WriteAttempt::Success(updated, count)
    })?;

    let msg = format_response_msg(&resp, "replace");
    resp.render(params.output, &msg);
    if let Some(timer) = timer.as_ref() {
        let (ti, to_str, tc) = write_tracking_args("replace", &content, &msg);
        timer.track(&format!("write replace {}", file.display()), tc, ti, to_str);
    }
    Ok(())
}

pub fn run_patch(file: &Path, old: &str, new: &str, all: bool, params: WriteParams) -> Result<()> {
    let timer = write_tracking_enabled().then(tracking::TimedExecution::start);
    if old.is_empty() {
        return Err(write_error(
            "EMPTY_PATTERN",
            "--old must be non-empty",
            params.output,
        ));
    }

    // changed: delegate to locked_write for flock + CAS + retry
    let old_owned = old.to_string();
    let new_owned = new.to_string();
    let (resp, content) = locked_write(file, params, "patch", |content| {
        let Some((old_match, new_match)) =
            resolve_match_pair(content, old_owned.as_str(), new_owned.as_str())
        else {
            return WriteAttempt::TerminalError {
                code: "NO_MATCH",
                hint: smart_no_match_hint("hunk not found", "--old", old_owned.as_str(), content),
            };
        };

        let (updated, count) = if all {
            replace_all_counted(content, old_match.as_ref(), new_match.as_ref())
        } else {
            (
                content.replacen(old_match.as_ref(), new_match.as_ref(), 1),
                1,
            )
        };
        if updated == content {
            return WriteAttempt::Unchanged;
        }
        WriteAttempt::Success(updated, count)
    })?;

    let msg = format_response_msg(&resp, "patch");
    resp.render(params.output, &msg);
    if let Some(timer) = timer.as_ref() {
        let (ti, to_str, tc) = write_tracking_args("patch", &content, &msg);
        timer.track(&format!("write patch {}", file.display()), tc, ti, to_str);
    }
    Ok(())
}

pub fn run_set(
    file: &Path,
    key: &str,
    value: &str,
    value_type: ConfigValueType,
    format: ConfigFormat,
    params: WriteParams,
) -> Result<()> {
    let timer = write_tracking_enabled().then(tracking::TimedExecution::start);
    let format = resolve_format(file, format)?;

    // changed: delegate to locked_write for flock + CAS + retry
    let key_owned = key.to_string();
    let value_owned = value.to_string();
    let (resp, content) = locked_write(file, params, "set", |content| {
        let result = match format {
            ConfigFormat::Json => {
                let mut root: JsonValue = match serde_json::from_str(content) {
                    Ok(v) => v,
                    Err(e) => {
                        return WriteAttempt::TerminalError {
                            code: "INVALID_JSON",
                            hint: format!("Invalid JSON: {}", e),
                        };
                    }
                };
                let parsed = match parse_json_value(&value_owned, value_type) {
                    Ok(v) => v,
                    Err(e) => {
                        return WriteAttempt::TerminalError {
                            code: "INVALID_VALUE",
                            hint: format!("Invalid value: {}", e),
                        };
                    }
                };
                if let Err(e) = set_json_path(&mut root, &key_owned, parsed) {
                    return WriteAttempt::TerminalError {
                        code: "INVALID_KEY_PATH",
                        hint: e.to_string(),
                    };
                }
                match serialize_json_preserving_style(&root, content) {
                    Ok(s) => s,
                    Err(e) => {
                        return WriteAttempt::TerminalError {
                            code: "SERIALIZE_JSON",
                            hint: e.to_string(),
                        };
                    }
                }
            }
            ConfigFormat::Toml => {
                let mut root: DocumentMut = match content.parse::<DocumentMut>() {
                    Ok(v) => v,
                    Err(e) => {
                        return WriteAttempt::TerminalError {
                            code: "INVALID_TOML",
                            hint: format!("Invalid TOML: {}", e),
                        };
                    }
                };
                let parsed = match parse_toml_value(&value_owned, value_type) {
                    Ok(v) => v,
                    Err(e) => {
                        return WriteAttempt::TerminalError {
                            code: "INVALID_VALUE",
                            hint: format!("Invalid value: {}", e),
                        };
                    }
                };
                if let Err(e) = set_toml_path(&mut root, &key_owned, parsed) {
                    return WriteAttempt::TerminalError {
                        code: "INVALID_KEY_PATH",
                        hint: e.to_string(),
                    };
                }
                root.to_string()
            }
            ConfigFormat::Auto => unreachable!(),
        };
        if result == content {
            return WriteAttempt::Unchanged;
        }
        WriteAttempt::Success(result, 1)
    })?;

    let msg = format_response_msg(&resp, "set");
    resp.render(params.output, &msg);
    if let Some(timer) = timer.as_ref() {
        let (ti, to_str, tc) = write_tracking_args("set", &content, &msg);
        timer.track(&format!("write set {}", file.display()), tc, ti, to_str);
    }
    Ok(())
}

/// Create a new file with the given content (atomic, idempotent). // changed: new create command
/// - File does not exist -> create it.
/// - File exists with same content -> noop.
/// - File exists with different content -> error (use replace/patch instead).
pub fn run_create(file: &Path, content: &str, force: bool, params: WriteParams) -> Result<()> {
    let content_val = expand_at_ref(content)?; // changed: support @filename expansion
    let content_str = content_val.as_ref();
    let timer = write_tracking_enabled().then(tracking::TimedExecution::start);

    let existing = match fs::read_to_string(file) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", file.display())),
    };

    // Idempotent: content already matches -> noop
    if existing.as_deref() == Some(content_str) {
        let resp = WriteResponse::noop("create");
        let msg = format_response_msg(&resp, "create");
        resp.render(params.output, &msg);
        return Ok(());
    }

    // File exists but content differs -> refuse (create != overwrite) unless --force
    if existing.is_some() && !force {
        return Err(write_error(
            "FILE_EXISTS",
            &format!(
                "{} already exists with different content; use replace/patch to modify it, or pass --force to overwrite",
                file.display()
            ),
            params.output,
        ));
    }

    if params.dry_run {
        let resp = WriteResponse::dry_run("create", 1);
        let msg = format_response_msg(&resp, "create");
        resp.render(params.output, &msg);
        return Ok(());
    }

    // Create parent dirs if needed
    if let Some(parent) = file.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory for {}", file.display())
            })?;
        }
    }

    let options = if params.fast {
        WriteOptions::fast()
    } else {
        WriteOptions::durable()
    };
    let writer = AtomicWriter::new(options);
    writer
        .write_str(file, content_str)
        .with_context(|| format!("Failed to create {}", file.display()))?;

    let resp = WriteResponse::success("create", 1);
    let msg = format_response_msg(&resp, "create");
    resp.render(params.output, &msg);
    if let Some(timer) = timer.as_ref() {
        let (ti, to_str, tc) = write_tracking_args("create", content_str, &msg);
        timer.track(&format!("write create {}", file.display()), tc, ti, to_str);
    }
    Ok(())
}

/// Execute a batch of write operations from a JSON plan.
/// Single process startup, grouped fsync, one summary output.
pub fn run_batch(
    plan_json: &str,
    params: WriteParams, // changed: bundled dry_run/fast/verbose/output into WriteParams
) -> Result<()> {
    let timer = write_tracking_enabled().then(tracking::TimedExecution::start);

    let ops: Vec<BatchOp> =
        serde_json::from_str(plan_json).context("Failed to parse batch plan JSON")?;

    if ops.is_empty() {
        bail!("Batch plan is empty");
    }

    let total = ops.len();
    let grouped_indices = group_batch_indices_by_file(&ops);
    let mut outcomes: Vec<Option<Result<usize, String>>> = vec![None; total];

    for indices in grouped_indices {
        for (idx, result) in execute_batch_file_group(&ops, &indices, params) {
            outcomes[idx] = Some(result);
        }
    }

    let mut applied = 0usize;
    let mut failed = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for (i, op) in ops.iter().enumerate() {
        let result = outcomes[i]
            .take()
            .unwrap_or_else(|| Err("batch internal error: missing operation outcome".to_string()));
        match result {
            Ok(count) => {
                applied += count;
                if params.verbose > 0 && params.output == OutputMode::Concise {
                    println!("[{}/{}] OK {} {}", i + 1, total, op.op, op.file.display());
                    // changed: stdout to avoid stderr/stdout split duplication
                }
            }
            Err(e) => {
                failed += 1;
                let msg = format!(
                    "[{}/{}] {} {}: {}",
                    i + 1,
                    total,
                    op.op,
                    op.file.display(),
                    e
                );
                errors.push(msg.clone());
                if params.output == OutputMode::Concise {
                    println!("{}", msg); // changed: stdout to avoid stderr/stdout split duplication
                }
            }
        }
    }

    // Render summary
    let mut resp = WriteResponse::success("batch", applied);
    resp.failed = Some(failed);
    if params.dry_run {
        resp.dry_run = Some(true);
    }
    if !errors.is_empty() && params.output == OutputMode::Json {
        resp.detail = Some(errors.join("; "));
    }
    if failed > 0 {
        resp.ok = failed < total; // partial failure is still ok=true if any succeeded
    }

    let concise_msg = if params.dry_run {
        format!("dry-run: batch {}/{} planned", applied, total)
    } else {
        format!(
            "OK batch applied={} failed={} total={}",
            applied, failed, total
        )
    };
    resp.render(params.output, &concise_msg);

    if let Some(timer) = timer.as_ref() {
        let (_, _, tc) = write_tracking_args("batch", plan_json, &concise_msg); // changed: normalize cmd
        timer.track(
            &format!("write batch ({})", total),
            tc,
            plan_json,
            &concise_msg,
        );
    }

    if failed == total {
        // changed: print to stdout (not bail! which adds "Error:" to stderr, causing duplicate display in Bash tool)
        println!("Error: All {} batch operations failed", total);
        std::process::exit(1);
    }
    Ok(())
}

fn group_batch_indices_by_file(ops: &[BatchOp]) -> Vec<Vec<usize>> {
    let mut by_file: HashMap<PathBuf, usize> = HashMap::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (idx, op) in ops.iter().enumerate() {
        if let Some(group_idx) = by_file.get(&op.file) {
            groups[*group_idx].push(idx);
            continue;
        }
        let next_idx = groups.len();
        by_file.insert(op.file.clone(), next_idx);
        groups.push(vec![idx]);
    }
    groups
}

fn execute_batch_file_group(
    ops: &[BatchOp],
    indices: &[usize],
    params: WriteParams,
) -> Vec<(usize, Result<usize, String>)> {
    if indices.is_empty() {
        return Vec::new();
    }

    let file = &ops[indices[0]].file;
    let mut op_results: Vec<(usize, Result<usize, String>)> = Vec::new();
    let mut write_params = params;
    write_params.output = OutputMode::Quiet;

    let write_result = locked_write(file, write_params, "batch", |content| {
        let mut working = content.to_string();
        let mut total_applied = 0usize;
        let mut computed_results: Vec<(usize, Result<usize, String>)> =
            Vec::with_capacity(indices.len());

        for idx in indices {
            let op = &ops[*idx];
            match apply_batch_op_in_memory(op, &working) {
                Ok(BatchApplyResult::Applied { updated, count }) => {
                    working = updated;
                    total_applied += count;
                    computed_results.push((*idx, Ok(count)));
                }
                Ok(BatchApplyResult::Noop) => {
                    computed_results.push((*idx, Ok(0)));
                }
                Err(err) => {
                    computed_results.push((*idx, Err(err.to_string())));
                }
            }
        }

        op_results = computed_results;
        if total_applied == 0 {
            WriteAttempt::Unchanged
        } else {
            WriteAttempt::Success(working, total_applied)
        }
    });

    match write_result {
        Ok(_) => op_results,
        Err(err) => {
            let msg = err.to_string();
            indices
                .iter()
                .map(|idx| (*idx, Err(msg.clone())))
                .collect::<Vec<_>>()
        }
    }
}

enum BatchApplyResult {
    Applied { updated: String, count: usize },
    Noop,
}

fn apply_batch_op_in_memory(op: &BatchOp, content: &str) -> Result<BatchApplyResult> {
    match op.op.as_str() {
        "replace" => {
            let from_raw = op
                .from
                .as_deref()
                .with_context(|| "batch replace: missing 'from' field")?;
            let to_raw = op
                .to
                .as_deref()
                .with_context(|| "batch replace: missing 'to' field")?;
            let from_cow = expand_batch_field(from_raw)?; // changed: expand @file refs in batch fields
            let to_cow = expand_batch_field(to_raw)?; // changed: expand @file refs in batch fields
            let from = from_cow.as_ref();
            let to = to_cow.as_ref();
            if from.is_empty() {
                bail!("batch replace: 'from' must be non-empty");
            }
            let Some((from_match, to_match)) = resolve_match_pair(content, from, to) else {
                bail!(
                    "NO_MATCH {}",
                    smart_no_match_hint("no matches for --from", "--from", from, content)
                );
            };

            let (updated, count) = if op.all {
                replace_all_counted(content, from_match.as_ref(), to_match.as_ref())
            } else {
                (
                    content.replacen(from_match.as_ref(), to_match.as_ref(), 1),
                    1,
                )
            };

            if updated == content {
                return Ok(BatchApplyResult::Noop);
            }
            Ok(BatchApplyResult::Applied { updated, count })
        }
        "patch" => {
            let old_raw = op
                .old
                .as_deref()
                .with_context(|| "batch patch: missing 'old' field")?;
            let new_raw = op
                .new
                .as_deref()
                .with_context(|| "batch patch: missing 'new' field")?;
            let old_cow = expand_batch_field(old_raw)?; // changed: expand @file refs in batch fields
            let new_cow = expand_batch_field(new_raw)?; // changed: expand @file refs in batch fields
            let old = old_cow.as_ref();
            let new = new_cow.as_ref();
            if old.is_empty() {
                // changed: allow empty 'old' when file is also empty (new file creation via patch)
                if content.is_empty() {
                    return Ok(BatchApplyResult::Applied {
                        updated: new.to_string(),
                        count: 1,
                    });
                }
                bail!("batch patch: 'old' must be non-empty (file has content; use 'create' op for new files, 'replace' to overwrite existing content)");
            }
            let Some((old_match, new_match)) = resolve_match_pair(content, old, new) else {
                bail!(
                    "NO_MATCH {}",
                    smart_no_match_hint("hunk not found", "--old", old, content)
                );
            };

            let (updated, count) = if op.all {
                replace_all_counted(content, old_match.as_ref(), new_match.as_ref())
            } else {
                (
                    content.replacen(old_match.as_ref(), new_match.as_ref(), 1),
                    1,
                )
            };

            if updated == content {
                return Ok(BatchApplyResult::Noop);
            }
            Ok(BatchApplyResult::Applied { updated, count })
        }
        "set" => {
            let key = op
                .key
                .as_deref()
                .with_context(|| "batch set: missing 'key' field")?;
            let value = op
                .value
                .as_deref()
                .with_context(|| "batch set: missing 'value' field")?;

            let vt = match op.value_type.as_deref() {
                Some("string") => ConfigValueType::String,
                Some("number") => ConfigValueType::Number,
                Some("bool") => ConfigValueType::Bool,
                Some("null") => ConfigValueType::Null,
                Some("json") => ConfigValueType::Json,
                _ => ConfigValueType::Auto,
            };
            let fmt = match op.format.as_deref() {
                Some("json") => ConfigFormat::Json,
                Some("toml") => ConfigFormat::Toml,
                _ => ConfigFormat::Auto,
            };
            let fmt = resolve_format(&op.file, fmt)?;

            let updated = match fmt {
                ConfigFormat::Json => {
                    let mut root: JsonValue = serde_json::from_str(content)
                        .with_context(|| format!("Invalid JSON: {}", op.file.display()))?;
                    set_json_path(&mut root, key, parse_json_value(value, vt)?)?;
                    serialize_json_preserving_style(&root, content)?
                }
                ConfigFormat::Toml => {
                    let mut root: DocumentMut = content
                        .parse::<DocumentMut>()
                        .with_context(|| format!("Invalid TOML: {}", op.file.display()))?;
                    set_toml_path(&mut root, key, parse_toml_value(value, vt)?)?;
                    root.to_string()
                }
                ConfigFormat::Auto => unreachable!(),
            };

            if updated == content {
                return Ok(BatchApplyResult::Noop);
            }
            Ok(BatchApplyResult::Applied { updated, count: 1 })
        }
        // changed: new create op — writes content to a new file (idempotent, fails if file exists with different content)
        "create" => {
            // Accept 'content' or 'new' field (new is more intuitive when converting from patch ops)
            let content_str = op.content.as_deref().or(op.new.as_deref()).unwrap_or(""); // changed: 'new' as alias
                                                                                         // Idempotent: file already has exactly the desired content
            if content == content_str {
                return Ok(BatchApplyResult::Noop);
            }
            // File already exists with different content (non-empty) → error
            if !content.is_empty() {
                bail!(
                    "FILE_EXISTS: {} already exists with different content; use replace/patch to modify it",
                    op.file.display()
                );
            }
            Ok(BatchApplyResult::Applied {
                updated: content_str.to_string(),
                count: 1,
            })
        }
        other => bail!("Unknown batch operation: '{}'", other),
    }
}

/// P0-2: single-pass replace that returns (result, count) without pre-scanning
fn replace_all_counted(content: &str, from: &str, to: &str) -> (String, usize) {
    if from.is_empty() {
        return (content.to_string(), 0);
    }

    let finder = Finder::new(from.as_bytes());
    let mut starts = finder.find_iter(content.as_bytes());
    let Some(first_start) = starts.next() else {
        return (content.to_string(), 0);
    };

    let mut result = String::with_capacity(content.len());
    let mut count = 0usize;
    let mut last_end = 0usize;
    for start in std::iter::once(first_start).chain(starts) {
        result.push_str(&content[last_end..start]);
        result.push_str(to);
        count += 1;
        last_end = start + from.len();
    }
    result.push_str(&content[last_end..]);
    (result, count)
}

fn resolve_format(file: &Path, format: ConfigFormat) -> Result<ConfigFormat> {
    if format != ConfigFormat::Auto {
        return Ok(format);
    }
    match file.extension().and_then(|s| s.to_str()) {
        Some("json") => Ok(ConfigFormat::Json),
        Some("toml") => Ok(ConfigFormat::Toml),
        _ => bail!(
            "Cannot infer config format for {}. Use --format json|toml",
            file.display()
        ),
    }
}

fn parse_json_value(raw: &str, value_type: ConfigValueType) -> Result<JsonValue> {
    match value_type {
        ConfigValueType::Auto | ConfigValueType::Json => {
            if let Ok(v) = serde_json::from_str::<JsonValue>(raw) {
                Ok(v)
            } else {
                Ok(JsonValue::String(raw.to_string()))
            }
        }
        ConfigValueType::String => Ok(JsonValue::String(raw.to_string())),
        ConfigValueType::Number => {
            if let Ok(i) = raw.parse::<i64>() {
                Ok(JsonValue::from(i))
            } else if let Ok(u) = raw.parse::<u64>() {
                Ok(JsonValue::from(u))
            } else {
                let n = raw
                    .parse::<f64>()
                    .with_context(|| format!("Invalid number value: {}", raw))?;
                Ok(JsonValue::from(n))
            }
        }
        ConfigValueType::Bool => {
            let b = raw
                .parse::<bool>()
                .with_context(|| format!("Invalid bool value: {}", raw))?;
            Ok(JsonValue::Bool(b))
        }
        ConfigValueType::Null => Ok(JsonValue::Null),
    }
}

fn parse_toml_value(raw: &str, value_type: ConfigValueType) -> Result<TomlEditValue> {
    let value = match value_type {
        ConfigValueType::Auto | ConfigValueType::Json => {
            if let Ok(v) = raw.parse::<TomlValue>() {
                v
            } else {
                TomlValue::String(raw.to_string())
            }
        }
        ConfigValueType::String => TomlValue::String(raw.to_string()),
        ConfigValueType::Number => {
            if let Ok(i) = raw.parse::<i64>() {
                TomlValue::Integer(i)
            } else {
                let f = raw
                    .parse::<f64>()
                    .with_context(|| format!("Invalid number value: {}", raw))?;
                TomlValue::Float(f)
            }
        }
        ConfigValueType::Bool => {
            let b = raw
                .parse::<bool>()
                .with_context(|| format!("Invalid bool value: {}", raw))?;
            TomlValue::Boolean(b)
        }
        ConfigValueType::Null => bail!("TOML does not support null values"),
    };

    toml_to_toml_edit_value(value)
}

fn toml_to_toml_edit_value(value: TomlValue) -> Result<TomlEditValue> {
    match value {
        TomlValue::String(s) => Ok(TomlEditValue::from(s)),
        TomlValue::Integer(i) => Ok(TomlEditValue::from(i)),
        TomlValue::Float(f) => Ok(TomlEditValue::from(f)),
        TomlValue::Boolean(b) => Ok(TomlEditValue::from(b)),
        TomlValue::Datetime(dt) => Ok(TomlEditValue::from(dt)),
        TomlValue::Array(arr) => {
            let mut out = toml_edit::Array::new();
            for v in arr {
                out.push_formatted(toml_to_toml_edit_value(v)?);
            }
            Ok(TomlEditValue::Array(out))
        }
        TomlValue::Table(_) => bail!("Setting table values is not supported for --value"),
    }
}

fn set_json_path(root: &mut JsonValue, key: &str, value: JsonValue) -> Result<()> {
    let parts: Vec<&str> = key.split('.').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        // P2-1: reject empty key — overwriting root is dangerous and non-obvious
        bail!("--key must be non-empty");
    }

    if !root.is_object() {
        bail!(
            "Cannot set nested key '{}': top-level JSON value is not an object",
            key
        );
    }

    let mut current = root.as_object_mut().expect("object checked above");
    for part in &parts[..parts.len() - 1] {
        let entry = current
            .entry((*part).to_string())
            .or_insert_with(|| JsonValue::Object(Map::new()));
        if !entry.is_object() {
            bail!(
                "Cannot set key '{}': path segment '{}' is not an object",
                key,
                part
            );
        }
        current = entry.as_object_mut().expect("object ensured");
    }

    current.insert(parts[parts.len() - 1].to_string(), value);
    Ok(())
}

fn set_toml_path(root: &mut DocumentMut, key: &str, value: TomlEditValue) -> Result<()> {
    let parts: Vec<&str> = key.split('.').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        bail!("--key must be non-empty");
    }

    let mut current = root.as_table_mut();
    for part in &parts[..parts.len() - 1] {
        if !current.contains_key(part) {
            current.insert(part, Item::Table(Table::new()));
        }
        let entry = current
            .get_mut(part)
            .expect("table entry must exist after insert");
        if !entry.is_table() {
            bail!(
                "Cannot set key '{}': path segment '{}' is not a table",
                key,
                part
            );
        }
        current = entry
            .as_table_mut()
            .expect("path segment is guaranteed to be a table");
    }

    current[parts[parts.len() - 1]] = Item::Value(value);
    Ok(())
}

fn serialize_json_preserving_style(root: &JsonValue, original: &str) -> Result<String> {
    let mut serialized = if !original.contains('\n') {
        serde_json::to_string(root).context("Failed to serialize JSON")?
    } else {
        let indent = detect_json_indent(original).unwrap_or_else(|| b"  ".to_vec());
        serialize_json_with_indent(root, &indent)?
    };

    if original.ends_with('\n') && !serialized.ends_with('\n') {
        serialized.push('\n');
    }
    if !original.ends_with('\n') {
        serialized = serialized.trim_end_matches('\n').to_string();
    }

    Ok(serialized)
}

fn serialize_json_with_indent<T: Serialize>(value: &T, indent: &[u8]) -> Result<String> {
    let mut out = Vec::new();
    let formatter = PrettyFormatter::with_indent(indent);
    let mut serializer = Serializer::with_formatter(&mut out, formatter);
    value
        .serialize(&mut serializer)
        .context("Failed to serialize JSON")?;
    String::from_utf8(out).context("Failed to encode JSON as UTF-8")
}

fn detect_json_indent(original: &str) -> Option<Vec<u8>> {
    for line in original.lines().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let trimmed = line.trim_start_matches([' ', '\t']);
        if trimmed.len() == line.len() {
            continue;
        }
        let indent = &line.as_bytes()[..line.len() - trimmed.len()];
        if indent.iter().all(|b| *b == b' ') || indent.iter().all(|b| *b == b'\t') {
            return Some(indent.to_vec());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Test-only helper: default WriteParams for concise, fast, quiet tests
    fn tp(output: OutputMode) -> WriteParams {
        WriteParams {
            dry_run: false,
            fast: true,
            verbose: 0,
            output,
            concurrency: ConcurrencyOpts::default(),
        } // changed: added concurrency
    }

    fn tp_dry(output: OutputMode) -> WriteParams {
        WriteParams {
            dry_run: true,
            fast: true,
            verbose: 0,
            output,
            concurrency: ConcurrencyOpts::default(),
        } // changed: added concurrency
    }

    /// Test-only helper: WriteParams with retry enabled // changed: new helper for retry tests
    fn tp_retry(output: OutputMode, max_retries: u32) -> WriteParams {
        WriteParams {
            dry_run: false,
            fast: true,
            verbose: 0,
            output,
            concurrency: ConcurrencyOpts {
                cas: false,
                max_retries,
            },
        }
    }

    #[test]
    fn replace_first_only() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.txt");
        fs::write(&file, "a a a").unwrap();

        run_replace(&file, "a", "b", false, tp(OutputMode::Concise)).unwrap(); // changed: use WriteParams
        assert_eq!(fs::read_to_string(&file).unwrap(), "b a a");
    }

    #[test]
    fn patch_all_occurrences() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("b.txt");
        fs::write(&file, "old\nold\n").unwrap();

        run_patch(&file, "old", "new", true, tp(OutputMode::Concise)).unwrap(); // changed: use WriteParams
        assert_eq!(fs::read_to_string(&file).unwrap(), "new\nnew\n");
    }

    #[test]
    fn replace_unescapes_shell_escaped_bang_as_fallback() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("escaped_bang_replace.txt");
        fs::write(&file, "if a != b { return; }\n").unwrap();

        run_replace(&file, r"\\!=", "==", false, tp(OutputMode::Quiet)).unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "if a == b { return; }\n"
        );
    }

    #[test]
    fn patch_unescapes_shell_escaped_bang_as_fallback() {
        // Pattern uses shell-escaped bang (\\! → !) with real newlines in file.
        // run_patch must unescape \\!= → \!= → != over two fallback steps.
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("escaped_bang_patch.txt");
        fs::write(&file, "if left != right { return; }\n").unwrap();

        run_patch(
            &file,
            r"if left \\!= right { return; }", // raw: literal \\!= needing 2 unescape steps
            "if left == right { return; }",
            false,
            tp(OutputMode::Quiet),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "if left == right { return; }\n"
        );
    }

    #[test]
    fn replace_prefers_exact_match_over_unescape_fallback() {
        // File literally contains \\!= — exact match wins over unescape fallback.
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("escaped_bang_exact_precedence.txt");
        fs::write(&file, "\\\\!= and !=").unwrap(); // file: \\!= and !=

        run_replace(&file, r"\\!=", "MATCH", false, tp(OutputMode::Quiet)).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "MATCH and !=");
    }

    #[test]
    fn replace_no_match_hint_mentions_shell_escaped_pattern() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("escaped_bang_nomatch.txt");
        fs::write(&file, "alpha beta\n").unwrap();

        let err = run_replace(&file, r"\\!=", "==", false, tp(OutputMode::Quiet)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("NO_MATCH"));
        assert!(msg.contains("shell-style escapes"));
    }

    #[test]
    fn no_shell_escape_hint_for_common_code_backslashes() {
        let hint = smart_no_match_hint("hunk not found", "--old", "replace('\\\\', \"/\")", "");
        assert!(
            !hint.contains("shell-style escapes"),
            "code backslashes should not trigger shell-escape hint"
        );
    }

    #[test]
    fn replace_unescapes_double_escaped_shell_punctuation() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("double_escaped_bang_replace.txt");
        fs::write(&file, "if a != b { return; }\n").unwrap();

        run_replace(&file, r"\\\\!=", "==", false, tp(OutputMode::Quiet)).unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "if a == b { return; }\n"
        );
    }

    #[test]
    fn patch_no_match_hint_mentions_literal_newline_escape() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("literal_newline_nomatch.txt");
        fs::write(&file, "line1\nline2\n").unwrap();

        let err = run_patch(
            &file,
            "line1\\nlineX",
            "line1\nlineX\n",
            false,
            tp(OutputMode::Quiet),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("NO_MATCH"));
        assert!(msg.contains("literal '\\\\n'"));
    }

    #[test]
    fn patch_multiline_bash_arg_flex_match() {
        // changed: test that embedded newlines from multiline bash args are handled via flex match
        // Simulates: --old 'BRE -> PCRE: \| -> |\n      strip escapes like \!'
        // while the file has it all on one line.
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("flex_match.rs");
        fs::write(
            &file,
            "    // Translate BRE -> PCRE: \\| -> | (alternation), strip shell-injected escapes like \\!\n",
        )
        .unwrap();

        // Pattern as if passed from a multiline bash single-quoted arg
        let old = "    // Translate BRE -> PCRE: \\| -> | (alternation), strip\n      shell-injected escapes like \\!";
        run_patch(&file, old, "    // REPLACED", false, tp(OutputMode::Quiet)).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "    // REPLACED\n");
    }

    #[test]
    fn patch_multiline_bash_arg_hints_multiline() {
        // changed: NO_MATCH hint for multi-line patterns — updated assertion to match new hint text
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("flex_hint.rs");
        fs::write(&file, "completely different content\n").unwrap();

        let err = run_patch(
            &file,
            "first line\n    second line",
            "replacement",
            false,
            tp(OutputMode::Quiet),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("NO_MATCH"));
        assert!(
            msg.contains("multi-line"),
            "hint should mention multi-line pattern: {msg}"
        );
        assert!(
            msg.contains("rtk read"),
            "hint should suggest rtk read to inspect file: {msg}"
        );
        assert!(
            !msg.contains("@file syntax"),
            "hint must NOT repeat @file suggestion (already tried): {msg}"
        );
    }

    #[test]
    fn replace_multiline_bash_arg_flex_match() {
        // changed: replace also uses flex match for embedded-newline --from args
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("flex_replace.rs");
        fs::write(&file, "fn foo() -> bool { true }\n").unwrap();

        let old = "fn foo() -> bool\n    { true }";
        run_replace(
            &file,
            old,
            "fn foo() -> bool { false }",
            false,
            tp(OutputMode::Quiet),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "fn foo() -> bool { false }\n"
        );
    }

    #[test]
    fn set_json_nested_key() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("c.json");
        fs::write(&file, "{\"a\":{}}").unwrap();

        run_set(
            &file,
            "a.b",
            "42",
            ConfigValueType::Number,
            ConfigFormat::Json,
            tp(OutputMode::Concise),
        )
        .unwrap(); // changed: use WriteParams

        let v: JsonValue = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["a"]["b"], JsonValue::from(42));
    }

    #[test]
    fn set_toml_nested_key() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("d.toml");
        fs::write(&file, "[a]\n").unwrap();

        run_set(
            &file,
            "a.b",
            "true",
            ConfigValueType::Bool,
            ConfigFormat::Toml,
            tp(OutputMode::Concise),
        )
        .unwrap(); // changed: use WriteParams

        let v: TomlValue = fs::read_to_string(&file).unwrap().parse().unwrap();
        assert_eq!(v["a"]["b"], TomlValue::Boolean(true));
    }

    #[test]
    fn dry_run_does_not_modify_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("e.txt");
        fs::write(&file, "hello world").unwrap();

        run_replace(&file, "world", "rtk", false, tp_dry(OutputMode::Concise)).unwrap(); // changed: use WriteParams
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello world");
    }

    #[test]
    fn set_json_conflict_path_fails() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("f.json");
        fs::write(&file, "{\"a\":1}").unwrap();

        let err = run_set(
            &file,
            "a.b",
            "2",
            ConfigValueType::Number,
            ConfigFormat::Json,
            tp(OutputMode::Concise),
        )
        .unwrap_err(); // changed: use WriteParams
        assert!(err.to_string().contains("not an object"));
    }

    #[test]
    fn set_toml_preserves_comments() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("g.toml");
        fs::write(&file, "# top\n[a]\n# keep\nb = 1\n").unwrap();

        run_set(
            &file,
            "a.c",
            "true",
            ConfigValueType::Bool,
            ConfigFormat::Toml,
            tp(OutputMode::Concise),
        )
        .unwrap(); // changed: use WriteParams

        let out = fs::read_to_string(&file).unwrap();
        assert!(out.contains("# top"));
        assert!(out.contains("# keep"));
        let v: TomlValue = out.parse().unwrap();
        assert_eq!(v["a"]["c"], TomlValue::Boolean(true));
    }

    #[test]
    fn set_json_preserves_compact_style() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("h.json");
        fs::write(&file, "{\"a\":1}").unwrap();

        run_set(
            &file,
            "b",
            "2",
            ConfigValueType::Number,
            ConfigFormat::Json,
            tp(OutputMode::Concise),
        )
        .unwrap(); // changed: use WriteParams

        assert_eq!(fs::read_to_string(&file).unwrap(), "{\"a\":1,\"b\":2}");
    }

    #[test]
    fn replace_all_counted_single_pass() {
        let (result, count) = replace_all_counted("aXbXcX", "X", "Y");
        assert_eq!(result, "aYbYcY");
        assert_eq!(count, 3);
    }

    #[test]
    fn replace_all_counted_no_match() {
        let (result, count) = replace_all_counted("hello", "X", "Y");
        assert_eq!(result, "hello");
        assert_eq!(count, 0);
    }

    #[test]
    fn replace_all_counted_unicode_safe() {
        let (result, count) = replace_all_counted("привет мир привет", "привет", "hello");
        assert_eq!(result, "hello мир hello");
        assert_eq!(count, 2);
    }

    #[test]
    fn quiet_mode_produces_no_stdout() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("quiet.txt");
        fs::write(&file, "hello world").unwrap();

        // Quiet mode should succeed without panicking
        run_replace(&file, "world", "rtk", false, tp(OutputMode::Quiet)).unwrap(); // changed: use WriteParams
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello rtk");
    }

    #[test]
    fn json_mode_same_file_result_as_concise() {
        // Contract: file content must be identical regardless of output mode
        let tmp = TempDir::new().unwrap();
        let f1 = tmp.path().join("concise.txt");
        let f2 = tmp.path().join("json.txt");
        fs::write(&f1, "a b c").unwrap();
        fs::write(&f2, "a b c").unwrap();

        run_replace(&f1, "b", "X", false, tp(OutputMode::Concise)).unwrap(); // changed: use WriteParams
        run_replace(&f2, "b", "X", false, tp(OutputMode::Json)).unwrap(); // changed: use WriteParams

        assert_eq!(
            fs::read_to_string(&f1).unwrap(),
            fs::read_to_string(&f2).unwrap(),
        );
    }

    #[test]
    fn write_response_json_schema() {
        let resp = WriteResponse::success("replace", 3);
        let json_str = serde_json::to_string(&resp).unwrap();
        let v: JsonValue = serde_json::from_str(&json_str).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["ok"], true);
        assert_eq!(v["op"], "replace");
        assert_eq!(v["applied"], 3);
        // Optional fields should not be present
        assert!(v.get("error").is_none());
        assert!(v.get("dry_run").is_none());
    }

    #[test]
    fn write_response_noop_json() {
        let resp = WriteResponse::noop("patch");
        let json_str = serde_json::to_string(&resp).unwrap();
        let v: JsonValue = serde_json::from_str(&json_str).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["applied"], 0);
        assert_eq!(v["hint"], "no-op");
    }

    #[test]
    fn batch_replace_and_patch() {
        let tmp = TempDir::new().unwrap();
        let f1 = tmp.path().join("batch1.txt");
        let f2 = tmp.path().join("batch2.txt");
        fs::write(&f1, "hello world").unwrap();
        fs::write(&f2, "foo bar baz").unwrap();

        let plan = serde_json::json!([
            {"op":"replace","file": f1.to_str().unwrap(), "from":"world","to":"batch"},
            {"op":"patch","file": f2.to_str().unwrap(), "old":"bar","new":"PATCHED"}
        ]);

        run_batch(&plan.to_string(), tp(OutputMode::Quiet)).unwrap(); // changed: use WriteParams
        assert_eq!(fs::read_to_string(&f1).unwrap(), "hello batch");
        assert_eq!(fs::read_to_string(&f2).unwrap(), "foo PATCHED baz");
    }

    #[test]
    fn batch_replace_unescapes_shell_escaped_bang_as_fallback() {
        let tmp = TempDir::new().unwrap();
        let f1 = tmp.path().join("batch_escaped_bang.txt");
        fs::write(&f1, "x != y\n").unwrap();

        let plan = serde_json::json!([
            {"op":"replace","file": f1.to_str().unwrap(), "from":r"\\!=", "to":"=="}
        ]);

        run_batch(&plan.to_string(), tp(OutputMode::Quiet)).unwrap();
        assert_eq!(fs::read_to_string(&f1).unwrap(), "x == y\n");
    }

    #[test]
    fn batch_dry_run_no_writes() {
        let tmp = TempDir::new().unwrap();
        let f1 = tmp.path().join("batchdry.txt");
        fs::write(&f1, "original").unwrap();

        let plan = serde_json::json!([
            {"op":"replace","file": f1.to_str().unwrap(), "from":"original","to":"changed"}
        ]);

        run_batch(&plan.to_string(), tp_dry(OutputMode::Quiet)).unwrap(); // changed: use WriteParams
        assert_eq!(fs::read_to_string(&f1).unwrap(), "original");
    }

    #[test]
    fn batch_partial_failure_continues() {
        let tmp = TempDir::new().unwrap();
        let f1 = tmp.path().join("batchok.txt");
        fs::write(&f1, "hello").unwrap();

        let plan = serde_json::json!([
            {"op":"replace","file":"/nonexistent/file.txt","from":"x","to":"y"},
            {"op":"replace","file": f1.to_str().unwrap(), "from":"hello","to":"done"}
        ]);

        run_batch(&plan.to_string(), tp(OutputMode::Quiet)).unwrap(); // changed: use WriteParams
                                                                      // Second op should still succeed despite first failure
        assert_eq!(fs::read_to_string(&f1).unwrap(), "done");
    }

    #[test]
    fn batch_set_json_key() {
        let tmp = TempDir::new().unwrap();
        let f = tmp.path().join("batchset.json");
        fs::write(&f, "{\"a\":1}").unwrap();

        let plan = serde_json::json!([
            {"op":"set","file": f.to_str().unwrap(), "key":"b","value":"2","value_type":"number"}
        ]);

        run_batch(&plan.to_string(), tp(OutputMode::Quiet)).unwrap(); // changed: use WriteParams
        let v: JsonValue = serde_json::from_str(&fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["b"], JsonValue::from(2));
    }

    #[test]
    fn batch_same_file_ops_are_applied_in_memory_in_order() {
        let tmp = TempDir::new().unwrap();
        let f = tmp.path().join("batchsame.txt");
        fs::write(&f, "a b c d").unwrap();

        let plan = serde_json::json!([
            {"op":"replace","file": f.to_str().unwrap(), "from":"b","to":"B"},
            {"op":"replace","file": f.to_str().unwrap(), "from":"x","to":"X"},
            {"op":"patch","file": f.to_str().unwrap(), "old":"c","new":"C"}
        ]);

        run_batch(&plan.to_string(), tp(OutputMode::Quiet)).unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "a B C d");
    }

    #[test]
    fn empty_key_rejected_for_json_set() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("emptykey.json");
        fs::write(&file, "{\"a\":1}").unwrap();

        let err = run_set(
            &file,
            "",
            "42",
            ConfigValueType::Number,
            ConfigFormat::Json,
            tp(OutputMode::Concise),
        )
        .unwrap_err(); // changed: use WriteParams
        assert!(err.to_string().contains("non-empty"));
        // File must not be modified
        assert_eq!(fs::read_to_string(&file).unwrap(), "{\"a\":1}");
    }

    #[test]
    fn set_json_preserves_indent_and_newline() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("i.json");
        fs::write(&file, "{\n    \"a\": {\n        \"b\": 1\n    }\n}\n").unwrap();

        run_set(
            &file,
            "a.c",
            "2",
            ConfigValueType::Number,
            ConfigFormat::Json,
            tp(OutputMode::Concise),
        )
        .unwrap(); // changed: use WriteParams

        let out = fs::read_to_string(&file).unwrap();
        assert!(out.contains("\n    \"a\": {"));
        assert!(out.contains("\n        \"b\": 1"));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn set_json_idempotent_noop() {
        // changed: verify set returns noop when value already matches
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("idem.json");
        fs::write(&file, "{\"a\":1}").unwrap();

        // First set — applies the change
        run_set(
            &file,
            "a",
            "1",
            ConfigValueType::Number,
            ConfigFormat::Json,
            tp(OutputMode::Quiet),
        )
        .unwrap(); // changed: use WriteParams

        // File should be unchanged (value already 1)
        assert_eq!(fs::read_to_string(&file).unwrap(), "{\"a\":1}");
    }

    // --- Tracking semantics tests ---

    #[test]
    fn tracking_does_not_credit_an_unobserved_native_counterfactual() {
        // A complete file is not an observed native Edit response. Until a host provides a
        // comparable response, write utilization is neutral in the direct-savings ledger.
        let content = "a".repeat(1000); // 1000 chars ~ 250 tokens
        let rtk_msg = "OK replace applied=1"; // ~5 tokens
        let (input, output, cmd) = write_tracking_args("replace", &content, rtk_msg);
        assert_eq!(input, rtk_msg, "unobserved native output receives no credit");
        assert_eq!(output, rtk_msg, "output should be compact rtk message");
        assert_eq!(
            cmd, "rtk write",
            "rtk_cmd should be normalized to 'rtk write'"
        );
        assert_eq!(input, output, "write utilization must be savings-neutral");
    }

    #[test]
    fn tracking_normalized_cmd_for_all_ops() {
        for op in &[
            "replace",
            "patch",
            "set",
            "batch",
            "replace (dry-run)",
            "set (noop)",
        ] {
            let (_, _, cmd) = write_tracking_args(op, "content", "ok");
            assert_eq!(
                cmd, "rtk write",
                "op '{}' must normalize to 'rtk write'",
                op
            );
        }
    }

    // --- Concurrency safety tests --- // changed: new tests for flock + CAS + retry

    #[test]
    fn locked_write_basic_replace() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("lock_basic.txt");
        fs::write(&file, "hello world").unwrap();

        let (resp, _content) = locked_write(&file, tp(OutputMode::Quiet), "replace", |content| {
            if !content.contains("hello") {
                return WriteAttempt::TerminalError {
                    code: "NO_MATCH",
                    hint: "no match".to_string(),
                };
            }
            WriteAttempt::Success(content.replacen("hello", "hi", 1), 1)
        })
        .unwrap();

        assert!(resp.ok);
        assert_eq!(resp.applied, Some(1));
        assert!(resp.retries.is_none()); // no retries needed
        assert_eq!(fs::read_to_string(&file).unwrap(), "hi world");
    }

    #[test]
    fn locked_write_terminal_error_does_not_retry() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("lock_nomatch.txt");
        fs::write(&file, "hello world").unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_closure = Arc::clone(&attempts);

        let err = locked_write(
            &file,
            tp_retry(OutputMode::Quiet, 3),
            "replace",
            move |_content| {
                attempts_for_closure.fetch_add(1, Ordering::SeqCst);
                WriteAttempt::TerminalError {
                    code: "NO_MATCH",
                    hint: "pattern not found".to_string(),
                }
            },
        )
        .unwrap_err();

        assert!(err.to_string().contains("NO_MATCH"));
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "terminal errors must not trigger retry loop"
        );
    }

    #[test]
    fn locked_write_unchanged_is_noop() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("lock_noop.txt");
        fs::write(&file, "same").unwrap();

        let (resp, _) = locked_write(&file, tp(OutputMode::Quiet), "replace", |_content| {
            WriteAttempt::Unchanged
        })
        .unwrap();

        assert!(resp.ok);
        assert_eq!(resp.hint.as_deref(), Some("no-op"));
    }

    #[test]
    fn write_response_retries_field_omitted_when_none() {
        // Verify new optional fields don't appear in JSON when not set
        let resp = WriteResponse::success("replace", 1);
        let json_str = serde_json::to_string(&resp).unwrap();
        let v: JsonValue = serde_json::from_str(&json_str).unwrap();
        assert!(
            v.get("retries").is_none(),
            "retries should be omitted when None"
        );
        assert!(
            v.get("conflict_resolved").is_none(),
            "conflict_resolved should be omitted when None"
        );
    }

    #[test]
    fn write_response_retries_field_present_when_set() {
        let mut resp = WriteResponse::success("replace", 1);
        resp.retries = Some(2);
        resp.conflict_resolved = Some(true);
        let json_str = serde_json::to_string(&resp).unwrap();
        let v: JsonValue = serde_json::from_str(&json_str).unwrap();
        assert_eq!(v["retries"], 2);
        assert_eq!(v["conflict_resolved"], true);
    }

    #[test]
    fn concurrency_opts_default_is_off() {
        let opts = ConcurrencyOpts::default();
        assert!(!opts.cas);
        assert_eq!(opts.max_retries, 0);
    }

    #[test]
    fn format_response_msg_includes_retries() {
        let mut resp = WriteResponse::success("replace", 3);
        resp.retries = Some(2);
        let msg = format_response_msg(&resp, "replace");
        assert!(msg.contains("retries=2"), "msg={}", msg);
    }

    #[test]
    fn format_response_msg_normal_success() {
        let resp = WriteResponse::success("patch", 1);
        let msg = format_response_msg(&resp, "patch");
        assert_eq!(msg, "OK patch applied=1");
    }

    // --- expand_at_ref tests ---

    #[test]
    fn expand_at_ref_passthrough() {
        let result = expand_at_ref("hello world").unwrap();
        assert_eq!(result.as_ref(), "hello world");
    }

    #[test]
    fn expand_at_ref_reads_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("atref_content.txt");
        fs::write(&file, "file content here").unwrap();
        let path_str = format!("@{}", file.display());
        let result = expand_at_ref(&path_str).unwrap();
        assert_eq!(result.as_ref(), "file content here");
    }

    #[test]
    fn expand_at_ref_missing_file_errors() {
        let result = expand_at_ref("@/tmp/__rtk_nonexistent_test_xyz.txt");
        assert!(result.is_err(), "missing @ref file should error");
    }
    // --- run_create / write file tests (US-002) --- // changed: new tests for create/file subcommand

    #[test]
    fn create_new_file_success() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("new_file.txt");
        run_create(&path, "hello world", false, tp(OutputMode::Concise)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
    }

    #[test]
    fn create_idempotent_same_content() {
        // Second call with identical content must be a noop (exit 0, no error)
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("idem.txt");
        run_create(&path, "same", false, tp(OutputMode::Concise)).unwrap();
        run_create(&path, "same", false, tp(OutputMode::Concise)).unwrap(); // must not error
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "same");
    }

    #[test]
    fn create_refuses_overwrite_different_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("existing.txt");
        std::fs::write(&path, "original").unwrap();
        let result = run_create(&path, "different", false, tp(OutputMode::Concise));
        assert!(
            result.is_err(),
            "create must refuse to overwrite with different content"
        );
    }

    #[test]
    fn create_dry_run_no_write() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("dryrun.txt");
        run_create(&path, "content", false, tp_dry(OutputMode::Concise)).unwrap();
        assert!(!path.exists(), "dry_run must not create the file");
    }

    #[test]
    fn create_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/deep/file.txt");
        run_create(&path, "data", false, tp(OutputMode::Concise)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "data");
    }

    #[test]
    fn create_at_file_expansion() {
        // @/tmp/file content expansion via expand_at_ref inside run_create
        let dir = TempDir::new().unwrap();
        let tmp_content = dir.path().join("content.txt");
        std::fs::write(&tmp_content, "from @file").unwrap();
        let at_ref = format!("@{}", tmp_content.display());
        let dest = dir.path().join("dest.txt");
        run_create(&dest, &at_ref, false, tp(OutputMode::Concise)).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "from @file");
    }

    // --- shell-mangling NO_MATCH hints + create --force (fork.2) --- // changed: new tests

    #[test]
    fn no_match_hint_reports_closest_line() {
        let content = "line one\nconst label = `tag ${value.toFixed(1)} pct`;\nline three\n";
        let hint = smart_no_match_hint(
            "no matches for --from",
            "--from",
            "const label = tag  pct;",
            content,
        );
        assert!(
            hint.contains("closest candidate at line 2"),
            "hint should point to nearest line: {hint}"
        );
    }

    #[test]
    fn no_match_hint_detects_shell_substitution_mangling() {
        // Simulates zsh eating backticks/${...} via command substitution before rtk saw the arg
        let content = "  send(`tag ${result.model} | ${result.cost}`);\n";
        let pattern = "  send( | );";
        let hint = smart_no_match_hint("no matches for --from", "--from", pattern, content);
        assert!(
            hint.contains("command/parameter substitution"),
            "hint should explain shell mangling: {hint}"
        );
        assert!(hint.contains("@file"), "hint should suggest @file: {hint}");
    }

    #[test]
    fn no_match_hint_no_closest_line_for_unrelated_content() {
        let hint = smart_no_match_hint(
            "hunk not found",
            "--old",
            "totally unrelated pattern xyz",
            "alpha beta\ngamma delta\n",
        );
        assert!(
            !hint.contains("closest candidate"),
            "unrelated content must not produce a candidate: {hint}"
        );
    }

    #[test]
    fn create_force_overwrites_different_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("force.txt");
        std::fs::write(&path, "original").unwrap();
        run_create(&path, "different", true, tp(OutputMode::Concise)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "different");
    }

    #[test]
    fn create_force_hint_in_file_exists_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nohint.txt");
        std::fs::write(&path, "original").unwrap();
        let err = run_create(&path, "different", false, tp(OutputMode::Concise)).unwrap_err();
        assert!(
            err.to_string().contains("--force"),
            "FILE_EXISTS should mention --force: {err}"
        );
    }
}
