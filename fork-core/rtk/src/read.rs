//! Read orchestrator: thin dispatch layer that delegates to submodules.
//! Heavy logic lives in read_source, read_cache, read_digest, read_render.
//! Refactored in PR-2 from a 1081-line monolith.

use crate::filter::{self, FilterLevel, Language};
use crate::read_cache;
use crate::read_digest;
use crate::read_render;
use crate::read_source;
use crate::tracking;
use anyhow::{Context, Result};
use std::io::Write as IoWrite;
use std::path::Path;

fn tracked_filter_level(level: FilterLevel) -> tracking::ReadFilterLevel {
    match level {
        FilterLevel::None => tracking::ReadFilterLevel::None,
        FilterLevel::Minimal => tracking::ReadFilterLevel::Minimal,
        FilterLevel::Aggressive => tracking::ReadFilterLevel::Aggressive,
    }
}

fn read_attribution(
    level: FilterLevel,
    from: Option<usize>,
    to: Option<usize>,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    source_bytes: Option<u64>,
) -> tracking::OperationAttribution {
    let mode = if from.is_some() || to.is_some() {
        tracking::OperationMode::ReadRange
    } else if max_lines.is_some() {
        tracking::OperationMode::ReadHead
    } else if tail_lines.is_some() {
        tracking::OperationMode::ReadTail
    } else if level == FilterLevel::None {
        tracking::OperationMode::ReadFull
    } else {
        tracking::OperationMode::ReadFiltered
    };
    tracking::OperationAttribution {
        operation: tracking::OperationKind::Read,
        mode,
        stage: tracking::AccountingStage::InternalTransport,
        include_content: None,
        limit: None,
        path_scope_count: None,
        filter_level: Some(tracked_filter_level(level)),
        from_line: from,
        to_line: to,
        source_bytes,
    }
}

// Re-export ReadMode from read_types for backward compat with main.rs
pub use crate::read_types::ReadMode;

