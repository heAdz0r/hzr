use std::io;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use hzr_protocol::ErrorResponse;
use thiserror::Error;

use crate::DaemonLockError;

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("invalid HZR configuration: {0}")]
    Config(String),
    #[error("invalid engine lock: {0}")]
    EngineLock(toml::de::Error),
    #[error("failed to acquire daemon lifetime lock: {0}")]
    Lock(#[from] DaemonLockError),
    #[error("failed to initialize ICM: {0}")]
    Memory(hzr_memory::MemoryError),
    #[error("failed to stop context services: {0}")]
    Context(hzr_context::ContextError),
    #[error("daemon I/O error: {0}")]
    Io(io::Error),
}

pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    recoverable: bool,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            message: message.into(),
            recoverable: true,
        }
    }

    pub fn service(code: &'static str, message: impl Into<String>, recoverable: bool) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code,
            message: message.into(),
            recoverable,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: message.into(),
            recoverable: false,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let payload = ErrorResponse {
            trace_id: None,
            code: self.code.into(),
            message: self.message,
            recoverable: self.recoverable,
            details: serde_json::Value::Null,
        };
        (self.status, Json(payload)).into_response()
    }
}
