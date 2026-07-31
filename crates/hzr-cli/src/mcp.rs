//! Stdio MCP server exposing HZR-owned tools to external agents.
//!
//! Codex and the Claude desktop app can only reach a memory layer through an MCP
//! server, so before this existed they each spawned `icm serve` directly. That is the
//! duplication the engine is named after: several ICM writers, several stores, and — as
//! observed on a real machine — eight orphaned `icm serve` processes left behind by dead
//! Codex sessions.
//!
//! This adapter fixes the class of problem rather than one instance:
//!
//! * **No store of its own.** Every call is forwarded to the single `hzrd`, which owns
//!   the one supervised ICM process and the one canonical database. Running ten of these
//!   adapters concurrently is harmless, because they are stateless routers — the thing
//!   that must stay singular is the store, not the pipe.
//! * **Cannot orphan.** The process exits as soon as stdin reaches EOF, which is what
//!   happens the moment the parent agent dies. A stale session cannot survive its owner.
//! * **No fake liveness.** When `hzrd` is unreachable the tool call returns an MCP error
//!   result with remediation instead of pretending to have stored something, so a dead
//!   backend can never look like a successful write.

use std::io::IsTerminal;

use anyhow::{Context, Result};
use hzr_core::Config;
use hzr_protocol::{
    MemoryImportance, MemoryRecallApiRequest, MemoryStoreApiRequest, SearchApiRequest, SearchMode,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::cli::McpClientArg;
use crate::client::DaemonClient;

/// MCP revision this adapter implements. Clients that ask for a different revision still
/// receive this one, which the specification allows them to accept or reject.
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// JSON-RPC error codes used here (the standard subset).
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const PARSE_ERROR: i64 = -32700;

pub async fn serve(config: &Config, workspace: &std::path::Path) -> Result<()> {
    // A terminal stdin means a human ran this by hand; an MCP server would then hang
    // forever looking like a wedged session. Fail fast and say what it is for.
    if std::io::stdin().is_terminal() {
        anyhow::bail!(
            "`hzr mcp serve` speaks MCP over stdio and is launched by an agent, not \
             interactively; register it as an MCP server instead"
        );
    }

    let workspace = workspace.to_string_lossy().to_string();
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::stdout();

    // Reading until EOF is the whole anti-orphan mechanism: when the parent agent exits,
    // the pipe closes, `next_line()` returns None, and this process ends with it.
    while let Some(line) = lines
        .next_line()
        .await
        .context("failed to read the MCP request stream")?
    {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(response) = handle_line(config, &workspace, line).await else {
            // Notifications have no response by definition; staying silent is correct.
            continue;
        };
        let mut encoded = serde_json::to_vec(&response)?;
        encoded.push(b'\n');
        stdout.write_all(&encoded).await?;
        stdout.flush().await?;
    }
    Ok(())
}

/// Returns `None` for notifications, which must never be answered.
async fn handle_line(config: &Config, workspace: &str, line: &str) -> Option<Value> {
    let request: Value = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(error) => {
            return Some(error_response(
                Value::Null,
                PARSE_ERROR,
                &format!("invalid JSON-RPC payload: {error}"),
            ));
        }
    };
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let id = request.get("id").cloned();

    // Absent id => notification. `notifications/initialized` is the common one.
    let id = id?;

    match method {
        "initialize" => Some(success(id, initialize_result())),
        "ping" => Some(success(id, json!({}))),
        "tools/list" => Some(success(id, json!({"tools": tool_definitions()}))),
        "tools/call" => Some(call_tool(config, workspace, id, &request).await),
        _ => Some(error_response(
            id,
            METHOD_NOT_FOUND,
            &format!("unsupported MCP method: {method}"),
        )),
    }
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {"tools": {}},
        "serverInfo": {
            "name": "hzr",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": "HZR owns one centralized memory store and one semantic index. \
    Use these tools instead of calling icm, grepai or rtk directly: a direct call creates a \
    second store and unaccounted usage.",
    })
}

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "hzr_memory_recall",
            "description": "Recall durable facts, decisions and past context from the single \
        HZR-owned memory store, scoped to this repository.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "What to recall."},
                    "topic": {"type": "string", "description": "Optional topic filter."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 50},
                },
                "required": ["query"],
            },
        }),
        json!({
            "name": "hzr_memory_store",
            "description": "Store a durable fact, decision or resolved error in the single \
        HZR-owned memory store. Not for ephemeral session state.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": {"type": "string"},
                    "content": {"type": "string"},
                    "importance": {
                        "type": "string",
                        "enum": ["critical", "high", "medium", "low"],
                    },
                    "keywords": {"type": "array", "items": {"type": "string"}},
                },
                "required": ["topic", "content"],
            },
        }),
        json!({
            "name": "hzr_search",
            "description": "Search this repository through the one canonical HZR index \
        (semantic by default, exact on request).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "mode": {"type": "string", "enum": ["auto", "semantic", "exact"]},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 50},
                },
                "required": ["query"],
            },
        }),
    ]
}

