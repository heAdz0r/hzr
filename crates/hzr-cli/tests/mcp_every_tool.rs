//! 0.11.2: every advertised MCP tool is called once over real stdio against a real daemon.
//!
//! `hzr_search` failed every live call with "output contract violation … unknown property
//! index_generation" while the unit suite passed, because the suite validated hand-written
//! samples rather than what the handlers return. This test drives the built binaries end
//! to end, so a handler that emits a field its advertised `outputSchema` forbids fails CI.
#![cfg(unix)]

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use hzr_core::Config;
use hzr_exec::expected_engine_identity;
use serde_json::{Value, json};
use tempfile::tempdir;

fn write_fake_rtk(engines: &std::path::Path, config_path: &std::path::Path) {
    fs::create_dir_all(engines).expect("engine directory");
    let path = engines.join("rtk");
    let current = expected_engine_identity().expect("current engine metadata");
    let contract = serde_json::to_string(&json!({
        "contract_version": current.contract_version,
        "engine_version": current.engine_version,
        "manifest_sha256": current.manifest_sha256,
        "content_manifest_sha256": current.content_manifest_sha256,
    }))
    .expect("contract JSON");
    let script = format!(
        r#"#!/usr/bin/env python3
import json, os, pathlib, sys, time
a = sys.argv[1:]
engine = json.loads({contract:?})
def record_receipt(operation, mode):
    journal = os.environ.get('HZR_INTERNAL_ACCOUNTING_RECEIPT_JOURNAL')
    correlation = os.environ.get('HZR_INTERNAL_ACCOUNTING_CORRELATION')
    if not journal or not correlation:
        return
    receipt = {{"contract_version": 1, "engine": engine, "correlation_id": correlation,
        "sequence": 1, "occurred_at_unix_ms": int(time.time() * 1000), "baseline_tokens": 10,
        "delivered_tokens": 5, "execution_ms": 1, "measurement": "estimated",
        "route": "optimized", "attribution": {{"operation": operation, "mode": mode,
        "stage": "internal_transport"}}, "host_grant_applied": False}}
    with open(journal, 'a', encoding='utf-8') as handle:
        handle.write(json.dumps(receipt, separators=(',', ':')) + '\n')
if a == ['--version']:
    print('rtk 0.50.0-fork.1')
elif a == ['contract', '--json']:
    print(json.dumps(engine, separators=(',', ':')))
elif len(a) >= 2 and a[0] == 'rewrite' and a[1] == '--help':
    print('rtk rewrite\nRaw command to rewrite')
elif len(a) >= 2 and a[0] == 'proxy' and a[1] == '--help':
    print('rtk proxy\nwithout filtering')
elif a and a[0] == 'config':
    print(json.dumps({{"schema_version":2,"config_path":{config_path:?},"config_exists":False,"config_sha256":None,"config":{{"grepai":{{"enabled":True,"auto_init":True,"binary_path":None}}}}}}))
elif a and a[0] == 'rgai':
    query = a[-1]
    print(json.dumps({{"query": query, "total_hits": 1, "scanned_files": 2, "skipped_large": 0,
        "skipped_binary": 0, "backend": "builtin",
        "hits": [{{"path": "note.txt", "score": 1.0, "matched_lines": 1,
            "snippets": [{{"lines": [{{"line": 1, "text": "alpha"}}], "matched_terms": [query]}}]}}]}}))
elif a and a[0] == 'read':
    try:
        text = pathlib.Path(a[1]).read_text()
    except Exception as error:
        print(str(error), file=sys.stderr)
        sys.exit(2)
    record_receipt('read', 'read_filtered')
    sys.stdout.write(text)
elif len(a) >= 5 and a[:4] == ['write', '--output', 'json', 'create']:
    target = pathlib.Path(a[4])
    if target.exists():
        print(json.dumps({{"version":1,"ok":False,"op":"create","error":"exists"}}))
        sys.exit(4)
    target.write_text(sys.stdin.read())
    record_receipt('write', 'write')
    print(json.dumps({{"version":1,"ok":True,"op":"create","applied":1}}))
else:
    print('unsupported fake rtk invocation: ' + repr(a), file=sys.stderr)
    sys.exit(67)
"#,
        config_path = config_path.to_string_lossy(),
        contract = contract,
    );
    fs::write(&path, script).expect("fake rtk");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("fake executable");
}

