use std::fs;
use std::process::Command;

use tempfile::tempdir;

fn command(home: &std::path::Path, workspace: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hzr"));
    command
        .current_dir(workspace)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("xdg-config"))
        .env("XDG_DATA_HOME", home.join("xdg-data"))
        .env("HZR_ALLOW_DEV_CLIENT_WRITE", "1");
    command
}

fn custom_config(data_dir: &std::path::Path, engines: &std::path::Path) -> String {
    format!(
        "data_dir = {data_dir:?}\n\n[engines]\ndirectory = {engines:?}\nauto_start_icm = false\nauto_index = false\n\n[privacy]\ntelemetry = true\n",
    )
}

fn local_instruction_config(data_dir: &std::path::Path, engines: &std::path::Path) -> String {
    format!(
        "{}\n[instructions]\nscope = \"local\"\n",
        custom_config(data_dir, engines)
    )
}

#[test]
fn init_force_preserves_existing_config_bytes() {
    let fixture = tempdir().expect("fixture");
    let workspace = fixture.path().join("workspace");
    let home = fixture.path().join("home");
    let data = fixture.path().join("data");
    let engines = fixture.path().join("engines");
    for directory in [&workspace, &home, &engines] {
        fs::create_dir_all(directory).expect("fixture directory");
    }
    let config = fixture.path().join("config.toml");
    let original = custom_config(&data, &engines);
    fs::write(&config, &original).expect("config fixture");

    let status = command(&home, &workspace)
        .args([
            "--config",
            config.to_str().expect("config path"),
            "init",
            "--force",
            "--skip-service",
        ])
        .status()
        .expect("init runs");

    assert!(status.success());
    assert_eq!(
        fs::read_to_string(config).expect("preserved config"),
        original
    );
}

#[test]
fn init_dry_run_has_zero_filesystem_mutations() {
    let fixture = tempdir().expect("fixture");
    let workspace = fixture.path().join("workspace");
    let home = fixture.path().join("home");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(&home).expect("home");
    let config = fixture.path().join("missing-config.toml");

    let output = command(&home, &workspace)
        .args([
            "--json",
            "--config",
            config.to_str().expect("config path"),
            "init",
            "--dry-run",
            "--skip-service",
        ])
        .output()
        .expect("dry run");

    assert!(output.status.success());
    assert!(!config.exists());
    assert!(fs::read_dir(&home).expect("home read").next().is_none());
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).expect("dry-run JSON");
    assert_eq!(payload["dry_run"], true);
    let mutations = payload["mutations"].as_array().expect("mutation plan");
    assert!(
        !mutations.is_empty(),
        "dry-run must expose desired mutations"
    );
    assert!(
        mutations
            .iter()
            .any(|mutation| { mutation["action"] == "create_config" })
    );
    assert!(
        mutations
            .iter()
            .any(|mutation| { mutation["action"] == "create_managed_index_placement" })
    );
}

#[test]
fn init_reset_requires_force_and_backs_up_existing_config() {
    let fixture = tempdir().expect("fixture");
    let workspace = fixture.path().join("workspace");
    let home = fixture.path().join("home");
    let data = fixture.path().join("data");
    let engines = fixture.path().join("engines");
    for directory in [&workspace, &home, &engines] {
        fs::create_dir_all(directory).expect("fixture directory");
    }
    let config = fixture.path().join("config.toml");
    let original = custom_config(&data, &engines);
    fs::write(&config, &original).expect("config fixture");

    let status = command(&home, &workspace)
        .args([
            "--config",
            config.to_str().expect("config path"),
            "init",
            "--force",
            "--reset",
            "--skip-service",
        ])
        .status()
        .expect("reset init runs");

    assert!(status.success());
    let backup = fs::read_dir(fixture.path())
        .expect("backup directory")
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("config.toml.hzr-backup-")
        })
        .expect("timestamped config backup");
    assert_eq!(
        fs::read_to_string(backup.path()).expect("config backup"),
        original
    );
    let backup_name = backup.file_name().to_string_lossy().into_owned();
    let parts = backup_name.split('-').collect::<Vec<_>>();
    assert!(parts.len() >= 4, "backup name carries timestamp and digest");
    assert!(
        parts[2].parse::<u128>().is_ok(),
        "backup timestamp is numeric"
    );
    assert_eq!(parts[3].len(), 12, "backup digest prefix is auditable");
    assert_ne!(fs::read_to_string(config).expect("reset config"), original);
}