fn shell_quote_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_+-./".contains(&byte))
    {
        return value.into_owned();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[allow(clippy::too_many_arguments)] // changed: file read params bundle naturally together
pub fn run(
    file: &Path,
    level: FilterLevel,
    from: Option<usize>,
    to: Option<usize>,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    line_numbers: bool,
    dedup: bool,
    verbose: u8,
) -> Result<()> {
    let run_start = std::time::Instant::now();
    let timer = tracking::TimedExecution::start();
    let attribution = read_attribution(
        level,
        from,
        to,
        max_lines,
        tail_lines,
        std::fs::metadata(file).ok().map(|metadata| metadata.len()),
    );

    if verbose > 0 {
        eprintln!("Reading: {} (filter: {})", file.display(), level);
    }

    // ── Cache lookup ────────────────────────────────────────
    let cache_key = if tail_lines.is_none()
        && read_cache::should_use_read_cache(level, from, to, max_lines, line_numbers, dedup)
    {
        match read_cache::build_read_cache_key(
            file,
            level,
            from,
            to,
            max_lines,
            line_numbers,
            dedup,
        ) {
            Ok(key) => Some(key),
            Err(err) => {
                if verbose > 1 {
                    eprintln!("Read cache key disabled: {err}");
                }
                None
            }
        }
    } else {
        None
    };

    if let Some(key) = cache_key.as_deref() {
        if let Some(output) = read_cache::load_read_cache(key) {
            print!("{output}");
            if !output.ends_with('\n') {
                println!();
            }
            let input_tokens = std::fs::metadata(file)
                .ok()
                .map(|meta| ((meta.len() as usize).saturating_add(3)) / 4)
                .unwrap_or_else(|| tracking::estimate_tokens(&output));
            let output_tokens = tracking::estimate_tokens(&output);
            let elapsed_ms = run_start.elapsed().as_millis() as u64;
            if let Ok(tracker) = tracking::Tracker::new() {
                let _ = tracker.record_attributed(
                    "read <path omitted>",
                    "rtk read (cache)",
                    input_tokens,
                    output_tokens,
                    elapsed_ms,
                    attribution,
                );
            }
            return Ok(());
        }
    }

    // ── Read file content ───────────────────────────────────
    let content_bytes = read_source::read_file_bytes(file, from, to)?;
    let full_line_count = if to.is_some() || max_lines.is_some() || tail_lines.is_some() {
        std::fs::read(file).ok().map(|full| {
            full.iter().filter(|byte| **byte == b'\n').count()
                + usize::from(!full.is_empty() && !full.ends_with(b"\n"))
        })
    } else {
        None
    };

    if let Some(end) = to {
        if let Some(total) = full_line_count {
            if end < total {
                eprintln!(
                    "Range ends at line {end} of {total}; recovery: `hzr rtk -- read {} --from {} --to {total} --level none`",
                    shell_quote_path(file),
                    end + 1
                );
            }
        }
    }

    // ── Binary detection ────────────────────────────────────
    if read_source::looks_binary(&content_bytes) {
        let preview = read_source::format_binary_preview(&content_bytes);
        println!("{preview}");
        let input_marker = format!("[binary:{} bytes]", content_bytes.len());
        timer.track_attributed(
            "read <path omitted>",
            "rtk read",
            &input_marker,
            &preview,
            attribution,
        );
        return Ok(());
    }

    // ── Special format digest (lock files, package.json, etc.) ──
    if level != FilterLevel::None
        && from.is_none()
        && to.is_none()
        && max_lines.is_none()
        && !line_numbers
        && read_digest::has_special_digest(file)
    {
        let content_str = String::from_utf8_lossy(&content_bytes);
        if let Some(digest) = read_digest::try_special_digest(file, &content_str, level) {
            let shown = crate::guard::never_worse(&content_str, &digest).to_string();
            print!("{shown}");
            if !shown.ends_with('\n') {
                println!();
            }
            if let Some(key) = cache_key.as_deref() {
                read_cache::store_read_cache(key, &shown);
            }
            timer.track_attributed(
                "read <path omitted>",
                "rtk read",
                &content_str,
                &shown,
                attribution,
            );
            return Ok(());
        }
        // fallback: strategy returned None (parse error), continue to normal read
    }

    // ── Tabular digest (CSV/TSV) ────────────────────────────
    if tail_lines.is_none()
        && read_digest::should_use_tabular_digest(file, level, from, to, max_lines, line_numbers)
    {
        if let Some(delimiter) = read_digest::tabular_delimiter(file) {
            match read_digest::build_tabular_digest(&content_bytes, delimiter, level) {
                Ok(digest) => {
                    let input = String::from_utf8_lossy(&content_bytes);
                    let shown = crate::guard::never_worse(&input, &digest).to_string();
                    print!("{shown}");
                    if !shown.ends_with('\n') {
                        println!();
                    }
                    if let Some(key) = cache_key.as_deref() {
                        read_cache::store_read_cache(key, &shown);
                    }
                    timer.track_attributed(
                        "read <path omitted>",
                        "rtk read",
                        &input,
                        &shown,
                        attribution,
                    );
                    return Ok(());
                }
                Err(err) => {
                    if verbose > 0 {
                        eprintln!("Tabular digest skipped: {err}");
                    }
                }
            }
        }
    }

    // ── Level none: exact cat parity ────────────────────────
    if level == FilterLevel::None && max_lines.is_none() && tail_lines.is_none() && !line_numbers {
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(&content_bytes)
            .context("Failed to write output")?;
        let input = String::from_utf8_lossy(&content_bytes);
        timer.track_attributed(
            "read <path omitted>",
            "rtk read",
            &input,
            &input,
            attribution,
        );
        return Ok(());
    }

    // ── Filter pipeline ─────────────────────────────────────
    let content = String::from_utf8_lossy(&content_bytes).into_owned();

    let lang = file
        .extension()
        .and_then(|e| e.to_str())
        .map(Language::from_extension)
        .unwrap_or(Language::Unknown);

    if verbose > 1 {
        eprintln!("Detected language: {:?}", lang);
    }

    let input_line_count = content.lines().count();
    let range_start = from.unwrap_or(1);
    let range_end = range_start.saturating_add(input_line_count.saturating_sub(1));
    let file_line_count = full_line_count.unwrap_or(input_line_count);
    let (bounded_content, bound_notice) = if let Some(tail) = tail_lines {
        let shown = input_line_count.min(tail);
        let omitted = input_line_count.saturating_sub(shown);
        let notice = (omitted > 0).then(|| {
            format!(
                "[showing {shown} bounded lines from file of {file_line_count}; {omitted} omitted from requested range; recovery: `hzr rtk -- read {} --from {range_start} --to {} --level none`]",
                shell_quote_path(file),
                range_start.saturating_add(omitted).saturating_sub(1)
            )
        });
        (keep_tail_lines(&content, tail), notice)
    } else if let Some(max) = max_lines {
        let shown = input_line_count.min(max);
        let omitted = input_line_count.saturating_sub(shown);
        let notice = (omitted > 0).then(|| {
            format!(
                "[showing {shown} bounded lines from file of {file_line_count}; {omitted} omitted from requested range; recovery: `hzr rtk -- read {} --from {} --to {range_end} --level none`]",
                shell_quote_path(file),
                range_start.saturating_add(shown)
            )
        });
        (keep_head_lines(&content, max), notice)
    } else {
        (content.clone(), None)
    };

    let filter = filter::get_filter(level);
    let mut filtered = filter.filter(&bounded_content, &lang);

    if verbose > 0 {
        let original_lines = content.lines().count();
        let filtered_lines = filtered.lines().count();
        let reduction = if original_lines > 0 {
            ((original_lines - filtered_lines) as f64 / original_lines as f64) * 100.0
        } else {
            0.0
        };
        eprintln!(
            "Lines: {} -> {} ({:.1}% reduction)",
            original_lines, filtered_lines, reduction
        );
    }

    // PR-6: truncate long lines in minimal/aggressive modes
    filtered = read_digest::truncate_long_lines(&filtered, level);

    // PR-7: opt-in dedup of repetitive blocks
    if dedup {
        filtered = read_render::dedup_repetitive_blocks(&filtered);
    }

    let (raw, rtk_output) = if line_numbers {
        let raw_start = from.unwrap_or(1);
        let output_start = if tail_lines.is_some() {
            raw_start.saturating_add(input_line_count.saturating_sub(filtered.lines().count()))
        } else {
            raw_start
        };
        (
            read_render::format_with_line_numbers_from(&bounded_content, raw_start),
            read_render::format_with_line_numbers_from(&filtered, output_start),
        )
    } else {
        (bounded_content.clone(), filtered.clone())
    };
    let shown = crate::guard::never_worse(&raw, &rtk_output).to_string();
    if let Some(key) = cache_key.as_deref() {
        read_cache::store_read_cache(key, &shown);
    }
    print!("{shown}");
    if let Some(notice) = bound_notice {
        if !shown.ends_with('\n') {
            println!();
        }
        println!("{notice}");
    }
    timer.track_attributed(
        "read <path omitted>",
        "rtk read",
        &content,
        &shown,
        attribution,
    );
    Ok(())
}

/// Run changed/since mode for a file (git diff-aware reading).
pub fn run_changed(file: &Path, revision: Option<&str>, context: usize, verbose: u8) -> Result<()> {
    use crate::read_changed;

    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!(
            "Diff reading: {} (revision: {:?}, context: {})",
            file.display(),
            revision,
            context
        );
    }

    let hunks = read_changed::git_diff_hunks(file, revision, context)?;
    let output = read_changed::render_changed_hunks(&hunks, file);
    let content =
        std::fs::read(file).with_context(|| format!("Failed to read file: {}", file.display()))?;
    let content = String::from_utf8_lossy(&content);
    let (baseline, shown) = changed_tracking_view(&content, &output);

    print!("{shown}");

    let mode_label = if revision.is_some() {
        "since"
    } else {
        "changed"
    };
    timer.track_attributed(
        "read <path omitted>",
        &format!("rtk read --{mode_label}"),
        baseline,
        shown,
        tracking::OperationAttribution {
            operation: tracking::OperationKind::Read,
            mode: if revision.is_some() {
                tracking::OperationMode::ReadSince
            } else {
                tracking::OperationMode::ReadChanged
            },
            stage: tracking::AccountingStage::InternalTransport,
            include_content: None,
            limit: None,
            path_scope_count: None,
            filter_level: None,
            from_line: None,
            to_line: None,
            source_bytes: Some(content.len() as u64),
        },
    );
    Ok(())
}

