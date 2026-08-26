use std::fs;
use std::path::Path;
use std::process::Command;

use rusqlite::Connection;
use tempfile::tempdir;

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
    assert!(delivered.contains("recovery:"));

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
