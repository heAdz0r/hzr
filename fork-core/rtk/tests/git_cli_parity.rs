//! Exit-code parity tests for mutating `rtk git` commands.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn rtk_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
}

fn run_git(repo: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("run native git")
}

fn run_rtk_git(repo: &Path, args: &[&str]) -> Output {
    rtk_bin()
        .arg("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("run rtk git")
}

fn run_git_ok(repo: &Path, args: &[&str]) {
    let out = run_git(repo, args);
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn seed_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = dir.path();

    run_git_ok(repo, &["init", "-q"]);
    run_git_ok(repo, &["config", "user.name", "RTK Test"]);
    run_git_ok(repo, &["config", "user.email", "rtk@example.com"]);

    fs::write(repo.join("README.md"), "seed\n").expect("write seed file");
    run_git_ok(repo, &["add", "README.md"]);
    run_git_ok(repo, &["commit", "-m", "seed", "-q"]);

    dir
}

fn assert_exit_parity(repo: &Path, native_args: &[&str], rtk_args: &[&str]) {
    let native = run_git(repo, native_args);
    let rtk = run_rtk_git(repo, rtk_args);

    assert_eq!(
        native.status.code(),
        rtk.status.code(),
        "exit code mismatch\nnative git {:?}\nrtk git {:?}\n\nnative stdout:\n{}\n\nnative stderr:\n{}\n\nrtk stdout:\n{}\n\nrtk stderr:\n{}",
        native_args,
        rtk_args,
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr),
        String::from_utf8_lossy(&rtk.stdout),
        String::from_utf8_lossy(&rtk.stderr),
    );
}

fn git_stdout_ok(repo: &Path, args: &[&str]) -> String {
    let out = run_git(repo, args);
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn extract_stderr_signal(stderr: &[u8]) -> Option<String> {
    let raw = String::from_utf8_lossy(stderr);
    raw.lines().find_map(|line| {
        let l = line.trim();
        let lower = l.to_lowercase();
        if lower.starts_with("fatal:") || lower.starts_with("error:") {
            Some(lower)
        } else {
            None
        }
    })
}

fn assert_stderr_signal_parity(repo: &Path, args: &[&str]) {
    let native = run_git(repo, args);
    let rtk = run_rtk_git(repo, args);

    assert_eq!(
        native.status.code(),
        rtk.status.code(),
        "exit code mismatch for stderr parity\nargs: {:?}",
        args
    );

    if let Some(signal) = extract_stderr_signal(&native.stderr) {
        let rtk_combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&rtk.stderr),
            String::from_utf8_lossy(&rtk.stdout)
        )
        .to_lowercase();
        assert!(
            rtk_combined.contains(&signal),
            "missing stderr signal\nsignal: {}\nrtk stderr:\n{}\nrtk stdout:\n{}",
            signal,
            String::from_utf8_lossy(&rtk.stderr),
            String::from_utf8_lossy(&rtk.stdout),
        );
    }
}

fn set_file(repo: &Path, rel: &str, content: &str) {
    let path = repo.join(rel);
    fs::write(path, content).expect("write test file");
}

#[test]
fn parity_git_add_missing_path_failure() {
    let dir = seed_repo();
    assert_exit_parity(
        dir.path(),
        &["add", "__missing__.txt"],
        &["add", "__missing__.txt"],
    );
}

#[test]
fn parity_git_commit_nothing_to_commit_failure() {
    let dir = seed_repo();
    assert_exit_parity(
        dir.path(),
        &["commit", "-m", "noop"],
        &["commit", "-m", "noop"],
    );
}

#[test]
fn parity_git_push_without_remote_failure() {
    let dir = seed_repo();
    assert_exit_parity(dir.path(), &["push"], &["push"]);
}

#[test]
fn parity_git_pull_without_remote_failure() {
    let dir = seed_repo();
    assert_exit_parity(dir.path(), &["pull"], &["pull"]);
}

#[test]
fn parity_git_branch_delete_missing_branch_failure() {
    let dir = seed_repo();
    assert_exit_parity(
        dir.path(),
        &["branch", "-d", "__missing_branch__"],
        &["branch", "-d", "__missing_branch__"],
    );
}

#[test]
fn parity_git_fetch_missing_remote_failure() {
    let dir = seed_repo();
    assert_exit_parity(
        dir.path(),
        &["fetch", "__missing_remote__"],
        &["fetch", "__missing_remote__"],
    );
}

#[test]
fn parity_git_stash_drop_missing_stash_failure() {
    let dir = seed_repo();
    assert_exit_parity(
        dir.path(),
        &["stash", "drop", "stash@{0}"],
        &["stash", "drop", "stash@{0}"],
    );
}

#[test]
fn parity_git_worktree_remove_missing_path_failure() {
    let dir = seed_repo();
    let missing_path = dir.path().join("__missing_worktree__");
    let missing_path = missing_path.to_string_lossy().to_string();

    assert_exit_parity(
        dir.path(),
        &["worktree", "remove", &missing_path],
        &["worktree", "remove", &missing_path],
    );
}

#[test]
fn parity_git_status_outside_repo_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_exit_parity(dir.path(), &["status"], &["status"]);
}