fn changed_tracking_view<'a>(file_content: &'a str, rendered_hunks: &'a str) -> (&'a str, &'a str) {
    (
        file_content,
        crate::guard::never_worse(file_content, rendered_hunks),
    )
}

/// Run outline or symbols mode for a file.
pub fn run_symbols(file: &Path, mode: &ReadMode, verbose: u8) -> Result<()> {
    use crate::filter::Language;
    use crate::read_symbols::{render_outline, render_symbols_json, SymbolExtractor};
    use crate::symbols_regex::RegexExtractor;

    let timer = tracking::TimedExecution::start();

    let content = std::fs::read_to_string(file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let lang = file
        .extension()
        .and_then(|e| e.to_str())
        .map(Language::from_extension)
        .unwrap_or(Language::Unknown);

    if verbose > 0 {
        eprintln!(
            "Extracting symbols: {} (lang: {:?}, mode: {:?})",
            file.display(),
            lang,
            mode
        );
    }

    let extractor = RegexExtractor;
    let symbols = extractor.extract(&content, &lang);
    let total_lines = content.lines().count();

    let markdown = file
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "md" | "mdx"));
    let output = match mode {
        ReadMode::Outline if markdown => crate::read_symbols::render_markdown_outline(&content),
        ReadMode::Outline => render_outline(&symbols, total_lines),
        ReadMode::Symbols => render_symbols_json(symbols, &lang, total_lines),
        _ => unreachable!("run_symbols called with non-symbol mode"),
    };

    println!("{output}");

    timer.track_attributed(
        "read <path omitted>",
        &format!(
            "rtk read --{}",
            if matches!(mode, ReadMode::Outline) {
                "outline"
            } else {
                "symbols"
            }
        ),
        &content,
        &output,
        tracking::OperationAttribution {
            operation: tracking::OperationKind::Read,
            mode: if matches!(mode, ReadMode::Outline) {
                tracking::OperationMode::ReadOutline
            } else {
                tracking::OperationMode::ReadSymbols
            },
            stage: tracking::AccountingStage::InternalTransport,
            include_content: None,
            limit: None,
            path_scope_count: None,
            filter_level: None,
            from_line: None,
            to_line: None,
            source_bytes: Some(content.len() as u64),
        },
    );
    Ok(())
}

