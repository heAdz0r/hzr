use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;

use crate::error::MemoryError;
use crate::installation::bounded_text;

pub(crate) const RESPONSE_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub(crate) enum AttemptError {
    Connect(reqwest::Error),
    Timeout,
    Request(reqwest::Error),
    Status(StatusCode, String),
    Decode(serde_json::Error),
    TooLarge(usize),
}

impl AttemptError {
    pub(crate) fn affects_availability(&self) -> bool {
        matches!(
            self,
            Self::Connect(_)
                | Self::Timeout
                | Self::Request(_)
                | Self::Decode(_)
                | Self::TooLarge(_)
        ) || matches!(self, Self::Status(status, _) if status.is_server_error())
    }

    pub(crate) fn safe_recall_fallback(&self) -> bool {
        self.affects_availability()
    }

    pub(crate) fn safe_store_fallback(&self) -> bool {
        matches!(self, Self::Connect(_))
    }

    pub(crate) fn into_public(self, operation: &'static str, timeout: Duration) -> MemoryError {
        match self {
            Self::Connect(source) | Self::Request(source) => {
                MemoryError::HttpRequest { operation, source }
            }
            Self::Timeout => MemoryError::HttpTimeout { operation, timeout },
            Self::Status(status, body) => MemoryError::HttpStatus {
                operation,
                status,
                body,
            },
            Self::Decode(source) => MemoryError::Protocol { operation, source },
            Self::TooLarge(limit) => MemoryError::ResponseTooLarge { operation, limit },
        }
    }
}

pub(crate) async fn get_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    token: &str,
    base_url: &str,
    path: &str,
) -> std::result::Result<T, AttemptError> {
    let request = client
        .request(Method::GET, format!("{base_url}{path}"))
        .bearer_auth(token)
        .header(reqwest::header::ACCEPT, "application/json");
    decode_response(request.send().await, RESPONSE_LIMIT).await
}

pub(crate) async fn post_json<T: DeserializeOwned, B: serde::Serialize + ?Sized>(
    client: &reqwest::Client,
    token: &str,
    base_url: &str,
    path: &str,
    body: &B,
) -> std::result::Result<T, AttemptError> {
    let request = client
        .request(Method::POST, format!("{base_url}{path}"))
        .bearer_auth(token)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(body);
    decode_response(request.send().await, RESPONSE_LIMIT).await
}

async fn decode_response<T: DeserializeOwned>(
    response: std::result::Result<reqwest::Response, reqwest::Error>,
    limit: usize,
) -> std::result::Result<T, AttemptError> {
    let mut response = response.map_err(|error| {
        if error.is_timeout() {
            AttemptError::Timeout
        } else if error.is_connect() {
            AttemptError::Connect(error)
        } else {
            AttemptError::Request(error)
        }
    })?;
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(AttemptError::TooLarge(limit));
    }
    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or_default()
            .min(limit as u64) as usize,
    );
    loop {
        let chunk = response.chunk().await.map_err(|error| {
            if error.is_timeout() {
                AttemptError::Timeout
            } else {
                AttemptError::Request(error)
            }
        })?;
        let Some(chunk) = chunk else {
            break;
        };
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(AttemptError::TooLarge(limit));
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        return Err(AttemptError::Status(status, bounded_text(&bytes, 8 * 1024)));
    }
    serde_json::from_slice(&bytes).map_err(AttemptError::Decode)
}
