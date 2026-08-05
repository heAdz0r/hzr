//! Integration tests for `rtk read` CLI behavior.
//! These tests freeze the current behavior as a safety net before refactoring.

use std::io::Write;
use std::process::Command;

fn rtk_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
}

// ── Helper: create a temp file with content ─────────────────

fn write_temp(suffix: &str, content: &[u8]) -> tempfile::NamedTempFile {
    let mut f = tempfile::Builder::new()
        .suffix(suffix)
        .tempfile()
        .expect("create temp file");
    f.write_all(content).expect("write temp file");
    f.flush().expect("flush");
    f
}

// ── Level none: exact cat parity ────────────────────────────

#[test]
fn read_level_none_exact_output() {
    let content = b"line1\nline2\nline3\n";
    let f = write_temp(".txt", content);

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--level", "none"])
        .output()
        .expect("run rtk read");

    assert!(out.status.success(), "exit 0");
    assert_eq!(out.stdout, content, "exact bytes preserved in level=none");
}

#[test]
fn read_level_none_no_trailing_newline() {
    let content = b"no trailing newline";
    let f = write_temp(".txt", content);

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--level", "none"])
        .output()
        .expect("run rtk read");

    assert!(out.status.success());
    assert_eq!(out.stdout, content, "no trailing newline preserved");
}

// ── Level minimal: filters applied ──────────────────────────

#[test]
fn read_level_minimal_filters_comments() {
    let content = b"// this is a comment\nfn main() {}\n";
    let f = write_temp(".rs", content);

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--level", "minimal"])
        .output()
        .expect("run rtk read");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    // minimal filter should strip single-line comments for Rust
    assert!(
        !stdout.contains("// this is a comment"),
        "comment should be filtered in minimal mode"
    );
    assert!(stdout.contains("fn main()"), "code should remain");
}

// ── Line range: --from / --to ───────────────────────────────

#[test]
fn read_from_to_range() {
    let content = b"a\nb\nc\nd\ne\n";
    let f = write_temp(".txt", content);

    let out = rtk_bin()
        .args([
            "read",
            f.path().to_str().unwrap(),
            "--level",
            "none",
            "--from",
            "2",
            "--to",
            "4",
        ])
        .output()
        .expect("run rtk read");

    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "b\nc\nd\n",
        "--from 2 --to 4 extracts lines 2-4"
    );
}

#[test]
fn read_from_only() {
    let content = b"a\nb\nc\n";
    let f = write_temp(".txt", content);

    let out = rtk_bin()
        .args([
            "read",
            f.path().to_str().unwrap(),
            "--level",
            "none",
            "--from",
            "2",
        ])
        .output()
        .expect("run rtk read");

    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "b\nc\n",
        "--from 2 without --to gives lines 2+"
    );
}

#[test]
fn read_to_only() {
    let content = b"a\nb\nc\n";
    let f = write_temp(".txt", content);

    let out = rtk_bin()
        .args([
            "read",
            f.path().to_str().unwrap(),
            "--level",
            "none",
            "--to",
            "2",
        ])
        .output()
        .expect("run rtk read");

    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "a\nb\n",
        "--to 2 gives lines 1-2"
    );
}

#[test]
fn read_invalid_range_from_zero() {
    let f = write_temp(".txt", b"a\n");

    let out = rtk_bin()
        .args([
            "read",
            f.path().to_str().unwrap(),
            "--level",
            "none",
            "--from",
            "0",
        ])
        .output()
        .expect("run rtk read");

    assert!(!out.status.success(), "--from 0 should fail");
}

#[test]
fn read_invalid_range_from_gt_to() {
    let f = write_temp(".txt", b"a\nb\nc\n");

    let out = rtk_bin()
        .args([
            "read",
            f.path().to_str().unwrap(),
            "--level",
            "none",
            "--from",
            "3",
            "--to",
            "1",
        ])
        .output()
        .expect("run rtk read");

    assert!(!out.status.success(), "--from > --to should fail");
}

// ── Line numbers ────────────────────────────────────────────

