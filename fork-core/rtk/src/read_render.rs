//! Rendering utilities for `read`: line numbers, truncation helpers.
//! Extracted from read.rs (PR-2).

/// Format text content with line numbers in "N │ line" format.
pub fn format_with_line_numbers(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let width = lines.len().to_string().len();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        out.push_str(&format!("{:>width$} │ {}\n", i + 1, line, width = width));
    }
    out
}

// ── Dedup repetitive blocks (PR-7) ──────────────────────────

/// Minimum consecutive identical lines to trigger dedup.
const DEDUP_MIN_REPEAT: usize = 3;

/// Deduplicate repetitive consecutive blocks in content.
/// Conservative: only collapses blocks of 3+ identical consecutive lines.
/// Shows first occurrence + "... (N more identical lines)" marker.
pub fn dedup_repetitive_blocks(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() < DEDUP_MIN_REPEAT {
        return content.to_string();
    }

    let mut out = String::new();
    let mut i = 0;

    while i < lines.len() {
        let current = lines[i];

        // Count consecutive identical lines
        let mut run_len = 1;
        while i + run_len < lines.len() && lines[i + run_len] == current {
            run_len += 1;
        }

        if run_len >= DEDUP_MIN_REPEAT {
            // Show first line + dedup marker
            out.push_str(current);
            out.push('\n');
            out.push_str(&format!("  ... ({} more identical lines)\n", run_len - 1));
            i += run_len;
        } else {
            // Output all lines in this non-repeating group
            for _ in 0..run_len {
                out.push_str(lines[i]);
                out.push('\n');
                i += 1;
            }
        }
    }

    // Match trailing newline behavior
    if !content.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_numbers_single_digit() {
        let result = format_with_line_numbers("a\nb\nc");
        assert_eq!(result, "1 │ a\n2 │ b\n3 │ c\n");
    }

    #[test]
    fn line_numbers_double_digit() {
        let input: String = (1..=12)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let result = format_with_line_numbers(&input);
        assert!(result.starts_with(" 1 │ line1\n"));
        assert!(result.contains("12 │ line12\n"));
    }

    #[test]
    fn line_numbers_empty_content() {
        let result = format_with_line_numbers("");
        assert_eq!(result, "");
    }

    // ── Dedup tests (PR-7) ──────────────────────────────────

    #[test]
    fn dedup_collapses_3_plus_identical() {
        let input = "a\na\na\nb\n";
        let result = dedup_repetitive_blocks(input);
        assert!(result.contains("a\n"));
        assert!(result.contains("2 more identical lines"));
        assert!(result.contains("b\n"));
    }

    #[test]
    fn dedup_preserves_2_identical() {
        let input = "a\na\nb\n";
        let result = dedup_repetitive_blocks(input);
        assert_eq!(
            result, "a\na\nb\n",
            "2 identical lines should NOT be deduped"
        );
    }

    #[test]
    fn dedup_preserves_unique_lines() {
        let input = "a\nb\nc\n";
        let result = dedup_repetitive_blocks(input);
        assert_eq!(result, input);
    }

    #[test]
    fn dedup_multiple_groups() {
        let input = "x\nx\nx\nx\ny\nz\nz\nz\n";
        let result = dedup_repetitive_blocks(input);
        assert!(result.contains("3 more identical lines"), "first group");
        assert!(result.contains("2 more identical lines"), "second group");
        assert!(result.contains("y\n"), "unique line preserved");
    }

    #[test]
    fn dedup_empty_lines() {
        let input = "\n\n\n\n\ncode\n";
        let result = dedup_repetitive_blocks(input);
        assert!(
            result.contains("4 more identical lines"),
            "blank lines deduped"
        );
        assert!(result.contains("code"), "code preserved");
    }

    #[test]
    fn dedup_short_content_passthrough() {
        let input = "a\nb\n";
        let result = dedup_repetitive_blocks(input);
        assert_eq!(result, input);
    }
}
