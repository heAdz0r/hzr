use std::process::Command;

use serde_json::Value;
use tempfile::tempdir;

fn run_init(workspace: &std::path::Path, config: &std::path::Path, arguments: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .arg("--config")
        .arg(config)
        .arg("init")
        .args(arguments)
        .args(["--skip-service", "--json"])
        .current_dir(workspace)
        .output()
        .expect("run hzr init");
    assert!(
        output.status.success(),
        "hzr init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("init JSON")
}

#[test]
fn init_is_idempotent_and_registers_the_visualizer_workspace() {
    let directory = tempdir().expect("temporary init root");
    let workspace = directory.path().join("workspace");
    let config = directory.path().join("config.toml");
    let data = directory.path().join("data");
    std::fs::create_dir(&workspace).expect("workspace directory");

    let first = run_init(
        &workspace,
        &config,
        &["--data-dir", data.to_str().expect("UTF-8 data path")],
    );
    let second = run_init(&workspace, &config, &["--if-needed"]);

    assert_eq!(first["outcome"], "initialized_without_git");
    assert_eq!(second["outcome"], "already_initialized");
    assert_eq!(first["repository_id"], second["repository_id"]);
    assert_eq!(first["worktree_id"], second["worktree_id"]);
    assert_eq!(
        first["registration"]["registered_at_ms"],
        second["registration"]["registered_at_ms"]
    );
    assert!(
        second["registration"]["last_seen_at_ms"]
            .as_u64()
            .expect("last seen timestamp")
            >= first["registration"]["last_seen_at_ms"]
                .as_u64()
                .expect("first last seen timestamp")
    );

    let repository_id = first["repository_id"].as_str().expect("repository id");
    let worktree_id = first["worktree_id"].as_str().expect("worktree id");
    let registration = data
        .join("workspaces")
        .join(repository_id)
        .join(worktree_id)
        .join("workspace.json");
    assert!(registration.is_file(), "registration is missing");
}
