//! 1.1.0 (US-005): end-to-end permission matrix for `rtk rewrite-plan`.
//!
//! HZR's managed form is opaque to the host's `Bash(...)` rules, so the plan's verdict is the only
//! place an operator's deny rule meets a rewritten command. Every case below was reproduced as an
//! auto-approval on `0.50.0-fork.1` (docs/PRD_HZR_UPSTREAM_RTK_SYNC_1_1_0.md §3, P1–P9).

use std::path::Path;
use std::process::Command;

use serde_json::Value;

const SETTINGS: &str = r#"{"permissions":{
  "allow":["Bash(ls:*)","Bash(env:*)","Bash(timeout:*)","Bash(nohup:*)","Bash(bash:*)","Bash(git status:*)"],
  "deny":["Bash(git push:*)","Bash(rm -rf /)"],
  "ask":["Bash(npm publish:*)"]
}}"#;

fn plan(project: &Path, home: &Path, config_dir: &Path, command: &str) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["rewrite-plan", command])
        .current_dir(project)
        .env("HOME", home)
        .env("CLAUDE_CONFIG_DIR", config_dir)
        .env("RTK_TELEMETRY_DISABLED", "1")
        .env("RTK_TRACKING_DISABLED", "1")
        .output()
        .expect("run rtk rewrite-plan");
    assert!(output.status.success(), "{command}: {output:?}");
    serde_json::from_slice(&output.stdout).expect("plan JSON")
}

struct Fixture {
    _root: tempfile::TempDir,
    project: std::path::PathBuf,
    home: std::path::PathBuf,
    config_dir: std::path::PathBuf,
}

fn fixture(project_settings: &str, user_settings: Option<&[u8]>) -> Fixture {
    let root = tempfile::tempdir().expect("tempdir");
    let project = root.path().join("project");
    let home = root.path().join("home");
    let config_dir = root.path().join("claude");
    for dir in [&project, &home, &config_dir] {
        std::fs::create_dir_all(dir).expect("dir");
    }
    std::fs::create_dir_all(project.join(".claude")).expect(".claude");
    std::fs::write(project.join(".claude/settings.json"), project_settings).expect("settings");
    if let Some(bytes) = user_settings {
        std::fs::write(config_dir.join("settings.json"), bytes).expect("user settings");
    }
    Fixture {
        _root: root,
        project,
        home,
        config_dir,
    }
}

#[test]
fn denied_commands_are_denied_through_every_wrapper() {
    let fx = fixture(SETTINGS, None);
    for command in [
        "git push origin main",
        "timeout 30 git push",
        "env FOO=1 git push",
        "ls; timeout 5 git push",
        "HUSKY=0 git push",
        "nohup git push",
        "command git push",
        "uv run git push",
        "bash -c 'git push'",
        "env bash -c 'git push'",
        "bash -c \"bash -c 'git push'\"",
        "2>&1 git push --force",
        "ls && { rm -rf / ; }",
    ] {
        let plan = plan(&fx.project, &fx.home, &fx.config_dir, command);
        assert_eq!(plan["decision"], "deny", "{command}: {plan}");
        assert_eq!(plan["host_permission"], "deny", "{command}: {plan}");
    }
}

#[test]
fn constructs_the_gate_cannot_read_are_never_auto_allowed() {
    let fx = fixture(SETTINGS, None);
    for command in [
        r"ls -la $'\'' ; rm -rf /",
        "env bash -c 'ls'",
        "ls > out.txt",
    ] {
        let plan = plan(&fx.project, &fx.home, &fx.config_dir, command);
        assert_ne!(plan["host_permission"], "allow", "{command}: {plan}");
        assert_ne!(plan["decision"], "deny", "{command}: {plan}");
    }
}

#[test]
fn ask_rules_are_read_past_wrappers() {
    let fx = fixture(SETTINGS, None);
    let plan = plan(
        &fx.project,
        &fx.home,
        &fx.config_dir,
        "timeout 60 npm publish",
    );
    assert_eq!(plan["host_permission"], "ask", "{plan}");
}

#[test]
fn ordinary_allowed_commands_keep_their_allow() {
    let fx = fixture(SETTINGS, None);
    for command in ["ls -la", "git status", "ls 2>&1", "ls 2>/dev/null"] {
        let plan = plan(&fx.project, &fx.home, &fx.config_dir, command);
        assert_eq!(plan["host_permission"], "allow", "{command}: {plan}");
    }
}

#[test]
fn user_settings_with_a_bom_still_deny() {
    let mut bom = b"\xef\xbb\xbf".to_vec();
    bom.extend_from_slice(br#"{"permissions":{"deny":["Bash(git push:*)"]}}"#);
    let fx = fixture("{}", Some(&bom));
    let plan = plan(&fx.project, &fx.home, &fx.config_dir, "git push");
    assert_eq!(plan["decision"], "deny", "{plan}");
}

#[test]
fn unparseable_user_settings_fail_closed() {
    let fx = fixture("{}", Some(b"{not json"));
    let plan = plan(&fx.project, &fx.home, &fx.config_dir, "git status");
    assert_eq!(plan["host_permission"], "ask", "{plan}");
}
