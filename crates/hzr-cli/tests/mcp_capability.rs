#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use hzr_core::{Config, Ledger};
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
    receipt = {{
        "contract_version": 1,
        "engine": engine,
        "correlation_id": correlation,
        "sequence": 1,
        "occurred_at_unix_ms": int(time.time() * 1000),
        "baseline_tokens": 10,
        "delivered_tokens": 5,
        "execution_ms": 1,
        "measurement": "estimated",
        "route": "optimized",
        "attribution": {{
            "operation": operation,
            "mode": mode,
            "stage": "internal_transport"
        }},
        "host_grant_applied": False
    }}
    with open(journal, 'a', encoding='utf-8') as handle:
        handle.write(json.dumps(receipt, separators=(',', ':')) + '\n')
if a == ['--version']:
    print('rtk 0.44.1-fork.1')
elif a == ['contract', '--json']:
    print(json.dumps(engine, separators=(',', ':')))
elif len(a) >= 2 and a[0] == 'rewrite' and a[1] == '--help':
    print('rtk rewrite\nRaw command to rewrite')
elif len(a) >= 2 and a[0] == 'proxy' and a[1] == '--help':
    print('rtk proxy\nwithout filtering')
elif a and a[0] == 'config':
    print(json.dumps({{"schema_version":2,"config_path":{config_path:?},"config_exists":False,"config_sha256":None,"config":{{"grepai":{{"enabled":True,"auto_init":True,"binary_path":None}}}}}}))
elif a and a[0] == 'read':
    try:
        text = pathlib.Path(a[1]).read_text()
    except Exception as error:
        print(str(error), file=sys.stderr)
        sys.exit(2)
    lines = text.splitlines(True)
    if '--from' in a:
        start = int(a[a.index('--from') + 1]) - 1
    else:
        start = 0
    if '--to' in a:
        end = int(a[a.index('--to') + 1])
    elif '--max-lines' in a:
        end = start + int(a[a.index('--max-lines') + 1])
    else:
        end = len(lines)
    record_receipt('read', 'read_range' if '--from' in a or '--to' in a else 'read_filtered')
    sys.stdout.write(''.join(lines[start:end]))
elif len(a) >= 5 and a[:4] == ['write', '--output', 'json', 'patch']:
    target = pathlib.Path(a[4])
    old_arg = a[a.index('--old') + 1]
    new_arg = a[a.index('--new') + 1]
    old = pathlib.Path(old_arg[1:]).read_text() if old_arg.startswith('@') else old_arg
    new = pathlib.Path(new_arg[1:]).read_text() if new_arg.startswith('@') else new_arg
    text = target.read_text()
    if old not in text:
        print(json.dumps({{"version":1,"ok":False,"op":"patch","error":"CAS block missing"}}))
        sys.exit(3)
    target.write_text(text.replace(old, new, 1))
    record_receipt('write', 'write')
    print(json.dumps({{"version":1,"ok":True,"op":"patch","applied":1}}))
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

fn request(
    stdin: &mut ChildStdin,
    stdout: &mut std::io::Lines<BufReader<ChildStdout>>,
    id: u64,
    name: &str,
    arguments: Value,
) -> Value {
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
    serde_json::from_str(
        &stdout
            .next()
            .expect("tool response line")
            .expect("read tool response"),
    )
    .expect("tool response JSON")
}

fn stop(child: &mut Child) {
    child.kill().ok();
    child.wait().ok();
}

