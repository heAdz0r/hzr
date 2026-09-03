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

use std::collections::HashMap;
use std::io::IsTerminal;

use anyhow::{Context, Result};
use directories::BaseDirs;
use hzr_core::{AccountingGapSurface, Config, ConfigPaths, ambient_session_id};
use hzr_index::IndexPlacement;
use hzr_protocol::{
    AccountingAttribution, AccountingOperationKind, AccountingOperationMode, AccountingStage,
    CodecApiRequest, CodecProfile, ContextPlanApiRequest, FidelityClass, ForkManagedWrite,
    ForkRunApiRequest, ForkRunApiResponse, MemoryForgetApiRequest, MemoryImportance,
    MemoryPruneApiRequest, MemoryRecallApiRequest, MemoryScopeSelector, MemoryStoreApiRequest,
    MemoryUpdateApiRequest, MemoryWriteScope, RiskClass, SearchAccountingMetadata,
    SearchApiRequest, SearchApiResponse, SearchMode, build_search_accounting_attribution,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use toml_edit::{Array, Value as TomlValue};

use crate::cli::McpClientArg;
use crate::client::DaemonClient;
use arguments::{
    bounded_f32, bounded_usize, optional_bool, optional_enum, optional_string, parse_codec_profile,
    parse_fidelity, parse_importance, parse_mode, parse_recall_scope, parse_risk,
    parse_write_scope, required_string, string_array,
};
use tools::{
    MCP_CREATE_CONTENT_MAX_BYTES, MCP_EXEC_TIMEOUT_MAX_MS, MCP_PATCH_BLOCK_MAX_BYTES,
    MCP_PATH_MAX_BYTES, ToolKind, tool_contract, tool_definitions, validate_tool_input,
    validate_tool_output,
};

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

/// Whether the directory the client launched this server from can own a project memory
/// namespace.
///
/// The project namespace is derived from the launch directory, which the *client* picks —
/// and clients pick badly. Claude Desktop launches from `/`, so a store that looked
/// successful actually landed in the namespace of the filesystem root, where no CLI recall
/// from inside a repository will ever find it again. That is the exact "fake success" this
/// adapter was written to prevent, arriving through the one input the adapter does not
/// control. Classify the binding up front so an unusable one is refused by name instead of
/// being hashed into a namespace nobody reads.
#[derive(Clone, Debug)]
pub(crate) enum WorkspaceBinding {
    /// A directory that can plausibly be a project, including one not yet `git init`-ed.
    Project(std::path::PathBuf),
    /// A directory that can never be a project, with the remediation the agent must report.
    Refused {
        path: std::path::PathBuf,
        reason: String,
    },
}

impl WorkspaceBinding {
    pub(crate) fn project_root(&self) -> Option<&std::path::Path> {
        match self {
            Self::Project(root) => Some(root.as_path()),
            Self::Refused { .. } => None,
        }
    }

    pub(crate) fn refusal(&self) -> Option<&str> {
        match self {
            Self::Project(_) => None,
            Self::Refused { reason, .. } => Some(reason.as_str()),
        }
    }

    /// The directory the client actually launched from, reportable whether or not it bound.
    pub(crate) fn resolved_path(&self) -> &std::path::Path {
        match self {
            Self::Project(root) => root.as_path(),
            Self::Refused { path, .. } => path.as_path(),
        }
    }

    /// The workspace string to send to `hzrd`, or the empty string when refused. Callers
    /// must check [`Self::refusal`] first; this exists so the happy path stays a one-liner.
    pub(crate) fn as_request_value(&self) -> String {
        self.project_root()
            .map(|root| root.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Classify a resolved launch directory. `home` is passed in rather than looked up so the
/// rule is testable without depending on the machine running the test.
pub(crate) fn classify_workspace_binding(
    resolved: &std::path::Path,
    home: Option<&std::path::Path>,
) -> WorkspaceBinding {
    let refuse = |what: &str| WorkspaceBinding::Refused {
        path: resolved.to_path_buf(),
        reason: format!(
            "`hzr mcp serve` was launched from {}, which is {what} and cannot own a project \
             memory namespace. Nothing was read or written. Register the server with an \
             explicit `--workspace <project directory>` (see `hzr mcp config --client \
             <client> --workspace <dir> --apply`), then retry.",
            resolved.display()
        ),
    };

    if resolved.parent().is_none() {
        return refuse("the filesystem root");
    }
    if let Some(home) = home {
        if resolved == home {
            return refuse("your home directory");
        }
        if home.starts_with(resolved) {
            return refuse("an ancestor of your home directory");
        }
    }
    WorkspaceBinding::Project(resolved.to_path_buf())
}

async fn apply_workspace_policy(config: &Config, binding: WorkspaceBinding) -> WorkspaceBinding {
    let WorkspaceBinding::Project(path) = binding else {
        return binding;
    };
    let workspace = match crate::activation::discover(config, &path).await {
        Ok(workspace) => workspace,
        Err(error) => {
            return WorkspaceBinding::Refused {
                path,
                reason: format!(
                    "HZR could not resolve the MCP workspace safely ({error:#}). Nothing was read or written."
                ),
            };
        }
    };
    if !config.activation.allows(
        &workspace.identity.repository_id,
        &workspace.identity.worktree_id,
    ) {
        return WorkspaceBinding::Refused {
            path,
            reason: format!(
                "HZR project-only activation is enabled and {} is not an enabled workspace. \
                 Nothing was read or written. Run `hzr enable --workspace {}` from an \
                 operator shell, then reconnect the MCP client.",
                workspace.identity.root.display(),
                workspace.identity.root.display()
            ),
        };
    }
    match workspace.placement() {
        Ok(IndexPlacement::ManagedSymlink { .. }) => {
            WorkspaceBinding::Project(workspace.identity.root)
        }
        Ok(_) => WorkspaceBinding::Refused {
            path,
            reason: format!(
                "{} is not an initialized HZR workspace. Nothing was read or written. Run \
                 `cd {}` followed by `hzr init --if-needed`, or pin an initialized project with \
                 `hzr mcp config --client <client> --workspace <dir> --apply`.",
                workspace.identity.root.display(),
                workspace.identity.root.display()
            ),
        },
        Err(error) => WorkspaceBinding::Refused {
            path,
            reason: format!(
                "HZR refused the MCP workspace placement ({error}). Nothing was read or written."
            ),
        },
    }
}

pub async fn serve(
    config: &Config,
    config_path: &std::path::Path,
    workspace: Option<&std::path::Path>,
) -> Result<()> {
    // A terminal stdin means a human ran this by hand; an MCP server would then hang
    // forever looking like a wedged session. Fail fast and say what it is for.
    if std::io::stdin().is_terminal() {
        anyhow::bail!(
            "`hzr mcp serve` speaks MCP over stdio and is launched by an agent, not \
             interactively; register it as an MCP server instead"
        );
    }

    let pinned_workspace = workspace.is_some();
    let launch_workspace = workspace
        .map(std::path::Path::to_path_buf)
        .or_else(|| std::env::var_os("CLAUDE_PROJECT_DIR").map(std::path::PathBuf::from))
        .unwrap_or(std::env::current_dir()?);
    let launch_workspace = std::fs::canonicalize(&launch_workspace).unwrap_or(launch_workspace);
    let binding = classify_workspace_binding(
        &launch_workspace,
        BaseDirs::new().as_ref().map(|base| base.home_dir()),
    );
    let mut binding = apply_workspace_policy(config, binding).await;
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::stdout();
    let mut session = SessionState::default();

    let (completed_tx, mut completed_rx) = mpsc::channel::<(String, Value)>(32);
    let (progress_tx, mut progress_rx) = mpsc::channel::<Value>(32);
    let mut pending = HashMap::<String, tokio::task::AbortHandle>::new();

    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line.context("failed to read the MCP request stream")? else {
                    break;
                };
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let parsed = serde_json::from_str::<Value>(line).ok();
                if let Some(cancelled) = parsed.as_ref().and_then(cancelled_request_id) {
                    if let Some(key) = request_id_key(&cancelled) {
                        if let Some(handle) = pending.remove(&key) {
                            handle.abort();
                        }
                    }
                    continue;
                }
                if session.initialized {
                    if let Some(request) = parsed.filter(|request| {
                        request.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
                            && request.get("method").and_then(Value::as_str) == Some("tools/call")
                    }) {
                        if let Some(id) = request.get("id").cloned() {
                            let Some(key) = request_id_key(&id) else {
                                write_response(
                                    &mut stdout,
                                    &error_response(Value::Null, INVALID_REQUEST, "request id must be a string or number"),
                                )
                                .await?;
                                continue;
                            };
                            if pending.contains_key(&key) {
                                write_response(
                                    &mut stdout,
                                    &error_response(id, INVALID_REQUEST, "request id is already in progress"),
                                )
                                .await?;
                                continue;
                            }
                            let task_config = config.clone();
                            let task_binding = binding.clone();
                            let task_config_path = config_path.to_path_buf();
                            let sender = completed_tx.clone();
                            let progress_sender = progress_tx.clone();
                            let completed_key = key.clone();
                            let handle = tokio::spawn(async move {
                                let progress_token = request
                                    .pointer("/params/_meta/progressToken")
                                    .cloned();
                                let progress_enabled = request
                                    .pointer("/params/name")
                                    .and_then(Value::as_str)
                                    .is_some_and(|name| matches!(name, "hzr_search" | "hzr_context_plan"));
                                let call = call_tool(&task_config, &task_config_path, &task_binding, id, &request);
                                tokio::pin!(call);
                                let response = if progress_enabled && progress_token.is_some() {
                                    let mut ticks = tokio::time::interval_at(
                                        tokio::time::Instant::now() + std::time::Duration::from_secs(5),
                                        std::time::Duration::from_secs(5),
                                    );
                                    let mut progress = 0_u64;
                                    loop {
                                        tokio::select! {
                                            response = &mut call => break response,
                                            _ = ticks.tick() => {
                                                progress = progress.saturating_add(1);
                                                let notification = json!({
                                                    "jsonrpc": "2.0",
                                                    "method": "notifications/progress",
                                                    "params": {
                                                        "progressToken": progress_token,
                                                        "progress": progress,
                                                        "message": "HZR is still working"
                                                    }
                                                });
                                                let _ = progress_sender.send(notification).await;
                                            }
                                        }
                                    }
                                } else {
                                    call.await
                                };
                                let _ = sender.send((completed_key, response)).await;
                            });
                            pending.insert(key, handle.abort_handle());
                            continue;
                        }
                    }
                }
                if let Some(response) = handle_line(
                    config,
                    &mut binding,
                    pinned_workspace,
                    &mut session,
                    line,
                )
                .await
                {
                    write_response(&mut stdout, &response).await?;
                }
            }
            Some((key, response)) = completed_rx.recv(), if !pending.is_empty() => {
                if pending.remove(&key).is_some() {
                    write_response(&mut stdout, &response).await?;
                }
            }
            Some(notification) = progress_rx.recv(), if !pending.is_empty() => {
                write_response(&mut stdout, &notification).await?;
            }
        }
    }
    pending.into_values().for_each(|handle| handle.abort());
    Ok(())
}

async fn write_response(stdout: &mut tokio::io::Stdout, response: &Value) -> Result<()> {
    let mut encoded = serde_json::to_vec(response)?;
    encoded.push(b'\n');
    stdout.write_all(&encoded).await?;
    stdout.flush().await?;
    Ok(())
}

fn request_id_key(id: &Value) -> Option<String> {
    match id {
        Value::String(value) => Some(format!("s:{value}")),
        Value::Number(value) => Some(format!("n:{value}")),
        _ => None,
    }
}

fn cancelled_request_id(request: &Value) -> Option<Value> {
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || request.get("method").and_then(Value::as_str) != Some("notifications/cancelled")
        || request.get("id").is_some()
    {
        return None;
    }
    let id = request.pointer("/params/requestId")?;
    request_id_key(id).map(|_| id.clone())
}

/// Returns `None` for notifications, which must never be answered.
async fn handle_line(
    config: &Config,
    binding: &mut WorkspaceBinding,
    pinned_workspace: bool,
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
        "initialize" => {
            match initialize_workspace_root(&request) {
                Ok(Some(root)) if pinned_workspace => {
                    let root = std::fs::canonicalize(&root).unwrap_or(root);
                    let selected = std::fs::canonicalize(binding.resolved_path())
                        .unwrap_or_else(|_| binding.resolved_path().to_path_buf());
                    if root != selected {
                        return Some(error_response(
                            id,
                            INVALID_PARAMS,
                            &format!(
                                "configured MCP workspace {} conflicts with client root {}; no HZR tool was exposed. Re-register this client for the current workspace and reconnect.",
                                selected.display(),
                                root.display()
                            ),
                        ));
                    }
                }
                Ok(Some(root)) => {
                    let root = std::fs::canonicalize(&root).unwrap_or(root);
                    let candidate = classify_workspace_binding(
                        &root,
                        BaseDirs::new().as_ref().map(|base| base.home_dir()),
                    );
                    *binding = apply_workspace_policy(config, candidate).await;
                }
                Ok(None) => {}
                Err(error) => {
                    return Some(error_response(id, INVALID_PARAMS, &error.to_string()));
                }
            }
            match initialize_result(&request, binding) {
                Ok(result) => {
                    session.initialized = true;
                    Some(success(id, result))
                }
                Err(error) => Some(error_response(id, INVALID_PARAMS, &error.to_string())),
            }
        }
        "ping" => Some(success(id, json!({}))),
        "tools/list" if session.initialized => {
            Some(success(id, json!({"tools": tool_definitions()})))
        }
        "tools/call" if session.initialized => Some(
            call_tool(
                config,
                &ConfigPaths::discover().config_file,
                binding,
                id,
                &request,
            )
            .await,
        ),
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

fn initialize_workspace_root(request: &Value) -> Result<Option<std::path::PathBuf>> {
    let Some(roots) = request
        .pointer("/params/roots")
        .or_else(|| request.pointer("/params/_meta/roots"))
    else {
        return Ok(None);
    };
    let roots = roots
        .as_array()
        .context("initialize params.roots must be an array")?;
    if roots.is_empty() {
        return Ok(None);
    }
    if roots.len() != 1 {
        anyhow::bail!(
            "initialize supplied {} roots; pin one workspace with `--workspace <dir>`",
            roots.len()
        );
    }
    let uri = roots[0]
        .get("uri")
        .and_then(Value::as_str)
        .context("initialize root requires a string uri")?;
    file_uri_path(uri).map(Some)
}

fn file_uri_path(uri: &str) -> Result<std::path::PathBuf> {
    let encoded = uri
        .strip_prefix("file://")
        .context("initialize root uri must use the file scheme")?;
    let encoded = encoded.strip_prefix("localhost").unwrap_or(encoded);
    if !encoded.starts_with("/") {
        anyhow::bail!("initialize root uri must not name a remote host");
    }
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == 37 {
            let Some(high) = bytes.get(index + 1).and_then(|byte| hex_value(*byte)) else {
                anyhow::bail!("initialize root uri has invalid percent encoding");
            };
            let Some(low) = bytes.get(index + 2).and_then(|byte| hex_value(*byte)) else {
                anyhow::bail!("initialize root uri has invalid percent encoding");
            };
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    if decoded.contains(&0) {
        anyhow::bail!("initialize root uri contains a null byte");
    }
    let decoded = String::from_utf8(decoded).context("initialize root uri is not UTF-8")?;
    let path = std::path::PathBuf::from(decoded);
    if !path.is_absolute() {
        anyhow::bail!("initialize root uri must resolve to an absolute path");
    }
    Ok(path)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        48..=57 => Some(byte - 48),
        65..=70 => Some(byte - 65 + 10),
        97..=102 => Some(byte - 97 + 10),
        _ => None,
    }
}

fn initialize_result(request: &Value, binding: &WorkspaceBinding) -> Result<Value> {
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

    // Naming the binding in the handshake is the only way an agent can tell which project
    // its memory belongs to. Without it, a session bound to the wrong directory looks
    // identical to a correct one until a recall silently comes back empty.
    let workspace = json!({
        "bound": binding.refusal().is_none(),
        "project": binding.project_root().map(|root| root.to_string_lossy()),
        "resolved_from": binding.resolved_path().to_string_lossy(),
        "note": "The project memory namespace is selected at initialization from one client root, CLAUDE_PROJECT_DIR, cwd, or an explicit --workspace pin. It cannot change per call.",
    });

    const GUIDANCE: &str = "Use hzr_context_plan first for unfamiliar or cross-cutting work, \
    hzr_search for targeted code discovery, hzr_memory_recall before re-reading prior work, \
    and hzr_memory_store only for durable decisions or resolved errors. TDD is opt-in: use \
    `hzr tdd` only when the user or repository requires it, or regression risk justifies \
    test-first overhead; skip it when token or time efficiency matters while preserving \
    repository-required verification. HZR owns the single \
    context planner, semantic index and memory store; never launch icm, grepai or rtk directly.";

    let instructions = match binding.refusal() {
        Some(reason) => format!(
            "{reason}\n\nUntil that is fixed, only hzr_codec works in this session; every \
             project-scoped tool will return isError with this same reason. {GUIDANCE}"
        ),
        None => GUIDANCE.to_owned(),
    };

    Ok(json!({
        "protocolVersion": negotiated,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {
            "name": "hzr",
            "title": "HZR Zero-Redundancy Gateway",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Local stdio gateway to the single HZR context, index and memory owners.",
            "workspace": workspace,
        },
        "instructions": instructions,
    }))
}

async fn call_tool(
    config: &Config,
    config_path: &std::path::Path,
    binding: &WorkspaceBinding,
    id: Value,
    request: &Value,
) -> Value {
    let Some(params) = request.get("params").and_then(Value::as_object) else {
        return error_response(id, INVALID_PARAMS, "tools/call requires object params");
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return error_response(id, INVALID_PARAMS, "tools/call requires a string name");
    };
    let Some(contract) = tool_contract(name) else {
        return error_response(id, INVALID_PARAMS, &format!("unknown tool: {name}"));
    };
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

    if let Err(error) = validate_tool_arguments(name, &arguments) {
        return success(id, tool_error(&error.to_string()));
    }

    if name != "hzr_codec" {
        if let Some(reason) = binding.refusal() {
            return success(id, tool_error(reason));
        }
    }
    let workspace = binding.as_request_value();
    let workspace = workspace.as_str();

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

    let outcome = match contract.kind {
        ToolKind::MemoryRecall => recall(&client, workspace, &arguments).await,
        ToolKind::MemoryStore => store(&client, workspace, &arguments).await,
        ToolKind::MemoryForget => forget(&client, workspace, &arguments).await,
        ToolKind::MemoryUpdate => update(&client, workspace, &arguments).await,
        ToolKind::MemoryPrune => prune(&client, workspace, &arguments).await,
        ToolKind::Search => search(&client, workspace, &arguments).await,
        ToolKind::ContextPlan => context_plan(&client, workspace, &arguments).await,
        ToolKind::Codec => codec(&client, workspace, &arguments).await,
        ToolKind::Read => read(&client, workspace, &arguments).await,
        ToolKind::Write => write(&client, workspace, &arguments).await,
        ToolKind::Exec => exec(&client, workspace, &arguments).await,
        ToolKind::Observability => observability(&client).await,
        ToolKind::Doctor => doctor(config_path, config, workspace, &arguments).await,
    };

    match outcome {
        Ok(value) => {
            if let Err(error) = validate_tool_output(name, &value) {
                return success(
                    id,
                    tool_error(&format!(
                        "HZR internal output contract violation for {name}: {error}. No success was confirmed."
                    )),
                );
            }
            if !matches!(name, "hzr_codec" | "hzr_exec") {
                if let Ok(accounting) =
                    mcp_operation_request(contract.kind, name, workspace, &arguments, &value)
                {
                    let session = ambient_session_id();
                    if client.record_operation(&accounting).await.is_err() {
                        let _ = crate::hook_runner::record_accounting_gap_now(
                            config,
                            AccountingGapSurface::Mcp,
                            Some(workspace),
                            session.as_deref(),
                            Some(name.trim_start_matches("hzr_")),
                        );
                    } else {
                        let _ = crate::hook_runner::recover_accounting_gap_now(
                            config,
                            AccountingGapSurface::Mcp,
                            Some(workspace),
                            session.as_deref(),
                            Some(name.trim_start_matches("hzr_")),
                        );
                    }
                }
            }
            success(id, tool_success(&value))
        }
        Err(error)
            if matches!(
                name,
                "hzr_memory_store" | "hzr_memory_update" | "hzr_memory_forget" | "hzr_memory_prune"
            ) =>
        {
            success(
                id,
                tool_error(&format!(
                    "{error:#}. The memory mutation did not report success and HZR did not use a \
                 fallback store. If transport failed after dispatch, completion is unknown; \
                 recall or list the target before retrying. If the daemon is down, start it with \
                 `hzr daemon serve`."
                )),
            )
        }
        Err(error) => success(
            id,
            tool_error(&format!(
                "{error:#}. No fallback engine or store was used. If the daemon is down, start \
                 it with `hzr daemon serve`."
            )),
        ),
    }
}

fn validate_tool_arguments(name: &str, arguments: &Value) -> Result<()> {
    validate_tool_input(name, arguments).map_err(anyhow::Error::msg)
}

fn mcp_operation_request(
    kind: ToolKind,
    tool_name: &str,
    workspace: &str,
    arguments: &Value,
    response: &Value,
) -> Result<hzr_protocol::OperationApiRequest> {
    let bytes = serde_json::to_vec(response)?.len();
    let delivered = u64::try_from(bytes / 4).unwrap_or(u64::MAX).max(1);
    let command = tool_name.replace('_', " ");
    let attribution = Some(match kind {
        ToolKind::Search => search_accounting_attribution(arguments, response)?,
        ToolKind::Read => read_accounting_attribution(arguments),
        // Fork-backed operations already have an included transport receipt.
        ToolKind::Write => simple_accounting_attribution(
            AccountingOperationKind::Write,
            AccountingOperationMode::Write,
            AccountingStage::FinalDelivery,
        ),
        ToolKind::ContextPlan => simple_accounting_attribution(
            AccountingOperationKind::Context,
            AccountingOperationMode::ContextPlan,
            AccountingStage::StandaloneDelivery,
        ),
        ToolKind::MemoryRecall => simple_accounting_attribution(
            AccountingOperationKind::Memory,
            AccountingOperationMode::MemoryRecall,
            AccountingStage::StandaloneDelivery,
        ),
        ToolKind::MemoryStore => simple_accounting_attribution(
            AccountingOperationKind::Memory,
            AccountingOperationMode::MemoryStore,
            AccountingStage::StandaloneDelivery,
        ),
        ToolKind::MemoryForget => simple_accounting_attribution(
            AccountingOperationKind::Memory,
            AccountingOperationMode::MemoryForget,
            AccountingStage::StandaloneDelivery,
        ),
        ToolKind::MemoryUpdate => simple_accounting_attribution(
            AccountingOperationKind::Memory,
            AccountingOperationMode::MemoryUpdate,
            AccountingStage::StandaloneDelivery,
        ),
        ToolKind::MemoryPrune => simple_accounting_attribution(
            AccountingOperationKind::Memory,
            AccountingOperationMode::MemoryPrune,
            AccountingStage::StandaloneDelivery,
        ),
        ToolKind::Observability => simple_accounting_attribution(
            AccountingOperationKind::Observability,
            AccountingOperationMode::ObservabilitySnapshot,
            AccountingStage::ControlPlane,
        ),
        ToolKind::Doctor => simple_accounting_attribution(
            AccountingOperationKind::Doctor,
            AccountingOperationMode::DoctorCheck,
            AccountingStage::ControlPlane,
        ),
        ToolKind::Codec | ToolKind::Exec => {
            anyhow::bail!("{tool_name} records accounting through its dedicated route")
        }
    });
    Ok(hzr_protocol::OperationApiRequest {
        original_command: command.clone(),
        recorded_command: command,
        baseline_tokens_estimated: delivered,
        delivered_tokens_estimated: delivered,
        execution_ms: 0,
        project_path: workspace.to_owned(),
        channel: hzr_protocol::AccountingChannel::Mcp,
        measurement: hzr_protocol::AccountingMeasurement::Estimated,
        route: hzr_protocol::AccountingRoute::Optimized,
        agent: Some("mcp".to_owned()),
        session_id: hzr_core::ambient_session_id(),
        attribution,
    })
}

fn simple_accounting_attribution(
    operation: AccountingOperationKind,
    mode: AccountingOperationMode,
    stage: AccountingStage,
) -> AccountingAttribution {
    AccountingAttribution {
        operation,
        mode,
        stage,
        requested_mode: None,
        effective_mode: Some(mode),
        search_strategy: None,
        search_fallback_code: None,
        include_content: None,
        limit: None,
        path_scope_count: None,
        filter_level: None,
        from_line: None,
        to_line: None,
        source_bytes: None,
        evasion: None,
    }
}

fn read_accounting_attribution(arguments: &Value) -> AccountingAttribution {
    let mode = if arguments
        .get("outline")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        AccountingOperationMode::ReadOutline
    } else if arguments.get("from").is_some() || arguments.get("to").is_some() {
        AccountingOperationMode::ReadRange
    } else if arguments.get("max_lines").is_some() {
        AccountingOperationMode::ReadHead
    } else {
        AccountingOperationMode::ReadFiltered
    };
    let mut attribution = simple_accounting_attribution(
        AccountingOperationKind::Read,
        mode,
        AccountingStage::FinalDelivery,
    );
    attribution.from_line = arguments.get("from").and_then(Value::as_u64);
    attribution.to_line = arguments.get("to").and_then(Value::as_u64);
    attribution.limit = arguments.get("max_lines").and_then(Value::as_u64);
    attribution.path_scope_count = Some(1);
    attribution
}

fn search_accounting_attribution(
    arguments: &Value,
    response: &Value,
) -> Result<AccountingAttribution> {
    let requested_mode = optional_enum(
        arguments,
        "mode",
        SearchMode::Auto,
        parse_mode,
        "auto, semantic, exact",
    )?;
    let response: SearchApiResponse = serde_json::from_value(response.clone())?;
    Ok(build_search_accounting_attribution(
        requested_mode,
        &response,
        SearchAccountingMetadata {
            stage: AccountingStage::FinalDelivery,
            include_content: optional_bool(arguments, "include_content", false)?,
            limit: u64::try_from(bounded_usize(arguments, "limit", 10, 50)?)?,
            path_scope_count: 1,
        },
    ))
}

/// Compile a response-density contract through the daemon.
///
async fn codec(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<Value> {
    let request = CodecApiRequest {
        content: required_string(arguments, "content")?,
        project_path: workspace.to_owned(),
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
        channel: Some(hzr_protocol::AccountingChannel::Mcp),
    };
    let transform = client.codec_compile(&request).await?;
    Ok(serde_json::to_value(transform)?)
}

async fn recall(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<Value> {
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
    Ok(serde_json::to_value(client.memory_recall(&request).await?)?)
}

async fn store(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<Value> {
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

async fn forget(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<Value> {
    let request = MemoryForgetApiRequest {
        workspace: workspace.to_owned(),
        id: required_string(arguments, "id")?,
        scope: optional_enum(
            arguments,
            "scope",
            MemoryWriteScope::default(),
            parse_write_scope,
            "project, global",
        )?,
    };
    Ok(serde_json::to_value(client.memory_forget(&request).await?)?)
}

async fn update(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<Value> {
    let keywords = if arguments.get("keywords").is_some() {
        Some(string_array(arguments, "keywords", 32)?)
    } else {
        None
    };
    let request = MemoryUpdateApiRequest {
        workspace: workspace.to_owned(),
        id: required_string(arguments, "id")?,
        content: required_string(arguments, "content")?,
        importance: arguments
            .get("importance")
            .map(|_| {
                optional_enum(
                    arguments,
                    "importance",
                    MemoryImportance::default(),
                    parse_importance,
                    "critical, high, medium, low",
                )
            })
            .transpose()?,
        keywords,
        scope: optional_enum(
            arguments,
            "scope",
            MemoryWriteScope::default(),
            parse_write_scope,
            "project, global",
        )?,
    };
    Ok(serde_json::to_value(client.memory_update(&request).await?)?)
}

async fn prune(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<Value> {
    let request = MemoryPruneApiRequest {
        workspace: workspace.to_owned(),
        threshold: bounded_f32(arguments, "threshold", 0.1)?,
        dry_run: optional_bool(arguments, "dry_run", true)?,
        scope: optional_enum(
            arguments,
            "scope",
            MemoryWriteScope::default(),
            parse_write_scope,
            "project, global",
        )?,
    };
    Ok(serde_json::to_value(client.memory_prune(&request).await?)?)
}

async fn search(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<Value> {
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

async fn exec(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<Value> {
    let timeout_ms = match arguments.get("timeout_ms") {
        None => None,
        Some(value) => {
            let value = value
                .as_u64()
                .context("argument `timeout_ms` must be a positive integer")?;
            if !(1..=MCP_EXEC_TIMEOUT_MAX_MS).contains(&value) {
                anyhow::bail!(
                    "argument `timeout_ms` must be between 1 and {MCP_EXEC_TIMEOUT_MAX_MS}"
                );
            }
            Some(value)
        }
    };
    let request = hzr_protocol::ExecApiRequest {
        cwd: workspace.to_owned(),
        command: required_string(arguments, "command")?,
        fidelity_requested: false,
        fidelity_reason: None,
        timeout_ms,
        caller_path: None,
        agent: Some("mcp".to_owned()),
        session_id: hzr_core::ambient_session_id(),
        host_execution_grant: hzr_core::inspect_ambient_host_grant().and_then(Result::ok),
    };
    Ok(serde_json::to_value(client.exec_run(&request).await?)?)
}

async fn read(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<Value> {
    let request = read_fork_request(workspace, arguments)?;
    let response = client.fork_run(&request).await?;
    fork_result(response, "content", false)
}

fn read_fork_request(workspace: &str, arguments: &Value) -> Result<ForkRunApiRequest> {
    let mut args = vec![
        "read".to_owned(),
        bounded_required_string(arguments, "path", MCP_PATH_MAX_BYTES)?,
    ];
    if optional_bool(arguments, "outline", false)? {
        args.push("--outline".to_owned());
    }
    for (key, flag) in [
        ("from", "--from"),
        ("to", "--to"),
        ("max_lines", "--max-lines"),
    ] {
        if let Some(value) = optional_positive_u64(arguments, key, 100_000)? {
            args.extend([flag.to_owned(), value.to_string()]);
        }
    }
    if let (Some(from), Some(to)) = (
        arguments.get("from").and_then(Value::as_u64),
        arguments.get("to").and_then(Value::as_u64),
    ) {
        if from > to {
            anyhow::bail!("argument `from` must not exceed `to`");
        }
    }
    Ok(ForkRunApiRequest {
        cwd: workspace.to_owned(),
        args,
        stdin: None,
        timeout_ms: None,
        agent: Some("mcp".to_owned()),
        session_id: hzr_core::ambient_session_id(),
        managed_write: None,
    })
}

async fn write(client: &DaemonClient, workspace: &str, arguments: &Value) -> Result<Value> {
    let request = write_fork_request(workspace, arguments)?;
    let response = client.fork_run(&request).await?;
    fork_result(response, "receipt", true)
}

fn write_fork_request(workspace: &str, arguments: &Value) -> Result<ForkRunApiRequest> {
    if !optional_bool(arguments, "cas", true)? {
        anyhow::bail!("argument `cas` must be true; MCP writes cannot bypass CAS");
    }
    let operation = required_string(arguments, "operation")?;
    let path = bounded_required_string(arguments, "path", MCP_PATH_MAX_BYTES)?;
    let managed_write = match operation.as_str() {
        "patch" => {
            if arguments.get("content").is_some() {
                anyhow::bail!("patch does not accept `content`; use `old` and `new`");
            }
            ForkManagedWrite::Patch {
                path,
                old: bounded_required_string(arguments, "old", MCP_PATCH_BLOCK_MAX_BYTES)?,
                new: bounded_required_string(arguments, "new", MCP_PATCH_BLOCK_MAX_BYTES)?,
            }
        }
        "create" => {
            if arguments.get("old").is_some() || arguments.get("new").is_some() {
                anyhow::bail!("create does not accept `old` or `new`; use `content`");
            }
            ForkManagedWrite::Create {
                path,
                content: bounded_required_string(
                    arguments,
                    "content",
                    MCP_CREATE_CONTENT_MAX_BYTES,
                )?,
            }
        }
        _ => anyhow::bail!("argument `operation` must be one of: patch, create"),
    };
    Ok(ForkRunApiRequest {
        cwd: workspace.to_owned(),
        args: Vec::new(),
        stdin: None,
        timeout_ms: None,
        agent: Some("mcp".to_owned()),
        session_id: hzr_core::ambient_session_id(),
        managed_write: Some(managed_write),
    })
}

fn bounded_required_string(arguments: &Value, key: &str, maximum_bytes: usize) -> Result<String> {
    let value = required_string(arguments, key)?;
    if value.len() > maximum_bytes {
        anyhow::bail!("argument `{key}` must contain at most {maximum_bytes} UTF-8 bytes");
    }
    Ok(value)
}

fn optional_positive_u64(arguments: &Value, key: &str, maximum: u64) -> Result<Option<u64>> {
    let Some(value) = arguments.get(key) else {
        return Ok(None);
    };
    let value = value
        .as_u64()
        .with_context(|| format!("argument `{key}` must be a positive integer"))?;
    if !(1..=maximum).contains(&value) {
        anyhow::bail!("argument `{key}` must be between 1 and {maximum}");
    }
    Ok(Some(value))
}

fn fork_result(response: ForkRunApiResponse, content_key: &str, parse_json: bool) -> Result<Value> {
    if response.termination != hzr_protocol::CommandTermination::Exited
        || response.exit_code != Some(0)
    {
        let stderr = bounded_error_text(&response.stderr, 4096);
        anyhow::bail!(
            "fork-core {:?} with exit code {:?}; stderr: {}",
            response.termination,
            response.exit_code,
            stderr
        );
    }
    let mut value = serde_json::to_value(&response)?;
    let object = value
        .as_object_mut()
        .context("fork response must be an object")?;
    let stdout = object
        .remove("stdout")
        .and_then(|value| value.as_str().map(str::to_owned))
        .context("fork response must contain stdout")?;
    let content = if parse_json {
        let receipt: Value =
            serde_json::from_str(stdout.trim()).context("fork write receipt is not valid JSON")?;
        if receipt.get("ok") != Some(&Value::Bool(true)) {
            anyhow::bail!("fork write receipt did not confirm success");
        }
        receipt
    } else {
        Value::String(stdout)
    };
    object.insert(content_key.to_owned(), content);
    Ok(value)
}

fn bounded_error_text(value: &str, maximum_bytes: usize) -> String {
    if value.len() <= maximum_bytes {
        return value.to_owned();
    }
    let mut boundary = maximum_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}…[truncated]", &value[..boundary])
}

async fn observability(client: &DaemonClient) -> Result<Value> {
    Ok(serde_json::to_value(client.health().await?)?)
}

async fn doctor(
    config_path: &std::path::Path,
    config: &Config,
    workspace: &str,
    _arguments: &Value,
) -> Result<Value> {
    let report =
        crate::diagnostics::doctor(config_path, config, std::path::Path::new(workspace)).await;
    Ok(serde_json::to_value(report)?)
}

fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Print the registration a user pastes into their client configuration.
///
/// For a write path use `hzr mcp config --apply` (or `hzr install`), which owns Codex and
/// Claude Desktop registrations the same way install does. Print mode remains for paste
/// workflows and clients HZR must not rewrite.
///
/// `workspace` pins the project the server's memory is scoped to. Dynamic clients can omit it
/// when they provide exactly one root during initialization or set `CLAUDE_PROJECT_DIR`; the
/// process working directory is the final fallback. Claude Desktop remains a singleton and
/// launches from `/`, so its registration must be pinned before project tools can run.
pub fn registration_snippet(
    client: McpClientArg,
    binary: &std::path::Path,
    workspace: Option<&std::path::Path>,
) -> String {
    let binary_text = binary.to_string_lossy().into_owned();
    let toml_binary = TomlValue::from(binary_text.as_str()).to_string();
    let json_binary = Value::String(binary_text).to_string();
    let (toml_args, json_args) = match workspace {
        Some(path) => {
            let path = path.to_string_lossy().into_owned();
            let args = ["mcp", "serve", "--workspace", path.as_str()];
            let mut toml = Array::new();
            toml.extend(args);
            (toml.to_string(), json!(args).to_string())
        }
        None => (
            "[\"mcp\", \"serve\"]".to_owned(),
            "[\"mcp\", \"serve\"]".to_owned(),
        ),
    };
    let unpinned_warning = workspace.is_none();

    match client {
        McpClientArg::Codex => {
            let hint = if unpinned_warning {
                "# Without `--workspace <dir>` HZR requires exactly one client root during\n\
                 # initialize; the process working directory is the final fallback.\n"
            } else {
                ""
            };
            format!(
                "# ~/.codex/config.toml — replace the [mcp_servers.icm] block with this.\n\
                 # Routing through hzr keeps one store and one supervised ICM; a direct\n\
                 # `icm serve` entry spawns a second writer per session and leaks orphans.\n\
                 {hint}\
                 [mcp_servers.hzr]\n\
                 command = {toml_binary}\n\
                 args = {toml_args}\n"
            )
        }
        McpClientArg::ClaudeDesktop => {
            let hint = if unpinned_warning {
                "// The desktop app launches MCP servers from `/`, which can never be a project.\n\
                 // Re-run with `--workspace <dir>` or the project-scoped tools will refuse.\n"
            } else {
                ""
            };
            format!(
                "// claude_desktop_config.json — replace the \"icm\" server with this.\n\
                 {hint}{{\n  \"mcpServers\": {{\n    \"hzr\": {{\n      \"command\": {json_binary},\n      \"args\": {json_args}\n    }}\n  }}\n}}\n"
            )
        }
        McpClientArg::ClaudeCode => {
            let hint = if unpinned_warning {
                "// HZR resolves this session from one initialize root, then\n\
                 // `CLAUDE_PROJECT_DIR`, then the process working directory.\n"
            } else {
                ""
            };
            format!(
                "// .mcp.json — project scope is isolated per worktree. HZR never writes\n\
                 // Claude Code's user state. Equivalent CLI: `claude mcp add -s project hzr\n\
                 // -- <hzr-binary> mcp serve --workspace <dir>`.\n\
                 {hint}{{\n  \"mcpServers\": {{\n    \"hzr\": {{\n      \"command\": {json_binary},\n      \"args\": {json_args}\n    }}\n  }}\n}}\n"
            )
        }
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
