//! `hzr init` must survive a host that has no index engine installed.
//!
//! CI checks out the sources and runs `cargo test` without ever assembling a bundle, so the
//! pinned `grepai` binary is absent. Warming the index is an optional part of initialization —
//! the workspace registration is the part `init` owns — but a hard failure there took the whole
//! command down, which broke the `rust` job and would equally break the SessionStart hook on a
//! partially installed host.

use std::fs;
use std::process::Command;

use tempfile::tempdir;

#[test]
fn init_succeeds_when_the_index_engine_is_not_installed() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let data_dir = directory.path().join("data");
    let engines = directory.path().join("engines-without-binaries");
    let home = directory.path().join("home");
    for path in [&workspace, &data_dir, &engines, &home] {
        fs::create_dir_all(path).expect("fixture directory");
    }

    let config_path = directory.path().join("config.toml");
    fs::write(
        &config_path,
        format!(
            "data_dir = {data:?}\n\n[engines]\ndirectory = {engines:?}\n",
            data = data_dir,
            engines = engines,
        ),
    )
    .expect("config write");

    let status = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .args([
            "--config",
            config_path.to_str().expect("UTF-8 config path"),
            "init",
            "--if-needed",
            "--quiet",
            "--skip-service",
        ])
        .current_dir(&workspace)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .env("HZR_ALLOW_DEV_CLIENT_WRITE", "1")
        .status()
        .expect("hzr init runs");

    assert!(
        status.success(),
        "init must degrade when the index engine is absent, not fail the command"
    );
}

#[test]
fn acceptance_gate_local_instruction_scope_never_mutates_shared_repository_rules() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let data_dir = directory.path().join("data");
    let engines = directory.path().join("engines-without-binaries");
    let home = directory.path().join("home");
    for path in [&workspace, &data_dir, &engines, &home] {
        fs::create_dir_all(path).expect("fixture directory");
    }
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&workspace)
            .status()
            .expect("git init")
            .success()
    );
    let shared_agents = b"# Team agent policy\nNever replace this shared file.\n";
    let shared_claude = b"# Team Claude policy\nKeep this repository rule.\n";
    fs::write(workspace.join("AGENTS.md"), shared_agents).expect("shared AGENTS");
    fs::write(workspace.join("CLAUDE.md"), shared_claude).expect("shared CLAUDE");

    let config_path = directory.path().join("config.toml");
    fs::write(
        &config_path,
        format!(
            "data_dir = {data:?}\n\n[engines]\ndirectory = {engines:?}\n",
            data = data_dir,
            engines = engines,
        ),
    )
    .expect("config write");

    let status = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .args([
            "--config",
            config_path.to_str().expect("UTF-8 config path"),
            "init",
            "--force",
            "--skip-service",
            "--instruction-scope",
            "local",
        ])
        .current_dir(&workspace)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .env("HZR_ALLOW_DEV_CLIENT_WRITE", "1")
        .status()
        .expect("local init runs");
    assert!(status.success());

    assert_eq!(
        fs::read(workspace.join("AGENTS.md")).expect("AGENTS"),
        shared_agents
    );
    assert_eq!(
        fs::read(workspace.join("CLAUDE.md")).expect("CLAUDE"),
        shared_claude
    );
    let override_text =
        fs::read_to_string(workspace.join("AGENTS.override.md")).expect("Codex override");
    assert!(override_text.contains("read\n`./AGENTS.md` completely"));
    assert!(override_text.contains("hzr:begin managed agent contract"));
    assert!(workspace.join("CLAUDE.local.md").is_file());
    for local in ["AGENTS.override.md", "CLAUDE.local.md"] {
        assert!(
            Command::new("git")
                .args(["check-ignore", "--quiet", local])
                .current_dir(&workspace)
                .status()
                .expect("git check-ignore")
                .success(),
            "{local} must be machine-local through .git/info/exclude"
        );
    }
    let config = fs::read_to_string(&config_path).expect("persisted config");
    assert!(config.contains("scope = \"local\""));

    let doctor = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .args([
            "--config",
            config_path.to_str().expect("UTF-8 config path"),
            "--json",
            "doctor",
            "--workspace",
            workspace.to_str().expect("UTF-8 workspace"),
        ])
        .current_dir(&workspace)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .output()
        .expect("local doctor runs");
    let report: serde_json::Value =
        serde_json::from_slice(&doctor.stdout).expect("doctor JSON output");
    for name in [
        "workspace_claude_instructions",
        "workspace_codex_instructions",
    ] {
        let check = report["checks"]
            .as_array()
            .expect("doctor checks")
            .iter()
            .find(|check| check["name"] == name)
            .expect("doctor omitted a local instruction check");
        assert_eq!(check["status"], "pass", "doctor must audit local surfaces");
    }
}