#[test]
fn init_data_dir_override_preserves_unknown_toml_and_comments() {
    let fixture = tempdir().expect("fixture");
    let workspace = fixture.path().join("workspace");
    let home = fixture.path().join("home");
    let old_data = fixture.path().join("old-data");
    let new_data = fixture.path().join("new-data");
    let engines = fixture.path().join("engines");
    for directory in [&workspace, &home, &engines] {
        fs::create_dir_all(directory).expect("fixture directory");
    }
    let config = fixture.path().join("config.toml");
    let original = format!(
        "# user comment must survive\ndata_dir = {old_data:?}\n\n[engines]\ndirectory = {engines:?}\nauto_start_icm = false\nauto_index = false\n\n[user_extension]\nfuture_key = \"keep-me\" # inline user comment\n"
    );
    fs::write(&config, &original).expect("config fixture");

    let output = command(&home, &workspace)
        .args([
            "--config",
            config.to_str().expect("config path"),
            "init",
            "--force",
            "--skip-service",
            "--data-dir",
            new_data.to_str().expect("new data path"),
        ])
        .output()
        .expect("init runs");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let updated = fs::read_to_string(&config).expect("updated config");
    assert!(updated.contains("# user comment must survive"));
    assert!(updated.contains("[user_extension]"));
    assert!(updated.contains("future_key = \"keep-me\" # inline user comment"));
    assert!(updated.contains(&format!("data_dir = {new_data:?}")));
}

#[test]
fn init_rolls_back_config_instructions_index_and_registration_after_injected_failure() {
    let fixture = tempdir().expect("fixture");
    let workspace = fixture.path().join("workspace");
    let home = fixture.path().join("home");
    let old_data = fixture.path().join("old-data");
    let new_data = fixture.path().join("new-data");
    let engines = fixture.path().join("engines");
    for directory in [&workspace, &home, &engines] {
        fs::create_dir_all(directory).expect("fixture directory");
    }
    let config = fixture.path().join("config.toml");
    let original = custom_config(&old_data, &engines);
    fs::write(&config, &original).expect("config fixture");

    let output = command(&home, &workspace)
        .env("HZR_TEST_INIT_FAIL_AFTER", "after_workspace")
        .args([
            "--config",
            config.to_str().expect("config path"),
            "init",
            "--force",
            "--skip-service",
            "--data-dir",
            new_data.to_str().expect("new data path"),
        ])
        .output()
        .expect("init runs");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("injected init failure"));
    assert_eq!(
        fs::read_to_string(&config).expect("rolled back config"),
        original
    );
    assert!(!new_data.exists(), "new data layout must be removed");
    assert!(
        fs::read_dir(fixture.path())
            .expect("fixture read")
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().contains("hzr-backup")),
        "failed init must not retain a backup side effect"
    );
}

#[test]
fn init_rollback_preserves_concurrent_file_and_config_edit() {
    let fixture = tempdir().expect("fixture");
    let workspace = fixture.path().join("workspace");
    let home = fixture.path().join("home");
    let old_data = fixture.path().join("old-data");
    let new_data = fixture.path().join("new-data");
    let concurrent_file = new_data.join("memory/concurrent-user-file.txt");
    let engines = fixture.path().join("engines");
    for directory in [&workspace, &home, &engines] {
        fs::create_dir_all(directory).expect("fixture directory");
    }
    let config = fixture.path().join("config.toml");
    fs::write(&config, custom_config(&old_data, &engines)).expect("config fixture");

    let output = command(&home, &workspace)
        .env("HZR_TEST_INIT_FAIL_AFTER", "after_workspace")
        .env("HZR_TEST_INIT_CONCURRENT_FILE", &concurrent_file)
        .env("HZR_TEST_INIT_CONCURRENT_EDIT", &config)
        .args([
            "--config",
            config.to_str().expect("config path"),
            "init",
            "--force",
            "--skip-service",
            "--data-dir",
            new_data.to_str().expect("new data path"),
        ])
        .output()
        .expect("init runs");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("rollback did not fully restore filesystem state"));
    assert_eq!(
        fs::read_to_string(&concurrent_file).expect("concurrent file preserved"),
        "concurrent-user-file\n"
    );
    let config_after = fs::read_to_string(&config).expect("concurrent config preserved");
    assert!(config_after.contains("# concurrent-user-edit"));
    assert!(config_after.contains(&format!("data_dir = {new_data:?}")));
}

