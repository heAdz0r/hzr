use std::fs;
use std::path::Path;
use std::process::Command;

use rusqlite::Connection;
use tempfile::tempdir;

#[test]
fn numbered_exact_reads_compare_equivalent_presentations() {
    let directory = tempdir().expect("temp directory");
    let source = directory.path().join("numbered.txt");
    let content = "первая 🎵\nsecond\nlast";
    fs::write(&source, content).expect("write source");
    for range in [false, true] {
        let ledger = directory.path().join(format!("numbered-{range}.sqlite"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_rtk"));
        command.args([
            "read",
            source.to_str().expect("utf-8 path"),
            "--line-numbers",
            "--level",
            "none",
        ]);
        if range {
            command.args(["--from", "2", "--to", "3"]);
        }
        let output = command
            .env("RTK_DB_PATH", &ledger)
            .env_remove("HZR_INTERNAL_ACCOUNTING_RECEIPT_JOURNAL")
            .env_remove("HZR_INTERNAL_ACCOUNTING_FAILURE_JOURNAL")
            .env_remove("HZR_INTERNAL_ACCOUNTING_CORRELATION")
            .env("RTK_TRACKING_DISABLED", "0")
            .output()
            .expect("run numbered read");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let delivered = String::from_utf8(output.stdout).expect("utf-8 stdout");
        assert!(delivered.contains("2\tsecond"));
        assert_eq!(delivered.contains("1\tпервая 🎵"), !range);
        let connection = Connection::open(&ledger).expect("open ledger");
        let (baseline, recorded): (u64, u64) = connection
            .query_row(
                "SELECT input_tokens, output_tokens FROM commands ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read measurement");
        assert_eq!(recorded, delivered.len().div_ceil(4) as u64);
        assert_eq!(
            baseline, recorded,
            "exact numbered reads have zero transform savings"
        );
    }
}

fn assert_accounted_read(source: &Path, ledger: &Path, max_lines: usize) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args([
            "read",
            source.to_str().expect("utf-8 path"),
            "--max-lines",
            &max_lines.to_string(),
            "--level",
            "none",
        ])
        .env("RTK_DB_PATH", ledger)
        .env_remove("HZR_INTERNAL_ACCOUNTING_RECEIPT_JOURNAL")
        .env_remove("HZR_INTERNAL_ACCOUNTING_FAILURE_JOURNAL")
        .env_remove("HZR_INTERNAL_ACCOUNTING_CORRELATION")
        .env("RTK_TRACKING_DISABLED", "0")
        .output()
        .expect("run bounded read");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let delivered = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let connection = Connection::open(ledger).expect("open ledger");
    let recorded: u64 = connection
        .query_row(
            "SELECT output_tokens FROM commands ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("recorded output tokens");
    assert_eq!(recorded, delivered.len().div_ceil(4) as u64);
    delivered
}

#[test]
fn bounded_read_ledger_counts_notices_unicode_and_newline_shapes() {
    let directory = tempdir().expect("temp directory");
    let ledger = directory.path().join("history.sqlite");

    let unicode = directory.path().join("unicode.txt");
    fs::write(&unicode, "первая 🎵\nвторая строка\nтретья строка\n").expect("write unicode");
    let delivered = assert_accounted_read(&unicode, &ledger, 1);
    assert!(delivered.starts_with("первая 🎵\n"));
    assert!(delivered.contains("[lines 1-1 of 3]"), "{delivered}"); // 0.10.0: explicit bound marker

    let without_newline = directory.path().join("without-newline.txt");
    fs::write(&without_newline, "exact terminal line").expect("write no-newline source");
    assert_eq!(
        assert_accounted_read(&without_newline, &ledger, 1),
        "exact terminal line"
    );

    let with_newline = directory.path().join("with-newline.txt");
    fs::write(&with_newline, "exact terminal line\n").expect("write newline source");
    assert_eq!(
        assert_accounted_read(&with_newline, &ledger, 1),
        "exact terminal line\n"
    );
}

// 0.11.2: every read, cached or failed, reaches the HZR ledger as a receipt and never the
// engine's own history database.
fn receipt_read(directory: &Path, args: &[&str]) -> (std::process::Output, Vec<serde_json::Value>) {
    let receipts = directory.join("receipts.jsonl"); // 0.11.2
    let output = Command::new(env!("CARGO_BIN_EXE_rtk")) // 0.11.2
        .args(args)
        .current_dir(directory)
        .env("HOME", directory.join("home"))
        .env("XDG_CACHE_HOME", directory.join("home/cache"))
        .env("RTK_DB_PATH", directory.join("must-not-exist.sqlite"))
        .env("RTK_TRACKING_DISABLED", "0")
        .env("HZR_INTERNAL_ACCOUNTING_RECEIPT_JOURNAL", &receipts)
        .env("HZR_INTERNAL_ACCOUNTING_FAILURE_JOURNAL", directory.join("failures.jsonl"))
        .env("HZR_INTERNAL_ACCOUNTING_CORRELATION", "0123456789abcdef0123456789abcdef")
        .output()
        .expect("run read");
    let journal = fs::read_to_string(&receipts).unwrap_or_default(); // 0.11.2
    let parsed = journal // 0.11.2
        .lines()
        .map(|line| serde_json::from_str(line).expect("receipt json"))
        .collect();
    (output, parsed)
}

#[test]
fn cached_reads_write_receipts_and_never_the_history_database() {
    let directory = tempdir().expect("temp directory"); // 0.11.2
    let source = "fn main() {\n    // a comment the minimal filter drops\n    println!(\"hi\");\n}\n"; // 0.11.2
    fs::write(directory.path().join("main.rs"), source).expect("write source"); // 0.11.2
    for _ in 0..2 {
        let (output, _) = receipt_read(directory.path(), &["read", "main.rs"]); // 0.11.2
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr)); // 0.11.2
    }
    let (_, receipts) = receipt_read(directory.path(), &["read", "main.rs"]); // 0.11.2
    assert_eq!(receipts.len(), 3, "a cache hit wrote no receipt: {receipts:?}"); // 0.11.2
    let whole_file = source.len().div_ceil(4) as u64; // 0.11.2
    for receipt in &receipts {
        assert_eq!(receipt["baseline_tokens"], whole_file, "{receipt}"); // 0.11.2
    }
    assert!(!directory.path().join("must-not-exist.sqlite").exists()); // 0.11.2
}

