//! 0.11.0 (US-001): `rtk diff` answers exactly what `diff(1)` answers — bytes and
//! exit code — and bounds only very large outputs, naming the exact recovery.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::tempdir;

fn native(dir: &Path, args: &[&str]) -> Output {
    Command::new("diff").args(args).current_dir(dir).output().expect("run diff")
}

fn rtk(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("diff")
        .args(args)
        .current_dir(dir)
        .env("RTK_DB_PATH", dir.join("history.sqlite"))
        .env("RTK_TEE", "0")
        .output()
        .expect("run rtk diff")
}

#[test]
fn small_diffs_match_the_native_command_exactly() {
    let dir = tempdir().expect("temp dir");
    let d = dir.path();
    fs::write(d.join("a"), "alpha\nbeta\ngamma\n").unwrap();
    fs::write(d.join("modified"), "alpha\nBETA\ngamma\n").unwrap();
    fs::write(d.join("inserted"), "x\nalpha\nbeta\ngamma\n").unwrap();
    fs::write(d.join("crlf"), "alpha\r\nbeta\r\ngamma\r\n").unwrap();
    fs::write(d.join("noeol"), "alpha\nbeta\ngamma").unwrap();
    fs::write(d.join("latin"), b"caf\xe9\nbeta\ngamma\n").unwrap();
    fs::create_dir_all(d.join("d1")).unwrap();
    fs::create_dir_all(d.join("d2")).unwrap();
    fs::write(d.join("d1/f"), "x\n").unwrap();
    fs::write(d.join("d2/f"), "y\n").unwrap();

    let cases: &[&[&str]] = &[
        &["a", "a"],
        &["a", "modified"], // was reported with both files dumped and exit 0
        &["a", "inserted"],
        &["a", "crlf"],
        &["a", "noeol"],
        &["a", "latin"],
        &["-u", "a", "modified"],
        &["-q", "a", "modified"],
        &["-r", "d1", "d2"],
        &["a", "missing"],
    ];
    for case in cases {
        let want = native(d, case);
        let got = rtk(d, case);
        assert_eq!(got.status.code(), want.status.code(), "diff {case:?}");
        assert_eq!(got.stdout, want.stdout, "diff {case:?}");
        assert_eq!(got.stderr, want.stderr, "diff {case:?}");
    }
}

#[test]
fn large_diffs_are_bounded_with_the_exact_recovery_command() {
    let dir = tempdir().expect("temp dir");
    let d = dir.path();
    let old: String = (0..1000).map(|i| format!("line {i}\n")).collect();
    let new: String = (0..1000).map(|i| format!("LINE {i}\n")).collect();
    fs::write(d.join("old"), old).unwrap();
    fs::write(d.join("new"), new).unwrap();
    let want = native(d, &["old", "new"]);
    let got = rtk(d, &["old", "new"]);
    assert_eq!(got.status.code(), Some(1));
    assert!(got.stdout.len() < want.stdout.len());
    let text = String::from_utf8_lossy(&got.stdout);
    assert!(want.stdout.starts_with(text.lines().next().unwrap().as_bytes()));
    let notice = text.lines().last().unwrap();
    assert!(notice.contains("line(s) omitted"), "{notice}");
    assert!(notice.contains("hzr exec run 'diff old new'"), "{notice}");
}
