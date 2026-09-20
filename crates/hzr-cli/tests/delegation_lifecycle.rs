use hzr_core::Config;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn cli(config: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hzr"))
        .arg("--config")
        .arg(config)
        .args(args)
        .output()
        .expect("CLI fixture")
}

#[test]
fn settings_are_private_and_provider_selection_survives_reload() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path().canonicalize().expect("canonical fixture");
    let path = root.join("config.toml");
    let mut config = Config {
        data_dir: root.join("data"),
        ..Config::default()
    };
    config.daemon.request_timeout_ms = 12345;
    config.write(&path).expect("config");

    let selected = cli(
        &path,
        &[
            "settings",
            "delegation",
            "--provider",
            "openrouter",
            "--model",
            "vendor/chosen-worker",
            "--enabled",
            "true",
            "--max-turns",
            "7",
            "--timeout-ms",
            "30000",
        ],
    );
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );
    let loaded = Config::load(&path).expect("reload");
    assert_eq!(loaded.delegation.provider, "openrouter");
    assert_eq!(loaded.delegation.model, "vendor/chosen-worker");
    assert_eq!(loaded.delegation.max_turns, 7);
    assert_eq!(loaded.daemon.request_timeout_ms, 12345);

    let key_path = root.join("input.secret");
    fs::write(&key_path, "fixture-private-key").expect("fixture key");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).expect("private mode");
    }
    let login = cli(
        &path,
        &[
            "settings",
            "login",
            "--provider",
            "openrouter",
            "--key-file",
            key_path.to_str().expect("path"),
            "--json",
        ],
    );
    assert!(
        login.status.success(),
        "{}",
        String::from_utf8_lossy(&login.stderr)
    );
    let shown = cli(&path, &["settings", "--json"]);
    assert!(shown.status.success());
    let report: serde_json::Value = serde_json::from_slice(&shown.stdout).expect("JSON");
    assert_eq!(report["credential_configured"], true);
    for bytes in [
        &login.stdout,
        &login.stderr,
        &shown.stdout,
        &fs::read(&path).expect("config"),
    ] {
        assert!(!String::from_utf8_lossy(bytes).contains("fixture-private-key"));
    }
    let disabled = cli(&path, &["settings", "delegation", "--enabled", "false"]);
    assert!(disabled.status.success());
    let blocked = cli(&path, &["delegate", "Never send this task to a provider"]);
    assert!(!blocked.status.success());
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("delegation is disabled"));
}

#[test]
fn missing_credentials_and_invalid_limits_fail_before_daemon_or_inference() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path().canonicalize().expect("canonical fixture");
    let path = root.join("config.toml");
    let config = Config {
        data_dir: root.join("data"),
        ..Config::default()
    };
    config.write(&path).expect("config");
    let invalid = cli(&path, &["settings", "delegation", "--max-turns", "0"]);
    assert!(!invalid.status.success());
    assert_eq!(
        Config::load(&path).expect("reload").delegation.max_turns,
        12
    );
    assert!(
        cli(&path, &["settings", "delegation", "--enabled", "true"])
            .status
            .success()
    );
    let missing = cli(&path, &["delegate", "Do not make a request"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("settings login"));
}