#[test]
fn failed_reads_write_a_zero_saving_receipt() {
    let directory = tempdir().expect("temp directory"); // 0.11.2
    let (output, receipts) = receipt_read(directory.path(), &["read", "missing.rs"]); // 0.11.2
    assert!(!output.status.success()); // 0.11.2
    assert_eq!(receipts.len(), 1, "a failed read wrote no receipt"); // 0.11.2
    assert_eq!(receipts[0]["baseline_tokens"], receipts[0]["delivered_tokens"]); // 0.11.2
    assert!(!directory.path().join("must-not-exist.sqlite").exists()); // 0.11.2
}

#[test]
fn batch_reads_write_receipts_and_never_the_history_database() {
    let directory = tempdir().expect("temp directory"); // 0.11.2
    fs::write(directory.path().join("a.txt"), "alpha\n").expect("write a"); // 0.11.2
    fs::write(directory.path().join("b.txt"), "beta\n").expect("write b"); // 0.11.2
    let (output, receipts) = receipt_read(
        directory.path(),
        &["read", "--batch", "a.txt", "b.txt", "--max-tokens", "400"],
    ); // 0.11.2
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr)); // 0.11.2
    assert_eq!(receipts.len(), 1, "a batch read wrote no receipt"); // 0.11.2
    assert!(!directory.path().join("must-not-exist.sqlite").exists()); // 0.11.2
}
