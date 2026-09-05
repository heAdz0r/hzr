use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use hzr_engine_contract::{
    AccountingFailureEvent, ENGINE_CONTRACT_VERSION, EngineAccountingReceipt, valid_correlation_id,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::adapter::ForkAccountingHandle;
use crate::{ExecError, expected_engine_identity};

const MAX_DRAIN_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccountingJournalInventory {
    pub undrained_receipt_journals: usize,
    pub undrained_receipts: usize,
    pub oldest_modified_at_unix: Option<u64>,
}

pub fn accounting_journal_inventory(
    data_root: &Path,
) -> Result<AccountingJournalInventory, ExecError> {
    let directory = data_root.join("fork");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(AccountingJournalInventory::default());
        }
        Err(source) => {
            return Err(ExecError::PrepareForkRuntime {
                path: directory,
                source,
            });
        }
    };
    let mut inventory = AccountingJournalInventory::default();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("accounting-receipts-")
            || !(name.ends_with(".jsonl") || name.ends_with(".jsonl.pending"))
        {
            continue;
        }
        let correlation_id = name
            .strip_prefix("accounting-receipts-")
            .and_then(|name| name.strip_suffix(".pending").or(Some(name)))
            .and_then(|name| name.strip_suffix(".jsonl"));
        if correlation_id.is_some_and(|correlation_id| {
            directory
                .join(format!("accounting-context-{correlation_id}.json"))
                .is_file()
        }) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() || metadata.len() == 0 {
            continue;
        }
        inventory.undrained_receipt_journals += 1;
        inventory.undrained_receipts = inventory
            .undrained_receipts
            .saturating_add(receipt_line_count(&path));
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs());
        inventory.oldest_modified_at_unix = match (inventory.oldest_modified_at_unix, modified) {
            (Some(current), Some(candidate)) => Some(current.min(candidate)),
            (None, candidate) => candidate,
            (current, None) => current,
        };
    }
    Ok(inventory)
}

