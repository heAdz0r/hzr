//! 0.11.0 (US-006): `git diff` / `git show` / `git log` answer the caller's own
//! command, survive colour configuration, and announce RTK's limit only when it
//! actually hid something. Upstream RTK 4a2e58d, 5d3c1e5.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::{tempdir, TempDir};

fn git(cwd: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn rtk(dir: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("git")
        .args(args)
        .current_dir(dir.path())
        .env("RTK_DB_PATH", dir.path().join("history.sqlite"))
        .env("RTK_TEE", "0")
        .output()
        .expect("run rtk git")
}

/// A repository with `commits` commits, each adding `file<N>.txt`.
fn repo(commits: usize) -> TempDir {
    let dir = tempdir().expect("temp directory");
    let repo = dir.path();
    git(repo, &["init", "-b", "main"]);
    git(repo, &["config", "user.email", "test@example.com"]);
    git(repo, &["config", "user.name", "HZR Test"]);
    for i in 1..=commits {
        let name = format!("file{i}.txt");
        fs::write(repo.join(&name), format!("line {i}\n")).expect("write");
        git(repo, &["add", &name]);
        git(repo, &["commit", "-m", &format!("commit {i}")]);
    }
    dir
}

#[test]
fn log_limit_is_announced_only_when_older_commits_exist() {
    let long = repo(12);
    let out = rtk(&long, &["log"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(text.contains("10 most recent commits shown"), "{text}");
    assert_eq!(text.lines().filter(|l| l.contains("commit ")).count(), 10, "{text}");

    let explicit = rtk(&long, &["log", "-n", "3"]);
    assert!(!String::from_utf8_lossy(&explicit.stdout).contains("most recent commits"));

    let short = repo(4);
    let text = String::from_utf8_lossy(&rtk(&short, &["log"]).stdout).to_string();
    assert!(!text.contains("most recent commits"), "{text}");
    // A skip larger than the history leaves nothing past the limit.
    let skipped = rtk(&long, &["log", "--skip=5"]);
    assert!(!String::from_utf8_lossy(&skipped.stdout).contains("most recent commits"));
}

#[test]
fn show_with_oneline_and_patch_reports_the_requested_commit() {
    let dir = repo(3);
    let out = rtk(&dir, &["show", "--oneline", "-p", "HEAD~1"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("file2.txt"), "{text}");
    assert!(!text.contains("file3.txt"), "HEAD leaked into the output: {text}");
    assert_eq!(text.matches("commit 2").count(), 1, "header repeated: {text}");
}

#[test]
fn coloured_configuration_does_not_empty_the_compacted_diff() {
    let dir = repo(1);
    git(dir.path(), &["config", "color.ui", "always"]);
    fs::write(dir.path().join("file1.txt"), "changed\n").expect("write");
    for args in [&["diff"][..], &["diff", "--color=always"][..]] {
        let out = rtk(&dir, args);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success());
        assert!(!text.contains('\u{1b}'), "{args:?} kept escapes: {text:?}");
        assert!(text.contains("file1.txt"), "{args:?}: {text}");
        assert!(text.contains("+changed"), "{args:?}: {text}");
    }
}

#[test]
fn diff_failure_is_the_callers_own_error() {
    let dir = repo(1);
    let native = Command::new("git")
        .args(["diff", "-Uabc", "no-such-ref"])
        .current_dir(dir.path())
        .output()
        .expect("native git diff");
    let out = rtk(&dir, &["diff", "-Uabc", "no-such-ref"]);
    assert_eq!(out.status.code(), native.status.code());
    assert_eq!(out.stderr, native.stderr);
}

// 0.11.0 (US-013): `git show rev:path` — large UTF-8 blobs are windowed with an
// exact recovery; other encodings, trees and missing paths are git's own bytes.
#[test]
fn blob_show_windows_only_large_utf8_blobs() {
    let dir = repo(1);
    let big: String = (0..3000).map(|i| format!("line {i} of a large generated file\n")).collect();
    fs::write(dir.path().join("big.txt"), &big).unwrap();
    fs::write(dir.path().join("latin.txt"), b"caf\xe9\n".repeat(8000)).unwrap();
    fs::create_dir_all(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("sub/x.txt"), "x\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "blobs"]);

    let out = rtk(&dir, &["show", "HEAD:big.txt"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(text.starts_with("line 0 of"), "{}", &text[..40]);
    let last = text.lines().last().unwrap();
    assert!(last.contains("lines 1-400 of 3000"), "{last}");
    assert!(last.contains("hzr exec run 'git show HEAD:big.txt'"), "{last}");

    for args in [
        &["show", "HEAD:latin.txt"][..],
        &["show", "HEAD:sub"][..],
        &["show", "HEAD:missing.txt"][..],
    ] {
        let native = Command::new("git").args(args).current_dir(dir.path()).output().unwrap();
        let got = rtk(&dir, args);
        assert_eq!(got.stdout, native.stdout, "{args:?}");
        assert_eq!(got.stderr, native.stderr, "{args:?}");
        assert_eq!(got.status.code(), native.status.code(), "{args:?}");
    }
}