async fn call_tool(config: &Config, workspace: &str, id: Value, request: &Value) -> Value {
    let params = request.get("params");
    let name = params
        .and_then(|params| params.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = params
        .and_then(|params| params.get("arguments"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let client = match DaemonClient::from_config(config) {
        Ok(client) => client,
        Err(error) => {
            // Fail-closed, but as a tool error rather than a transport crash: the agent
            // keeps working and sees why, and nothing is silently recorded as stored.
            return success(
                id,
                tool_error(&format!(
                    "HZR daemon is unavailable ({error}); start it with `hzr daemon serve`. \
                 Nothing was read or written."
                )),
            );
        }
    };

    let outcome = match name {
        "hzr_memory_recall" => recall(&client, workspace, &arguments).await,
        "hzr_memory_store" => store(&client, workspace, &arguments).await,
        "hzr_search" => search(&client, workspace, &arguments).await,
        other => {
            return error_response(id, INVALID_PARAMS, &format!("unknown tool: {other}"));
        }
    };

    match outcome {
        Ok(text) => success(id, tool_text(&text)),
        // A failed write must state that nothing landed. Without that, an agent can read
        // "error" and still assume a partial store happened, then skip retrying.
        Err(error) => success(
            id,
            tool_error(&format!(
                "{error:#}. Nothing was read or written. If the daemon is down, start it \
                 with `hzr daemon serve`; HZR never falls back to a second store."
            )),
        ),
    }
}

async fn recall(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<String> {
    let query = required_string(arguments, "query")?;
    let request = MemoryRecallApiRequest {
        workspace: workspace.to_owned(),
        query,
        topic: arguments
            .get("topic")
            .and_then(Value::as_str)
            .map(str::to_owned),
        limit: arguments
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(10)
            .clamp(1, 50) as usize,
        keyword: None,
    };
    let records = client.memory_recall(&request).await?;
    if records.is_empty() {
        return Ok("No stored memory matched.".to_owned());
    }
    Ok(serde_json::to_string_pretty(&records)?)
}

async fn store(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<String> {
    let request = MemoryStoreApiRequest {
        workspace: workspace.to_owned(),
        topic: required_string(arguments, "topic")?,
        content: required_string(arguments, "content")?,
        importance: arguments
            .get("importance")
            .and_then(Value::as_str)
            .and_then(parse_importance)
            .unwrap_or_default(),
        keywords: arguments
            .get("keywords")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        raw: None,
    };
    let response = client.memory_store(&request).await?;
    Ok(serde_json::to_string_pretty(&response)?)
}

async fn search(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<String> {
    let request = SearchApiRequest {
        workspace: workspace.to_owned(),
        query: required_string(arguments, "query")?,
        path: None,
        mode: arguments
            .get("mode")
            .and_then(Value::as_str)
            .and_then(parse_mode)
            .unwrap_or(SearchMode::Auto),
        limit: arguments
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(10)
            .clamp(1, 50) as usize,
        include_content: false,
    };
    let response = client.search(&request).await?;
    Ok(serde_json::to_string_pretty(&response)?)
}

fn parse_importance(value: &str) -> Option<MemoryImportance> {
    match value {
        "critical" => Some(MemoryImportance::Critical),
        "high" => Some(MemoryImportance::High),
        "medium" => Some(MemoryImportance::Medium),
        "low" => Some(MemoryImportance::Low),
        _ => None,
    }
}

fn parse_mode(value: &str) -> Option<SearchMode> {
    match value {
        "auto" => Some(SearchMode::Auto),
        "semantic" => Some(SearchMode::Semantic),
        "exact" => Some(SearchMode::Exact),
        _ => None,
    }
}

fn required_string(arguments: &Value, key: &str) -> Result<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .with_context(|| format!("missing required argument `{key}`"))
}

fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Registration snippet for an external agent's MCP configuration.
///
/// Emitted as text instead of written directly: `settings.json` and `CLAUDE.md` are files
/// HZR owns, but a third-party agent's own config is not, and silently rewriting it would
/// be the same overreach HZR refuses elsewhere.
pub fn registration_snippet(client: McpClientArg, binary: &std::path::Path) -> String {
    let binary = binary.display();
    match client {
        McpClientArg::Codex => format!(
            "# ~/.codex/config.toml — replace the [mcp_servers.icm] block with this.\n\
             # Routing through hzr keeps one store and one supervised ICM; a direct\n\
             # `icm serve` entry spawns a second writer per session and leaks orphans.\n\
             [mcp_servers.hzr]\n\
             command = \"{binary}\"\n\
             args = [\"mcp\", \"serve\"]\n"
        ),
        McpClientArg::ClaudeDesktop => format!(
            "// claude_desktop_config.json — replace the \"icm\" server with this.\n{{\n  \"mcpServers\": {{\n    \"hzr\": {{\n      \"command\": \"{binary}\",\n      \"args\": [\"mcp\", \"serve\"]\n    }}\n  }}\n}}\n"
        ),
    }
}

fn tool_text(text: &str) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": false})
}

/// Tool-level failure. Reported through `isError` rather than a JSON-RPC error so the
/// agent can read the remediation text and continue.
fn tool_error(message: &str) -> Value {
    json!({"content": [{"type": "text", "text": message}], "isError": true})
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{
        MCP_PROTOCOL_VERSION, METHOD_NOT_FOUND, PARSE_ERROR, initialize_result, tool_definitions,
        tool_error, tool_text,
    };

    /// Mirror of the notification rule in `handle_line`, which cannot be exercised
    /// directly without a daemon: a request without `id` gets no response.
    fn is_notification(request: &Value) -> bool {
        request.get("id").is_none()
    }

    #[test]
    fn test_initialize_advertises_tools_and_version() {
        let result = initialize_result();
        assert_eq!(result["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], "hzr");
        assert!(
            result["instructions"]
                .as_str()
                .expect("instructions")
                .contains("second store"),
            "clients must be told why not to call icm/grepai directly"
        );
    }

    #[test]
    fn test_every_tool_has_a_name_description_and_schema() {
        let tools = tool_definitions();
        assert_eq!(tools.len(), 3);
        for tool in &tools {
            let name = tool["name"].as_str().expect("tool name");
            assert!(
                name.starts_with("hzr_"),
                "tools must be namespaced to HZR: {name}"
            );
            assert!(
                !tool["description"]
                    .as_str()
                    .expect("description")
                    .is_empty()
            );
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(
                tool["inputSchema"]["required"].is_array(),
                "{name} must declare required arguments"
            );
        }
    }

    #[test]
    fn test_tools_expose_no_direct_engine_access() {
        let encoded = serde_json::to_string(&tool_definitions()).expect("serialize");
        for forbidden in ["icm serve", "grepai watch", "rtk proxy"] {
            assert!(
                !encoded.contains(forbidden),
                "the MCP surface must not offer direct engine control: {forbidden}"
            );
        }
    }

    #[test]
    fn test_notifications_are_never_answered() {
        assert!(is_notification(
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        ));
        assert!(!is_notification(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})
        ));
    }

    #[test]
    fn test_unavailable_backend_reports_an_error_not_a_fake_success() {
        let payload = tool_error("HZR daemon is unavailable; nothing was written.");
        assert_eq!(payload["isError"], true);
        assert!(
            payload["content"][0]["text"]
                .as_str()
                .expect("text")
                .contains("nothing was written"),
            "a dead backend must never look like a successful store"
        );
        // And a real result must be clearly distinguishable.
        assert_eq!(tool_text("ok")["isError"], false);
    }

    #[test]
    fn test_error_codes_are_standard_json_rpc() {
        assert_eq!(METHOD_NOT_FOUND, -32601);
        assert_eq!(PARSE_ERROR, -32700);
    }
}
