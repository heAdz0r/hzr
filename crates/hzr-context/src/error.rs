use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("invalid context {field}: {reason}")]
    InvalidRequest { field: &'static str, reason: String },

    #[error("context index lifecycle failed: {0}")]
    Index(#[from] hzr_index::IndexError),

    #[error("managed fork-core is unavailable: {0}")]
    ForkUnavailable(String),

    /// The canonical semantic index is not usable *for this request*. Distinct from
    /// `Index`, which is a lifecycle failure: this one is expected during a cold warm-up
    /// and must degrade to exact search rather than fail the request.
    #[error("canonical semantic index is not ready: {0}")]
    IndexNotReady(String),

    #[error("managed fork-core invocation failed: {0}")]
    Fork(#[from] hzr_exec::ExecError),

    #[error("fork-core accounting registration failed: {0}")]
    Accounting(#[from] hzr_core::AccountingCoverageError),

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
