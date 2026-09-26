#![cfg(unix)]
//! 0.11.2: a recorded baseline is what the caller's own command would have printed, never
//! the richer form rtk ran internally (`ls -la`, `go test -json`).

use std::fs;
use std::path::Path;
use std::process::Command;

use rusqlite::Connection;

fn tracked_run(directory: &Path, ledger: &Path, args: &[&str]) -> (String, u64) {
    let output = Command::new(env!("CARGO_BIN_EXE_rtk")) // 0.11.2
        .args(args)
        .current_dir(directory)
        .env("RTK_DB_PATH", ledger)
        .env("RTK_TRACKING_DISABLED", "0")
        .env_remove("HZR_INTERNAL_ACCOUNTING_RECEIPT_JOURNAL")
        .env_remove("HZR_INTERNAL_ACCOUNTING_FAILURE_JOURNAL")
        .env_remove("HZR_INTERNAL_ACCOUNTING_CORRELATION")
        .output()
        .expect("run rtk");
    let baseline = Connection::open(ledger) // 0.11.2
        .expect("ledger")
        .query_row(
            "SELECT input_tokens FROM commands ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("baseline");
    (String::from_utf8_lossy(&output.stdout).into_owned(), baseline) // 0.11.2
}

fn raw_tokens(program: &str, args: &[&str], directory: &Path) -> u64 {
    let output = Command::new(program).args(args).current_dir(directory).output().expect("raw"); // 0.11.2
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ); // 0.11.2
    text.len().div_ceil(4) as u64 // 0.11.2
}

fn assert_close(recorded: u64, raw: u64) {
    let slack = (raw / 10).max(2); // 0.11.2
    assert!(recorded.abs_diff(raw) <= slack, "baseline {recorded} vs raw {raw} tokens"); // 0.11.2
}

#[test]
fn ls_baseline_is_the_plain_listing_and_long_format_keeps_its_columns() {
    let directory = tempfile::tempdir().expect("tempdir"); // 0.11.2
    let work = directory.path().join("work"); // 0.11.2
    fs::create_dir_all(work.join("src")).expect("src"); // 0.11.2
    fs::write(work.join("main.rs"), "fn main() {}\n").expect("file"); // 0.11.2
    fs::write(work.join("README.md"), "# readme\n").expect("file"); // 0.11.2
    let ledger = directory.path().join("history.sqlite"); // 0.11.2

    let (_, plain) = tracked_run(&work, &ledger, &["ls"]); // 0.11.2
    assert_close(plain, raw_tokens("ls", &[], &work)); // 0.11.2

    let (long_out, long) = tracked_run(&work, &ledger, &["ls", "-l"]); // 0.11.2
    assert_close(long, raw_tokens("ls", &["-l"], &work)); // 0.11.2
    assert!(long_out.contains("-rw-r--r--") || long_out.contains("-rw-"), "{long_out}"); // 0.11.2
    let (all_out, _) = tracked_run(&work, &ledger, &["ls", "-la"]); // 0.11.2
    let owner = String::from_utf8(Command::new("id").arg("-un").output().expect("id").stdout)
        .expect("user"); // 0.11.2
    assert!(all_out.contains(owner.trim()), "owner column dropped:\n{all_out}"); // 0.11.2
    assert!(all_out.lines().any(|l| l.starts_with('d') && l.ends_with(" src")), "{all_out}"); // 0.11.2
}

fn raw_stdout(program: &str, args: &[&str], directory: &Path) -> String {
    let output = Command::new(program).args(args).current_dir(directory).output().expect("raw"); // 0.11.2
    String::from_utf8(output.stdout).expect("utf-8") // 0.11.2
}