#[test]
fn read_line_numbers() {
    let content = b"alpha\nbeta\n";
    let f = write_temp(".txt", content);

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--level", "none", "-n"])
        .output()
        .expect("run rtk read");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    // format_with_line_numbers produces "N │ line" format
    assert!(stdout.contains("1 │ alpha"), "line 1 numbered");
    assert!(stdout.contains("2 │ beta"), "line 2 numbered");
}

#[test]
fn read_line_numbers_default_to_exact_source_lines() {
    let content = b"// source line one\npub fn run() {}\n";
    let f = write_temp(".rs", content);

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "-n"])
        .output()
        .expect("run rtk read with default line numbers");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1 │ // source line one"));
    assert!(stdout.contains("2 │ pub fn run() {}"));
}

#[test]
fn read_range_line_numbers_keep_source_coordinates() {
    let content = b"one\ntwo\nthree\nfour\nfive\n";
    let f = write_temp(".txt", content);

    let out = rtk_bin()
        .args([
            "read",
            f.path().to_str().unwrap(),
            "--from",
            "3",
            "--to",
            "4",
            "-n",
        ])
        .output()
        .expect("run ranged rtk read with line numbers");

    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "3 │ three\n4 │ four\n"
    );
}

// ── Binary detection ────────────────────────────────────────

#[test]
fn read_binary_file_shows_hex() {
    let mut data = vec![0u8; 64];
    data[0] = 0x89;
    data[1] = b'P';
    data[2] = b'N';
    data[3] = b'G';
    let f = write_temp(".bin", &data);

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap()])
        .output()
        .expect("run rtk read");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Binary data detected"),
        "binary file triggers hex preview"
    );
}

// ── CSV tabular digest ──────────────────────────────────────

#[test]
fn read_csv_minimal_produces_digest() {
    let mut csv = String::from("id,name,score\n");
    for id in 1..=100 {
        csv.push_str(&format!("{id},user_{id},{}\n", 70 + id % 30));
    }
    let f = write_temp(".csv", csv.as_bytes());

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--level", "minimal"])
        .output()
        .expect("run rtk read");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Tabular digest (CSV)"),
        "CSV triggers tabular digest"
    );
    assert!(stdout.contains("Columns: 3"), "shows column count");
    assert!(stdout.contains("Rows"), "shows row estimate");
}

#[test]
fn read_csv_level_none_shows_raw() {
    let csv = b"id,name\n1,Alice\n";
    let f = write_temp(".csv", csv);

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--level", "none"])
        .output()
        .expect("run rtk read");

    assert!(out.status.success());
    assert_eq!(out.stdout, csv, "level=none bypasses CSV digest");
}

#[test]
fn read_markdown_digest_is_self_describing_and_recoverable() {
    let mut markdown = String::from("# Title\n\n");
    for number in 1..=100 {
        markdown.push_str(&format!(
            "Important body line {number} with enough detail for a realistic document.\n"
        ));
    }
    markdown.push_str("\n## Details\n\nExact detail.\n");
    let f = write_temp(".md", markdown.as_bytes());

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap()])
        .output()
        .expect("run rtk read markdown digest");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Markdown digest (content omitted)"));
    assert!(stdout.contains("Source:"));
    assert!(stdout.contains("bytes"));
    assert!(stdout.contains("Sections: 2/2 shown"));
    assert!(stdout.contains("Lead preview:"));
    assert!(stdout.contains("Important body line 1"));
    assert!(stdout.contains("`--level none` for exact content"));
    assert!(stdout.contains("`--from N --to M` for an exact range"));
}

#[test]
fn read_markdown_level_none_is_byte_exact() {
    let markdown = b"# Title\n\nImportant body.\n";
    let f = write_temp(".md", markdown);

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--level", "none"])
        .output()
        .expect("run exact rtk read markdown");

    assert!(out.status.success());
    assert_eq!(out.stdout, markdown);
}

#[test]
fn read_csv_aggressive_no_numeric_stats() {
    let mut csv = String::from("id,val\n");
    for id in 1..=100 {
        csv.push_str(&format!("{id},{}\n", id * 10));
    }
    let f = write_temp(".csv", csv.as_bytes());

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--level", "aggressive"])
        .output()
        .expect("run rtk read");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Tabular digest"),
        "aggressive still does digest"
    );
    assert!(
        !stdout.contains("Numeric stats"),
        "aggressive skips numeric stats"
    );
}

