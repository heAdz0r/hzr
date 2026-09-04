use super::{canonical_workspace, fork_run, validate_managed_fork_tool};
use crate::{AppState, error::ApiError};
use axum::{Json, extract::State};
use hzr_protocol::{
    CommandTermination, ForkRunApiRequest, ReadApiRequest, ReadApiResponse, ReadFileResult,
};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

const MAX_SOURCE_BYTES: u64 = 4 * 1024 * 1024;

#[cfg(all(test, unix))]
mod tests;

pub async fn read_files(
    State(state): State<AppState>,
    Json(request): Json<ReadApiRequest>,
) -> Result<Json<ReadApiResponse>, ApiError> {
    let limit = request.max_tokens.unwrap_or(8192);
    if !(1024..=48_000).contains(&limit) || request.paths.is_empty() || request.paths.len() > 32 {
        return Err(ApiError::bad_request(
            "read requires 1..32 paths and a token budget of 1024..48000",
        ));
    }
    let from = request.from.unwrap_or(1);
    if from == 0 || request.to.is_some_and(|to| to < from) || request.max_lines == Some(0) {
        return Err(ApiError::bad_request(
            "read ranges must be positive and ordered",
        ));
    }
    if request.expected_sha256.is_some() && request.paths.len() != 1 {
        return Err(ApiError::bad_request(
            "expected_sha256 requires exactly one path",
        ));
    }
    if request
        .paths
        .iter()
        .any(|path| path.trim().is_empty() || path.len() > 4096)
    {
        return Err(ApiError::bad_request(
            "read paths must contain 1..4096 UTF-8 bytes",
        ));
    }
    if request
        .expected_sha256
        .as_ref()
        .is_some_and(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(ApiError::bad_request(
            "expected_sha256 must be 64 hexadecimal digits",
        ));
    }
    if request
        .context_epoch
        .as_ref()
        .is_some_and(|epoch| epoch.is_empty() || epoch.len() > 128)
    {
        return Err(ApiError::bad_request(
            "context_epoch must contain 1..128 UTF-8 bytes",
        ));
    }
    if request.context_epoch.is_some()
        && request
            .session_id
            .as_ref()
            .is_none_or(|session| session.is_empty() || session.len() > 256)
    {
        return Err(ApiError::bad_request(
            "context_epoch requires a session_id of 1..256 UTF-8 bytes",
        ));
    }
    let advice_reserve = if request.context_epoch.is_some() {
        512
    } else {
        0
    };
    let cwd = canonical_workspace(&request.cwd)?;
    let mut response = ReadApiResponse {
        files: Vec::new(),
        remaining_paths: request.paths.clone(),
        estimated_tokens: 0,
        max_tokens: limit,
    };
    if serde_json::to_vec(&response)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .len() as u64
        + 128
        > limit * 4
    {
        return Err(ApiError::bad_request(
            "path metadata exceeds the read budget; reduce the batch",
        ));
    }
    for path in &request.paths {
        let mut args = vec!["read".into(), path.clone(), "--level".into(), "none".into()];
        validate_managed_fork_tool(&args, &cwd)?;
        let absolute = tokio::fs::canonicalize(cwd.join(path))
            .await
            .map_err(|error| ApiError::bad_request(format!("resolve source: {error}")))?;
        if !absolute.starts_with(&cwd) {
            return Err(ApiError::bad_request("read source escapes workspace"));
        }
        args[1] = absolute.to_string_lossy().into_owned();
        let bytes = read_bounded_source(&absolute).await?;
        let source = std::str::from_utf8(&bytes)
            .map_err(|_| ApiError::bad_request("typed read requires UTF-8"))?;
        let digest = hex::encode(Sha256::digest(&bytes));
        if request
            .expected_sha256
            .as_ref()
            .is_some_and(|expected| expected != &digest)
        {
            return Err(ApiError::service(
                "source_changed",
                "source hash changed; request fresh evidence before expanding",
                false,
            ));
        }
        let lines: Vec<&str> = source.split_inclusive('\n').collect();
        let total = lines.len() as u64;
        let end = request
            .to
            .unwrap_or(total)
            .min(total)
            .min(from.saturating_add(request.max_lines.unwrap_or(u64::MAX).saturating_sub(1)));
        let mut file = ReadFileResult {
            cost_advice: None,
            path: path.clone(),
            source_sha256: digest,
            source_bytes: bytes.len() as u64,
            total_lines: total,
            from,
            to: from.saturating_sub(1),
            next_line: (from <= total).then_some(from),
            complete: total == 0,
            content: String::new(),
        };
        response.remaining_paths.remove(0);
        response.files.push(file.clone());
        let base = serde_json::to_vec(&response)
            .map_err(|error| ApiError::internal(error.to_string()))?
            .len() as u64;
        response.files.pop();
        // Reserve the complete response envelope, not just raw source text. JSON escaping
        // can expand individual lines, so account for it before launching the fork.
        let available = limit
            .saturating_mul(4)
            .saturating_sub(base + 128 + advice_reserve);
        let mut used = 0;
        for number in from..=end {
            let line = lines[(number - 1) as usize];
            let encoded = serde_json::to_vec(line)
                .map_err(|error| ApiError::internal(error.to_string()))?
                .len() as u64;
            if used + encoded + 2 > available {
                break;
            }
            used += encoded + 2;
            file.to = number;
        }
        if file.to < from && from <= total {
            if response.files.is_empty() {
                return Err(ApiError::bad_request(
                    "one source line exceeds the read budget; increase max_tokens or use tracked exact CLI recovery",
                ));
            }
            response.remaining_paths.insert(0, path.clone());
            break;
        }
        if from > total {
            file.to = total;
            file.next_line = None;
        }
        if total != 0 && from <= total {
            args.extend([
                "--from".into(),
                from.to_string(),
                "--to".into(),
                file.to.to_string(),
            ]);
            let Json(result) = fork_run(
                State(state.clone()),
                Json(ForkRunApiRequest {
                    cwd: request.cwd.clone(),
                    args,
                    stdin: None,
                    timeout_ms: None,
                    agent: request.agent.clone(),
                    session_id: request.session_id.clone(),
                    managed_write: None,
                }),
            )
            .await?;
            if result.termination != CommandTermination::Exited
                || result.exit_code != Some(0)
                || result.stdout_truncated
            {
                return Err(ApiError::service(
                    "read_incomplete",
                    format!("fork read did not complete: {}", result.stderr),
                    true,
                ));
            }
            let after = read_bounded_source(&absolute).await?;
            if Sha256::digest(&after) != Sha256::digest(&bytes) {
                return Err(ApiError::service(
                    "source_changed",
                    "source changed during read; result was discarded",
                    true,
                ));
            }
            let expected: String = source
                .split_inclusive('\n')
                .skip((from - 1) as usize)
                .take((file.to - from + 1) as usize)
                .collect();
            if result.stdout != expected {
                return Err(ApiError::service(
                    "read_fidelity_invalid",
                    "fork output differs from the exact requested source range",
                    false,
                ));
            }
            file.content = result.stdout;
            file.next_line = (file.to < total).then_some(file.to + 1);
            file.complete = from == 1 && file.to == total;
        }
        if let (Some(epoch), Some(session)) = (&request.context_epoch, &request.session_id) {
            file.cost_advice = Some(
                state
                    .read_costs
                    .observe(
                        [
                            &cwd.to_string_lossy(),
                            &absolute.to_string_lossy(),
                            session,
                            epoch,
                            request.agent.as_deref().unwrap_or("unknown"),
                        ],
                        &file,
                        &lines,
                    )
                    .await,
            );
        }
        response.files.push(file);
        let encoded = serde_json::to_vec(&response)
            .map_err(|error| ApiError::internal(error.to_string()))?
            .len() as u64;
        if encoded + 32 > limit * 4 {
            response.files.pop();
            response.remaining_paths.insert(0, path.clone());
            break;
        }
    }
    response.estimated_tokens = (serde_json::to_vec(&response)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .len() as u64)
        .div_ceil(4);
    Ok(Json(response))
}

async fn read_bounded_source(path: &std::path::Path) -> Result<Vec<u8>, ApiError> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| ApiError::bad_request(format!("open source: {error}")))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|error| ApiError::bad_request(format!("read metadata: {error}")))?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES {
        return Err(ApiError::bad_request(
            "typed read requires a UTF-8 file no larger than 4 MiB",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| ApiError::bad_request(format!("read source: {error}")))?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(ApiError::bad_request(
            "source grew beyond the 4 MiB read limit",
        ));
    }
    Ok(bytes)
}