#[test]
fn parity_git_status_with_args_outside_repo_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_exit_parity(dir.path(), &["status", "--short"], &["status", "--short"]);
}

#[test]
fn parity_git_stash_list_outside_repo_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_exit_parity(dir.path(), &["stash", "list"], &["stash", "list"]);
}

#[test]
fn parity_git_stash_show_outside_repo_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_exit_parity(dir.path(), &["stash", "show"], &["stash", "show"]);
}

#[test]
fn parity_git_worktree_list_outside_repo_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_exit_parity(dir.path(), &["worktree", "list"], &["worktree", "list"]);
}

#[test]
fn parity_git_commit_multibyte_branch_does_not_panic() {
    let dir = seed_repo();
    run_git_ok(dir.path(), &["checkout", "-q", "-b", "ветка"]);
    set_file(dir.path(), "unicode.txt", "unicode\n");
    run_git_ok(dir.path(), &["add", "unicode.txt"]);

    let output = run_rtk_git(dir.path(), &["commit", "-m", "unicode"]);
    assert!(
        output.status.success(),
        "rtk git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("ok"));
}

#[test]
fn parity_git_push_failure_stderr_signal() {
    let dir = seed_repo();
    assert_stderr_signal_parity(dir.path(), &["push"]);
}

#[test]
fn parity_git_add_success_side_effects() {
    let native = seed_repo();
    let rtk = seed_repo();
    set_file(native.path(), "new.txt", "hello\n");
    set_file(rtk.path(), "new.txt", "hello\n");

    let native_add = run_git(native.path(), &["add", "new.txt"]);
    let rtk_add = run_rtk_git(rtk.path(), &["add", "new.txt"]);
    assert!(
        native_add.status.success(),
        "{}",
        String::from_utf8_lossy(&native_add.stderr)
    );
    assert!(
        rtk_add.status.success(),
        "{}",
        String::from_utf8_lossy(&rtk_add.stderr)
    );

    let native_cached = git_stdout_ok(native.path(), &["diff", "--cached", "--name-status"]);
    let rtk_cached = git_stdout_ok(rtk.path(), &["diff", "--cached", "--name-status"]);
    assert_eq!(native_cached, rtk_cached);

    let native_status = git_stdout_ok(native.path(), &["status", "--porcelain=v1"]);
    let rtk_status = git_stdout_ok(rtk.path(), &["status", "--porcelain=v1"]);
    assert_eq!(native_status, rtk_status);
}

#[test]
fn parity_git_commit_success_side_effects() {
    let native = seed_repo();
    let rtk = seed_repo();
    set_file(native.path(), "feat.txt", "native\n");
    set_file(rtk.path(), "feat.txt", "native\n");
    run_git_ok(native.path(), &["add", "feat.txt"]);
    run_git_ok(rtk.path(), &["add", "feat.txt"]);

    let native_commit = run_git(native.path(), &["commit", "-m", "feat", "-q"]);
    let rtk_commit = run_rtk_git(rtk.path(), &["commit", "-m", "feat"]);
    assert!(
        native_commit.status.success(),
        "{}",
        String::from_utf8_lossy(&native_commit.stderr)
    );
    assert!(
        rtk_commit.status.success(),
        "{}",
        String::from_utf8_lossy(&rtk_commit.stderr)
    );

    let native_subject = git_stdout_ok(native.path(), &["log", "-1", "--pretty=%s"]);
    let rtk_subject = git_stdout_ok(rtk.path(), &["log", "-1", "--pretty=%s"]);
    assert_eq!(native_subject.trim(), rtk_subject.trim());

    let native_tree = git_stdout_ok(native.path(), &["rev-parse", "HEAD^{tree}"]);
    let rtk_tree = git_stdout_ok(rtk.path(), &["rev-parse", "HEAD^{tree}"]);
    assert_eq!(native_tree.trim(), rtk_tree.trim());

    let native_status = git_stdout_ok(native.path(), &["status", "--porcelain=v1"]);
    let rtk_status = git_stdout_ok(rtk.path(), &["status", "--porcelain=v1"]);
    assert_eq!(native_status, rtk_status);
}

#[test]
fn parity_git_stash_push_success_side_effects() {
    let native = seed_repo();
    let rtk = seed_repo();
    set_file(native.path(), "README.md", "changed\n");
    set_file(rtk.path(), "README.md", "changed\n");

    let native_stash = run_git(native.path(), &["stash", "push", "-m", "tmp"]);
    let rtk_stash = run_rtk_git(rtk.path(), &["stash", "push", "-m", "tmp"]);
    assert!(
        native_stash.status.success(),
        "{}",
        String::from_utf8_lossy(&native_stash.stderr)
    );
    assert!(
        rtk_stash.status.success(),
        "{}",
        String::from_utf8_lossy(&rtk_stash.stderr)
    );

    let native_stashes = git_stdout_ok(native.path(), &["stash", "list"]);
    let rtk_stashes = git_stdout_ok(rtk.path(), &["stash", "list"]);
    assert_eq!(native_stashes.lines().count(), rtk_stashes.lines().count());

    let native_status = git_stdout_ok(native.path(), &["status", "--porcelain=v1"]);
    let rtk_status = git_stdout_ok(rtk.path(), &["status", "--porcelain=v1"]);
    assert_eq!(native_status, rtk_status);
}
