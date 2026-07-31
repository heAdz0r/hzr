//! Source I/O for `read`: file/stdin reading, line-range logic, binary detection.
//! Extracted from read.rs (PR-2).

use anyhow::{Context, Result};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read as IoRead};
use std::path::Path;

const BINARY_PREVIEW_BYTES: usize = 256;
const BINARY_SAMPLE_BYTES: usize = 4096;

// ── Line range normalization ────────────────────────────────

pub fn normalize_line_range(
    from: Option<usize>,
    to: Option<usize>,
) -> Result<(usize, Option<usize>)> {
    let start = from.unwrap_or(1);
    if start == 0 {
        anyhow::bail!("--from must be >= 1");
    }
    if let Some(end) = to {
        if end == 0 {
            anyhow::bail!("--to must be >= 1");
        }
        if start > end {
            anyhow::bail!("Invalid range: --from ({start}) is greater than --to ({end})");
        }
    }
    Ok((start, to))
}

// ── File bytes with optional range ──────────────────────────

pub fn read_file_bytes(file: &Path, from: Option<usize>, to: Option<usize>) -> Result<Vec<u8>> {
    let (start, end) = normalize_line_range(from, to)?;
    if from.is_none() && to.is_none() {
        return fs::read(file).with_context(|| format!("Failed to read file: {}", file.display()));
    }

    let handle =
        File::open(file).with_context(|| format!("Failed to read file: {}", file.display()))?;
    let mut reader = BufReader::new(handle);
    let mut selected = Vec::new();
    let mut line_buf = Vec::new();
    let mut line_no = 0usize;

    loop {
        line_buf.clear();
        let read = reader
            .read_until(b'\n', &mut line_buf)
            .with_context(|| format!("Failed to read file: {}", file.display()))?;
        if read == 0 {
            break;
        }
        line_no += 1;

        if line_no < start {
            continue;
        }
        if let Some(end_line) = end {
            if line_no > end_line {
                break;
            }
        }

        selected.extend_from_slice(&line_buf);
    }

    Ok(selected)
}

// ── Stdin bytes ─────────────────────────────────────────────

pub fn read_stdin_bytes() -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .read_to_end(&mut bytes)
        .context("Failed to read from stdin")?;
    Ok(bytes)
}

// ── Text line range (for already-loaded content) ────────────

pub fn apply_line_range(content: &str, from: Option<usize>, to: Option<usize>) -> Result<String> {
    if from.is_none() && to.is_none() {
        return Ok(content.to_string());
    }
    let (start, end) = normalize_line_range(from, to)?;
    let start_idx = start - 1;
    let end_exclusive = end.unwrap_or(usize::MAX);
    let mut selected = Vec::new();

    for (idx, line) in content.lines().enumerate() {
        if idx < start_idx {
            continue;
        }
        if idx >= end_exclusive {
            break;
        }
        selected.push(line);
    }

    Ok(selected.join("\n"))
}

// ── Byte-level line range (for stdin none-mode) ─────────────

pub fn apply_line_range_bytes(
    input: &[u8],
    from: Option<usize>,
    to: Option<usize>,
) -> Result<Vec<u8>> {
    if from.is_none() && to.is_none() {
        return Ok(input.to_vec());
    }
    let (start, end) = normalize_line_range(from, to)?;
    let mut out = Vec::new();
    let mut line_no = 0usize;
    let mut idx = 0usize;

    while idx < input.len() {
        line_no += 1;
        let mut end_idx = idx;
        while end_idx < input.len() && input[end_idx] != b'\n' {
            end_idx += 1;
        }
        if end_idx < input.len() {
            end_idx += 1; // include '\n'
        }

        if line_no >= start {
            if let Some(end_line) = end {
                if line_no > end_line {
                    break;
                }
            }
            out.extend_from_slice(&input[idx..end_idx]);
        }

        idx = end_idx;
    }

    Ok(out)
}

// ── Binary detection ────────────────────────────────────────

