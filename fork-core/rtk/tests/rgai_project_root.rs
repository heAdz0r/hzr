#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn explicit_project_root_prevents_nested_grepai_auto_init() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let source = workspace.join("src");
    let config_root = directory.path().join("config");
    let home_root = directory.path().join("home");
    let fake_grepai_used = directory.path().join("fake-grepai-used");
    fs::create_dir_all(&source).expect("source directory");
    fs::write(source.join("lib.rs"), "pub fn owner() {}\n").expect("source fixture");

    let grepai = directory.path().join("grepai");
    fs::write(
        &grepai,
        "#!/bin/sh\n: > \"${FAKE_GREPAI_USED:?}\"\ncase \"$1\" in\n  init) mkdir -p .grepai; : > .grepai/config.yaml ;;\n  watch) exit 0 ;;\n  search) printf '[]\\n' ;;\n  *) exit 64 ;;\nesac\n",
    )
    .expect("fake grepai");
    fs::set_permissions(&grepai, fs::Permissions::from_mode(0o755)).expect("grepai mode");
    let config = format!(
        "[grepai]\nenabled = true\nauto_init = true\nbinary_path = {:?}\n",
        grepai
    );
    for directory in [
        config_root.join("rtk"),
        home_root.join(".config/rtk"),
        home_root.join("Library/Application Support/rtk"),
    ] {
        fs::create_dir_all(&directory).expect("config directory");
        fs::write(directory.join("config.toml"), &config).expect("rtk config");
    }

    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args([
            "rgai",
            "workspace owner",
            "--path",
            "src",
            "--project-root",
            workspace.to_str().expect("UTF-8 workspace"),
            "--json",
        ])
        .env("HOME", &home_root)
        .env("XDG_CONFIG_HOME", &config_root)
        .env("FAKE_GREPAI_USED", &fake_grepai_used)
        .current_dir(&workspace)
        .output()
        .expect("rtk rgai");

    assert!(
        output.status.success(),
        "rtk failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fake_grepai_used.is_file(), "test must use the fake grepai");
    assert!(workspace.join(".grepai/config.yaml").is_file());
    assert!(
        !source.join(".grepai").exists(),
        "--path must never become the grepai index owner"
    );
}
