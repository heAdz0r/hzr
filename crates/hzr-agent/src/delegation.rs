//! Parent-owned terminal receipt, separate from the bridge's heartbeat file.
//! The bridge can never overwrite a timeout/cancellation outcome with a late heartbeat.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) struct DelegationOutcome {
    directory: PathBuf,
    status: &'static str,
}

impl DelegationOutcome {
    pub(crate) fn new(directory: &Path) -> Self {
        Self {
            directory: directory.to_owned(),
            status: "cancelled",
        }
    }

    pub(crate) fn finish(&mut self, result: &Result<super::AgentRun, super::RunError>) {
        self.status = match result {
            Ok(_) => "completed",
            Err(super::RunError::Timeout) => "timed_out",
            Err(_) => "failed",
        };
    }

    fn persist(&self) -> std::io::Result<()> {
        if !self.directory.is_dir() {
            return Ok(());
        }
        let finished_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let receipt = serde_json::json!({
            "schema_version": 1, "status": self.status, "finished_at_ms": finished_at_ms,
        });
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        temporary.write_all(receipt.to_string().as_bytes())?;
        temporary.persist(self.directory.join("delegation-outcome.json"))?;
        Ok(())
    }
}

impl Drop for DelegationOutcome {
    fn drop(&mut self) {
        if self.persist().is_err() {
            eprintln!("HZR could not persist the delegation terminal status.");
        }
    }
}