pub fn looks_binary(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }

    let sample_len = bytes.len().min(BINARY_SAMPLE_BYTES);
    let sample = &bytes[..sample_len];
    if sample.contains(&0) {
        return true;
    }

    let mut suspicious = 0usize;
    for &b in sample {
        let looks_text =
            matches!(b, b'\n' | b'\r' | b'\t') || (0x20..=0x7E).contains(&b) || b >= 0x80;
        if !looks_text {
            suspicious += 1;
        }
    }

    suspicious * 100 / sample_len > 30
}

pub fn format_binary_preview(bytes: &[u8]) -> String {
    let shown = bytes.len().min(BINARY_PREVIEW_BYTES);
    let mut out = String::new();
    out.push_str(&format!(
        "Binary data detected ({} bytes). Showing first {} bytes as hex:\n",
        bytes.len(),
        shown
    ));

    for (i, chunk) in bytes[..shown].chunks(16).enumerate() {
        let offset = i * 16;
        let hex = chunk
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join(" ");
        let ascii = chunk
            .iter()
            .map(|b| {
                if b.is_ascii_graphic() || *b == b' ' {
                    *b as char
                } else {
                    '.'
                }
            })
            .collect::<String>();

        out.push_str(&format!("{offset:08x}  {hex:<47}  |{ascii}|\n"));
    }

    if bytes.len() > shown {
        out.push_str(&format!("... {} more bytes omitted\n", bytes.len() - shown));
    }

    out.push_str("Tip: use `rtk read <file> --level none --from N --to M` for exact text ranges.");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn normalize_valid_range() -> Result<()> {
        let (start, end) = normalize_line_range(Some(2), Some(5))?;
        assert_eq!(start, 2);
        assert_eq!(end, Some(5));
        Ok(())
    }

    #[test]
    fn normalize_defaults_start_to_1() -> Result<()> {
        let (start, _) = normalize_line_range(None, Some(3))?;
        assert_eq!(start, 1);
        Ok(())
    }

    #[test]
    fn normalize_rejects_zero_from() {
        assert!(normalize_line_range(Some(0), None).is_err());
    }

    #[test]
    fn normalize_rejects_zero_to() {
        assert!(normalize_line_range(None, Some(0)).is_err());
    }

    #[test]
    fn normalize_rejects_inverted_range() {
        assert!(normalize_line_range(Some(5), Some(2)).is_err());
    }

    #[test]
    fn read_file_bytes_full() -> Result<()> {
        let mut f = NamedTempFile::new()?;
        writeln!(f, "a")?;
        writeln!(f, "b")?;
        let bytes = read_file_bytes(f.path(), None, None)?;
        assert_eq!(String::from_utf8(bytes)?, "a\nb\n");
        Ok(())
    }

    #[test]
    fn read_file_bytes_range() -> Result<()> {
        let mut f = NamedTempFile::new()?;
        writeln!(f, "a")?;
        writeln!(f, "b")?;
        writeln!(f, "c")?;
        let bytes = read_file_bytes(f.path(), Some(2), Some(3))?;
        assert_eq!(String::from_utf8(bytes)?, "b\nc\n");
        Ok(())
    }

    #[test]
    fn apply_line_range_subset() -> Result<()> {
        assert_eq!(apply_line_range("a\nb\nc\nd", Some(2), Some(3))?, "b\nc");
        Ok(())
    }

    #[test]
    fn apply_line_range_open_end() -> Result<()> {
        assert_eq!(apply_line_range("a\nb\nc", Some(2), None)?, "b\nc");
        Ok(())
    }

    #[test]
    fn apply_line_range_bytes_subset() -> Result<()> {
        let out = apply_line_range_bytes(b"a\nb\nc\nd\n", Some(2), Some(3))?;
        assert_eq!(out, b"b\nc\n");
        Ok(())
    }

    #[test]
    fn looks_binary_detects_nul() {
        assert!(looks_binary(b"abc\0def"));
        assert!(!looks_binary(b"hello\nworld\n"));
    }

    #[test]
    fn looks_binary_empty_is_false() {
        assert!(!looks_binary(b""));
    }

    #[test]
    fn binary_preview_format() {
        let rendered = format_binary_preview(b"\x00\x01\x02abc");
        assert!(rendered.contains("Binary data detected"));
        assert!(rendered.contains("Tip: use"));
    }
}
