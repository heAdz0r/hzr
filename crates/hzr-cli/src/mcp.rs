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

mod arguments;
#[cfg(test)]
mod tests;
mod tools;

use std::io::IsTerminal;

use anyhow::{Context, Result};
use hzr_core::Config;
use hzr_protocol::{
    CodecApiRequest, CodecProfile, ContextPlanApiRequest, FidelityClass, MemoryImportance,
    MemoryRecallApiRequest, MemoryScopeSelector, MemoryStoreApiRequest, MemoryWriteScope,
    RiskClass, SearchApiRequest, SearchMode,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::cli::McpClientArg;
use crate::client::DaemonClient;
use arguments::{
    bounded_usize, optional_bool, optional_enum, optional_string, parse_codec_profile,
    parse_fidelity, parse_importance, parse_mode, parse_recall_scope, parse_risk,
    parse_write_scope, reject_unknown, required_string, string_array,
};
use tools::tool_definitions;

/// Latest stable MCP revision implemented by the gateway. The 2026-07-28 revision is
/// still a release candidate, so production clients negotiate against this stable line.
const LATEST_MCP_PROTOCOL_VERSION: &str = "2025-11-25";
const SUPPORTED_MCP_PROTOCOL_VERSIONS: [&str; 4] = [
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    LATEST_MCP_PROTOCOL_VERSION,
];

/// JSON-RPC error codes used here (the standard subset).
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INVALID_REQUEST: i64 = -32600;
const PARSE_ERROR: i64 = -32700;

pub fn lifecycle_metadata() -> Value {
    json!({
        "mode": crate::client_config::MCP_LIFECYCLE,
        "started_by_init": false,
        "registered_by": "hzr install --force",
        "launched_by": "MCP client on connection",
        "shutdown": "client closes stdio",
    })
}

#[derive(Default)]
struct SessionState {
    initialized: bool,
}

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
    let mut session = SessionState::default();

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
        let Some(response) = handle_line(config, &workspace, &mut session, line).await else {
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
async fn handle_line(
    config: &Config,
    workspace: &str,
    session: &mut SessionState,
    line: &str,
) -> Option<Value> {
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
    let id = request.get("id").cloned();
    if !request.is_object()
        || request.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || request.get("method").and_then(Value::as_str).is_none()
    {
        return Some(error_response(
            id.unwrap_or(Value::Null),
            INVALID_REQUEST,
            "request must be a JSON-RPC 2.0 object with a method",
        ));
    }
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return Some(error_response(
            id.unwrap_or(Value::Null),
            INVALID_REQUEST,
            "request method must be a string",
        ));
    };

    // Absent id => notification. `notifications/initialized` is the common one.
    let id = id?;

    match method {
        "initialize" if session.initialized => Some(error_response(
            id,
            INVALID_REQUEST,
            "MCP session is already initialized",
        )),
        "initialize" => match initialize_result(&request) {
            Ok(result) => {
                session.initialized = true;
                Some(success(id, result))
            }
            Err(error) => Some(error_response(id, INVALID_PARAMS, &error.to_string())),
        },
        "ping" => Some(success(id, json!({}))),
        "tools/list" if session.initialized => {
            Some(success(id, json!({"tools": tool_definitions()})))
        }
        "tools/call" if session.initialized => {
            Some(call_tool(config, workspace, id, &request).await)
        }
        "tools/list" | "tools/call" => Some(error_response(
            id,
            INVALID_REQUEST,
            "initialize the MCP session before using tools",
        )),
        _ => Some(error_response(
            id,
            METHOD_NOT_FOUND,
            &format!("unsupported MCP method: {method}"),
        )),
    }
}

fn initialize_result(request: &Value) -> Result<Value> {
    let requested = request
        .pointer("/params/protocolVersion")
        .and_then(Value::as_str)
        .context("initialize requires params.protocolVersion")?;
    // An unknown revision negotiates down to our latest rather than failing, so a newer
    // client still gets a working session instead of no session at all.
    let negotiated = if SUPPORTED_MCP_PROTOCOL_VERSIONS.contains(&requested) {
        requested
    } else {
        LATEST_MCP_PROTOCOL_VERSION
    };

    Ok(json!({
        "protocolVersion": negotiated,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {
            "name": "hzr",
            "title": "HZR Zero-Redundancy Gateway",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Local stdio gateway to the single HZR context, index and memory owners.",
        },
        "instructions": "Use hzr_context_plan first for unfamiliar or cross-cutting work, \
    hzr_search for targeted code discovery, hzr_memory_recall before re-reading prior work, \
    hzr_memory_store only for durable decisions or resolved errors, and `hzr tdd` before \
    production changes. HZR owns the single \
    context planner, semantic index and memory store; never launch icm, grepai or rtk directly.",
    }))
}

async fn call_tool(config: &Config, workspace: &str, id: Value, request: &Value) -> Value {
    let Some(params) = request.get("params").and_then(Value::as_object) else {
        return error_response(id, INVALID_PARAMS, "tools/call requires object params");
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return error_response(id, INVALID_PARAMS, "tools/call requires a string name");
    };
    if !matches!(
        name,
        "hzr_memory_recall" | "hzr_memory_store" | "hzr_search" | "hzr_context_plan" | "hzr_codec"
    ) {
        return error_response(id, INVALID_PARAMS, &format!("unknown tool: {name}"));
    }
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return success(
            id,
            tool_error("Tool arguments must be an object. No operation was attempted."),
        );
    }

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
        "hzr_context_plan" => context_plan(&client, workspace, &arguments).await,
        "hzr_codec" => codec(&client, &arguments).await,
        _ => {
            return error_response(id, INVALID_PARAMS, &format!("unknown tool: {name}"));
        }
    };

    match outcome {
        Ok(value) => success(id, tool_success(&value)),
        Err(error) if name == "hzr_memory_store" => success(
            id,
            tool_error(&format!(
                "{error:#}. The store did not report success and HZR did not use a fallback \
                 store. If transport failed after dispatch, completion is unknown; recall the \
                 fact before retrying. If the daemon is down, start it with `hzr daemon serve`."
            )),
        ),
        Err(error) => success(
            id,
            tool_error(&format!(
                "{error:#}. No fallback engine or store was used. If the daemon is down, start \
                 it with `hzr daemon serve`."
            )),
        ),
    }
}

