use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("invalid context {field}: {reason}")]
    InvalidRequest { field: &'static str, reason: String },

    #[error("context index lifecycle failed: {0}")]
    Index(#[from] hzr_index::IndexError),

    #[error("managed fork-core is unavailable: {0}")]
    ForkUnavailable(String),

    #[error("managed fork-core invocation failed: {0}")]
    Fork(#[from] hzr_exec::ExecError),

    #[error("fork-core {operation} failed with exit code {exit_code:?}: {stderr}")]
    ForkCommand {
        operation: &'static str,
        exit_code: Option<i32>,
        stderr: String,
    },

    #[error("invalid fork-core output during {operation}: {detail}")]
    InvalidForkOutput {
        operation: &'static str,
        detail: String,
    },

    #[error("context planning invariant failed: {0}")]
    Invariant(String),
}

pub type Result<T> = std::result::Result<T, ContextError>;
