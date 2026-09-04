use sha2::{Digest, Sha256};
use std::io::Read;

use axum::{Json, extract::State};
use hzr_exec::{CapturedContent, ExecJobState, ExecutionOutcome};
use hzr_protocol::{
    ExecJobApiRequest, ExecOutputApiRequest, ExecOutputApiResponse, ExecOutputStream,
};

use crate::{AppState, error::ApiError};

const DEFAULT_BYTES: u64 = 32 * 1024;
const MAX_BYTES: u64 = 256 * 1024;

pub(crate) async fn read_output(
    State(state): State<AppState>,
    Json(request): Json<ExecOutputApiRequest>,
) -> Result<Json<ExecOutputApiResponse>, ApiError> {
    tokio::task::spawn_blocking(move || read_page(&state, request))
        .await
        .map_err(|error| ApiError::internal(format!("output reader interrupted: {error}")))?
}

fn read_page(
    state: &AppState,
    request: ExecOutputApiRequest,
) -> Result<Json<ExecOutputApiResponse>, ApiError> {
    let limit = request.max_bytes.unwrap_or(DEFAULT_BYTES);
    if !(1..=MAX_BYTES).contains(&limit) {
        return Err(ApiError::bad_request(
            "max_bytes must be between 1 and 262144",
        ));
    }
    let record = state.exec_jobs.scoped_record(&ExecJobApiRequest {
        operation_id: request.operation_id.clone(),
        cwd: request.cwd.clone(),
        wait_ms: None,
        after_revision: None,
        max_output_bytes: None,
    })?;
    if record.snapshot.state == ExecJobState::Running {
        return Err(ApiError::service(
            "execution_output_pending",
            "wait for command completion before reading captured output",
            true,
        ));
    }
    let result = match record.snapshot.outcome.as_ref() {
        Some(
            ExecutionOutcome::Completed { result }
            | ExecutionOutcome::ExecutedAccountingIncomplete { result, .. },
        ) => result,
        _ => {
            return Err(ApiError::not_found(
                "execution_output_unavailable",
                "this operation has no captured output",
            ));
        }
    };
    let stream = match request.stream {
        ExecOutputStream::Stdout => &result.stdout,
        ExecOutputStream::Stderr => &result.stderr,
    };
    if stream.stored_bytes > 64 * 1024 * 1024 {
        return Err(ApiError::internal(
            "stored output exceeds the capture limit",
        ));
    }
    if request.offset > stream.stored_bytes {
        return Err(ApiError::bad_request("offset exceeds captured output"));
    }
    let length = limit.min(stream.stored_bytes - request.offset);
    let mut bytes = Vec::with_capacity(length as usize);
    let mut digest = Sha256::new();
    match &stream.content {
        CapturedContent::Inline { bytes: stored } => {
            if stored.len() as u64 != stream.stored_bytes {
                return Err(ApiError::internal(
                    "inline output length does not match its receipt",
                ));
            }
            digest.update(stored);
            bytes.extend_from_slice(
                &stored[request.offset as usize..(request.offset + length) as usize],
            );
        }
        CapturedContent::Spilled { path } => {
            let expected_directory = state
                .exec_jobs
                .directory
                .join(format!("{}.output", request.operation_id))
                .canonicalize()
                .map_err(|error| {
                    ApiError::internal(format!("owned output directory unavailable: {error}"))
                })?;
            let canonical_root = state
                .exec_jobs
                .directory
                .canonicalize()
                .map_err(|error| ApiError::internal(error.to_string()))?;
            if expected_directory != canonical_root.join(format!("{}.output", request.operation_id))
            {
                return Err(ApiError::internal(
                    "operation output directory is not owned by this job",
                ));
            }
            let canonical = path.canonicalize().map_err(|error| {
                ApiError::internal(format!("captured output unavailable: {error}"))
            })?;
            if canonical.parent() != Some(expected_directory.as_path()) {
                return Err(ApiError::internal(
                    "captured output is outside its operation directory",
                ));
            }
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW);
            }
            let mut file = options
                .open(path)
                .map_err(|error| ApiError::internal(format!("open captured output: {error}")))?;
            let metadata = file
                .metadata()
                .map_err(|error| ApiError::internal(error.to_string()))?;
            if !metadata.is_file() || metadata.len() != stream.stored_bytes {
                return Err(ApiError::internal(
                    "captured output length does not match its receipt",
                ));
            }
            let mut position = 0u64;
            let mut buffer = [0u8; 16 * 1024];
            loop {
                let count = file
                    .read(&mut buffer)
                    .map_err(|error| ApiError::internal(error.to_string()))?;
                if count == 0 {
                    break;
                }
                let end = position + count as u64;
                if end > stream.stored_bytes {
                    return Err(ApiError::internal("captured output grew during retrieval"));
                }
                digest.update(&buffer[..count]);
                let start_in_chunk =
                    request.offset.saturating_sub(position).min(count as u64) as usize;
                let end_in_chunk = (request.offset + length)
                    .saturating_sub(position)
                    .min(count as u64) as usize;
                if start_in_chunk < end_in_chunk {
                    bytes.extend_from_slice(&buffer[start_in_chunk..end_in_chunk]);
                }
                position = end;
            }
            let after = file
                .metadata()
                .map_err(|error| ApiError::internal(error.to_string()))?;
            if position != stream.stored_bytes
                || bytes.len() as u64 != length
                || after.len() != metadata.len()
                || after.modified().ok() != metadata.modified().ok()
            {
                return Err(ApiError::internal(
                    "captured output changed during retrieval",
                ));
            }
        }
    }
    let source_sha256 = hex::encode(digest.finalize());
    if (!stream.truncated && source_sha256 != stream.sha256)
        || request
            .expected_sha256
            .as_ref()
            .is_some_and(|hash| hash != &source_sha256)
    {
        return Err(ApiError::service(
            "execution_output_changed",
            "captured output hash does not match the requested result",
            false,
        ));
    }
    // A continuation starts at a UTF-8 boundary when a prefix can be returned exactly.
    // Arbitrary offsets and binary data use lossless hex instead of replacement characters.
    if let Err(error) = std::str::from_utf8(&bytes) {
        if error.error_len().is_none() && error.valid_up_to() > 0 {
            bytes.truncate(error.valid_up_to());
        }
    }
    let next = request.offset + bytes.len() as u64;
    let (encoding, content) = match String::from_utf8(bytes) {
        Ok(text) => ("utf8".to_owned(), text),
        Err(error) => ("hex".to_owned(), hex::encode(error.into_bytes())),
    };
    Ok(Json(ExecOutputApiResponse {
        operation_id: request.operation_id,
        revision: record.snapshot.revision,
        stream: request.stream,
        offset: request.offset,
        next_offset: (next < stream.stored_bytes).then_some(next),
        total_bytes: stream.total_bytes,
        stored_bytes: stream.stored_bytes,
        source_sha256,
        capture_truncated: stream.truncated,
        complete: request.offset == 0 && next == stream.stored_bytes && !stream.truncated,
        encoding,
        content,
    }))
}