#[test]
fn init_failure_does_not_rewrite_existing_index_contents() {
    let fixture = tempdir().expect("fixture");
    let workspace = fixture.path().join("workspace");
    let home = fixture.path().join("home");
    let data = fixture.path().join("data");
    let engines = fixture.path().join("engines");
    for directory in [&workspace, &home, &engines] {
        fs::create_dir_all(directory).expect("fixture directory");
    }
    let config = fixture.path().join("config.toml");
    fs::write(&config, custom_config(&data, &engines)).expect("config fixture");

    let first = command(&home, &workspace)
        .args([
            "--config",
            config.to_str().expect("config path"),
            "init",
            "--force",
            "--skip-service",
        ])
        .output()
        .expect("first init");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let managed_index = fs::canonicalize(workspace.join(".grepai")).expect("managed index");
    let sentinel = managed_index.join("existing-user-engine-state.bin");
    fs::write(&sentinel, b"do-not-touch").expect("existing index state");

    let failed = command(&home, &workspace)
        .env("HZR_TEST_INIT_FAIL_AFTER", "after_workspace")
        .args([
            "--config",
            config.to_str().expect("config path"),
            "init",
            "--force",
            "--skip-service",
        ])
        .output()
        .expect("failing init");

    assert!(!failed.status.success());
    assert_eq!(
        fs::read(&sentinel).expect("existing state preserved"),
        b"do-not-touch"
    );
    assert_eq!(
        fs::canonicalize(workspace.join(".grepai")).expect("placement preserved"),
        managed_index
    );
}

#[test]
fn concurrent_init_is_serialized_for_one_workspace_and_config() {
    let fixture = tempdir().expect("fixture");
    let workspace = fixture.path().join("workspace");
    let home = fixture.path().join("home");
    let data = fixture.path().join("data");
    let engines = fixture.path().join("engines");
    for directory in [&workspace, &home, &engines] {
        fs::create_dir_all(directory).expect("fixture directory");
    }
    let config = fixture.path().join("config.toml");
    let original = custom_config(&data, &engines);
    fs::write(&config, &original).expect("config fixture");

    let args = [
        "--config",
        config.to_str().expect("config path"),
        "init",
        "--force",
        "--skip-service",
    ];
    let mut first = command(&home, &workspace)
        .args(args)
        .spawn()
        .expect("first init");
    let mut second = command(&home, &workspace)
        .args(args)
        .spawn()
        .expect("second init");
    let first_status = first.wait().expect("first status");
    let second_status = second.wait().expect("second status");

    assert!(first_status.success());
    assert!(second_status.success());
    assert_eq!(
        fs::read_to_string(&config).expect("config preserved"),
        original
    );
    assert!(workspace.join(".grepai").exists());
}

