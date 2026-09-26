//! 0.11.0 (US-008): a tool that reports only on stderr and exits non-zero must
//! reach the caller with that report and its exit code — never as a green summary.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

const MARKER: &str = "FATAL-STDERR-MARKER";

#[test]
fn stderr_only_failures_stay_visible_with_their_exit_code() {
    let dir = tempfile::tempdir().expect("temp dir");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for tool in ["go", "next", "bun", "pnpm", "ruff", "pytest", "mypy", "tsc", "cargo", "npx"] {
        let path = bin.join(tool);
        std::fs::write(&path, format!("#!/bin/sh\necho '{MARKER} {tool}' >&2\nexit 2\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let cases: &[&[&str]] = &[
        &["go", "build", "./..."], // rendered "✓ Go build: Success"
        &["next", "build"],        // rendered "Errors: 0"
        &["bun", "test"],          // rendered "✓ bun test … errors: 0"
        &["bun", "run", "build"],
        &["pnpm", "install", "react@^18"], // clap panic, then a bail! that hid pnpm's output
        &["ruff", "check", "."],
        &["pytest"],
        &["mypy", "."],
        &["tsc"],
        &["cargo", "build"],
    ];
    for case in cases {
        let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(*case)
            .current_dir(dir.path())
            .env("PATH", &path)
            .env("RTK_DB_PATH", dir.path().join("history.sqlite"))
            .env("RTK_TEE", "0")
            .output()
            .expect("run rtk");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.status.code(), Some(2), "rtk {case:?}: {text}");
        assert!(text.contains(MARKER), "rtk {case:?} lost stderr: {text}");
        assert!(!text.contains("Success"), "rtk {case:?} claimed success: {text}");
    }
}
