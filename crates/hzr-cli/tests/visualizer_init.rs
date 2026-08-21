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

    // The first outcome depends on whether the pinned index engine is installed: a bundle warms
    // the index, a bare source checkout (CI) only registers the workspace. Both are correct, and
    // pinning one of them made this test pass or fail on host state rather than on behavior.
    assert!(
        matches!(
            first["outcome"].as_str(),
            Some("index_initialized" | "repository_graph_enabled" | "initialized")
        ),
        "unexpected first init outcome: {}",
        first["outcome"]
    );
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
    assert!(workspace.join("CLAUDE.md").is_file());
    assert!(workspace.join("AGENTS.md").is_file());
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
    assert!(migrated_local.contains("`hzr read <file>`"));
    assert!(migrated_local.contains("<!-- hzr:begin managed agent contract"));
    let local_codex =
        std::fs::read_to_string(workspace.join("AGENTS.md")).expect("local Codex instructions");
    assert!(local_codex.contains("<!-- hzr:begin managed agent contract"));
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
fn acceptance_gate_legacy_init_requires_explicit_migration_without_registration() {
    let directory = tempdir().expect("temporary init root");
    let workspace = directory.path().join("workspace");
    let config = directory.path().join("config.toml");
    let data = directory.path().join("data");
    let legacy_index = workspace.join(".grepai");
    std::fs::create_dir_all(&legacy_index).expect("legacy index");
    let legacy_index = legacy_index.canonicalize().expect("canonical legacy index");
    std::fs::write(legacy_index.join("config.yaml"), "version: 1\n").expect("legacy index config");
    std::fs::write(workspace.join("CLAUDE.md"), "# Claude project rules\n")
        .expect("Claude project rules");
    std::fs::write(workspace.join("AGENTS.md"), "# Codex project rules\n")
        .expect("Codex project rules");

    let initialized = run_init(
        &workspace,
        &config,
        &["--data-dir", data.to_str().expect("UTF-8 data path")],
    );
    let forced = run_init(
        &workspace,
        &config,
        &[
            "--force",
            "--data-dir",
            data.to_str().expect("UTF-8 data path"),
        ],
    );
    let if_needed = run_init(&workspace, &config, &["--if-needed"]);

    for outcome in [&initialized, &forced, &if_needed] {
        assert_eq!(outcome["outcome"], "migration_required");
        assert_eq!(outcome["changed"], false);
        assert!(outcome["registration"].is_null());
        assert_eq!(
            outcome["index"].as_str().expect("legacy index path"),
            legacy_index.to_str().expect("UTF-8 legacy index path")
        );
    }
    assert!(legacy_index.is_dir());
    assert!(
        !std::fs::symlink_metadata(&legacy_index)
            .expect("legacy index metadata")
            .file_type()
            .is_symlink()
    );
    assert!(
        std::fs::read_to_string(workspace.join("CLAUDE.md"))
            .expect("managed Claude instructions")
            .contains("# Claude project rules")
    );
    assert!(
        std::fs::read_to_string(workspace.join("AGENTS.md"))
            .expect("managed Codex instructions")
            .contains("# Codex project rules")
    );

    let registration = data
        .join("workspaces")
        .join(
            initialized["repository_id"]
                .as_str()
                .expect("repository id"),
        )
        .join(initialized["worktree_id"].as_str().expect("worktree id"))
        .join("workspace.json");
    assert!(
        !registration.exists(),
        "legacy workspace must not be registered"
    );
}

#[test]
fn acceptance_gate_init_repairs_instructions_before_legacy_migration() {
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
        .expect("run legacy hzr init");

    assert!(
        output.status.success(),
        "legacy init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).expect("init JSON");
    assert_eq!(result["outcome"], "migration_required");
    assert!(result["registration"].is_null());
    let migrated = std::fs::read_to_string(local_claude).expect("migrated instructions");
    assert!(migrated.contains("# Project rules"));
    assert!(!migrated.contains("rtk-instructions"));
    assert!(migrated.contains("<!-- hzr:begin managed agent contract"));
}
