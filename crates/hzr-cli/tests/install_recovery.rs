use std::fs;
use std::process::Command;

use tempfile::tempdir;

fn install_command(
    home: &std::path::Path,
    workspace: &std::path::Path,
    config: &std::path::Path,
    prefix: &std::path::Path,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hzr"));
    command
        .current_dir(workspace)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("xdg-config"))
        .env("XDG_DATA_HOME", home.join("xdg-data"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env("CODEX_HOME", home.join("codex"))
        .env("CLAUDE_DESKTOP_CONFIG", home.join("claude-desktop.json"))
        .args([
            "--config",
            config.to_str().expect("config path"),
            "install",
            "--force",
            "--allow-dev-path",
            "--skip-service",
            "--prefix",
            prefix.to_str().expect("prefix path"),
            "--binary",
            env!("CARGO_BIN_EXE_hzr"),
            "--workspace",
            workspace.to_str().expect("workspace path"),
        ]);
    command
}

#[test]
fn every_install_stage_is_forward_recoverable() {
    for stage in [
        "config",
        "workspace",
        "prefix",
        "hooks",
        "instructions",
        "client_configs",
        "project_mcp",
        "service",
    ] {
        let fixture = tempdir().expect("fixture");
        let home = fixture.path().join("home");
        let workspace = fixture.path().join("workspace");
        let data = fixture.path().join("data");
        let prefix = fixture.path().join("bin");
        fs::create_dir_all(&home).expect("home");
        fs::create_dir_all(&workspace).expect("workspace");
        let config = fixture.path().join("config.toml");
        fs::write(
            &config,
            format!(
                "data_dir = {data:?}\n\n[engines]\nauto_start_icm = false\nauto_index = false\n"
            ),
        )
        .expect("config fixture");

        let failed = install_command(&home, &workspace, &config, &prefix)
            .env("HZR_TEST_INSTALL_FAIL_AFTER", stage)
            .output()
            .expect("injected install");
        assert!(!failed.status.success(), "{stage} must inject failure");
        let recovered = install_command(&home, &workspace, &config, &prefix)
            .output()
            .expect("recovery install");
        assert!(
            recovered.status.success(),
            "stage {stage}: {}",
            String::from_utf8_lossy(&recovered.stderr)
        );
        let journal: serde_json::Value = serde_json::from_slice(
            &fs::read(data.join("runtime/install-transaction.json")).expect("journal"),
        )
        .expect("journal JSON");
        assert_eq!(journal["state"], "complete");
        assert_eq!(
            journal["completed_stages"]
                .as_array()
                .expect("completed stages")
                .len(),
            8,
            "stage {stage} recovery must receipt every planned stage"
        );
    }
}

#[test]
fn install_rejects_corrupt_journal_and_serializes_concurrent_runs() {
    let fixture = tempdir().expect("fixture");
    let home = fixture.path().join("home");
    let workspace = fixture.path().join("workspace");
    let data = fixture.path().join("data");
    let prefix = fixture.path().join("bin");
    fs::create_dir_all(&home).expect("home");
    fs::create_dir_all(&workspace).expect("workspace");
    let config = fixture.path().join("config.toml");
    fs::write(
        &config,
        format!("data_dir = {data:?}\n\n[engines]\nauto_start_icm = false\nauto_index = false\n"),
    )
    .expect("config fixture");

    let mut first = install_command(&home, &workspace, &config, &prefix)
        .spawn()
        .expect("first install");
    let mut second = install_command(&home, &workspace, &config, &prefix)
        .spawn()
        .expect("second install");
    assert!(first.wait().expect("first status").success());
    assert!(second.wait().expect("second status").success());

    let journal_path = data.join("runtime/install-transaction.json");
    let mut journal: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal_path).expect("journal")).expect("journal JSON");
    journal["schema_version"] = serde_json::json!(999);
    fs::write(
        &journal_path,
        serde_json::to_vec_pretty(&journal).expect("journal JSON"),
    )
    .expect("corrupt journal");
    let rejected = install_command(&home, &workspace, &config, &prefix)
        .output()
        .expect("rejected install");
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("unsupported install journal schema")
    );
}

#[test]
fn completed_install_journal_moves_from_workspace_a_to_b() {
    let fixture = tempdir().expect("fixture");
    let home = fixture.path().join("home");
    let workspace_a = fixture.path().join("workspace-a");
    let workspace_b = fixture.path().join("workspace-b");
    let data = fixture.path().join("data");
    let prefix = fixture.path().join("bin");
    for path in [&home, &workspace_a, &workspace_b] {
        fs::create_dir_all(path).expect("fixture directory");
    }
    let config_a = fixture.path().join("config-a.toml");
    let config_b = fixture.path().join("config-b.toml");
    let config_text =
        format!("data_dir = {data:?}\n\n[engines]\nauto_start_icm = false\nauto_index = false\n");
    fs::write(&config_a, &config_text).expect("config A");
    fs::write(&config_b, &config_text).expect("config B");

    assert!(
        install_command(&home, &workspace_a, &config_a, &prefix)
            .status()
            .expect("install A")
            .success()
    );
    assert!(
        install_command(&home, &workspace_b, &config_b, &prefix)
            .status()
            .expect("install B")
            .success()
    );
    let journal: serde_json::Value = serde_json::from_slice(
        &fs::read(data.join("runtime/install-transaction.json")).expect("journal"),
    )
    .expect("journal JSON");
    assert_eq!(journal["state"], "complete");
    assert_eq!(journal["config_path"], config_b.to_string_lossy().as_ref());
    assert_eq!(
        journal["workspace"],
        fs::canonicalize(&workspace_b)
            .expect("canonical workspace B")
            .to_string_lossy()
            .as_ref()
    );

    let doctor = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .current_dir(&workspace_b)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("xdg-config"))
        .env("XDG_DATA_HOME", home.join("xdg-data"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env("CODEX_HOME", home.join("codex"))
        .env("CLAUDE_DESKTOP_CONFIG", home.join("claude-desktop.json"))
        .args([
            "--json",
            "--config",
            config_b.to_str().expect("config B path"),
            "doctor",
        ])
        .output()
        .expect("doctor B");
    let report: serde_json::Value = serde_json::from_slice(&doctor.stdout).expect("doctor JSON");
    let install_check = report["checks"]
        .as_array()
        .expect("doctor checks")
        .iter()
        .find(|check| check["name"] == "install_transaction")
        .expect("install transaction check");
    assert_eq!(install_check["status"], "pass");
}
