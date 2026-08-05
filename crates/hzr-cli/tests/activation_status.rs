use std::process::Command;

use hzr_core::{ActivationMode, Config, EnabledWorkspace};
use serde_json::Value;
use tempfile::tempdir;

#[test]
fn activation_status_lists_enabled_workspaces_as_json() {
    let directory = tempdir().expect("temporary activation home");
    let config_path = directory.path().join("config.toml");
    let data_dir = directory.path().join("data");
    let root_a = directory.path().join("project-a");
    let root_b = directory.path().join("project-b");
    std::fs::create_dir_all(&root_a).expect("project-a");
    std::fs::create_dir_all(&root_b).expect("project-b");

    let mut config = Config {
        data_dir,
        ..Config::default()
    };
    config.activation.mode = ActivationMode::Selected;
    config.activation.enabled_workspaces = vec![
        EnabledWorkspace {
            repository_id: "a".repeat(64),
            worktree_id: "b".repeat(64),
            root: root_a.clone(),
        },
        EnabledWorkspace {
            repository_id: "c".repeat(64),
            worktree_id: "d".repeat(64),
            root: root_b.clone(),
        },
    ];
    config.write(&config_path).expect("write config");

    let output = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .args(["--config"])
        .arg(&config_path)
        .args(["activation", "status", "--json"])
        .env("HOME", directory.path())
        .output()
        .expect("run activation status");

    assert!(
        output.status.success(),
        "hzr activation status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("status JSON");
    assert_eq!(report["mode"], "selected");
    let workspaces = report["enabled_workspaces"]
        .as_array()
        .expect("enabled_workspaces array");
    assert_eq!(workspaces.len(), 2);
    assert_eq!(
        workspaces[0]["root"].as_str().expect("root a"),
        root_a.to_str().expect("utf-8 root a")
    );
    assert_eq!(
        workspaces[1]["root"].as_str().expect("root b"),
        root_b.to_str().expect("utf-8 root b")
    );
}

#[test]
fn activation_status_human_output_names_mode_and_roots() {
    let directory = tempdir().expect("temporary activation home");
    let config_path = directory.path().join("config.toml");
    let data_dir = directory.path().join("data");
    let root = directory.path().join("only-project");
    std::fs::create_dir_all(&root).expect("project");

    let mut config = Config {
        data_dir,
        ..Config::default()
    };
    config.activation.mode = ActivationMode::Selected;
    config.activation.enabled_workspaces = vec![EnabledWorkspace {
        repository_id: "e".repeat(64),
        worktree_id: "f".repeat(64),
        root: root.clone(),
    }];
    config.write(&config_path).expect("write config");

    let output = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .args(["--config"])
        .arg(&config_path)
        .args(["activation", "status"])
        .env("HOME", directory.path())
        .output()
        .expect("run activation status");

    assert!(
        output.status.success(),
        "hzr activation status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(stdout.contains("activation=selected"));
    assert!(stdout.contains(root.to_str().expect("utf-8 root")));
}