fn receipt_line_count(path: &Path) -> usize {
    let Ok(bytes) = fs::read(path) else {
        return 1;
    };
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| line.iter().any(|byte| !byte.is_ascii_whitespace()))
        .count()
        .max(1)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingJournalKind {
    Receipt,
    Failure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingDrainIssueKind {
    CapacityExceeded,
    InvalidJson,
    ContractMismatch,
    EngineIdentityMismatch,
    CorrelationMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccountingDrainIssue {
    pub journal: AccountingJournalKind,
    pub kind: AccountingDrainIssueKind,
    pub correlation_id: Option<String>,
    pub line: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AccountingDrainStatus {
    Empty,
    Ready { batch_id: String },
    Rejected { issue: AccountingDrainIssue },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccountingDrain {
    pub receipts: Vec<EngineAccountingReceipt>,
    pub failures: Vec<AccountingFailureEvent>,
    pub status: AccountingDrainStatus,
}

/// 0.8.1: which engine build a drained receipt may come from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountingEngineIdentityPolicy {
    /// The receipt must come from the currently pinned fork-core build (live producers).
    Exact,
    /// The receipt must be self-consistent under the identity it recorded. Used for journals
    /// recovered after an upgrade, whose producer was an earlier build of the same contract.
    RecordedBuild,
}

pub fn drain_accounting(handle: &ForkAccountingHandle) -> Result<AccountingDrain, ExecError> {
    drain_accounting_with_policy(handle, AccountingEngineIdentityPolicy::Exact)
}

pub fn drain_accounting_with_policy(
    handle: &ForkAccountingHandle,
    policy: AccountingEngineIdentityPolicy,
) -> Result<AccountingDrain, ExecError> {
    let lock = open_drain_lock(handle)?;
    FileExt::lock_exclusive(&lock).map_err(|source| accounting_io_error(handle, source))?;
    let result = drain_accounting_locked(handle, policy);
    let _ = FileExt::unlock(&lock);
    result
}

pub fn acknowledge_accounting(
    handle: &ForkAccountingHandle,
    expected_batch_id: &str,
) -> Result<(), ExecError> {
    let lock = open_drain_lock(handle)?;
    FileExt::lock_exclusive(&lock).map_err(|source| accounting_io_error(handle, source))?;
    let result = (|| {
        let pending = pending_journals(handle);
        let actual_batch_id = batch_id(&pending)?;
        if actual_batch_id.as_deref() != Some(expected_batch_id) {
            return Err(ExecError::AccountingBatchMismatch);
        }
        for journal in pending {
            fs::remove_file(&journal.path).map_err(|source| ExecError::PrepareForkRuntime {
                path: journal.path,
                source,
            })?;
        }
        Ok(())
    })();
    let _ = FileExt::unlock(&lock);
    result
}

fn drain_accounting_locked(
    handle: &ForkAccountingHandle,
    policy: AccountingEngineIdentityPolicy,
) -> Result<AccountingDrain, ExecError> {
    let mut pending = pending_journals(handle);
    if pending.is_empty() {
        rotate_active_journals(handle)?;
        pending = pending_journals(handle);
    }
    if pending.is_empty() {
        return Ok(AccountingDrain {
            receipts: Vec::new(),
            failures: Vec::new(),
            status: AccountingDrainStatus::Empty,
        });
    }

    let Some(batch_id) = batch_id(&pending)? else {
        return Ok(AccountingDrain {
            receipts: Vec::new(),
            failures: Vec::new(),
            status: AccountingDrainStatus::Empty,
        });
    };
    let expected_engine =
        expected_engine_identity().map_err(|reason| ExecError::ForkCoreUnavailable { reason })?;
    let mut receipts = Vec::new();
    let mut failures = Vec::new();
    let mut total_bytes = 0_u64;

    for journal in &pending {
        let bytes = fs::read(&journal.path).map_err(|source| ExecError::PrepareForkRuntime {
            path: journal.path.clone(),
            source,
        })?;
        total_bytes = total_bytes.saturating_add(bytes.len() as u64);
        if total_bytes > MAX_DRAIN_BYTES {
            return Ok(rejected(
                journal,
                AccountingDrainIssueKind::CapacityExceeded,
                None,
            ));
        }
        for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
            if line.is_empty() {
                continue;
            }
            let line_number = u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1);
            match journal.kind {
                AccountingJournalKind::Receipt => {
                    let Ok(receipt) = serde_json::from_slice::<EngineAccountingReceipt>(line)
                    else {
                        return Ok(rejected(
                            journal,
                            AccountingDrainIssueKind::InvalidJson,
                            Some(line_number),
                        ));
                    };
                    if receipt.contract_version != ENGINE_CONTRACT_VERSION {
                        return Ok(rejected(
                            journal,
                            AccountingDrainIssueKind::ContractMismatch,
                            Some(line_number),
                        ));
                    }
                    // 0.8.1: recovered journals validate against the build that wrote them.
                    let accepted_engine = match policy {
                        AccountingEngineIdentityPolicy::Exact => &expected_engine,
                        AccountingEngineIdentityPolicy::RecordedBuild => &receipt.engine,
                    };
                    if receipt.engine != *accepted_engine {
                        return Ok(rejected(
                            journal,
                            AccountingDrainIssueKind::EngineIdentityMismatch,
                            Some(line_number),
                        ));
                    }
                    if receipt.correlation_id != journal.correlation_id
                        || !receipt.is_valid_for(accepted_engine)
                    {
                        return Ok(rejected(
                            journal,
                            AccountingDrainIssueKind::CorrelationMismatch,
                            Some(line_number),
                        ));
                    }
                    receipts.push(receipt);
                }
                AccountingJournalKind::Failure => {
                    let Ok(failure) = serde_json::from_slice::<AccountingFailureEvent>(line) else {
                        return Ok(rejected(
                            journal,
                            AccountingDrainIssueKind::InvalidJson,
                            Some(line_number),
                        ));
                    };
                    if failure.contract_version != ENGINE_CONTRACT_VERSION {
                        return Ok(rejected(
                            journal,
                            AccountingDrainIssueKind::ContractMismatch,
                            Some(line_number),
                        ));
                    }
                    if policy == AccountingEngineIdentityPolicy::Exact
                        && failure.engine != expected_engine
                    {
                        return Ok(rejected(
                            journal,
                            AccountingDrainIssueKind::EngineIdentityMismatch,
                            Some(line_number),
                        ));
                    }
                    if failure.correlation_id != journal.correlation_id
                        || !valid_correlation_id(&failure.correlation_id)
                    {
                        return Ok(rejected(
                            journal,
                            AccountingDrainIssueKind::CorrelationMismatch,
                            Some(line_number),
                        ));
                    }
                    failures.push(failure);
                }
            }
        }
    }

    Ok(AccountingDrain {
        receipts,
        failures,
        status: AccountingDrainStatus::Ready { batch_id },
    })
}

fn rejected(
    journal: &PendingJournal,
    kind: AccountingDrainIssueKind,
    line: Option<u64>,
) -> AccountingDrain {
    AccountingDrain {
        receipts: Vec::new(),
        failures: Vec::new(),
        status: AccountingDrainStatus::Rejected {
            issue: AccountingDrainIssue {
                journal: journal.kind,
                kind,
                correlation_id: Some(journal.correlation_id.clone()),
                line,
            },
        },
    }
}

#[derive(Debug)]
struct PendingJournal {
    path: PathBuf,
    kind: AccountingJournalKind,
    correlation_id: String,
}

fn rotate_active_journals(handle: &ForkAccountingHandle) -> Result<(), ExecError> {
    for journal in active_journals(handle) {
        let lock = open_journal_lock(&journal.path)?;
        FileExt::lock_exclusive(&lock).map_err(|source| accounting_io_error(handle, source))?;
        let result = if journal
            .path
            .metadata()
            .is_ok_and(|metadata| metadata.len() > 0)
        {
            fs::rename(&journal.path, pending_path(&journal.path)).map_err(|source| {
                ExecError::PrepareForkRuntime {
                    path: journal.path.clone(),
                    source,
                }
            })
        } else {
            Ok(())
        };
        let _ = FileExt::unlock(&lock);
        result?;
    }
    Ok(())
}

fn active_journals(handle: &ForkAccountingHandle) -> Vec<PendingJournal> {
    journals_for_handle(handle, false)
}

fn pending_journals(handle: &ForkAccountingHandle) -> Vec<PendingJournal> {
    journals_for_handle(handle, true)
}

fn journals_for_handle(handle: &ForkAccountingHandle, pending: bool) -> Vec<PendingJournal> {
    let mut journals = Vec::new();
    for (path, kind) in [
        (&handle.receipt_journal, AccountingJournalKind::Receipt),
        (&handle.failure_journal, AccountingJournalKind::Failure),
    ] {
        let path = if pending {
            pending_path(path)
        } else {
            PathBuf::from(path)
        };
        if path.is_file() {
            journals.push(PendingJournal {
                path,
                kind,
                correlation_id: handle.correlation_id.clone(),
            });
        }
    }
    journals.sort_by(|left, right| left.path.cmp(&right.path));
    journals
}

fn batch_id(journals: &[PendingJournal]) -> Result<Option<String>, ExecError> {
    if journals.is_empty() {
        return Ok(None);
    }
    let mut digest = Sha256::new();
    for journal in journals {
        digest.update([match journal.kind {
            AccountingJournalKind::Receipt => 0,
            AccountingJournalKind::Failure => 1,
        }]);
        digest.update(journal.correlation_id.as_bytes());
        let bytes = fs::read(&journal.path).map_err(|source| ExecError::PrepareForkRuntime {
            path: journal.path.clone(),
            source,
        })?;
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    Ok(Some(format!("sha256:{:x}", digest.finalize())))
}

fn pending_path(path: &Path) -> PathBuf {
    let mut value: OsString = path.as_os_str().to_owned();
    value.push(".pending");
    PathBuf::from(value)
}

fn open_journal_lock(path: &Path) -> Result<File, ExecError> {
    let mut value: OsString = path.as_os_str().to_owned();
    value.push(".lock");
    open_private_lock(Path::new(&value))
}

fn open_drain_lock(handle: &ForkAccountingHandle) -> Result<File, ExecError> {
    let mut value: OsString = handle.receipt_journal.as_os_str().to_owned();
    value.push(".drain.lock");
    open_private_lock(Path::new(&value))
}

fn open_private_lock(path: &Path) -> Result<File, ExecError> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|source| ExecError::PrepareForkRuntime {
            path: path.to_owned(),
            source,
        })
}

