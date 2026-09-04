use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::{Json, extract::State};
use hzr_exec::{ExecJobDelivery, ExecJobSnapshot, ExecJobState};
use hzr_protocol::{ExecJobApiRequest, ExecStartApiRequest};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use tokio::sync::{Mutex, watch};
use uuid::Uuid;

use crate::{AppState, api, error::ApiError};

mod output;
pub(crate) use output::read_output;

#[cfg(all(test, unix))]
mod tests;

const MAX_JOB_MS: u64 = 30 * 60 * 1000;
const DEFAULT_JOB_MS: u64 = 30 * 60 * 1000;
const MAX_ACTIVE_JOBS: usize = 32;
const MAX_RECORD_BYTES: u64 = 8 * 1024 * 1024;
const MAX_STORE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_JOB_OUTPUT_BYTES: u64 = 2 * 64 * 1024 * 1024;
const MAX_RECORDS: usize = 20_000;
const DEFAULT_OUTPUT_BYTES: u64 = 64 * 1024;

#[derive(Clone)]
pub(crate) struct ExecJobs {
    directory: PathBuf,
    active: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
}

#[derive(Serialize, Deserialize)]
struct JobRecord {
    workspace: PathBuf,
    request_hash: String,
    snapshot: ExecJobSnapshot,
}