/// Compile a response-density contract through the daemon.
///
/// Takes no workspace: the codec is a pure text transform over content the agent already
/// holds, so binding it to a repository would imply an index lookup that never happens.
async fn codec(client: &DaemonClient, arguments: &Value) -> Result<Value> {
    reject_unknown(arguments, &["content", "fidelity", "risk", "profile"])?;
    let request = CodecApiRequest {
        content: required_string(arguments, "content")?,
        fidelity: optional_enum(
            arguments,
            "fidelity",
            FidelityClass::default(),
            parse_fidelity,
            "exact, lossless_structural, semantic, summary",
        )?,
        risk: optional_enum(
            arguments,
            "risk",
            RiskClass::default(),
            parse_risk,
            "low, medium, high, irreversible",
        )?,
        profile: optional_enum(
            arguments,
            "profile",
            CodecProfile::default(),
            parse_codec_profile,
            "off, safe, adaptive, compact, shadow",
        )?,
    };
    let transform = client.codec_compile(&request).await?;
    Ok(serde_json::to_value(transform)?)
}

async fn recall(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<Value> {
    reject_unknown(arguments, &["query", "topic", "keyword", "limit", "scope"])?;
    let query = required_string(arguments, "query")?;
    let request = MemoryRecallApiRequest {
        workspace: workspace.to_owned(),
        query,
        topic: optional_string(arguments, "topic")?,
        limit: bounded_usize(arguments, "limit", 10, 50)?,
        keyword: optional_string(arguments, "keyword")?,
        scope: optional_enum(
            arguments,
            "scope",
            MemoryScopeSelector::default(),
            parse_recall_scope,
            "project, global, project_and_global",
        )?,
    };
    let records = client.memory_recall(&request).await?;
    Ok(json!({"count": records.len(), "memories": records}))
}

async fn store(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<Value> {
    reject_unknown(
        arguments,
        &["topic", "content", "importance", "keywords", "scope"],
    )?;
    let request = MemoryStoreApiRequest {
        workspace: workspace.to_owned(),
        topic: required_string(arguments, "topic")?,
        content: required_string(arguments, "content")?,
        importance: optional_enum(
            arguments,
            "importance",
            MemoryImportance::default(),
            parse_importance,
            "critical, high, medium, low",
        )?,
        keywords: string_array(arguments, "keywords", 32)?,
        raw: None,
        scope: optional_enum(
            arguments,
            "scope",
            MemoryWriteScope::default(),
            parse_write_scope,
            "project, global",
        )?,
    };
    let response = client.memory_store(&request).await?;
    Ok(serde_json::to_value(response)?)
}

async fn search(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<Value> {
    reject_unknown(
        arguments,
        &["query", "path", "mode", "limit", "include_content"],
    )?;
    let request = SearchApiRequest {
        workspace: workspace.to_owned(),
        query: required_string(arguments, "query")?,
        path: optional_string(arguments, "path")?,
        mode: optional_enum(
            arguments,
            "mode",
            SearchMode::Auto,
            parse_mode,
            "auto, semantic, exact",
        )?,
        limit: bounded_usize(arguments, "limit", 10, 50)?,
        include_content: optional_bool(arguments, "include_content", false)?,
    };
    let response = client.search(&request).await?;
    Ok(serde_json::to_value(response)?)
}

async fn context_plan(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<Value> {
    reject_unknown(
        arguments,
        &["intent", "path", "topic", "search_limit", "memory_limit"],
    )?;
    let request = ContextPlanApiRequest {
        workspace: workspace.to_owned(),
        intent: required_string(arguments, "intent")?,
        path: optional_string(arguments, "path")?,
        topic: optional_string(arguments, "topic")?,
        search_limit: bounded_usize(arguments, "search_limit", 10, 50)?,
        memory_limit: bounded_usize(arguments, "memory_limit", 5, 50)?,
    };
    let response = client.context_plan(&request).await?;
    Ok(serde_json::to_value(response)?)
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

fn tool_success(value: &Value) -> Value {
    json!({
        "content": [{"type": "text", "text": value.to_string()}],
        "structuredContent": value,
        "isError": false,
    })
}

/// Tool-level failure. Reported through `isError` rather than a JSON-RPC error so the
/// agent can read the remediation text and continue.
fn tool_error(message: &str) -> Value {
    json!({"content": [{"type": "text", "text": message}], "isError": true})
}
