#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

use hzr_core::{ActivationMode, Config};
use hzr_exec::{PINNED_RTK_VERSION, expected_engine_identity};
use serde_json::{Value, json};

#[test]
fn bash_dispatch_keeps_original_permission_surface_without_a_confirmed_grant() {
    let root = tempfile::tempdir().expect("fixture");
    let workspace = root.path().join("workspace");
    let home = root.path().join("home");
    let engines = root.path().join("engines");
    for path in [&workspace, &home, &engines] {
        std::fs::create_dir(path).expect("directory");
    }
    let workspace = workspace.canonicalize().expect("canonical workspace");
    let contract = serde_json::to_string(&expected_engine_identity().expect("identity"))
        .expect("contract JSON");
    let engine = engines.join("rtk");
    std::fs::write(
        &engine,
        format!(
            r#"#!/bin/sh
case "$1 $2" in
  "--version ") printf 'rtk {PINNED_RTK_VERSION}\n';;
  "contract --json") printf '%s\n' '{contract}';;
  "rewrite --help") printf 'Usage: rtk rewrite [ARGS]... Raw command to rewrite\n';;
  "proxy --help") printf 'Usage: rtk proxy [ARGS]... Execute command without filtering\n';;
  *) test "$1" = rewrite-plan || exit 64
     cat "$PWD/plan.json";;
esac
"#
        ),
    )
    .expect("engine");
    std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o700)).expect("executable");
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").expect("port");
    let mut config = Config {
        data_dir: root.path().join("data"),
        ..Config::default()
    };
    config.daemon.bind = reservation.local_addr().expect("address");
    drop(reservation);
    config.engines.directory = Some(engines);
    config.activation.mode = ActivationMode::All;
    config.ensure_layout().expect("layout");
    let config_path = root.path().join("config.toml");
    config.write(&config_path).expect("config");

    for mode in [
        None,
        Some("auto"),
        Some("default"),
        Some("acceptEdits"),
        Some("plan"),
        Some("future-mode"),
        Some("bypassPermissions"),
    ] {
        for verdict in [
            None,
            Some("default"),
            Some("allow"),
            Some("ask"),
            Some("deny"),
        ] {
            let decision = match verdict {
                Some("ask") => "ask",
                Some("deny") => "deny",
                _ => "rewrite",
            };
            let mut plan = json!({
                "decision": decision,
                "proposed": "rtk find . -type f | tail -5; echo hi",
                "reason": "permission_policy"
            });
            if decision == "rewrite" {
                plan.as_object_mut().expect("plan").remove("reason");
            } else if decision == "deny" {
                plan.as_object_mut().expect("plan").remove("proposed");
            }
            if let Some(verdict) = verdict {
                plan["host_permission"] = json!(verdict);
            }
            std::fs::write(
                workspace.join("plan.json"),
                serde_json::to_vec(&plan).expect("plan"),
            )
            .expect("write plan");
            let mut input = json!({
                "hook_event_name": "PreToolUse", "tool_name": "Bash",
                "tool_input": {"command": "find . -type f | tail -5; echo hi",
                    "timeout": 1234, "description": "keep this field"},
                "cwd": workspace, "session_id": format!("{mode:?}-{verdict:?}")
            });
            if let Some(mode) = mode {
                input["permission_mode"] = json!(mode);
            }
            let mut child = Command::new(env!("CARGO_BIN_EXE_hzr"))
                .arg("--config")
                .arg(&config_path)
                .args(["hooks", "dispatch", "--native-mode", "steer"])
                .current_dir(&workspace)
                .env("HOME", &home)
                .env("XDG_CONFIG_HOME", home.join("xdg"))
                .env("CLAUDE_CONFIG_DIR", home.join("claude"))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("hook");
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(&serde_json::to_vec(&input).expect("input"))
                .expect("send input");
            let output = child.wait_with_output().expect("output");
            assert!(output.status.success(), "{mode:?} {verdict:?}: {output:?}");
            let response: Value = if output.stdout.is_empty() {
                json!({})
            } else {
                serde_json::from_slice(&output.stdout).expect("hook JSON")
            };
            let hook = &response["hookSpecificOutput"];
            if verdict == Some("deny") {
                assert_eq!(hook["permissionDecision"], "deny", "{response}");
            } else if verdict == Some("ask") && mode != Some("bypassPermissions") {
                assert_eq!(hook["permissionDecision"], "ask", "{response}");
            } else if verdict == Some("allow") || mode == Some("bypassPermissions") {
                assert_eq!(hook["permissionDecision"], "allow", "{response}");
                assert!(
                    hook["updatedInput"]["command"]
                        .as_str()
                        .expect("command")
                        .contains("rtk find"),
                    "{response}"
                );
                assert_eq!(hook["updatedInput"]["timeout"], 1234);
                assert_eq!(hook["updatedInput"]["description"], "keep this field");
            } else {
                assert!(
                    hook.get("updatedInput").is_none(),
                    "{mode:?} {verdict:?}: {response}"
                );
                assert!(hook.get("permissionDecision").is_none(), "{response}");
            }
        }
    }
}