fn delivered_and_baseline(ledger: &Path) -> (u64, u64) {
    Connection::open(ledger) // 0.11.2
        .expect("ledger")
        .query_row(
            "SELECT input_tokens, output_tokens FROM commands ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("measurement")
}

// 0.11.2: in an ordinary directory the compact form (sizes, noise footer) is larger than
// the plain names, so rtk ls prints exactly what `ls` prints and records zero savings.
#[test]
fn plain_ls_of_a_small_directory_is_byte_identical_to_ls() {
    let directory = tempfile::tempdir().expect("tempdir"); // 0.11.2
    let work = directory.path().join("work"); // 0.11.2
    fs::create_dir_all(work.join("src")).expect("src"); // 0.11.2
    fs::create_dir_all(work.join("node_modules")).expect("noise"); // 0.11.2
    for name in ["main.rs", "README.md", "Cargo.toml", ".env"] {
        fs::write(work.join(name), "x\n").expect("file"); // 0.11.2
    }
    let ledger = directory.path().join("history.sqlite"); // 0.11.2
    for args in [&["ls"][..], &["ls", "-1"][..], &["ls", "-A"][..]] {
        let (stdout, _) = tracked_run(&work, &ledger, args); // 0.11.2
        assert_eq!(stdout, raw_stdout("ls", &args[1..], &work), "rtk {args:?}"); // 0.11.2
        let (baseline, delivered) = delivered_and_baseline(&ledger); // 0.11.2
        assert_eq!(baseline, delivered, "savings must be zero, not negative: {args:?}"); // 0.11.2
    }
}

#[test]
fn plain_ls_of_a_huge_directory_stays_compact_and_smaller() {
    let directory = tempfile::tempdir().expect("tempdir"); // 0.11.2
    let work = directory.path().join("work"); // 0.11.2
    fs::create_dir_all(work.join("node_modules")).expect("noise"); // 0.11.2
    fs::create_dir_all(work.join("target")).expect("noise"); // 0.11.2
    for index in 0..400 {
        fs::write(work.join(format!("generated_module_fixture_{index:04}.js")), "").expect("file"); // 0.11.2
    }
    let ledger = directory.path().join("history.sqlite"); // 0.11.2
    let (stdout, _) = tracked_run(&work, &ledger, &["ls"]); // 0.11.2
    let raw = raw_stdout("ls", &[], &work); // 0.11.2
    assert!(stdout.len() < raw.len(), "{} >= {} bytes", stdout.len(), raw.len()); // 0.11.2
    assert!(stdout.contains("more; full listing"), "{stdout}"); // 0.11.2
    assert!(stdout.contains("noise hidden: node_modules, target"), "{stdout}"); // 0.11.2
    let (baseline, delivered) = delivered_and_baseline(&ledger); // 0.11.2
    assert!(delivered < baseline, "{delivered} >= {baseline}"); // 0.11.2
}

#[test]
fn go_test_baseline_is_the_plain_text_not_the_injected_json() {
    let go_available = Command::new("go").arg("version").output().is_ok_and(|o| o.status.success()); // 0.11.2
    if !go_available {
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir"); // 0.11.2
    let module = directory.path().join("module"); // 0.11.2
    fs::create_dir_all(&module).expect("module"); // 0.11.2
    fs::write(module.join("go.mod"), "module example.com/m\n\ngo 1.21\n").expect("go.mod"); // 0.11.2
    let test = "package m\n\nimport \"testing\"\n\nfunc TestA(t *testing.T) { t.Log(\"quiet\") }\nfunc TestB(t *testing.T) {}\nfunc TestC(t *testing.T) { t.Run(\"x\", func(t *testing.T) {}) }\n"; // 0.11.2
    fs::write(module.join("m_test.go"), test).expect("test file"); // 0.11.2
    let ledger = directory.path().join("history.sqlite"); // 0.11.2
    let (_, recorded) = tracked_run(&module, &ledger, &["go", "test", "-count=1", "./..."]); // 0.11.2
    let raw = raw_tokens("go", &["test", "-count=1", "./..."], &module); // 0.11.2
    assert!(recorded.abs_diff(raw) <= 4, "baseline {recorded} vs raw {raw} tokens"); // 0.11.2
}