#[test]
fn stdio_tools_call_executes_bounded_read_write_and_accounts_success_once() {
    let fixture = tempdir().expect("fixture");
    let home = fixture.path().join("home");
    let workspace = fixture.path().join("workspace");
    let engines = fixture.path().join("engines");
    let data = fixture.path().join("data");
    for directory in [&home, &workspace] {
        fs::create_dir_all(directory).expect("fixture directory");
    }
    fs::write(workspace.join("note.txt"), "alpha\nbeta\n").expect("read fixture");
    let config_path = fixture.path().join("config.toml");
    write_fake_rtk(&engines, &fixture.path().join("rtk-config.toml"));
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve daemon port");
    let address = listener.local_addr().expect("daemon address");
    drop(listener);
    let mut config = Config {
        data_dir: data.clone(),
        ..Config::default()
    };
    config.engines.directory = Some(engines);
    config.engines.auto_start_icm = false;
    config.engines.auto_index = false;
    config.daemon.bind = address;
    config.write(&config_path).expect("config");

    let common_env = |command: &mut Command| {
        command
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join("xdg"))
            .env("XDG_DATA_HOME", home.join("xdg-data"))
            .env("CLAUDE_CONFIG_DIR", home.join("claude"))
            .env("CODEX_HOME", home.join("codex"));
    };
    let mut init = Command::new(env!("CARGO_BIN_EXE_hzr"));
    common_env(&mut init);
    let initialized = init
        .current_dir(&workspace)
        .args([
            "--config",
            config_path.to_str().expect("config path"),
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
        .args([
            "--config",
            config_path.to_str().expect("config path"),
            "daemon",
            "serve",
        ])
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
            config_path.to_str().expect("config path"),
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
    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc":"2.0", "id":1, "method":"initialize",
            "params":{"protocolVersion":"2025-11-25","capabilities":{}}
        })
    )
    .expect("initialize MCP");
    stdin.flush().expect("flush initialize");
    let _: Value = serde_json::from_str(
        &stdout
            .next()
            .expect("initialize line")
            .expect("initialize response"),
    )
    .expect("initialize JSON");
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .expect("initialized notification");

    let read = request(
        &mut stdin,
        &mut stdout,
        2,
        "hzr_read",
        json!({"path":"note.txt","from":2,"to":2}),
    );
    assert_eq!(read["result"]["isError"], false, "{read:#}");
    assert_eq!(read["result"]["structuredContent"]["content"], "beta\n");
    let missing = request(
        &mut stdin,
        &mut stdout,
        3,
        "hzr_read",
        json!({"path":"missing.txt"}),
    );
    assert_eq!(missing["result"]["isError"], true);

    let patch = request(
        &mut stdin,
        &mut stdout,
        4,
        "hzr_write",
        json!({
            "operation":"patch", "path":"note.txt", "old":"beta", "new":"gamma", "cas":true
        }),
    );
    assert_eq!(patch["result"]["isError"], false);
    assert_eq!(patch["result"]["structuredContent"]["receipt"]["ok"], true);
    let cas_failure = request(
        &mut stdin,
        &mut stdout,
        5,
        "hzr_write",
        json!({
            "operation":"patch", "path":"note.txt", "old":"absent", "new":"no", "cas":true
        }),
    );
    assert_eq!(cas_failure["result"]["isError"], true);
    fs::write(workspace.join("exists.txt"), "owned\n").expect("existing create target");
    let create_existing = request(
        &mut stdin,
        &mut stdout,
        6,
        "hzr_write",
        json!({
            "operation":"create", "path":"exists.txt", "content":"replace"
        }),
    );
    assert_eq!(create_existing["result"]["isError"], true);
    let create = request(
        &mut stdin,
        &mut stdout,
        7,
        "hzr_write",
        json!({
            "operation":"create", "path":"created.txt", "content":"created\n"
        }),
    );
    assert_eq!(create["result"]["isError"], false, "{create:#}");
    assert_eq!(create["result"]["structuredContent"]["receipt"]["ok"], true);
    let outside = request(
        &mut stdin,
        &mut stdout,
        8,
        "hzr_read",
        json!({"path":"../outside.txt"}),
    );
    assert_eq!(outside["result"]["isError"], true);
    let outside_write = request(
        &mut stdin,
        &mut stdout,
        9,
        "hzr_write",
        json!({
            "operation":"create", "path":"../outside.txt", "content":"escape"
        }),
    );
    assert_eq!(outside_write["result"]["isError"], true);
    assert_eq!(
        fs::read_to_string(workspace.join("note.txt")).expect("patched file"),
        "alpha\ngamma\n"
    );
    assert_eq!(
        fs::read_to_string(workspace.join("exists.txt")).expect("existing file"),
        "owned\n"
    );
    assert_eq!(
        fs::read_to_string(workspace.join("created.txt")).expect("created file"),
        "created\n"
    );
    assert!(!fixture.path().join("outside.txt").exists());

    drop(stdin);
    mcp.wait().expect("MCP exit");
    stop(&mut daemon);
    let (_, efficiency) =
        Ledger::summaries_read_only(&data.join("ledger/hzr.sqlite")).expect("ledger summary");
    assert_eq!(
        efficiency.operations, 3,
        "one successful read, patch, and create must each be recorded exactly once"
    );
}
