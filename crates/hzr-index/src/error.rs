use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidInput,
    Io,
    CommandUnavailable,
    CommandFailed,
    DeadlineExceeded,
    UnsupportedVersion,
    InvalidEngineOutput,
    NotInitialized,
    DuplicateIndexes,
    InvalidIndexPlacement,
    IndexOwnerBusy,
    WatchExited,
    UnsupportedWatchTopology,
    MigrationConflict,
    LegacyIndexRequiresMigration,
}

#[derive(Debug, Error)]
pub enum IndexError {
    #[error("invalid {field}: {reason}")]
    InvalidInput { field: &'static str, reason: String },

    #[error("{operation} failed for {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot start {program} for {operation}: {source}")]
    CommandUnavailable {
        operation: &'static str,
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{operation} failed with exit code {code:?}: {stderr}")]
    CommandFailed {
        operation: &'static str,
        code: Option<i32>,
        stderr: String,
    },

    #[error("{operation} exceeded its {duration:?} deadline")]
    DeadlineExceeded {
        operation: &'static str,
        duration: Duration,
    },

    #[error("grepai {found} is unsupported; HZR requires exactly {expected}")]
    UnsupportedVersion {
        expected: &'static str,
        found: String,
    },

    #[error("invalid {engine} output during {operation}: {detail}")]
    InvalidEngineOutput {
        engine: &'static str,
        operation: &'static str,
        detail: String,
    },

    #[error("grepai is not initialized at {config_path}")]
    NotInitialized { config_path: PathBuf },

    #[error(
        "non-canonical grepai indexes found; canonical={canonical:?}, duplicates={duplicates:?}"
    )]
    DuplicateIndexes {
        canonical: PathBuf,
        duplicates: Vec<PathBuf>,
    },

    #[error("project grepai symlink {link} targets {target}, expected {expected}")]
    ForeignIndexSymlink {
        link: PathBuf,
        target: PathBuf,
        expected: PathBuf,
    },

    #[error("project grepai entry is neither a directory nor a managed symlink: {path}")]
    IndexEntryConflict { path: PathBuf },

    #[error("grepai index writer is already owned; lock={lock_path}")]
    IndexOwnerBusy { lock_path: PathBuf },

    #[error("grepai watch exited before becoming ready with code {code:?}; log={log_path}")]
    WatchExited {
        code: Option<i32>,
        log_path: PathBuf,
    },

    #[error(
        "grepai watch would fan out across {worktrees} worktrees; engine lacks {required_flag}"
    )]
    UnsupportedWatchTopology {
        worktrees: usize,
        required_flag: &'static str,
    },

    #[error("grepai index migration conflict: {reason}")]
    MigrationConflict { reason: String },

    #[error(
        "legacy project-local grepai index at {directory} must be centralized with `hzr migrate apply --workspace {workspace}`"
    )]
    LegacyIndexRequiresMigration {
        directory: PathBuf,
        workspace: PathBuf,
    },
}

impl IndexError {
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidInput { .. } => ErrorCode::InvalidInput,
            Self::Io { .. } => ErrorCode::Io,
            Self::CommandUnavailable { .. } => ErrorCode::CommandUnavailable,
            Self::CommandFailed { .. } => ErrorCode::CommandFailed,
            Self::DeadlineExceeded { .. } => ErrorCode::DeadlineExceeded,
            Self::UnsupportedVersion { .. } => ErrorCode::UnsupportedVersion,
            Self::InvalidEngineOutput { .. } => ErrorCode::InvalidEngineOutput,
            Self::NotInitialized { .. } => ErrorCode::NotInitialized,
            Self::DuplicateIndexes { .. } => ErrorCode::DuplicateIndexes,
            Self::ForeignIndexSymlink { .. } | Self::IndexEntryConflict { .. } => {
                ErrorCode::InvalidIndexPlacement
            }
            Self::IndexOwnerBusy { .. } => ErrorCode::IndexOwnerBusy,
            Self::WatchExited { .. } => ErrorCode::WatchExited,
            Self::UnsupportedWatchTopology { .. } => ErrorCode::UnsupportedWatchTopology,
            Self::MigrationConflict { .. } => ErrorCode::MigrationConflict,
            Self::LegacyIndexRequiresMigration { .. } => ErrorCode::LegacyIndexRequiresMigration,
        }
    }

    pub const fn recoverable(&self) -> bool {
        matches!(
            self,
            Self::CommandFailed { .. }
                | Self::DeadlineExceeded { .. }
                | Self::NotInitialized { .. }
                | Self::IndexOwnerBusy { .. }
                | Self::WatchExited { .. }
                | Self::UnsupportedWatchTopology { .. }
                | Self::LegacyIndexRequiresMigration { .. }
        )
    }
}

pub type Result<T> = std::result::Result<T, IndexError>;
