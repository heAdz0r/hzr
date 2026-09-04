use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use hzr_core::Config;
use serde_json::Value;
use tempfile::tempdir;

#[test]
fn test_mcp_status_reports_native_client_managed_lifecycle() -> anyhow::Result<()> {
    let directory = tempdir().expect("temporary MCP home");
    let output = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .args(["mcp", "status", "--json"])
        .env("HOME", directory.path())
        .env("CODEX_HOME", directory.path().join("codex"))
        .env(
            "CLAUDE_DESKTOP_CONFIG",
            directory.path().join("claude-desktop.json"),
        )
        .output()?;

    assert!(
        output.status.success(),
        "MCP status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(status["lifecycle"]["mode"], "client_managed_stdio");
    assert_eq!(status["lifecycle"]["started_by_init"], false);
    let clients = status["clients"].as_array().expect("client status array");
    // Claude Code is audited alongside the two writable clients: its `~/.claude.json` can
    // hold a direct `icm` server, and leaving it unread hid exactly that for a release.
    let audited: Vec<&str> = clients
        .iter()
        .map(|client| client["client"].as_str().expect("client name"))
        .collect();
    assert_eq!(audited, ["codex", "claude-desktop", "claude-code"]);
    assert!(clients.iter().all(|client| client["registered"] == false));
    Ok(())
}

#[test]
fn test_stdio_mcp_negotiates_lists_typed_tools_and_exits_on_eof() -> anyhow::Result<()> {
    let directory = tempdir().expect("temporary MCP home");
    let config = directory.path().join("config.toml");
    let mut child = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .args([
            "--config",
            config.to_str().expect("UTF-8 config path"),
            "mcp",
            "serve",
            "--workspace",
            directory.path().to_str().expect("UTF-8 workspace path"),
        ])
        .env("HOME", directory.path())
        .env("XDG_CONFIG_HOME", directory.path().join("xdg"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn MCP server");
    let mut stdin = child.stdin.take().expect("MCP stdin");
    let stdout = child.stdout.take().expect("MCP stdout");
    let mut lines = BufReader::new(stdout).lines();

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-11-25","capabilities":{{}},"clientInfo":{{"name":"test","version":"1"}}}}}}"#
    )
    .expect("write initialize");
    stdin.flush().expect("flush initialize");
    let initialize: Value = serde_json::from_str(
        &lines
            .next()
            .expect("initialize response line")
            .expect("read initialize response"),
    )
    .expect("parse initialize response");
    assert_eq!(initialize["result"]["protocolVersion"], "2025-11-25");
    let instructions = initialize["result"]["instructions"]
        .as_str()
        .expect("initialize instructions");
    assert!(instructions.contains("TDD is opt-in"));
    assert!(!instructions.contains("`hzr tdd` before production changes"));

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .expect("write initialized notification");
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{{}}}}"#
    )
    .expect("write tools/list");
    stdin.flush().expect("flush tools/list");
    let list: Value = serde_json::from_str(
        &lines
            .next()
            .expect("tools/list response line")
            .expect("read tools/list response"),
    )
    .expect("parse tools/list response");
    let tools = list["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 14);
    assert!(tools.iter().any(|tool| tool["name"] == "hzr_context_plan"));
    assert!(
        tools.iter().any(|tool| tool["name"] == "hzr_codec"),
        "the density codec must be reachable over the wire, not only in the tool table"
    );
    for required in [
        "hzr_memory_get",
        "hzr_read",
        "hzr_write",
        "hzr_exec",
        "hzr_observability",
        "hzr_doctor",
    ] {
        assert!(
            tools.iter().any(|tool| tool["name"] == required),
            "missing first-class MCP capability {required}"
        );
    }
    assert!(tools.iter().all(|tool| {
        tool["inputSchema"]["additionalProperties"] == false
            && tool["outputSchema"]["type"] == "object"
    }));

    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = child.try_wait().expect("poll MCP server") {
            assert!(status.success(), "MCP server failed after stdin EOF");
            break;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            anyhow::bail!("MCP server did not exit after stdin EOF");
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

#[test]
fn test_stdio_mcp_cancels_in_flight_tool_without_late_response() -> anyhow::Result<()> {
    let integration_timeout = Duration::from_secs(10);
    let directory = tempdir().expect("temporary MCP home");
    let home_dir = directory.path().join("home");
    let workspace_dir = directory.path().join("workspace");
    fs::create_dir_all(&home_dir)?;
    fs::create_dir_all(&workspace_dir)?;
    let data_dir = directory.path().join("data");
    let runtime = data_dir.join("runtime");
    fs::create_dir_all(&runtime)?;
    let token_path = runtime.join("hzrd.token");
    fs::write(&token_path, "a".repeat(64))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600))?;
    }

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let (accepted_tx, accepted_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (_stream, _) = listener.accept().expect("accept pending tool request");
        accepted_tx.send(()).expect("report accepted request");
        let _ = release_rx.recv_timeout(Duration::from_secs(5));
    });

    let config_path = directory.path().join("config.toml");
    let mut config = Config {
        data_dir,
        ..Config::default()
    };
    config.daemon.bind = address;
    config.daemon.request_timeout_ms = 10_000;
    config.write(&config_path)?;
    let initialized = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .args([
            "--config",
            config_path.to_str().expect("UTF-8 config path"),
            "init",
            "--if-needed",
            "--quiet",
            "--skip-service",
        ])
        .current_dir(&workspace_dir)
        .env("HOME", &home_dir)
        .env("XDG_CONFIG_HOME", home_dir.join("xdg"))
        .env("HZR_ALLOW_DEV_CLIENT_WRITE", "1")
        .status()?;
    assert!(initialized.success(), "workspace initialization failed");

    let mut child = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .args([
            "--config",
            config_path.to_str().expect("UTF-8 config path"),
            "mcp",
            "serve",
            "--workspace",
            workspace_dir.to_str().expect("UTF-8 workspace path"),
        ])
        .env("HOME", &home_dir)
        .env("XDG_CONFIG_HOME", home_dir.join("xdg"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().expect("MCP stdin");
    let stdout = child.stdout.take().expect("MCP stdout");
    let mut lines = BufReader::new(stdout).lines();

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-11-25","capabilities":{{}}}}}}"#
    )?;
    stdin.flush()?;
    let initialized: Value = serde_json::from_str(
        &lines
            .next()
            .expect("initialize response line")
            .expect("read initialize response"),
    )?;
    assert_eq!(initialized["id"], 1);

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )?;
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{{"name":"hzr_context_plan","arguments":{{"intent":"blocked request"}}}}}}"#
    )?;
    stdin.flush()?;
    accepted_rx
        .recv_timeout(integration_timeout)
        .expect("tool request must reach the delayed daemon");

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/cancelled","params":{{"requestId":7}}}}"#
    )?;
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":8,"method":"tools/list","params":{{}}}}"#
    )?;
    stdin.flush()?;

    let (line_tx, line_rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        line_tx.send(lines.next()).expect("send next MCP response");
    });
    let next = line_rx
        .recv_timeout(integration_timeout)
        .expect("cancellation must unblock the MCP loop")
        .expect("tools/list response line")
        .expect("read tools/list response");
    let response: Value = serde_json::from_str(&next)?;
    assert_eq!(
        response["id"], 8,
        "cancelled request emitted a late response"
    );
    reader.join().expect("join MCP response reader");

    drop(stdin);
    release_tx.send(()).ok();
    server.join().expect("join delayed daemon");
    let deadline = Instant::now() + integration_timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            assert!(status.success(), "MCP server failed after cancellation");
            break;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            anyhow::bail!("MCP server did not exit after cancellation and stdin EOF");
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
