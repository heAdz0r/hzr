#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, process::Stdio, time::Duration};

use hzr_core::{ActivationMode, Config};
use hzr_exec::{PINNED_RTK_VERSION, expected_engine_identity};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

async fn observer(
    config: &std::path::Path,
    workspace: &std::path::Path,
    home: &std::path::Path,
    input: &Value,
) -> std::process::Output {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_hzr"))
        .arg("--config")
        .arg(config)
        .args([
            "hooks",
            "observe",
            "--native-mode",
            "observe",
            "--replace-output",
        ])
        .current_dir(workspace)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("observer");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&serde_json::to_vec(input).expect("input"))
        .await
        .expect("send input");
    tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .expect("observer deadline")
        .expect("observer output")
}

#[tokio::test]
async fn post_tool_output_requires_accounted_rpc_and_durable_original_before_replacement() {
    let fixture = tempfile::tempdir().expect("fixture");
    let workspace = fixture.path().join("workspace");
    let home = fixture.path().join("home");
    let engines = fixture.path().join("engines");
    for path in [&workspace, &home, &engines] {
        fs::create_dir(path).expect("directory");
    }
    let workspace = fs::canonicalize(workspace).expect("workspace");
    let identity = expected_engine_identity().expect("identity");
    let contract = serde_json::to_string(&identity).expect("contract");
    let receipt = serde_json::to_string(&json!({
        "contract_version":identity.contract_version, "engine":identity, "correlation_id":"CORRELATION",
        "sequence":0, "occurred_at_unix_ms":0, "baseline_tokens":0, "delivered_tokens":0,
        "execution_ms":0, "measurement":"unmeasured", "route":"optimized", "host_grant_applied":false,
        "attribution":{"operation":"exec","mode":"exec_run","stage":"internal_transport"}
    })).expect("receipt");
    let (before, after) = receipt.split_once("CORRELATION").expect("correlation");
    let engine = engines.join("rtk");
    fs::write(&engine, format!(r#"#!/bin/sh
case "$1 $2" in
  "--version ") printf 'rtk {PINNED_RTK_VERSION}\n';;
  "contract --json") printf '%s\n' '{contract}';;
  "rewrite --help") printf 'Usage: rtk rewrite [ARGS]... Raw command to rewrite\n';;
  "proxy --help") printf 'Usage: rtk proxy [ARGS]... Execute command without filtering\n';;
  "pipe --filter")
    test "$#" -eq 3 && test "$3" = cargo-test || exit 64
    test "$PWD" = '{workspace}' || exit 65
    cat > observed-stdin.txt
    printf 'pipe\n' >> invocations.txt
    printf 'cargo test: 180 passed\n'
    if test ! -f no-receipt; then
      printf '%s%s%s\n' '{before}' "$HZR_INTERNAL_ACCOUNTING_CORRELATION" '{after}' >> "$HZR_INTERNAL_ACCOUNTING_RECEIPT_JOURNAL"
    fi
    ;;
  *) exit 66;;
esac
"#, workspace=workspace.display())).expect("fake engine");
    fs::set_permissions(&engine, fs::Permissions::from_mode(0o700)).expect("executable");
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").expect("port");
    let address = reservation.local_addr().expect("address");
    drop(reservation);
    let mut config = Config {
        data_dir: fixture.path().join("data"),
        ..Config::default()
    };
    config.daemon.bind = address;
    config.engines.directory = Some(engines);
    config.engines.auto_start_icm = false;
    config.engines.auto_index = false;
    config.activation.mode = ActivationMode::All;
    let config_path = fixture.path().join("config.toml");
    config.write(&config_path).expect("config");
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(hzr_daemon::serve(config.clone(), async {
        let _ = stopped.await;
    }));
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(250))
        .build()
        .expect("HTTP");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(token) = fs::read_to_string(config.data_dir.join("runtime/hzrd.token")) {
            if http
                .get(format!("http://{address}/v1/health"))
                .bearer_auth(token.trim())
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                break;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "fixture daemon readiness"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let raw = format!(
        "running 180 tests\n{}\ntest result: ok. 180 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n",
        (0..180)
            .map(|n| format!("test module::case_{n} ... ok"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let input = json!({"hook_event_name":"PostToolUse","tool_name":"Bash","cwd":workspace,"session_id":"post-fixture",
        "tool_input":{"command":"cargo test --lib"},
        "tool_response":{"stdout":raw,"stderr":"retained warning\n","interrupted":false,"isImage":false,"metadata":{"exit":"success"}}});
    let output = observer(&config_path, &workspace, &home, &input).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let replacement: Value =
        serde_json::from_slice(&output.stdout).expect("replacement JSON from real observer");
    let result = &replacement["hookSpecificOutput"]["updatedToolOutput"];
    assert_eq!(
        replacement["hookSpecificOutput"]["hookEventName"],
        "PostToolUse"
    );
    assert!(
        result["stdout"]
            .as_str()
            .expect("stdout")
            .contains("[HZR post-tool cargo-test v1")
    );
    for key in ["stderr", "interrupted", "isImage", "metadata"] {
        assert_eq!(result[key], input["tool_response"][key]);
    }
    let original = config.data_dir.join("hook-output").join(format!(
        "cargo-test-v1-{}.txt",
        hex::encode(Sha256::digest(raw.as_bytes()))
    ));
    assert_eq!(
        fs::read_to_string(&original).expect("durable original"),
        raw
    );
    assert_eq!(
        fs::read_to_string(workspace.join("observed-stdin.txt")).expect("stdin"),
        raw
    );
    assert_eq!(
        fs::read_to_string(workspace.join("invocations.txt")).expect("invocation"),
        "pipe\n"
    );

    fs::write(workspace.join("no-receipt"), "").expect("disable receipt");
    let failure = observer(&config_path, &workspace, &home, &input).await;
    assert!(failure.status.success());
    assert!(
        failure.stdout.is_empty(),
        "missing receipt must retain host output"
    );
    assert_eq!(
        fs::read_to_string(original).expect("original retained"),
        raw
    );
    assert_eq!(
        fs::read_to_string(workspace.join("invocations.txt")).expect("no command replay"),
        "pipe\npipe\n"
    );
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("shutdown deadline")
        .expect("server task")
        .expect("server shutdown");
}
