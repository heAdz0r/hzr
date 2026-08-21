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
        .status()
        .expect("hzr init runs");

    assert!(
        status.success(),
        "init must degrade when the index engine is absent, not fail the command"
    );
}
