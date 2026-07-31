use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

use crate::error::{MemoryError, Result};
use crate::release::{ICM_MCP_SERVER_VERSION, ICM_VERSION};
use crate::types::{ServiceHealth, StoreRequest};

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

pub(crate) type SharedMcp = Arc<Mutex<Option<McpConnection>>>;

pub(crate) fn shared() -> SharedMcp {
    Arc::new(Mutex::new(None))
}

pub(crate) async fn attach(
    shared: &SharedMcp,
    child: &mut Child,
    timeout: Duration,
    has_embedder: bool,
) -> Result<ServiceHealth> {
    let stdin = child.stdin.take().ok_or_else(|| MemoryError::McpProtocol {
        operation: "initialize",
        message: "ICM stdin was not piped".into(),
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| MemoryError::McpProtocol {
            operation: "initialize",
            message: "ICM stdout was not piped".into(),
        })?;
    *shared.lock().await = Some(McpConnection {
        stdin,
        stdout: BufReader::new(stdout),
        next_id: 1,
    });

    let initialized: InitializeResult =
        match request(shared, "initialize", "initialize", json!({}), timeout).await {
            Ok(initialized) => initialized,
            Err(error) => {
                disconnect(shared).await;
                return Err(error);
            }
        };
    if initialized.protocol_version != MCP_PROTOCOL_VERSION
        || initialized.server_info.name != "icm"
        || initialized.server_info.version != ICM_MCP_SERVER_VERSION
    {
        disconnect(shared).await;
        return Err(MemoryError::McpProtocol {
            operation: "initialize",
            message: format!(
                "expected ICM {ICM_VERSION} MCP server {ICM_MCP_SERVER_VERSION} / protocol {MCP_PROTOCOL_VERSION}, received {} {} / MCP {}",
                initialized.server_info.name,
                initialized.server_info.version,
                initialized.protocol_version
            ),
        });
    }
    notify_initialized(shared, timeout).await?;
    Ok(ServiceHealth {
        status: "ok".into(),
        has_embedder,
    })
}

pub(crate) async fn ping(
    shared: &SharedMcp,
    timeout: Duration,
    has_embedder: bool,
) -> Result<ServiceHealth> {
    let _: Value = request(shared, "ping", "ping", json!({}), timeout).await?;
    Ok(ServiceHealth {
        status: "ok".into(),
        has_embedder,
    })
}

pub(crate) async fn store(
    shared: &SharedMcp,
    request_body: &StoreRequest,
    timeout: Duration,
) -> Result<()> {
    let arguments = json!({
        "topic": &request_body.topic,
        "content": &request_body.content,
        "importance": request_body.importance,
        "keywords": &request_body.keywords,
        "raw_excerpt": &request_body.raw,
    });
    let result: ToolResult = request(
        shared,
        "store",
        "tools/call",
        json!({"name":"icm_memory_store","arguments":arguments}),
        timeout,
    )
    .await?;
    if result.is_error {
        return Err(MemoryError::McpTool {
            tool: "icm_memory_store",
            message: result
                .content
                .into_iter()
                .map(|content| content.text)
                .collect::<Vec<_>>()
                .join("\n"),
        });
    }
    Ok(())
}

pub(crate) async fn disconnect(shared: &SharedMcp) {
    *shared.lock().await = None;
}

async fn notify_initialized(shared: &SharedMcp, timeout: Duration) -> Result<()> {
    let mut guard = shared.lock().await;
    let connection = guard.as_mut().ok_or(MemoryError::McpUnavailable)?;
    let message = serde_json::to_vec(&json!({
        "jsonrpc":"2.0",
        "method":"notifications/initialized",
        "params":{}
    }))
    .map_err(|error| MemoryError::McpProtocol {
        operation: "initialize notification",
        message: error.to_string(),
    })?;
    let write = async {
        connection.stdin.write_all(&message).await?;
        connection.stdin.write_all(b"\n").await?;
        connection.stdin.flush().await
    };
    match tokio::time::timeout(timeout, write).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(source)) => {
            *guard = None;
            Err(MemoryError::McpIo {
                operation: "initialize notification",
                source,
            })
        }
        Err(_) => {
            *guard = None;
            Err(MemoryError::McpTimeout {
                operation: "initialize notification",
                timeout,
            })
        }
    }
}

