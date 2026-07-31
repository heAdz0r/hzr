use std::io;
use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExecError {
    #[error("command program cannot be empty")]
    EmptyProgram,
    #[error("shell program cannot be empty")]
    EmptyShell,
    #[error("capture memory limit must be greater than zero")]
    InvalidMemoryLimit,
    #[error("capture byte limit must be greater than zero")]
    InvalidCaptureLimit,
    #[error("capture memory limit cannot exceed the total byte limit")]
    MemoryLimitExceedsCaptureLimit,
    #[error("stream event buffer must be greater than zero")]
    InvalidEventBuffer,
    #[error("failed to create spill directory {path}: {source}")]
    CreateSpillDirectory { path: PathBuf, source: io::Error },
    #[error("failed to open spill file {path}: {source}")]
    OpenSpill { path: PathBuf, source: io::Error },
    #[error("failed to write spill file {path}: {source}")]
    WriteSpill { path: PathBuf, source: io::Error },
    #[error("failed to flush spill file {path}: {source}")]
    FlushSpill { path: PathBuf, source: io::Error },
    #[error("capture spill state was unavailable after initialization")]
    MissingSpillState,
    #[error("failed to configure stdin for {program}: {source}")]
    ConfigureStdin { program: String, source: io::Error },
    #[error("failed to write stdin for {program}: {source}")]
    WriteStdin { program: String, source: io::Error },
    #[error("failed to spawn {program}: {source}")]
    Spawn { program: String, source: io::Error },
    #[error("child process {program} did not expose captured {stream}")]
    MissingPipe {
        program: String,
        stream: &'static str,
    },
    #[error("failed to wait for {program}: {source}")]
    Wait { program: String, source: io::Error },
    #[error("managed fork-core runtime paths were not configured")]
    MissingForkRuntimePaths,
    #[error("fork-core runtime path has no parent directory: {path}")]
    InvalidForkRuntimePath { path: PathBuf },
    #[error("fork-core binary path has no parent directory: {path}")]
    InvalidForkBinaryPath { path: PathBuf },
    #[error("fork-core binary path is not valid UTF-8: {path}")]
    NonUtf8ForkBinaryPath { path: PathBuf },
    #[error("fork-core runtime path is not valid UTF-8: {path}")]
    NonUtf8ForkRuntimePath { path: PathBuf },
    #[error("failed to construct fork-core PATH: {reason}")]
    InvalidForkPathEnvironment { reason: String },
    #[error("failed to prepare private fork-core runtime path {path}: {source}")]
    PrepareForkRuntime { path: PathBuf, source: io::Error },
    #[error("managed fork-core is unavailable: {reason}")]
    ForkCoreUnavailable { reason: String },
    #[error("failed to join {task} task: {source}")]
    Join {
        task: &'static str,
        source: tokio::task::JoinError,
    },
    #[error("RTK adapter output was not valid UTF-8")]
    InvalidRtkUtf8,
    #[error("RTK adapter returned invalid JSON: {0}")]
    InvalidRtkJson(#[from] serde_json::Error),
    #[error("execution completion channel closed unexpectedly")]
    CompletionClosed,
}
