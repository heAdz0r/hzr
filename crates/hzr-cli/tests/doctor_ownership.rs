use serde_json::Value;
use std::{fs, process::Command};

#[test]
fn doctor_fix_previews_then_repairs_opencode_with_backup_and_no_false_health() {
    let root = tempfile::tempdir().expect("fixture");
    let home = root.path().join("home");
    let workspace = root.path().join("project");
    fs::create_dir_all(workspace.join(".opencode")).expect("project");
    fs::create_dir_all(&home).expect("home");
    let config = root.path().join("hzr.toml");
    let data = root.path().join("data");
    fs::write(
        &config,
        format!("data_dir = {data:?}\n[engines]\nauto_start_icm = false\nauto_index = false\n"),
    )
    .expect("config");
    let client = workspace.join(".opencode/opencode.jsonc");
    let original = r#"{
      // preserved operator note
      "mcp":{"legacy":{"type":"local","command":["/fixture-not-installed/icm","serve"],"enabled":true}}
    }"#;
    fs::write(&client, original).expect("client config");
    let invoke = |dry_run: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hzr"));
        command
            .current_dir(&workspace)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("CODEX_HOME", home.join("codex"))
            .env("CLAUDE_CONFIG_PATH", home.join(".claude.json"))
            .env("CLAUDE_DESKTOP_CONFIG", home.join("desktop.json"))
            .env_remove("OPENCODE_CONFIG")
            .env_remove("OPENCODE_CONFIG_DIR")
            .args([
                "--config",
                config.to_str().expect("path"),
                "doctor",
                "--fix",
                "--json",
            ]);
        if dry_run {
            command.arg("--dry-run");
        }
        let output = command.output().expect("doctor");
        let report = serde_json::from_slice::<Value>(&output.stdout);
        assert!(
            report.is_ok(),
            "doctor did not produce JSON: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        report.expect("validated doctor JSON")
    };
    let preview = invoke(true);
    assert_eq!(fs::read_to_string(&client).expect("unmodified"), original);
    let preview_repair = &preview["client_ownership_repair"][0];
    assert_eq!(preview_repair["disabled_registrations"], 1);
    assert!(
        !std::path::Path::new(preview_repair["backup_path"].as_str().expect("backup path"))
            .exists()
    );

    let repaired = invoke(false);
    assert_eq!(
        repaired["client_ownership_repair"][0]["disabled_registrations"],
        1
    );
    let backup = repaired["client_ownership_repair"][0]["backup_path"]
        .as_str()
        .expect("backup");
    assert_eq!(
        fs::read_to_string(backup).expect("backup content"),
        original
    );
    assert!(
        fs::read_to_string(&client)
            .expect("repaired")
            .contains("// preserved operator note")
    );
    assert_eq!(
        repaired["healthy"], false,
        "missing daemon must remain visible"
    );
    assert!(repaired["accounting_gap_repair"].is_null());
    let again = invoke(false);
    assert_eq!(
        again["client_ownership_repair"][0]["disabled_registrations"],
        0
    );
    assert!(
        again["checks"]
            .as_array()
            .expect("checks")
            .iter()
            .any(|check| check["name"] == "client_mcp_ownership" && check["status"] == "pass")
    );
}
