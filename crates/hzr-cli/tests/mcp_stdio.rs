use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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
    assert_eq!(tools.len(), 5);
    assert!(tools.iter().any(|tool| tool["name"] == "hzr_context_plan"));
    assert!(
        tools.iter().any(|tool| tool["name"] == "hzr_codec"),
        "the density codec must be reachable over the wire, not only in the tool table"
    );
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
