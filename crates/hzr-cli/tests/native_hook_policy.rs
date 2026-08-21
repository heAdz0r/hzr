use std::io::Write;
use std::process::{Command, Output, Stdio};

use hzr_core::{Config, FidelityAllowance, Ledger};
use serde_json::{Value, json};
use tempfile::tempdir;

fn run_hook(
    config: &std::path::Path,
    workspace: &std::path::Path,
    mode: &str,
    input: &[u8],
) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .arg("--config")
        .arg(config)
        .args(["hooks", "dispatch", "--native-mode", mode])
        .current_dir(workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hook");
    child
        .stdin
        .take()
        .expect("hook stdin")
        .write_all(input)
        .expect("write hook input");
    child.wait_with_output().expect("hook output")
}

fn run_observer(
    config: &std::path::Path,
    workspace: &std::path::Path,
    mode: &str,
    input: &[u8],
) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .arg("--config")
        .arg(config)
        .args(["hooks", "observe", "--native-mode", mode])
        .current_dir(workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn native observer");
    child
        .stdin
        .take()
        .expect("observer stdin")
        .write_all(input)
        .expect("write observer input");
    child.wait_with_output().expect("observer output")
}

#[test]
fn acceptance_gate_native_pretool_modes_are_fail_open_and_prescriptive() {
    let directory = tempdir().expect("temporary root");
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).expect("workspace");
    let config_path = directory.path().join("config.toml");
    let config = Config {
        data_dir: directory.path().join("data"),
        ..Config::default()
    };
    config.ensure_layout().expect("data layout");
    config.write(&config_path).expect("config");

    let read = serde_json::to_vec(&json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Read",
        "tool_input": {"file_path": "/work/file with spaces.md"},
        "cwd": workspace,
        "session_id": "session-native",
        "agent_type": "claude-code",
        "agent_id": "agent-private"
    }))
    .expect("read input");
    let steered = run_hook(&config_path, &workspace, "steer", &read);
    assert!(steered.status.success());
    let decision: Value = serde_json::from_slice(&steered.stdout).expect("steer decision");
    assert_eq!(
        decision.pointer("/hookSpecificOutput/permissionDecision"),
        Some(&Value::String("deny".into()))
    );
    let reason = decision
        .pointer("/hookSpecificOutput/permissionDecisionReason")
        .and_then(Value::as_str)
        .expect("replacement reason");
    assert!(reason.contains("hzr read '/work/file with spaces.md'"));
    assert!(reason.contains("session avoidable-bypass count=1"));

    let observed = run_hook(&config_path, &workspace, "observe", &read);
    assert!(observed.status.success());
    assert!(observed.stdout.is_empty());

    let glob = serde_json::to_vec(&json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Glob",
        "tool_input": {"pattern": "**/*"},
        "cwd": workspace,
        "session_id": "session-native"
    }))
    .expect("glob input");
    let glob = run_hook(&config_path, &workspace, "strict", &glob);
    assert!(glob.status.success());
    assert!(glob.stdout.is_empty(), "Glob must always be allowed");

    let allowed = serde_json::to_vec(&json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Glob",
        "tool_response": {"files": ["src/lib.rs"]},
        "cwd": workspace,
        "session_id": "session-native-allowed",
        "agent_type": "claude-code"
    }))
    .expect("allowed observer input");
    let allowed = run_observer(&config_path, &workspace, "steer", &allowed);
    assert!(allowed.status.success());
    assert!(allowed.stdout.is_empty());
    let ledger = Ledger::open(&config.data_dir.join("ledger/hzr.sqlite")).expect("ledger");
    let efficiency = ledger.efficiency_summary().expect("efficiency summary");
    assert_eq!(efficiency.native_unaccounted_operations, 0);
    let evasion = ledger
        .session_evasion_summary("session-native-allowed", FidelityAllowance::default())
        .expect("evasion summary");
    assert_eq!(
        evasion.top_class,
        Some(hzr_protocol::EvasionClass::E10CapabilityGap)
    );
    assert_eq!(evasion.avoidable_operations, 0);

    let malformed = run_hook(&config_path, &workspace, "strict", b"{");
    assert!(
        malformed.status.success(),
        "hook parse failure must fail open"
    );
    assert!(malformed.stdout.is_empty());
}