// ── TSV digest ──────────────────────────────────────────────

#[test]
fn read_tsv_produces_digest() {
    let mut tsv = String::from("id\tname\n");
    for id in 1..=100 {
        tsv.push_str(&format!("{id}\tuser_{id}\n"));
    }
    let f = write_temp(".tsv", tsv.as_bytes());

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--level", "minimal"])
        .output()
        .expect("run rtk read");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Tabular digest (TSV)"),
        "TSV triggers digest"
    );
}

// ── Nonexistent file ────────────────────────────────────────

#[test]
fn read_nonexistent_file_fails() {
    let out = rtk_bin()
        .args(["read", "/tmp/rtk-test-nonexistent-file-12345.txt"])
        .output()
        .expect("run rtk read");

    assert!(!out.status.success(), "nonexistent file should fail");
}

// ── Stdin with - ────────────────────────────────────────────

#[test]
fn read_stdin_level_none() {
    let out = rtk_bin()
        .args(["read", "-", "--level", "none"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"hello stdin\n")
                .unwrap();
            child.wait_with_output()
        })
        .expect("run rtk read -");

    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "hello stdin\n",
        "stdin passthrough in level=none"
    );
}

// ── Max lines ───────────────────────────────────────────────

#[test]
fn read_max_lines_truncates() {
    let content = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n";
    let f = write_temp(".txt", content.as_bytes());

    let out = rtk_bin()
        .args([
            "read",
            f.path().to_str().unwrap(),
            "--level",
            "minimal",
            "--max-lines",
            "3",
        ])
        .output()
        .expect("run rtk read");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("line1\nline2\nline3\n"));
    assert!(stdout.contains("showing 3 bounded lines from file of 10"));
    assert!(stdout.contains("7 omitted from requested range"));
    assert!(stdout.contains("--from 4 --to 10 --level none"));
}

// ── Empty file ──────────────────────────────────────────────

#[test]
fn read_empty_file() {
    let f = write_temp(".txt", b"");

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--level", "none"])
        .output()
        .expect("run rtk read");

    assert!(out.status.success());
    assert!(out.stdout.is_empty(), "empty file produces empty output");
}

// ── Outline mode ────────────────────────────────────────────

#[test]
fn read_outline_rust_file() {
    let code =
        b"pub struct Config {\n    name: String,\n}\n\npub fn run() {\n}\n\nfn helper() {\n}\n";
    let f = write_temp(".rs", code);

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--outline"])
        .output()
        .expect("run rtk read --outline");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Config"), "outline shows struct name");
    assert!(stdout.contains("run"), "outline shows function name");
    assert!(stdout.contains("helper"), "outline shows private function");
    assert!(stdout.contains("struct"), "outline shows symbol kind");
    assert!(stdout.contains("fn"), "outline shows fn kind");
}

#[test]
fn read_outline_python_file() {
    let code = b"class Config:\n    def __init__(self, name):\n        self.name = name\n\ndef run():\n    pass\n";
    let f = write_temp(".py", code);

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--outline"])
        .output()
        .expect("run rtk read --outline for Python");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Config"), "outline shows class");
    assert!(stdout.contains("__init__"), "outline shows method");
    assert!(stdout.contains("run"), "outline shows function");
}

#[test]
fn read_outline_typescript_file() {
    let code = b"export interface User {\n  id: string;\n}\n\nexport function run(): void {\n}\n";
    let f = write_temp(".ts", code);

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--outline"])
        .output()
        .expect("run rtk read --outline for TypeScript");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("User"), "outline shows interface");
    assert!(stdout.contains("run"), "outline shows function");
}

#[test]
fn read_outline_markdown_file() {
    let markdown = b"# Title\n\nIntro.\n\n## First\n\nBody.\n\n### Nested\n\nMore.\n\n## Second\n";
    let f = write_temp(".md", markdown);

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--outline"])
        .output()
        .expect("run rtk read --outline for Markdown");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1-13  # Title"));
    assert!(stdout.contains("5-12    ## First"));
    assert!(stdout.contains("9-12      ### Nested"));
    assert!(stdout.contains("13-13    ## Second"));
}