async fn request<T: DeserializeOwned>(
    shared: &SharedMcp,
    operation: &'static str,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<T> {
    let mut guard = shared.lock().await;
    let connection = guard.as_mut().ok_or(MemoryError::McpUnavailable)?;
    let result = tokio::time::timeout(timeout, connection.request(operation, method, params)).await;
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            if connection_broken(&error) {
                *guard = None;
            }
            Err(error)
        }
        Err(_) => {
            *guard = None;
            Err(MemoryError::McpTimeout { operation, timeout })
        }
    }
}

fn connection_broken(error: &MemoryError) -> bool {
    matches!(
        error,
        MemoryError::McpIo { .. } | MemoryError::McpProtocol { .. }
    )
}

pub(crate) struct McpConnection {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpConnection {
    async fn request<T: DeserializeOwned>(
        &mut self,
        operation: &'static str,
        method: &str,
        params: Value,
    ) -> Result<T> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| MemoryError::McpProtocol {
                operation,
                message: "JSON-RPC request ID space was exhausted".into(),
            })?;
        let mut message = serde_json::to_vec(&json!({
            "jsonrpc":"2.0",
            "id":id,
            "method":method,
            "params":params
        }))
        .map_err(|error| MemoryError::McpProtocol {
            operation,
            message: error.to_string(),
        })?;
        if message.len() >= MAX_MESSAGE_SIZE {
            return Err(MemoryError::McpProtocol {
                operation,
                message: format!("request exceeded {MAX_MESSAGE_SIZE} bytes"),
            });
        }
        message.push(b'\n');
        self.stdin
            .write_all(&message)
            .await
            .map_err(|source| MemoryError::McpIo { operation, source })?;
        self.stdin
            .flush()
            .await
            .map_err(|source| MemoryError::McpIo { operation, source })?;

        let response = read_capped_line(&mut self.stdout, operation).await?;
        let response: JsonRpcResponse<T> =
            serde_json::from_slice(&response).map_err(|error| MemoryError::McpProtocol {
                operation,
                message: error.to_string(),
            })?;
        if response.jsonrpc != "2.0" || response.id != id {
            return Err(MemoryError::McpProtocol {
                operation,
                message: format!(
                    "expected JSON-RPC 2.0 response id {id}, received version {:?} id {}",
                    response.jsonrpc, response.id
                ),
            });
        }
        if let Some(error) = response.error {
            return Err(MemoryError::McpRemote {
                operation,
                code: error.code,
                message: error.message,
            });
        }
        response.result.ok_or_else(|| MemoryError::McpProtocol {
            operation,
            message: "response contained neither result nor error".into(),
        })
    }
}

async fn read_capped_line(
    reader: &mut BufReader<ChildStdout>,
    operation: &'static str,
) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    loop {
        let buffer = reader
            .fill_buf()
            .await
            .map_err(|source| MemoryError::McpIo { operation, source })?;
        if buffer.is_empty() {
            return Err(MemoryError::McpProtocol {
                operation,
                message: "ICM closed stdout before replying".into(),
            });
        }
        let consumed = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |position| position + 1);
        if output.len().saturating_add(consumed) > MAX_MESSAGE_SIZE {
            return Err(MemoryError::McpProtocol {
                operation,
                message: format!("response exceeded {MAX_MESSAGE_SIZE} bytes"),
            });
        }
        let complete = buffer.get(consumed.saturating_sub(1)) == Some(&b'\n');
        output.extend_from_slice(&buffer[..consumed]);
        reader.consume(consumed);
        if complete {
            return Ok(output);
        }
    }
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse<T> {
    jsonrpc: String,
    id: u64,
    result: Option<T>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializeResult {
    protocol_version: String,
    server_info: ServerInfo,
}

#[derive(Debug, Deserialize)]
struct ServerInfo {
    name: String,
    version: String,
}

#[derive(Debug, Deserialize)]
struct ToolResult {
    content: Vec<TextContent>,
    #[serde(rename = "isError", default)]
    is_error: bool,
}

#[derive(Debug, Deserialize)]
struct TextContent {
    text: String,
}
