//! Exit-code and stream parity tests for `rtk git` commands.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn accounted_diff_command(repo: &Path, args: &[&str], journal: &Path) -> Command {
    let mut command = rtk_bin();
    command.arg("git").args(args).current_dir(repo)
        .env("RTK_TRACKING_DISABLED", "0")
        .env("RTK_DB_PATH", repo.join("forbidden-diff-history.sqlite"))
        .env("HZR_INTERNAL_ACCOUNTING_RECEIPT_JOURNAL", journal)
        .env("HZR_INTERNAL_ACCOUNTING_CORRELATION", "0123456789abcdef0123456789abcdef");
    command
}

fn assert_exact_diff_receipt(journal: &Path) {
    use hzr_engine_contract::{AccountingMeasurement, AccountingStage, EngineAccountingReceipt};
    let encoded = fs::read_to_string(journal).expect("receipt before failure return");
    assert_eq!(encoded.lines().count(), 1, "one invocation receipt");
    let receipt: EngineAccountingReceipt = serde_json::from_str(encoded.trim()).expect("typed receipt");
    assert_eq!(receipt.attribution.stage, AccountingStage::InternalTransport);
    assert_eq!(receipt.measurement, AccountingMeasurement::Estimated);
    assert_eq!(receipt.baseline_tokens, receipt.delivered_tokens, "exact output must not claim reduction");
}

#[test]
fn parity_git_diff_status_modes_preserve_native_streams_and_failure_receipts() {
    let directory = seed_repo();
    let repo = directory.path();
    set_file(repo, "README.md", "changed with trailing whitespace \n\n");
    run_git_ok(repo, &["add", "README.md"]);
    let cases: &[&[&str]] = &[
        &["diff", "--cached", "--check"],
        &["diff", "--cached", "--exit-code"],
        &["diff", "--cached", "--quiet"],
        &["diff", "--cached", "--stat", "--check"],
        &["diff", "__HZR_nonexistent_revision__", "--"],
    ];
    for (index, args) in cases.iter().enumerate() {
        let native = run_git(repo, args);
        assert!(!native.status.success(), "fixture must expose nonzero status: {args:?}");
        let journal = repo.join(format!("receipt-{index}.jsonl"));
        let filtered = accounted_diff_command(repo, args, &journal).output().expect("rtk diff");
        assert_eq!(filtered.status.code(), native.status.code(), "{args:?}");
        assert_eq!(filtered.stdout, native.stdout, "stdout {args:?}");
        assert_eq!(filtered.stderr, native.stderr, "stderr {args:?}");
        assert_exact_diff_receipt(&journal);
    }
    assert!(!repo.join("forbidden-diff-history.sqlite").exists());
}

#[test]
fn parity_git_diff_no_compact_is_exact_and_quiet_success_emits_no_text() {
    let directory = seed_repo();
    let repo = directory.path();
    let quiet_receipt = repo.join("quiet-receipt.jsonl");
    let quiet = accounted_diff_command(repo, &["diff", "--quiet"], &quiet_receipt).output().expect("quiet");
    assert!(quiet.status.success());
    assert!(quiet.stdout.is_empty() && quiet.stderr.is_empty());
    assert_exact_diff_receipt(&quiet_receipt);

    set_file(repo, "README.md", "changed\n");
    let native = run_git(repo, &["diff"]);
    let journal = repo.join("exact-receipt.jsonl");
    let exact = accounted_diff_command(repo, &["diff", "--no-compact"], &journal).output().expect("exact");
    assert_eq!(exact.status.code(), native.status.code());
    assert_eq!(exact.stdout, native.stdout);
    assert_eq!(exact.stderr, native.stderr);
    assert_exact_diff_receipt(&journal);
}

#[cfg(unix)]
#[test]
fn parity_git_diff_signal_runs_once_preserves_partial_bytes_and_records_receipt() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().expect("fixture");
    let repo = directory.path();
    let bin = repo.join("bin");
    fs::create_dir(&bin).expect("bin");
    let git = bin.join("git");
    fs::write(&git, "#!/bin/sh\nprintf 'call\\n' >> \"$HZR_GIT_DIFF_FIXTURE_CALLS\"\nprintf 'partial stdout'\nprintf 'partial stderr' >&2\nkill -TERM $$\n").expect("git fixture");
    fs::set_permissions(&git, fs::Permissions::from_mode(0o700)).expect("executable");
    let mut paths = vec![bin];
    if let Some(existing) = std::env::var_os("PATH") { paths.extend(std::env::split_paths(&existing)); }
    let path = std::env::join_paths(paths).expect("fixture PATH");
    for (index, args) in [&["diff", "--check"][..], &["diff", "--quiet"], &["diff", "--exit-code"], &["diff"]].iter().enumerate() {
        let calls = repo.join(format!("calls-{index}"));
        let journal = repo.join(format!("receipt-{index}.jsonl"));
        let output = accounted_diff_command(repo, args, &journal)
            .env("PATH", &path).env("HZR_GIT_DIFF_FIXTURE_CALLS", &calls)
            .output().expect("signal fixture");
        assert_eq!(output.status.code(), Some(143), "SIGTERM maps to shell status");
        assert_eq!(output.stdout, b"partial stdout");
        assert_eq!(output.stderr, b"partial stderr");
        assert_eq!(fs::read_to_string(calls).expect("calls"), "call\n", "one Git invocation");
        assert_exact_diff_receipt(&journal);
    }
}


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

#[test]
fn blame_default_groups_ranges_below_ten_percent_of_porcelain() {
    let dir = seed_repo();
    let repo = dir.path();
    let content = (1..=100)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    set_file(repo, "README.md", &content);
    run_git_ok(repo, &["add", "README.md"]);
    run_git_ok(repo, &["commit", "-m", "bulk", "-q"]);

    let native = git_stdout_ok(repo, &["blame", "--line-porcelain", "README.md"]);
    let output = run_rtk_git(repo, &["blame", "README.md"]);
    assert!(output.status.success());
    let compact = String::from_utf8_lossy(&output.stdout);
    let commit = git_stdout_ok(repo, &["rev-parse", "HEAD"]);

    assert!(compact.starts_with("1-100 | "));
    assert!(compact.contains(commit.trim()));
    assert!(compact.contains("| RTK Test |"));
    assert!(compact.contains("| bulk"));
    assert!(compact.len() * 10 <= native.len());
}

#[test]
fn blame_explicit_fidelity_preserves_line_porcelain() {
    let dir = seed_repo();
    let repo = dir.path();
    let native = run_git(repo, &["blame", "--line-porcelain", "README.md"]);
    let managed = rtk_bin()
        .args(["git", "blame", "--line-porcelain", "README.md"])
        .env("HZR_RAW_FIDELITY", "1")
        .env("HZR_RAW_FIDELITY_REASON", "verbatim_source")
        .current_dir(repo)
        .output()
        .expect("run exact rtk git blame");

    assert_eq!(managed.status.code(), native.status.code());
    assert_eq!(managed.stdout, native.stdout);
}

#[test]
fn blame_incremental_requires_explicit_fidelity() {
    let dir = seed_repo();
    let output = run_rtk_git(dir.path(), &["blame", "--incremental", "README.md"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("verbatim_source"));
}

#[test]
fn blame_marker_without_closed_reason_is_refused() {
    let dir = seed_repo();
    let output = rtk_bin()
        .args(["git", "blame", "--line-porcelain", "README.md"])
        .env("HZR_RAW_FIDELITY", "1")
        .current_dir(dir.path())
        .output()
        .expect("reject reasonless exact blame");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("closed HZR_RAW_FIDELITY_REASON"));
}