#[test]
fn read_outline_empty_for_unsupported_lang() {
    let code = b"#!/bin/bash\necho hello\n";
    let f = write_temp(".sh", code);

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--outline"])
        .output()
        .expect("run rtk read --outline for shell");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no symbols found"),
        "unsupported lang shows no symbols"
    );
}

// ── Symbols mode (JSON) ─────────────────────────────────────

#[test]
fn read_symbols_json_valid() {
    let code = b"pub fn run() {\n}\n\npub struct Config {\n    name: String,\n}\n";
    let f = write_temp(".rs", code);

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--symbols"])
        .output()
        .expect("run rtk read --symbols");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);

    // Must be valid JSON
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON output");
    assert_eq!(parsed["version"], 1, "version field is 1");
    assert_eq!(parsed["language"], "rust", "language field");
    assert!(parsed["symbols"].is_array(), "symbols is array");
    let syms = parsed["symbols"].as_array().unwrap();
    assert!(syms.len() >= 2, "at least 2 symbols: {}", syms.len());
}

#[test]
fn read_symbols_stdin_rejected() {
    let out = rtk_bin()
        .args(["read", "-", "--symbols"])
        .output()
        .expect("run rtk read - --symbols");

    assert!(
        !out.status.success(),
        "--symbols with stdin should be rejected"
    );
}

// ── Outline and symbols are mutually exclusive ──────────────

#[test]
fn read_changed_stdin_rejected() {
    let out = rtk_bin()
        .args(["read", "-", "--changed"])
        .output()
        .expect("run rtk read - --changed");

    assert!(
        !out.status.success(),
        "--changed with stdin should be rejected"
    );
}

#[test]
fn read_since_stdin_rejected() {
    let out = rtk_bin()
        .args(["read", "-", "--since", "HEAD~1"])
        .output()
        .expect("run rtk read - --since");

    assert!(
        !out.status.success(),
        "--since with stdin should be rejected"
    );
}

// ── Outline and symbols are mutually exclusive ──────────────

#[test]
fn read_outline_and_symbols_conflict() {
    let f = write_temp(".rs", b"fn main() {}\n");

    let out = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--outline", "--symbols"])
        .output()
        .expect("run rtk read --outline --symbols");

    assert!(
        !out.status.success(),
        "--outline and --symbols are mutually exclusive"
    );
}

#[test]
fn read_changed_rejects_range_flags() {
    let f = write_temp(".rs", b"fn main() {}\n");
    let out = rtk_bin()
        .args([
            "read",
            f.path().to_str().unwrap(),
            "--changed",
            "--from",
            "1",
        ])
        .output()
        .expect("run rtk read --changed --from");

    assert!(
        !out.status.success(),
        "--from must be rejected with --changed"
    );
}

#[test]
fn read_outline_rejects_diff_context() {
    let f = write_temp(".rs", b"fn main() {}\n");
    let out = rtk_bin()
        .args([
            "read",
            f.path().to_str().unwrap(),
            "--outline",
            "--diff-context",
            "10",
        ])
        .output()
        .expect("run rtk read --outline --diff-context");

    assert!(
        !out.status.success(),
        "--diff-context must be rejected with --outline"
    );
}

#[test]
fn read_cache_key_isolated_by_dedup_flag() {
    let content = b"x\nx\nx\nx\ny\n";
    let f = write_temp(".txt", content);

    let first = rtk_bin()
        .args([
            "read",
            f.path().to_str().unwrap(),
            "--level",
            "minimal",
            "--dedup",
        ])
        .output()
        .expect("run rtk read with dedup");
    assert!(first.status.success());

    let second = rtk_bin()
        .args(["read", f.path().to_str().unwrap(), "--level", "minimal"])
        .output()
        .expect("run rtk read without dedup");
    assert!(second.status.success());

    let second_stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        !second_stdout.contains("more identical lines"),
        "non-dedup read must not reuse dedup cache output"
    );
}