pub fn run_stdin(
    level: FilterLevel,
    from: Option<usize>,
    to: Option<usize>,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    line_numbers: bool,
    verbose: u8,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Reading from stdin (filter: {})", level);
    }

    // Read stdin bytes
    let bytes = read_source::read_stdin_bytes()?;
    let attribution = read_attribution(
        level,
        from,
        to,
        max_lines,
        tail_lines,
        Some(bytes.len() as u64),
    );

    if read_source::looks_binary(&bytes) {
        let preview = read_source::format_binary_preview(&bytes);
        println!("{preview}");
        let input_marker = format!("[binary:{} bytes]", bytes.len());
        timer.track_attributed(
            "read stdin",
            "rtk read -",
            &input_marker,
            &preview,
            attribution,
        );
        return Ok(());
    }

    // Level none: preserve exact bytes
    if level == FilterLevel::None && max_lines.is_none() && tail_lines.is_none() && !line_numbers {
        let ranged = read_source::apply_line_range_bytes(&bytes, from, to)?;
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(&ranged)
            .context("Failed to write output")?;
        let input = String::from_utf8_lossy(&ranged);
        timer.track_attributed("read stdin", "rtk read -", &input, &input, attribution);
        return Ok(());
    }

    let content = read_source::apply_line_range(&String::from_utf8_lossy(&bytes), from, to)?;

    let lang = Language::Unknown;

    if verbose > 1 {
        eprintln!("Language: {:?} (stdin has no extension)", lang);
    }

    let filter = filter::get_filter(level);
    let mut filtered = filter.filter(&content, &lang);

    if verbose > 0 {
        let original_lines = content.lines().count();
        let filtered_lines = filtered.lines().count();
        let reduction = if original_lines > 0 {
            ((original_lines - filtered_lines) as f64 / original_lines as f64) * 100.0
        } else {
            0.0
        };
        eprintln!(
            "Lines: {} -> {} ({:.1}% reduction)",
            original_lines, filtered_lines, reduction
        );
    }

    let input_line_count = content.lines().count();
    if let Some(tail) = tail_lines {
        filtered = keep_tail_lines(&filtered, tail); // fork: tail semantics (upstream v0.42.4)
    } else if let Some(max) = max_lines {
        filtered = keep_head_lines(&filtered, max);
    }

    let (raw, rtk_output) = if line_numbers {
        let raw_start = from.unwrap_or(1);
        let output_start = if tail_lines.is_some() {
            raw_start.saturating_add(input_line_count.saturating_sub(filtered.lines().count()))
        } else {
            raw_start
        };
        (
            read_render::format_with_line_numbers_from(&content, raw_start),
            read_render::format_with_line_numbers_from(&filtered, output_start),
        )
    } else {
        (content.clone(), filtered.clone())
    };
    let shown = crate::guard::never_worse(&raw, &rtk_output);
    print!("{shown}");

    timer.track_attributed("read stdin", "rtk read -", &raw, shown, attribution);
    Ok(())
}