#[test]
fn init_in_git_workspace_leaves_no_lock_or_backup_residue() {
    let fixture = tempdir().expect("fixture");
    let workspace = fixture.path().join("workspace");
    let home = fixture.path().join("home");
    let data = fixture.path().join("data");
    let engines = fixture.path().join("engines");
    for directory in [&workspace, &home, &engines] {
        fs::create_dir_all(directory).expect("fixture directory");
    }
    fs::write(workspace.join("AGENTS.md"), "# User Codex rules\n").expect("AGENTS fixture");
    fs::write(workspace.join("CLAUDE.md"), "# User Claude rules\n").expect("CLAUDE fixture");
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&workspace)
            .args(args)
            .status()
            .expect("git command")
    };
    assert!(git(&["init", "-q"]).success());
    assert!(git(&["add", "AGENTS.md", "CLAUDE.md"]).success());
    let config = fixture.path().join("config.toml");
    fs::write(&config, custom_config(&data, &engines)).expect("config fixture");

    let output = command(&home, &workspace)
        .args([
            "--config",
            config.to_str().expect("config path"),
            "init",
            "--force",
            "--skip-service",
        ])
        .output()
        .expect("init runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = Command::new("git")
        .current_dir(&workspace)
        .args(["status", "--porcelain"])
        .output()
        .expect("git status");
    let status = String::from_utf8(status.stdout).expect("UTF-8 git status");
    assert!(status.contains("AGENTS.md"));
    assert!(status.contains("CLAUDE.md"));
    assert!(status.contains(".codex"));
    assert!(!status.contains("hzr-backup"), "{status}");
    assert!(!status.contains("hzr.lock"), "{status}");
}

#[test]
fn if_needed_reconciles_project_mcp_with_instruction_rollback() {
    let fixture = tempdir().expect("fixture");
    let workspace = fixture.path().join("workspace");
    let home = fixture.path().join("home");
    let data = fixture.path().join("data");
    let engines = fixture.path().join("engines");
    for directory in [&workspace, &home, &engines] {
        fs::create_dir_all(directory).expect("fixture directory");
    }
    let original = "# User rules stay exact\n";
    fs::write(workspace.join("AGENTS.md"), original).expect("AGENTS fixture");
    let config = fixture.path().join("config.toml");
    fs::write(&config, custom_config(&data, &engines)).expect("config fixture");

    let failed = command(&home, &workspace)
        .env("HZR_TEST_INIT_FAIL_AFTER", "after_session_mcp")
        .args([
            "--config",
            config.to_str().expect("config path"),
            "init",
            "--if-needed",
            "--quiet",
            "--skip-service",
        ])
        .output()
        .expect("failing if-needed");
    assert!(!failed.status.success());
    assert_eq!(
        fs::read_to_string(workspace.join("AGENTS.md")).expect("rolled back instructions"),
        original
    );
    assert!(
        !workspace.join(".codex/config.toml").exists(),
        "project MCP must roll back with the managed instructions"
    );

    let recovered = command(&home, &workspace)
        .args([
            "--config",
            config.to_str().expect("config path"),
            "init",
            "--if-needed",
            "--quiet",
            "--skip-service",
        ])
        .output()
        .expect("recovered if-needed");
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    let project_mcp =
        fs::read_to_string(workspace.join(".codex/config.toml")).expect("project MCP registration");
    assert!(project_mcp.contains("[mcp_servers.hzr]"));
    assert!(project_mcp.contains(workspace.to_str().expect("workspace UTF-8")));
}

