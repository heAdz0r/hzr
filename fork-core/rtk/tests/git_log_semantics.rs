use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::tempdir;

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

#[test]
fn filtered_git_log_preserves_merge_commits_and_subjects_exactly() {
    let directory = tempdir().expect("temp directory");
    let repo = directory.path();
    git(repo, &["init", "-b", "main"]);
    git(repo, &["config", "user.email", "test@example.com"]);
    git(repo, &["config", "user.name", "HZR Test"]);

    fs::write(repo.join("root.txt"), "root\n").expect("write root");
    git(repo, &["add", "root.txt"]);
    git(repo, &["commit", "-m", "root"]);
    git(repo, &["checkout", "-b", "feature"]);
    fs::write(repo.join("feature.txt"), "feature\n").expect("write feature");
    git(repo, &["add", "feature.txt"]);
    git(
        repo,
        &[
            "commit",
            "-m",
            "feature subject фича 🎵 must remain byte exact",
        ],
    );
    git(repo, &["checkout", "main"]);
    fs::write(repo.join("main.txt"), "main\n").expect("write main");
    git(repo, &["add", "main.txt"]);
    git(repo, &["commit", "-m", "main"]);
    git(
        repo,
        &[
            "merge",
            "--no-ff",
            "feature",
            "-m",
            "Merge feature exact subject",
        ],
    );

    let cases: &[&[&str]] = &[
        &["log", "--oneline", "--max-count=20"],
        &["log", "--format=%H%x09%s", "--reverse", "--max-count=3"],
        &[
            "log",
            "--format=%h|%ad|%s",
            "--date=iso-strict",
            "--max-count=4",
        ],
        &["log", "--merges", "--format=%s"],
    ];
    for case in cases {
        let raw = git(repo, case);
        let filtered = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .arg("git")
            .args(*case)
            .current_dir(repo)
            .env("RTK_DB_PATH", directory.path().join("history.sqlite"))
            .output()
            .expect("run managed git log");
        assert!(
            filtered.status.success(),
            "{:?}: {}",
            case,
            String::from_utf8_lossy(&filtered.stderr)
        );
        assert_eq!(filtered.stdout, raw.stdout, "git {case:?}");
    }
    let oneline = git(repo, &["log", "--oneline", "--max-count=20"]);
    let oneline = String::from_utf8(oneline.stdout).expect("UTF-8 git log");
    assert!(oneline.contains("Merge feature exact subject"));
    assert!(oneline.contains("фича 🎵"));
}
