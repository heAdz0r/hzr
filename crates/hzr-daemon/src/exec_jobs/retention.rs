use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use super::{
    ExecJobState, ExecJobs, JobRecord, MAX_JOB_OUTPUT_BYTES, MAX_RECORD_BYTES, MAX_RECORDS,
    MAX_STORE_BYTES,
};
use crate::approval::ApprovalStore;
use crate::error::ApiError;

#[cfg(all(test, unix))]
mod tests;

const KEEP_RECENT_RECORDS: usize = 1000;
// Approval capabilities expire after five minutes. Also leave slow result consumers a grace period.
const RESULT_GRACE: Duration = Duration::from_secs(60 * 60);
const COMPACTION_BATCH: usize = 256;

struct Usage {
    bytes: u64,
    records: Vec<(SystemTime, PathBuf)>,
}

impl Usage {
    fn fits(&self, reserved: u64) -> bool {
        self.records.len() < MAX_RECORDS && self.bytes.saturating_add(reserved) <= MAX_STORE_BYTES
    }
}

impl ExecJobs {
    fn usage(&self) -> io::Result<Usage> {
        let mut usage = Usage {
            bytes: 0,
            records: Vec::new(),
        };
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
                return Err(io::Error::other(
                    "execution store contains an invalid entry",
                ));
            }
            if metadata.is_dir() {
                for child in fs::read_dir(&path)? {
                    let metadata = fs::symlink_metadata(child?.path())?;
                    if !metadata.file_type().is_file() {
                        return Err(io::Error::other("invalid execution output storage"));
                    }
                    usage.bytes = usage.bytes.saturating_add(metadata.len());
                }
            } else {
                usage.bytes = usage.bytes.saturating_add(metadata.len());
                if path
                    .extension()
                    .is_some_and(|extension| extension == "json")
                {
                    usage.records.push((metadata.modified()?, path));
                }
            }
        }
        Ok(usage)
    }

    pub(super) fn reserve_record_capacity(
        &self,
        active_count: usize,
        approvals: &ApprovalStore,
    ) -> Result<(), ApiError> {
        // Retain the existing worst-case reservation for each active completion and its output.
        let reserved =
            (active_count as u64 + 1).saturating_mul(MAX_RECORD_BYTES + MAX_JOB_OUTPUT_BYTES);
        let mut usage = self
            .usage()
            .map_err(|error| ApiError::internal(error.to_string()))?;
        if usage.fits(reserved) {
            return Ok(());
        }
        usage.records.sort_unstable();
        let byte_pressure = usage.bytes.saturating_add(reserved) > MAX_STORE_BYTES;
        let retain = if byte_pressure {
            usage.records.len()
        } else {
            usage.records.len().saturating_sub(KEEP_RECENT_RECORDS)
        };
        let oldest = SystemTime::now()
            .checked_sub(RESULT_GRACE)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let mut batch = Vec::new();
        for (modified, path) in usage.records.iter().take(retain) {
            if *modified > oldest {
                break;
            }
            let mut record = self
                .read_full_record(path)
                .map_err(|error| ApiError::internal(error.to_string()))?;
            if record.snapshot.state == ExecJobState::Running {
                continue;
            }
            if let Some(hzr_exec::ExecutionOutcome::NotStarted {
                disposition:
                    hzr_exec::NotStarted::ApprovalRequired {
                        decision_id: Some(id),
                        ..
                    },
            }) = record.snapshot.outcome.as_ref()
            {
                if approvals.is_pending(id) {
                    continue;
                }
            }
            record.snapshot.outcome = None;
            record.snapshot.delivery = None;
            record.snapshot.revision = record.snapshot.revision.saturating_add(1);
            record.snapshot.error = Some(ApiError::service(
                "execution_result_expired",
                "old execution result was reclaimed; this operation ID remains bound and will not execute again",
                false,
            ).payload());
            batch.push(record);
            if batch.len() == COMPACTION_BATCH {
                self.compact(&batch).map_err(|error| {
                    ApiError::service(
                        "execution_receipt_storage",
                        format!("execution receipts could not be retained safely: {error}"),
                        false,
                    )
                })?;
                batch.clear();
                if byte_pressure
                    && self
                        .usage()
                        .map_err(|error| ApiError::internal(error.to_string()))?
                        .fits(reserved)
                {
                    break;
                }
            }
        }
        if !batch.is_empty() {
            self.compact(&batch).map_err(|error| {
                ApiError::service(
                    "execution_receipt_storage",
                    format!("execution receipts could not be retained safely: {error}"),
                    false,
                )
            })?;
        }
        if !self
            .usage()
            .map_err(|error| ApiError::internal(error.to_string()))?
            .fits(reserved)
        {
            return Err(ApiError::service(
                "execution_store_capacity",
                "execution storage is reserved by active or recent results; retry after the one-hour result grace period; replay receipts are never discarded",
                true,
            ));
        }
        Ok(())
    }

    fn compact(&self, records: &[JobRecord]) -> io::Result<()> {
        // The SQLite EXTRA-synchronous commit must precede every payload unlink.
        self.receipts.store(records)?;
        for record in records {
            let path = self
                .path(&record.snapshot.operation_id)
                .map_err(|_| io::Error::other("invalid execution operation identity"))?;
            fs::remove_file(path)?;
            self.remove_output(&record.snapshot.operation_id)?;
        }
        #[cfg(unix)]
        fs::File::open(&self.directory)?.sync_all()?;
        Ok(())
    }

    pub(super) fn recover_compaction(&self) -> io::Result<()> {
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "output")
            {
                let id = path
                    .file_stem()
                    .and_then(|id| id.to_str())
                    .ok_or_else(|| io::Error::other("invalid output identity"))?;
                let record_path = self
                    .path(id)
                    .map_err(|_| io::Error::other("invalid execution operation identity"))?;
                match fs::symlink_metadata(record_path) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        if self.receipts.read(id)?.is_some() {
                            self.remove_output(id)?;
                        }
                    }
                    Err(error) => return Err(error),
                    Ok(_) => {}
                }
            }
        }
        Ok(())
    }

    fn remove_output(&self, id: &str) -> io::Result<()> {
        let path = self.directory.join(format!("{id}.output"));
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
            Ok(metadata) if !metadata.file_type().is_dir() => {
                return Err(io::Error::other("invalid execution output directory"));
            }
            Ok(_) => {}
        }
        for entry in fs::read_dir(&path)? {
            let child = entry?.path();
            if !fs::symlink_metadata(&child)?.file_type().is_file() {
                return Err(io::Error::other("invalid execution output storage"));
            }
            fs::remove_file(child)?;
        }
        fs::remove_dir(path)
    }
}