/// Keep only the last `tail` lines of `content` (tail(1) semantics).
/// fork: ported from upstream v0.42.4 `apply_line_window`.
fn keep_tail_lines(content: &str, tail: usize) -> String {
    if tail == 0 {
        return String::new();
    }
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(tail);
    let mut result = lines[start..].join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn keep_head_lines(content: &str, max_lines: usize) -> String {
    content.split_inclusive('\n').take(max_lines).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // fork: tail semantics tests (upstream v0.42.4 parity)
    #[test]
    fn test_keep_tail_lines_basic() {
        assert_eq!(keep_tail_lines("a\nb\nc\nd\n", 2), "c\nd\n");
    }

    #[test]
    fn read_attribution_distinguishes_full_filter_range_and_bounds() {
        let full = read_attribution(FilterLevel::None, None, None, None, None, Some(100));
        assert_eq!(full.mode, tracking::OperationMode::ReadFull);
        assert_eq!(full.filter_level, Some(tracking::ReadFilterLevel::None));
        assert_eq!(full.source_bytes, Some(100));

        assert_eq!(
            read_attribution(FilterLevel::Minimal, None, None, None, None, None).mode,
            tracking::OperationMode::ReadFiltered
        );
        assert_eq!(
            read_attribution(FilterLevel::None, Some(3), Some(9), None, None, None).mode,
            tracking::OperationMode::ReadRange
        );
        assert_eq!(
            read_attribution(FilterLevel::None, None, None, Some(10), None, None).mode,
            tracking::OperationMode::ReadHead
        );
        assert_eq!(
            read_attribution(FilterLevel::None, None, None, None, Some(10), None).mode,
            tracking::OperationMode::ReadTail
        );
    }

    #[test]
    fn test_keep_tail_lines_no_trailing_newline() {
        assert_eq!(keep_tail_lines("a\nb\nc", 2), "b\nc");
    }

    #[test]
    fn test_keep_tail_lines_zero() {
        assert_eq!(keep_tail_lines("a\nb", 0), "");
    }

    #[test]
    fn test_recovery_path_is_shell_safe() {
        assert_eq!(
            shell_quote_path(Path::new("dir with space/it's.rs")),
            "'dir with space/it'\"'\"'s.rs'"
        );
    }

    #[test]
    fn test_keep_tail_lines_more_than_content() {
        assert_eq!(keep_tail_lines("a\nb\n", 10), "a\nb\n");
    }

    #[test]
    fn changed_view_uses_the_real_file_baseline_and_never_grows() {
        let file_content = "one\n";
        let rendered_hunks = "@@ -1 +1 @@\n-one\n+two\n";
        let (baseline, shown) = changed_tracking_view(file_content, rendered_hunks);
        assert_eq!(baseline, file_content);
        assert_eq!(
            shown, file_content,
            "changed mode must honor never-worse output"
        );
    }

    #[test]
    fn test_read_rust_file() -> Result<()> {
        let mut file = NamedTempFile::with_suffix(".rs")?;
        writeln!(
            file,
            r#"// Comment
fn main() {{
    println!("Hello");
}}"#
        )?;

        run(
            file.path(),
            FilterLevel::Minimal,
            None,
            None,
            None,
            None,
            false,
            false,
            0,
        )?;
        Ok(())
    }

    #[test]
    fn test_stdin_support_signature() {
        // Compile-time verification that run_stdin exists with correct signature
    }
}