impl ExecJobs {
    pub(crate) fn new(data_root: &Path) -> io::Result<Self> {
        let directory = data_root.join("runtime/exec-jobs");
        fs::create_dir_all(&directory)?;
        if !fs::symlink_metadata(&directory)?.file_type().is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "execution store must be a real directory",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        let jobs = Self {
            directory,
            active: Arc::default(),
        };
        // Daemon lifetime ownership is acquired before initialization. Never replay a
        // persisted running command: an abrupt death leaves its side effects unknown.
        for entry in fs::read_dir(&jobs.directory)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                let mut record = jobs.read_record(&path)?;
                if record.snapshot.state == ExecJobState::Running {
                    record.snapshot.state = ExecJobState::Interrupted;
                    record.snapshot.revision += 1;
                    jobs.write_record(&record)?;
                }
            }
        }
        Ok(jobs)
    }

    fn path(&self, id: &str) -> Result<PathBuf, ApiError> {
        let uuid = Uuid::parse_str(id)
            .map_err(|_| ApiError::bad_request("operation_id must be a canonical UUID"))?;
        if uuid.to_string() != id {
            return Err(ApiError::bad_request(
                "operation_id must be a canonical UUID",
            ));
        }
        Ok(self.directory.join(format!("{id}.json")))
    }

    fn read_record(&self, path: &Path) -> io::Result<JobRecord> {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_RECORD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid execution record",
            ));
        }
        let mut bytes = Vec::new();
        file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "execution record grew beyond limit",
            ));
        }
        let record: JobRecord = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        let expected = self.path(&record.snapshot.operation_id).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid operation identity")
        })?;
        if expected != path {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "operation identity does not match path",
            ));
        }
        Ok(record)
    }

    fn write_record(&self, record: &JobRecord) -> io::Result<()> {
        let bytes = serde_json::to_vec(record).map_err(io::Error::other)?;
        if bytes.len() as u64 >= MAX_RECORD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "execution record exceeds durable limit",
            ));
        }
        let mut temporary = NamedTempFile::new_in(&self.directory)?;
        temporary.write_all(&bytes)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(
                self.directory
                    .join(format!("{}.json", record.snapshot.operation_id)),
            )
            .map_err(|error| error.error)?;
        #[cfg(unix)]
        fs::File::open(&self.directory)?.sync_all()?;
        Ok(())
    }

    fn scoped_record(&self, request: &ExecJobApiRequest) -> Result<JobRecord, ApiError> {
        let workspace = fs::canonicalize(&request.cwd)
            .map_err(|error| ApiError::bad_request(format!("invalid workspace: {error}")))?;
        let path = self.path(&request.operation_id)?;
        let record = self.read_record(&path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                ApiError::not_found("execution_not_found", "execution operation is unknown")
            } else {
                ApiError::internal(format!("read execution record: {error}"))
            }
        })?;
        if record.workspace != workspace {
            return Err(ApiError::not_found(
                "execution_not_found",
                "execution operation is unknown",
            ));
        }
        Ok(record)
    }

    fn reserve_record_capacity(&self, active_count: usize) -> Result<(), ApiError> {
        let entries =
            fs::read_dir(&self.directory).map_err(|error| ApiError::internal(error.to_string()))?;
        let mut bytes = 0u64;
        let mut count = 0usize;
        for entry in entries {
            let entry = entry.map_err(|error| ApiError::internal(error.to_string()))?;
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| ApiError::internal(error.to_string()))?;
            if metadata.file_type().is_symlink() {
                return Err(ApiError::internal("execution store contains a symlink"));
            }
            if metadata.is_dir() {
                // Capture spill files live one level below their owning operation ID.
                for child in fs::read_dir(entry.path())
                    .map_err(|error| ApiError::internal(error.to_string()))?
                {
                    let child = child.map_err(|error| ApiError::internal(error.to_string()))?;
                    let stored = fs::symlink_metadata(child.path())
                        .map_err(|error| ApiError::internal(error.to_string()))?;
                    if !stored.is_file() || stored.file_type().is_symlink() {
                        return Err(ApiError::internal("invalid execution output storage"));
                    }
                    bytes = bytes.saturating_add(stored.len());
                }
            } else {
                bytes = bytes.saturating_add(metadata.len());
            }
            count += 1;
        }
        // Reserve each active completion's worst-case record. Never evict an ID and replay effects.
        let reserved =
            (active_count as u64 + 1).saturating_mul(MAX_RECORD_BYTES + MAX_JOB_OUTPUT_BYTES);
        if count >= MAX_RECORDS || bytes.saturating_add(reserved) > MAX_STORE_BYTES {
            return Err(ApiError::service(
                "execution_store_capacity",
                "execution history capacity reached; records were retained to prevent replay",
                false,
            ));
        }
        Ok(())
    }

    fn inactive_snapshot(&self, mut record: JobRecord, active: bool) -> ExecJobSnapshot {
        if record.snapshot.state == ExecJobState::Running && !active {
            record.snapshot.state = ExecJobState::Interrupted;
            record.snapshot.revision += 1;
            record.snapshot.error = Some(ApiError::service("execution_completion_unknown",
                "execution stopped without a durable completion; inspect side effects before any new operation", false).payload());
            if let Err(error) = self.write_record(&record) {
                tracing::error!(%error, "cannot persist interrupted execution");
            }
        }
        record.snapshot
    }

    pub(crate) async fn shutdown(&self) {
        let active = self.active.lock().await;
        for cancellation in active.values() {
            let _ = cancellation.send(true);
        }
        drop(active);
        let drained = tokio::time::timeout(Duration::from_secs(5), async {
            while !self.active.lock().await.is_empty() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        if drained.is_err() {
            tracing::warn!("execution jobs did not finish within shutdown grace");
        }
    }
}

pub(crate) async fn start(
    State(state): State<AppState>,
    Json(request): Json<ExecStartApiRequest>,
) -> Result<Json<ExecJobSnapshot>, ApiError> {
    let jobs = state.exec_jobs.clone();
    let path = jobs.path(&request.operation_id)?;
    let workspace = fs::canonicalize(&request.request.cwd)
        .map_err(|error| ApiError::bad_request(format!("invalid workspace: {error}")))?;
    if !workspace.is_dir() || request.request.command.trim().is_empty() {
        return Err(ApiError::bad_request(
            "execution requires a workspace directory and command",
        ));
    }
    let timeout = request.request.timeout_ms.unwrap_or(DEFAULT_JOB_MS);
    if timeout == 0 || timeout > MAX_JOB_MS {
        return Err(ApiError::bad_request(
            "execution timeout must be between 1 and 1800000 milliseconds",
        ));
    }
    let request_hash = hzr_core::privacy_identity_hash(
        "exec_job",
        &serde_json::to_string(&request.request)
            .map_err(|error| ApiError::internal(error.to_string()))?,
    );
    let mut active = jobs.active.lock().await;
    match jobs.read_record(&path) {
        Ok(record) => {
            if record.workspace != workspace
                || (!record.request_hash.is_empty() && record.request_hash != request_hash)
            {
                return Err(ApiError::bad_request(
                    "operation_id is already bound to a different request",
                ));
            }
            let snapshot =
                jobs.inactive_snapshot(record, active.contains_key(&request.operation_id));
            return Ok(Json(deliver(snapshot, None, None)?));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ApiError::internal(format!(
                "read execution record: {error}"
            )));
        }
    }
    if active.len() >= MAX_ACTIVE_JOBS {
        return Err(ApiError::service(
            "execution_capacity",
            "32 execution jobs are already active",
            true,
        ));
    }
    jobs.reserve_record_capacity(active.len())?;
    let record = JobRecord {
        workspace,
        request_hash,
        snapshot: ExecJobSnapshot {
            delivery: None,
            operation_id: request.operation_id.clone(),
            state: ExecJobState::Running,
            revision: 1,
            outcome: None,
            error: None,
        },
    };
    jobs.write_record(&record).map_err(|error| {
        ApiError::internal(format!("persist execution before dispatch: {error}"))
    })?;
    let snapshot = record.snapshot.clone();
    let (cancellation, receiver) = watch::channel(false);
    active.insert(request.operation_id.clone(), cancellation);
    let executing_jobs = jobs.clone();
    let output_directory = jobs
        .directory
        .join(format!("{}.output", request.operation_id));
    tokio::spawn(async move {
        let outcome = tokio::spawn(api::execute_command(
            state,
            request.request,
            Some(timeout),
            Some(receiver),
            Some(output_directory),
        ))
        .await
        .map_err(|error| ApiError::internal(format!("execution task interrupted: {error}")))
        .and_then(|result| result);
        let mut record = record;
        let mut active = executing_jobs.active.lock().await;
        let cancelled = active
            .get(&request.operation_id)
            .is_some_and(|sender| *sender.borrow());
        record.snapshot.revision += 1;
        match outcome {
            Ok(Json(outcome)) => {
                record.snapshot.state = if cancelled {
                    ExecJobState::Cancelled
                } else {
                    ExecJobState::Completed
                };
                record.snapshot.outcome = Some(outcome);
            }
            Err(error) => {
                record.snapshot.state = ExecJobState::Failed;
                record.snapshot.error = Some(error.payload());
            }
        }
        if let Err(error) = executing_jobs.write_record(&record) {
            // The previous running record remains durable and must not be replayed.
            tracing::error!(%error, operation_id = %request.operation_id, "execution completion persistence failed");
        }
        active.remove(&request.operation_id);
    });
    Ok(Json(snapshot))
}

