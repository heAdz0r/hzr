use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::time::Duration;

use thiserror::Error;

pub type Result<T, E = MemoryError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("invalid ICM configuration: {0}")]
    InvalidConfig(String),

    #[error("unable to determine the HZR data directory")]
    DataDirectoryUnavailable,

    #[error("{operation} failed for {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("ICM executable could not be started at {executable}: {source}")]
    BinaryUnavailable {
        executable: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("ICM version probe timed out after {timeout:?}")]
    VersionProbeTimeout { timeout: Duration },

    #[error("ICM version probe exited with {status}: {stderr}")]
    VersionProbeFailed { status: ExitStatus, stderr: String },

    #[error("unexpected ICM version output: {output:?}")]
    InvalidVersionOutput { output: String },

    #[error("ICM version mismatch: expected {expected}, found {actual}")]
    VersionMismatch {
        expected: &'static str,
        actual: String,
    },

    #[error("ICM executable checksum mismatch: expected {expected}, found {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("checksum task failed: {0}")]
    ChecksumTask(String),

    #[error("another HZR process owns the ICM supervisor lock at {lock_path}")]
    SupervisorLockHeld { lock_path: PathBuf },

    #[error("ICM process failed to start: {source}")]
    ProcessSpawn {
        #[source]
        source: io::Error,
    },

    #[error("ICM process exited before readiness with {status}")]
    ProcessExited { status: ExitStatus },

    #[error("ICM did not become ready at {endpoint} within {timeout:?}")]
    StartupTimeout { endpoint: String, timeout: Duration },

    #[error("ICM process operation failed: {source}")]
    ProcessControl {
        #[source]
        source: io::Error,
    },

    #[error("cannot restart an ICM process supervised by another HZR instance")]
    NotProcessOwner,

    #[error("ICM HTTP {operation} timed out after {timeout:?}")]
    HttpTimeout {
        operation: &'static str,
        timeout: Duration,
    },

    #[error("ICM HTTP {operation} failed: {source}")]
    HttpRequest {
        operation: &'static str,
        #[source]
        source: reqwest::Error,
    },

    #[error("ICM HTTP {operation} returned status {status}: {body}")]
    HttpStatus {
        operation: &'static str,
        status: reqwest::StatusCode,
        body: String,
    },

    #[error("invalid ICM {operation} response: {source}")]
    Protocol {
        operation: &'static str,
        #[source]
        source: serde_json::Error,
    },

    #[error("unexpected ICM {operation} response: {message}")]
    UnexpectedResponse {
        operation: &'static str,
        message: String,
    },

    #[error("ICM response to {operation} exceeded {limit} bytes")]
    ResponseTooLarge {
        operation: &'static str,
        limit: usize,
    },

    #[error("ICM circuit breaker is open for another {retry_after:?}")]
    CircuitOpen { retry_after: Duration },

    #[error("ICM CLI {operation} timed out after {timeout:?}")]
    CliTimeout {
        operation: &'static str,
        timeout: Duration,
    },

    #[error("ICM CLI {operation} exited with {status}: {stderr}")]
    CliFailed {
        operation: &'static str,
        status: ExitStatus,
        stderr: String,
    },

    #[error("ICM request is invalid: {0}")]
    InvalidRequest(String),

    #[error("ICM snapshot database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("ICM MCP transport is not connected")]
    McpUnavailable,

    #[error("ICM MCP {operation} timed out after {timeout:?}")]
    McpTimeout {
        operation: &'static str,
        timeout: Duration,
    },

    #[error("ICM MCP {operation} I/O failed: {source}")]
    McpIo {
        operation: &'static str,
        #[source]
        source: io::Error,
    },

    #[error("ICM MCP {operation} returned an invalid JSON-RPC response: {message}")]
    McpProtocol {
        operation: &'static str,
        message: String,
    },

    #[error("ICM MCP {operation} failed with JSON-RPC error {code}: {message}")]
    McpRemote {
        operation: &'static str,
        code: i64,
        message: String,
    },

    #[error("ICM MCP tool {tool} failed: {message}")]
    McpTool { tool: &'static str, message: String },

    #[error("ICM operation {operation} is unavailable over {transport}")]
    UnsupportedTransport {
        operation: &'static str,
        transport: &'static str,
    },
}
