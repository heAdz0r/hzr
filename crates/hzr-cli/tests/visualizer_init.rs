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
        .env(
            "CLAUDE_CONFIG_DIR",
            config.parent().expect("config parent").join("claude"),
        )
        .env(
            "CODEX_HOME",
            config.parent().expect("config parent").join("codex"),
        )
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
    assert!(directory.path().join("claude/CLAUDE.md").is_file());
    assert!(directory.path().join("codex/AGENTS.md").is_file());
    assert_eq!(
        second["instructions"]
            .as_array()
            .expect("instruction reports")
            .iter()
            .filter(|report| report["changed"] == true)
            .count(),
        0,
        "init --if-needed must leave current managed blocks untouched"
    );
}

#[test]
fn acceptance_gate_init_repairs_stale_managed_instructions() {
    let directory = tempdir().expect("temporary init root");
    let workspace = directory.path().join("workspace");
    let config = directory.path().join("config.toml");
    let data = directory.path().join("data");
    std::fs::create_dir(&workspace).expect("workspace directory");
    let local_claude = workspace.join("CLAUDE.md");
    std::fs::write(&local_claude, "# Project rules\n\nUse `rtk read <file>`.\n")
        .expect("legacy local instructions");

    run_init(
        &workspace,
        &config,
        &["--data-dir", data.to_str().expect("UTF-8 data path")],
    );
    let migrated_local =
        std::fs::read_to_string(&local_claude).expect("migrated local instructions");
    assert!(migrated_local.contains("# Project rules"));
    assert!(migrated_local.contains("`hzr rtk -- read <file>`"));
    assert!(migrated_local.contains("<!-- hzr:begin managed agent contract"));
    let codex = directory.path().join("codex/AGENTS.md");
    let stale = std::fs::read_to_string(&codex)
        .expect("managed Codex instructions")
        .replace("raw` is forbidden", "raw` is preferred");
    std::fs::write(&codex, stale).expect("stale managed instructions");

    let repaired = run_init(&workspace, &config, &["--if-needed"]);
    assert!(
        repaired["instructions"]
            .as_array()
            .expect("instruction reports")
            .iter()
            .any(|report| report["surface"] == "codex" && report["changed"] == true),
        "init --if-needed must refresh a stale managed block"
    );
    assert!(
        std::fs::read_to_string(codex)
            .expect("repaired Codex instructions")
            .contains("raw` is forbidden")
    );
}

#[test]
fn acceptance_gate_init_repairs_instructions_before_an_index_blocker() {
    let directory = tempdir().expect("temporary init root");
    let workspace = directory.path().join("workspace");
    let config = directory.path().join("config.toml");
    let data = directory.path().join("data");
    std::fs::create_dir_all(workspace.join(".grepai")).expect("legacy root index");
    std::fs::create_dir_all(workspace.join("nested/.grepai")).expect("duplicate nested index");
    std::fs::write(workspace.join(".grepai/config.yaml"), "version: 1\n")
        .expect("legacy root config");
    std::fs::write(workspace.join("nested/.grepai/config.yaml"), "version: 1\n")
        .expect("duplicate nested config");
    let local_claude = workspace.join("CLAUDE.md");
    std::fs::write(
        &local_claude,
        "# Project rules\n\n<!-- rtk-instructions v2 -->\nUse rtk directly.\n<!-- /rtk-instructions -->\n",
    )
    .expect("legacy local instructions");

    let output = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .arg("--config")
        .arg(&config)
        .arg("init")
        .arg("--data-dir")
        .arg(&data)
        .args(["--skip-service", "--json"])
        .env("CLAUDE_CONFIG_DIR", directory.path().join("claude"))
        .env("CODEX_HOME", directory.path().join("codex"))
        .current_dir(&workspace)
        .output()
        .expect("run blocked hzr init");

    assert!(
        !output.status.success(),
        "duplicate indexes must still block init"
    );
    let migrated = std::fs::read_to_string(local_claude).expect("migrated instructions");
    assert!(migrated.contains("# Project rules"));
    assert!(!migrated.contains("rtk-instructions"));
    assert!(migrated.contains("<!-- hzr:begin managed agent contract"));
}