fn accounting_io_error(handle: &ForkAccountingHandle, source: io::Error) -> ExecError {
    ExecError::PrepareForkRuntime {
        path: handle.receipt_journal.clone(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use crate::ForkRuntimePaths;
    use anyhow::{Context, Result, bail};
    use hzr_engine_contract::{
        AccountingAttribution, AccountingMeasurement, AccountingOperationKind,
        AccountingOperationMode, AccountingRoute, AccountingStage, ENGINE_CONTRACT_VERSION,
        EngineAccountingReceipt,
    };

    use super::*;

    fn receipt(correlation_id: &str) -> Result<EngineAccountingReceipt> {
        Ok(EngineAccountingReceipt {
            contract_version: ENGINE_CONTRACT_VERSION,
            engine: expected_engine_identity().map_err(anyhow::Error::msg)?,
            correlation_id: correlation_id.to_owned(),
            sequence: 7,
            occurred_at_unix_ms: 10,
            baseline_tokens: 20,
            delivered_tokens: 5,
            execution_ms: 2,
            measurement: AccountingMeasurement::Estimated,
            route: AccountingRoute::Optimized,
            attribution: AccountingAttribution {
                operation: AccountingOperationKind::Exec,
                mode: AccountingOperationMode::ExecRun,
                stage: AccountingStage::InternalTransport,
                requested_mode: None,
                effective_mode: None,
                search_strategy: None,
                search_fallback_code: None,
                include_content: None,
                limit: None,
                path_scope_count: None,
                filter_level: None,
                from_line: None,
                to_line: None,
                source_bytes: None,
                evasion: None,
            },
            host_grant_applied: false,
        })
    }

    #[test]
    fn drain_is_retryable_until_explicit_acknowledgement() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let paths = ForkRuntimePaths::from_data_root(directory.path());
        paths.ensure_layout()?;
        let correlation_id = "0123456789abcdef0123456789abcdef";
        let journal = correlated_fixture_path(&paths.accounting_receipt_journal, correlation_id)?;
        let handle = fixture_handle(&paths, correlation_id)?;
        fs::write(
            &journal,
            format!("{}\n", serde_json::to_string(&receipt(correlation_id)?)?),
        )?;

        let first = drain_accounting(&handle)?;
        let AccountingDrainStatus::Ready { batch_id } = &first.status else {
            bail!("expected ready batch: {:?}", first.status);
        };
        assert_eq!(first.receipts.len(), 1);
        assert!(!journal.exists());

        let retry = drain_accounting(&handle)?;
        assert_eq!(retry, first);
        acknowledge_accounting(&handle, batch_id)?;
        assert_eq!(
            drain_accounting(&handle)?.status,
            AccountingDrainStatus::Empty
        );
        Ok(())
    }

    #[test]
    fn invalid_identity_is_rejected_without_deleting_evidence() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let paths = ForkRuntimePaths::from_data_root(directory.path());
        paths.ensure_layout()?;
        let correlation_id = "fedcba9876543210fedcba9876543210";
        let journal = correlated_fixture_path(&paths.accounting_receipt_journal, correlation_id)?;
        let handle = fixture_handle(&paths, correlation_id)?;
        let mut invalid = receipt(correlation_id)?;
        invalid.engine.manifest_sha256 = "invalid".to_owned();
        fs::write(&journal, format!("{}\n", serde_json::to_string(&invalid)?))?;

        let drained = drain_accounting(&handle)?;
        assert!(matches!(
            drained.status,
            AccountingDrainStatus::Rejected {
                issue: AccountingDrainIssue {
                    kind: AccountingDrainIssueKind::EngineIdentityMismatch,
                    ..
                }
            }
        ));
        assert!(pending_path(&journal).exists());
        Ok(())
    }

    #[test]
    fn inventory_counts_only_orphaned_nonempty_receipt_journals() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let paths = ForkRuntimePaths::from_data_root(directory.path());
        paths.ensure_layout()?;
        let orphan = "0123456789abcdef0123456789abcdef";
        let registered = "fedcba9876543210fedcba9876543210";
        fs::write(
            correlated_fixture_path(&paths.accounting_receipt_journal, orphan)?,
            b"receipt one\nreceipt two\n",
        )?;
        fs::write(
            correlated_fixture_path(&paths.accounting_receipt_journal, registered)?,
            b"receipt\n",
        )?;
        fs::write(
            directory
                .path()
                .join("fork")
                .join(format!("accounting-context-{registered}.json")),
            b"{}\n",
        )?;

        let inventory = accounting_journal_inventory(directory.path())?;

        assert_eq!(inventory.undrained_receipt_journals, 1);
        assert_eq!(inventory.undrained_receipts, 2);
        assert!(inventory.oldest_modified_at_unix.is_some());
        Ok(())
    }

    fn correlated_fixture_path(base: &Path, correlation_id: &str) -> Result<PathBuf> {
        let parent = base.parent().context("fixture journal parent")?;
        let stem = base.file_stem().context("fixture journal stem")?;
        Ok(parent.join(format!("{}-{correlation_id}.jsonl", stem.to_string_lossy())))
    }

    fn fixture_handle(
        paths: &ForkRuntimePaths,
        correlation_id: &str,
    ) -> Result<ForkAccountingHandle> {
        Ok(ForkAccountingHandle {
            correlation_id: correlation_id.to_owned(),
            receipt_journal: correlated_fixture_path(
                &paths.accounting_receipt_journal,
                correlation_id,
            )?,
            failure_journal: correlated_fixture_path(
                &paths.accounting_failure_journal,
                correlation_id,
            )?,
        })
    }
}
