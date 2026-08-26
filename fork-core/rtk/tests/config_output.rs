use std::fs;
use std::process::Command;

use serde_json::Value;
use tempfile::tempdir;

fn config_json(config_home: &std::path::Path) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["config", "--format", "json"])
        .env("XDG_CONFIG_HOME", config_home)
        .env("HOME", config_home)
        .output()
        .expect("run typed config");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("typed JSON output")
}

#[test]
fn config_json_is_versioned_and_reflects_file_creation_and_change() {
    let directory = tempdir().expect("temporary directory");
    let first = config_json(directory.path());
    assert_eq!(first["schema_version"], 2);
    assert_eq!(first["config_exists"], false);
    assert!(first["config_sha256"].is_null());
    assert_eq!(first["config"]["grepai"]["enabled"], true);
    let config_path = first["config_path"].as_str().expect("config path");
    assert!(std::path::Path::new(config_path).is_absolute());

    let path = std::path::PathBuf::from(config_path);
    fs::create_dir_all(path.parent().expect("config parent")).expect("config directory");
    fs::write(&path, "[grepai]\nenabled = false\nauto_init = true\n").expect("config");
    let second = config_json(directory.path());
    assert_eq!(second["config_exists"], true);
    assert_eq!(second["config_sha256"].as_str().map(str::len), Some(64));
    assert_eq!(second["config"]["grepai"]["enabled"], false);

    fs::write(&path, "[grepai]\nenabled = true\nauto_init = false\n").expect("changed config");
    let third = config_json(directory.path());
    assert_ne!(third["config_sha256"], second["config_sha256"]);
    assert_eq!(third["config"]["grepai"]["enabled"], true);
    assert_eq!(third["config"]["grepai"]["auto_init"], false);
}