fn deliver(
    mut snapshot: ExecJobSnapshot,
    after_revision: Option<u64>,
    max_output_bytes: Option<u64>,
) -> Result<ExecJobSnapshot, ApiError> {
    let limit = max_output_bytes.unwrap_or(DEFAULT_OUTPUT_BYTES);
    if !(1024..=MAX_RECORD_BYTES).contains(&limit) {
        return Err(ApiError::bad_request(
            "max_output_bytes must be between 1024 and 8388608",
        ));
    }
    if after_revision.is_some_and(|revision| revision > snapshot.revision) {
        return Err(ApiError::bad_request(
            "after_revision exceeds the current revision",
        ));
    }
    let required_bytes = snapshot
        .outcome
        .as_ref()
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|error| ApiError::internal(error.to_string()))?
        .map_or(0, |bytes| bytes.len() as u64);
    let unchanged = after_revision == Some(snapshot.revision);
    let output_omitted = snapshot.outcome.is_some() && (unchanged || required_bytes > limit);
    if output_omitted {
        snapshot.outcome = None;
    }
    snapshot.delivery = Some(ExecJobDelivery {
        unchanged,
        output_omitted,
        required_bytes,
    });
    Ok(snapshot)
}

pub(crate) async fn wait(
    State(state): State<AppState>,
    Json(request): Json<ExecJobApiRequest>,
) -> Result<Json<ExecJobSnapshot>, ApiError> {
    let wait_ms = request.wait_ms.unwrap_or(0);
    if wait_ms > 10_000 {
        return Err(ApiError::bad_request("wait_ms must not exceed 10000"));
    }
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        let jobs = &state.exec_jobs;
        let active = jobs.active.lock().await;
        let record = jobs.scoped_record(&request)?;
        let snapshot = jobs.inactive_snapshot(record, active.contains_key(&request.operation_id));
        drop(active);
        if snapshot.state != ExecJobState::Running || tokio::time::Instant::now() >= deadline {
            return Ok(Json(deliver(
                snapshot,
                request.after_revision,
                request.max_output_bytes,
            )?));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub(crate) async fn cancel(
    State(state): State<AppState>,
    Json(request): Json<ExecJobApiRequest>,
) -> Result<Json<ExecJobSnapshot>, ApiError> {
    let jobs = &state.exec_jobs;
    let path = jobs.path(&request.operation_id)?;
    let workspace = fs::canonicalize(&request.cwd)
        .map_err(|error| ApiError::bad_request(format!("invalid workspace: {error}")))?;
    if !workspace.is_dir() {
        return Err(ApiError::bad_request("workspace must be a directory"));
    }
    let active = jobs.active.lock().await;
    match jobs.read_record(&path) {
        Ok(record) => {
            if record.workspace != workspace {
                return Err(ApiError::not_found(
                    "execution_not_found",
                    "execution operation is unknown",
                ));
            }
            if let Some(cancellation) = active.get(&request.operation_id) {
                let _ = cancellation.send(true);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // A cancelled HTTP start may still arrive later. Its ID must never execute.
            jobs.reserve_record_capacity(active.len())?;
            let record = JobRecord {
                workspace,
                request_hash: String::new(),
                snapshot: ExecJobSnapshot {
                    delivery: None,
                    operation_id: request.operation_id.clone(),
                    state: ExecJobState::Cancelled,
                    revision: 1,
                    outcome: None,
                    error: Some(
                        ApiError::service(
                            "execution_cancelled_before_dispatch",
                            "operation was cancelled before dispatch; no command was started",
                            false,
                        )
                        .payload(),
                    ),
                },
            };
            jobs.write_record(&record)
                .map_err(|error| ApiError::internal(error.to_string()))?;
        }
        Err(error) => return Err(ApiError::internal(error.to_string())),
    }
    drop(active);
    wait(
        State(state),
        Json(ExecJobApiRequest {
            wait_ms: Some(5000),
            ..request
        }),
    )
    .await
}
