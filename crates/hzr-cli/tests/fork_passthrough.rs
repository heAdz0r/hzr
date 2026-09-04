#![cfg(unix)]

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use hzr_core::{AccountingReceiptContextStore, Config};
use hzr_exec::{PINNED_RTK_VERSION, expected_engine_identity};
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};

fn fixture() -> (tempfile::TempDir, Config, std::path::PathBuf) {
    let directory = tempfile::tempdir().expect("valid supervised fork fixture");
    let engines = directory.path().join("engines");
    std::fs::create_dir(&engines).expect("valid supervised fork fixture");
    let binary = engines.join("rtk");
    let identity =
        serde_json::to_string(&expected_engine_identity().expect("valid supervised fork fixture"))
            .expect("valid supervised fork fixture");
    std::fs::write(
        &binary,
        format!(
            r#"#!/bin/sh
case "$1 $2" in
  "--version ") printf 'rtk {PINNED_RTK_VERSION}\n';;
  "contract --json") printf '%s\n' '{identity}';;
  "rewrite --help") printf 'Usage: rtk rewrite [ARGS]... Raw command to rewrite\n';;
  "proxy --help") printf 'Usage: rtk proxy [ARGS]... Execute command without filtering\n';;
  *) if test "$1" = test; then shift; exec "$@"; else exit 64; fi;;
esac
"#
        ),
    )
    .expect("valid supervised fork fixture");
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
        .expect("valid supervised fork fixture");
    let mut config = Config {
        data_dir: directory.path().join("data"),
        ..Config::default()
    };
    config.engines.directory = Some(engines);
    config.engines.auto_start_icm = false;
    config.engines.auto_index = false;
    config
        .ensure_layout()
        .expect("valid supervised fork fixture");
    let path = directory.path().join("config.toml");
    config.write(&path).expect("valid supervised fork fixture");
    (directory, config, path)
}

fn command(config_path: &std::path::Path, workspace: &std::path::Path, script: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hzr"));
    command
        .arg("--config")
        .arg(config_path)
        .args(["rtk", "--", "test", "/bin/sh", "-c", script])
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn completed_context(config: &Config) {
    let contexts = AccountingReceiptContextStore::new(&config.data_dir);
    let paths = std::fs::read_dir(config.data_dir.join("fork"))
        .expect("valid supervised fork fixture")
        .map(|entry| entry.expect("valid supervised fork fixture").path())
        .filter(|path| AccountingReceiptContextStore::is_context_path(path))
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 1);
    assert!(
        contexts
            .read(&paths[0])
            .expect("valid supervised fork fixture")
            .completed_at_unix
            .is_some()
    );
}

#[test]
fn direct_fork_preserves_stdio_exit_and_records_producer_completion() {
    let (directory, config, path) = fixture();
    let output = command(
        &path,
        directory.path(),
        "printf exact-output; printf exact-error >&2; exit 7",
    )
    .output()
    .expect("valid supervised fork fixture");
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"exact-output");
    assert_eq!(output.stderr, b"exact-error");
    completed_context(&config);
}

#[test]
fn direct_fork_forwards_termination_to_owned_group_and_waits() {
    let (directory, config, path) = fixture();
    let mut child = command(
        &path,
        directory.path(),
        "trap 'printf stopped > stopped; exit 0' TERM; printf started > started; sleep 30 & wait",
    )
    .spawn()
    .expect("valid supervised fork fixture");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !directory.path().join("started").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(directory.path().join("started").exists());
    kill(
        Pid::from_raw(i32::try_from(child.id()).expect("valid supervised fork fixture")),
        Signal::SIGTERM,
    )
    .expect("valid supervised fork fixture");
    let deadline = Instant::now() + Duration::from_secs(7);
    let status = loop {
        if let Some(status) = child.try_wait().expect("valid supervised fork fixture") {
            break Some(status);
        }
        if Instant::now() >= deadline {
            child.kill().expect("valid supervised fork fixture");
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    .expect("owned fork must finish after termination");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("valid supervised fork fixture")
        .read_to_string(&mut stderr)
        .expect("valid supervised fork fixture");
    assert!(status.success(), "{status:?}: {stderr}");
    assert_eq!(
        std::fs::read_to_string(directory.path().join("stopped"))
            .expect("valid supervised fork fixture"),
        "stopped"
    );
    completed_context(&config);
}