fn call(
    stdin: &mut ChildStdin,
    stdout: &mut std::io::Lines<BufReader<ChildStdout>>,
    id: u64,
    name: &str,
    arguments: Value,
) -> (Value, usize) {
    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": arguments}
        })
    )
    .expect("write tool call");
    stdin.flush().expect("flush tool call");
    let line = stdout
        .next()
        .expect("tool response line")
        .expect("read tool response");
    (
        serde_json::from_str(&line).expect("tool response JSON"),
        line.len(),
    )
}

fn stop(child: &mut Child) {
    child.kill().ok();
    child.wait().ok();
}

#[test]
fn every_advertised_tool_answers_within_its_output_contract() {
    let fixture = tempdir().expect("fixture");
    let home = fixture.path().join("home");
    let workspace = fixture.path().join("workspace");
    let engines = fixture.path().join("engines");
    let data = fixture.path().join("data");
    for directory in [&home, &workspace] {
        fs::create_dir_all(directory).expect("fixture directory");
    }
    fs::write(workspace.join("note.txt"), "alpha\nbeta\n").expect("note fixture");
    let large = "fn source_line() { let value = \"quoted\"; }\n".repeat(700);
    fs::write(workspace.join("large.rs"), &large).expect("large fixture");
    let config_path = fixture.path().join("config.toml");
    write_fake_rtk(&engines, &fixture.path().join("rtk-config.toml"));
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve daemon port");
    let address = listener.local_addr().expect("daemon address");
    drop(listener);
    let mut config = Config {
        data_dir: data,
        ..Config::default()
    };
    config.engines.directory = Some(engines);
    config.engines.auto_start_icm = false;
    config.engines.auto_index = false;
    config.daemon.bind = address;
    config.write(&config_path).expect("config");
    let config_arg = config_path.to_str().expect("config path");

    let common_env = |command: &mut Command| {
        command
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join("xdg"))
            .env("XDG_DATA_HOME", home.join("xdg-data"))
            .env("CLAUDE_CONFIG_DIR", home.join("claude"))
            .env("CODEX_HOME", home.join("codex"))
            .env("HZR_ALLOW_DEV_CLIENT_WRITE", "1");
    };
    let mut init = Command::new(env!("CARGO_BIN_EXE_hzr"));
    common_env(&mut init);
    let initialized = init
        .current_dir(&workspace)
        .args([
            "--config",
            config_arg,
            "init",
            "--if-needed",
            "--quiet",
            "--skip-service",
        ])
        .status()
        .expect("initialize workspace");
    assert!(initialized.success());

    let mut daemon_command = Command::new(env!("CARGO_BIN_EXE_hzr"));
    common_env(&mut daemon_command);
    let mut daemon = daemon_command
        .args(["--config", config_arg, "daemon", "serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon");
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(address).is_err() {
        assert!(Instant::now() < deadline, "daemon did not become ready");
        thread::sleep(Duration::from_millis(20));
    }

    let mut mcp_command = Command::new(env!("CARGO_BIN_EXE_hzr"));
    common_env(&mut mcp_command);
    let mut mcp = mcp_command
        .args([
            "--config",
            config_arg,
            "mcp",
            "serve",
            "--workspace",
            workspace.to_str().expect("workspace"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn MCP");
    let mut stdin = mcp.stdin.take().expect("MCP stdin");
    let mut stdout = BufReader::new(mcp.stdout.take().expect("MCP stdout")).lines();
    for message in [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2025-11-25","capabilities":{}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    ] {
        writeln!(stdin, "{message}").expect("write MCP message");
    }
    stdin.flush().expect("flush MCP handshake");
    let _: Value = serde_json::from_str(&stdout.next().expect("init").expect("init line"))
        .expect("initialize JSON");
    let list: Value = serde_json::from_str(&stdout.next().expect("list").expect("list line"))
        .expect("tools/list JSON");
    let advertised = list["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name").to_owned())
        .collect::<BTreeSet<_>>();

    // (tool, arguments, must succeed in this fixture). Memory tools have no ICM here, so
    // they must fail as tool errors — never as contract violations.
    let calls = [
        ("hzr_memory_recall", json!({"query": "decision"}), false),
        (
            "hzr_memory_get",
            json!({"id": "memory-1", "scope": "project"}),
            false,
        ),
        (
            "hzr_memory_store",
            json!({"topic": "architecture", "content": "durable fact"}),
            false,
        ),
        ("hzr_memory_forget", json!({"id": "memory-1"}), false),
        (
            "hzr_memory_update",
            json!({"id": "memory-1", "content": "replacement"}),
            false,
        ),
        ("hzr_memory_prune", json!({"dry_run": true}), false),
        (
            "hzr_search",
            json!({"query": "alpha", "mode": "exact", "include_content": true}),
            true,
        ),
        (
            "hzr_context_plan",
            json!({"intent": "where is alpha"}),
            true,
        ),
        ("hzr_codec", json!({"content": "one\n\none\n"}), true),
        (
            "hzr_read",
            json!({"path": "large.rs", "max_tokens": 16000}),
            true,
        ),
        (
            "hzr_write",
            json!({"operation": "create", "path": "created.txt", "content": "created\n"}),
            true,
        ),
        ("hzr_exec", json!({"command": "pwd"}), false),
        ("hzr_observability", json!({}), true),
        ("hzr_doctor", json!({}), true),
    ];
    let called = calls
        .iter()
        .map(|(name, _, _)| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        called, advertised,
        "every advertised tool must be called once here"
    );

    for (index, (name, arguments, must_succeed)) in calls.into_iter().enumerate() {
        let id = 10 + u64::try_from(index).expect("small index");
        let (response, bytes) = call(&mut stdin, &mut stdout, id, name, arguments);
        let result = &response["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        assert!(
            !text.contains("output contract violation"),
            "{name} violated its advertised outputSchema: {text}"
        );
        if must_succeed {
            assert_eq!(result["isError"], false, "{name}: {response:#}");
        }
        if result["isError"] == false {
            assert!(
                result["structuredContent"].is_object(),
                "{name} succeeded without structuredContent"
            );
            // The text block is a compact view, never a second copy of the payload.
            let structured = result["structuredContent"].to_string().len();
            assert!(
                text.len() < 4_096 || text.len() <= structured / 2,
                "{name} text block is {} bytes for a {structured}-byte payload",
                text.len()
            );
        }
        if name == "hzr_read" {
            let file = &result["structuredContent"]["files"][0];
            assert_eq!(file["content"], large.as_str());
            assert_eq!(file["complete"], true);
            assert!(text.contains("\"complete\":true"), "{text}");
            eprintln!(
                "hzr_read of a {}-byte file: {bytes}-byte MCP response, {}-byte text block",
                large.len(),
                text.len()
            );
            assert!(
                bytes < large.len() * 3 / 2,
                "response {bytes} bytes still carries the source twice"
            );
        }
        if name == "hzr_search" {
            let search = &result["structuredContent"];
            assert_eq!(search["effective_mode"], "exact");
            assert!(search["index_generation"].is_string(), "{search:#}");
        }
    }

    drop(stdin);
    mcp.wait().expect("MCP exit");
    stop(&mut daemon);
}
