#![cfg(unix)]
//! 0.11.2: a semantic backend that fails (embedder unreachable, garbage on stdout, non-zero
//! exit) must fall through to the exact ripgrep path with a one-line notice, never answer
//! with a parse error.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn run_with_failing_grepai(search_body: &str, json: bool) -> std::process::Output {
    let directory = tempfile::tempdir().expect("temporary directory"); // 0.11.2
    let workspace = directory.path().join("workspace"); // 0.11.2
    let config_root = directory.path().join("config"); // 0.11.2
    let home_root = directory.path().join("home"); // 0.11.2
    fs::create_dir_all(workspace.join("src")).expect("source directory"); // 0.11.2
    fs::create_dir_all(workspace.join(".grepai")).expect("grepai directory"); // 0.11.2
    fs::write(workspace.join(".grepai/config.yaml"), "").expect("grepai config"); // 0.11.2
    fs::write(workspace.join("src/lib.rs"), "pub fn owner_lookup() {}\n").expect("source"); // 0.11.2
    let grepai = directory.path().join("grepai"); // 0.11.2
    fs::write(
        &grepai,
        format!("#!/bin/sh\ncase \"$1\" in\n  search) {search_body} ;;\n  *) exit 0 ;;\nesac\n"),
    )
    .expect("fake grepai"); // 0.11.2
    fs::set_permissions(&grepai, fs::Permissions::from_mode(0o755)).expect("grepai mode"); // 0.11.2
    let config = format!("[grepai]\nenabled = true\nauto_init = false\nbinary_path = {grepai:?}\n"); // 0.11.2
    for directory in [
        config_root.join("rtk"),
        home_root.join(".config/rtk"),
        home_root.join("Library/Application Support/rtk"),
    ] {
        fs::create_dir_all(&directory).expect("config directory"); // 0.11.2
        fs::write(directory.join("config.toml"), &config).expect("rtk config"); // 0.11.2
    }
    let mut args = vec!["rgai", "owner_lookup"]; // 0.11.2
    if json {
        args.push("--json"); // 0.11.2
    }
    Command::new(env!("CARGO_BIN_EXE_rtk")) // 0.11.2
        .args(&args)
        .env("HOME", &home_root)
        .env("XDG_CONFIG_HOME", &config_root)
        .env_remove("RTK_QUIET")
        .current_dir(&workspace)
        .output()
        .expect("rtk rgai")
}

#[test]
fn unparseable_grepai_output_falls_back_to_exact_search() {
    let output = run_with_failing_grepai(
        "printf 'Error: embedder unreachable at http://localhost:11434\\n'",
        true,
    ); // 0.11.2
    let stdout = String::from_utf8_lossy(&output.stdout); // 0.11.2
    let stderr = String::from_utf8_lossy(&output.stderr); // 0.11.2
    assert!(output.status.success(), "{stderr}"); // 0.11.2
    assert!(!stdout.contains("parse_error"), "answered with a parse error:\n{stdout}"); // 0.11.2
    assert!(stdout.contains("lib.rs"), "exact fallback found nothing:\n{stdout}"); // 0.11.2
    assert_eq!(
        stderr.lines().filter(|line| line.contains("semantic search unavailable")).count(),
        1,
        "{stderr}"
    ); // 0.11.2
}

#[test]
fn failing_grepai_exit_falls_back_to_exact_search_with_a_notice() {
    let output = run_with_failing_grepai("echo 'embedder unreachable' >&2; exit 1", false); // 0.11.2
    let stdout = String::from_utf8_lossy(&output.stdout); // 0.11.2
    let stderr = String::from_utf8_lossy(&output.stderr); // 0.11.2
    assert!(output.status.success(), "{stderr}"); // 0.11.2
    assert!(stdout.contains("lib.rs"), "exact fallback found nothing:\n{stdout}"); // 0.11.2
    assert!(stderr.contains("semantic search unavailable"), "{stderr}"); // 0.11.2
}
