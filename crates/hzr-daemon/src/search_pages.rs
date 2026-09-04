//! Private immutable search pages. A cursor never invokes an engine or refreshes source content.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use hzr_protocol::{SearchApiRequest, SearchApiResponse, SearchPage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::ApiError;

pub const SNAPSHOT_HITS: usize = 100;
const TTL_MS: u64 = 15 * 60 * 1000;
const MAX_RECORD_BYTES: u64 = 2 * 1024 * 1024;
const MAX_RECORDS: usize = 128;
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u32,
    binding: String,
    expires_at_ms: u64,
    response: SearchApiResponse,
}

pub fn publish(
    data: &Path,
    request: &SearchApiRequest,
    response: SearchApiResponse,
) -> Result<SearchApiResponse, ApiError> {
    publish_at(data, request, response, now_ms()?)
}

pub fn read(data: &Path, request: &SearchApiRequest) -> Result<SearchApiResponse, ApiError> {
    read_at(data, request, now_ms()?)
}

fn now_ms() -> Result<u64, ApiError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_millis() as u64)
        .map_err(|_| ApiError::internal("system clock precedes epoch"))
}

fn directory(data: &Path) -> PathBuf {
    data.join("search-pages")
}

fn binding(request: &SearchApiRequest) -> Result<String, ApiError> {
    let identity = serde_json::to_vec(&(
        &request.workspace,
        &request.query,
        &request.path,
        request.limit,
        request.mode,
        request.include_content,
    ))
    .map_err(|_| ApiError::internal("cannot serialize search identity"))?;
    Ok(hex::encode(Sha256::digest(identity)))
}

fn publish_at(
    data: &Path,
    request: &SearchApiRequest,
    response: SearchApiResponse,
    now: u64,
) -> Result<SearchApiResponse, ApiError> {
    let directory = directory(data);
    fs::create_dir_all(&directory).map_err(cache_error)?;
    if !fs::symlink_metadata(&directory)
        .map_err(cache_error)?
        .file_type()
        .is_dir()
    {
        return Err(ApiError::bad_request(
            "search snapshot directory must not be a symlink",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).map_err(cache_error)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let lock = options
        .open(directory.join("store.lock"))
        .map_err(cache_error)?;
    fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| {
        ApiError::service(
            "search_snapshot_busy",
            "search snapshot publication is busy",
            true,
        )
    })?;
    let snapshot = Snapshot {
        version: 1,
        binding: binding(request)?,
        expires_at_ms: now.saturating_add(TTL_MS),
        response,
    };
    let bytes = serde_json::to_vec(&snapshot)
        .map_err(|_| ApiError::internal("cannot serialize search snapshot"))?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(ApiError::bad_request(
            "search snapshot exceeds 2 MiB; use a narrower scope or omit content",
        ));
    }
    let mut records = 0;
    let mut total = bytes.len() as u64;
    for (index, entry) in fs::read_dir(&directory).map_err(cache_error)?.enumerate() {
        if index > MAX_RECORDS * 2 {
            return Err(cache_full());
        }
        let entry = entry.map_err(cache_error)?;
        if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let file_name = entry.file_name();
        let Some(id) = file_name
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
        else {
            continue;
        };
        if !valid_id(id) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(cache_error)?;
        if !metadata.file_type().is_file() {
            return Err(cache_full());
        }
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|time| time.as_millis() as u64);
        if modified.is_some_and(|modified| now.saturating_sub(modified) >= TTL_MS) {
            fs::remove_file(entry.path()).map_err(cache_error)?;
            continue;
        }
        records += 1;
        total = total.saturating_add(metadata.len());
    }
    if records >= MAX_RECORDS || total > MAX_TOTAL_BYTES {
        return Err(cache_full());
    }
    let id = Uuid::new_v4().simple().to_string();
    let mut temporary = tempfile::NamedTempFile::new_in(&directory).map_err(cache_error)?;
    temporary.write_all(&bytes).map_err(cache_error)?;
    temporary.as_file().sync_all().map_err(cache_error)?;
    temporary
        .persist(directory.join(format!("{id}.json")))
        .map_err(|error| cache_error(error.error))?;
    page(snapshot, &id, 0, request.limit)
}

fn read_at(
    data: &Path,
    request: &SearchApiRequest,
    now: u64,
) -> Result<SearchApiResponse, ApiError> {
    let cursor = request
        .cursor
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("search cursor is required"))?;
    let (id, offset) = cursor
        .split_once(':')
        .ok_or_else(|| ApiError::bad_request("malformed search cursor"))?;
    if !valid_id(id) || cursor.len() > 128 {
        return Err(ApiError::bad_request("malformed search cursor"));
    }
    let offset = offset
        .parse::<usize>()
        .map_err(|_| ApiError::bad_request("malformed search cursor offset"))?;
    let bytes = hzr_core::read_bounded_regular_file(
        &directory(data).join(format!("{id}.json")),
        MAX_RECORD_BYTES,
    )
    .map_err(|_| {
        ApiError::not_found(
            "search_snapshot_expired",
            "search snapshot is unavailable or expired; explicitly start a new search",
        )
    })?;
    let snapshot: Snapshot = serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::bad_request("search snapshot is invalid"))?;
    if snapshot.version != 1 || snapshot.expires_at_ms <= now {
        return Err(ApiError::not_found(
            "search_snapshot_expired",
            "search snapshot expired; explicitly start a new search",
        ));
    }
    if snapshot.binding != binding(request)? {
        return Err(ApiError::bad_request(
            "search cursor does not match workspace/query/path/mode/content/page size",
        ));
    }
    if offset >= snapshot.response.hits.len() {
        return Err(ApiError::bad_request(
            "search cursor offset is outside the snapshot",
        ));
    }
    page(snapshot, id, offset, request.limit)
}

fn page(
    mut snapshot: Snapshot,
    id: &str,
    offset: usize,
    limit: usize,
) -> Result<SearchApiResponse, ApiError> {
    if limit == 0 || limit > SNAPSHOT_HITS {
        return Err(ApiError::bad_request("search page size must be 1..100"));
    }
    let available_hits = snapshot.response.hits.len();
    let end = offset.saturating_add(limit).min(available_hits);
    let next_cursor = (end < available_hits).then(|| format!("{id}:{end}"));
    let complete = available_hits >= snapshot.response.total_hits;
    snapshot.response.hits = snapshot
        .response
        .hits
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect();
    snapshot.response.shown_hits = snapshot.response.hits.len();
    snapshot.response.next_step = next_cursor.as_ref().map(|cursor| format!("Continue this immutable snapshot with --cursor {cursor} and the same query, scope and options."))
        .or_else(|| (!complete).then(|| "Snapshot exhausted at 100 hits; narrow the query/scope for additional matches. No automatic rescan occurred.".into()));
    snapshot.response.page = Some(SearchPage {
        snapshot_id: id.to_owned(),
        offset,
        available_hits,
        snapshot_complete: complete,
        next_cursor,
        expires_at_ms: snapshot.expires_at_ms,
    });
    Ok(snapshot.response)
}

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn cache_full() -> ApiError {
    ApiError::service(
        "search_snapshot_capacity",
        "private search snapshot capacity reached; wait for expiry or search without pagination",
        true,
    )
}
fn cache_error(error: std::io::Error) -> ApiError {
    ApiError::service(
        "search_snapshot_io",
        format!("search snapshot storage failed: {error}"),
        true,
    )
}

#[cfg(test)]
mod tests;