#[test]
fn activation_failure_restores_config_workspace_and_backup_artifacts() {
    let fixture = tempdir().expect("fixture");
    let workspace = fixture.path().join("workspace");
    let home = fixture.path().join("home");
    let data = fixture.path().join("data");
    let engines = fixture.path().join("engines");
    for directory in [&workspace, &home, &engines] {
        fs::create_dir_all(directory).expect("fixture directory");
    }
    let git = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&workspace)
        .status()
        .expect("git init");
    assert!(git.success());
    let exclude = workspace.join(".git/info/exclude");
    fs::write(&exclude, "user-pattern\n").expect("user exclude");
    let config = fixture.path().join("config.toml");
    let original_config = local_instruction_config(&data, &engines);
    fs::write(&config, &original_config).expect("config fixture");

    let failed = command(&home, &workspace)
        .env("HZR_TEST_ACTIVATION_FAIL_AFTER", "project_mcp")
        .args([
            "--config",
            config.to_str().expect("config path"),
            "enable",
            "--workspace",
            workspace.to_str().expect("workspace path"),
        ])
        .output()
        .expect("failing activation");

    assert!(!failed.status.success());
    assert_eq!(
        fs::read_to_string(&config).expect("rolled back config"),
        original_config
    );
    assert_eq!(
        fs::read_to_string(&exclude).expect("rolled back exclude"),
        "user-pattern\n"
    );
    assert!(!workspace.join(".grepai").exists());
    assert!(!workspace.join("CLAUDE.local.md").exists());
    assert!(!workspace.join("AGENTS.override.md").exists());
    assert!(!workspace.join(".codex/config.toml").exists());
    assert!(!workspace.join(".codex").exists());
    assert!(
        !data.exists(),
        "rollback residue: {:?}",
        fs::read_dir(&data)
            .map(|entries| {
                entries
                    .map(|entry| entry.expect("data entry").path())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    );
    assert!(
        fs::read_dir(&workspace)
            .expect("workspace entries")
            .all(|entry| !entry
                .expect("workspace entry")
                .file_name()
                .to_string_lossy()
                .contains("hzr-backup"))
    );
    assert!(
        fs::read_dir(exclude.parent().expect("exclude parent"))
            .expect("exclude entries")
            .all(|entry| !entry
                .expect("exclude entry")
                .file_name()
                .to_string_lossy()
                .contains("hzr-backup"))
    );
}

#[test]
fn concurrent_workspace_activation_retains_both_roots() {
    let fixture = tempdir().expect("fixture");
    let first = fixture.path().join("first");
    let second = fixture.path().join("second");
    let home = fixture.path().join("home");
    let data = fixture.path().join("data");
    let engines = fixture.path().join("engines");
    for directory in [&first, &second, &home, &engines] {
        fs::create_dir_all(directory).expect("fixture directory");
    }
    let config = fixture.path().join("config.toml");
    fs::write(&config, custom_config(&data, &engines)).expect("config fixture");

    let mut first_enable = command(&home, &first)
        .args([
            "--config",
            config.to_str().expect("config path"),
            "enable",
            "--workspace",
            first.to_str().expect("first workspace"),
        ])
        .spawn()
        .expect("first activation");
    let mut second_enable = command(&home, &second)
        .args([
            "--config",
            config.to_str().expect("config path"),
            "enable",
            "--workspace",
            second.to_str().expect("second workspace"),
        ])
        .spawn()
        .expect("second activation");

    assert!(first_enable.wait().expect("first status").success());
    assert!(second_enable.wait().expect("second status").success());
    let config = fs::read_to_string(config).expect("activation config");
    assert!(config.contains(first.to_str().expect("first UTF-8")));
    assert!(config.contains(second.to_str().expect("second UTF-8")));
}

#[test]
fn uninstall_cleans_every_registered_workspace_from_unrelated_cwd() {
    let fixture = tempdir().expect("fixture");
    let first = fixture.path().join("first");
    let second = fixture.path().join("second");
    let home = fixture.path().join("home");
    let data = fixture.path().join("data");
    let engines = fixture.path().join("engines");
    for directory in [&first, &second, &home, &engines] {
        fs::create_dir_all(directory).expect("fixture directory");
    }
    let config = fixture.path().join("config.toml");
    fs::write(&config, custom_config(&data, &engines)).expect("config fixture");
    for workspace in [&first, &second] {
        fs::write(workspace.join("AGENTS.md"), "user codex rule\n").expect("user AGENTS");
        fs::write(workspace.join("CLAUDE.md"), "user claude rule\n").expect("user CLAUDE");
        let output = command(&home, workspace)
            .args([
                "--config",
                config.to_str().expect("config path"),
                "enable",
                "--workspace",
                workspace.to_str().expect("workspace path"),
            ])
            .output()
            .expect("workspace activation");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = command(&home, fixture.path())
        .args([
            "--config",
            config.to_str().expect("config path"),
            "uninstall",
            "--force",
        ])
        .output()
        .expect("uninstall");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for workspace in [&first, &second] {
        let agents = fs::read_to_string(workspace.join("AGENTS.md")).unwrap_or_default();
        let claude = fs::read_to_string(workspace.join("CLAUDE.md")).unwrap_or_default();
        let project_mcp =
            fs::read_to_string(workspace.join(".codex/config.toml")).unwrap_or_default();
        assert_eq!(agents, "user codex rule\n");
        assert_eq!(claude, "user claude rule\n");
        assert!(!project_mcp.contains("[mcp_servers.hzr]"));
    }
}
