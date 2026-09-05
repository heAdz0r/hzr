use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hzr_core::{AccountingCoverageStore, AccountingReceiptContextStore};
use hzr_exec::{
    AccountingDrainStatus, AccountingEngineIdentityPolicy, accounting_lock_correlation,
    acknowledge_accounting, drain_accounting, drain_accounting_with_policy,
    remove_accounting_locks,
};
use hzr_protocol::AccountingChannel;

use crate::AppState;

/// 0.8.3: a registration that neither completed nor produced receipts within a day is retired.
/// The gap it opened stays in the coverage state (nothing recovers it), so retiring the file does
/// not fake a recovery; keeping it only made every sweep slower and the directory grow until the
/// 20 000-context registration cap rejected new commands.
const ABANDONED_CONTEXT_TTL_SECS: u64 = 24 * 60 * 60;
/// 0.8.3: producers registered this recently keep the sweeper on its one-second cadence.
const ACTIVE_CONTEXT_WINDOW_SECS: u64 = 120;
/// 0.8.3: sweep cadence while no producer is active and nothing drained.
const IDLE_SWEEP_SECS: u64 = 5;
/// 0.8.3: stale lock files removed per sweep, so one backlog cannot stall receipt draining.
const STALE_LOCKS_PER_SWEEP: usize = 500;

// 0.8.1: receipt journals without a registered context (daemon-free rewrites before 0.8.1,
// crashed producers) are drained after this grace period so they stop counting as undrained.
const ORPHAN_JOURNAL_GRACE_SECS: u64 = 600;
const ORPHAN_JOURNALS_PER_SWEEP: usize = 200;
/// Project attribution for receipts recovered without a context. It is a visible label, not a
/// guess at which workspace produced the operations.
pub const UNATTRIBUTED_PROJECT_PATH: &str = "unattributed";

pub fn register(
    state: &AppState,
    correlation_id: &str,
    project_path: &Path,
    agent: Option<&str>,
    session_id: Option<&str>,
    channel: AccountingChannel,
) -> Result<(), String> {
    let runner = state.rtk.runner().map_err(|error| error.to_string())?;
    runner
        .accounting_handle(correlation_id)
        .map_err(|error| error.to_string())?;
    AccountingReceiptContextStore::new(&state.config.data_dir)
        .register_with_channel(correlation_id, project_path, agent, session_id, channel)
        .map_err(|error| error.to_string())
}

pub async fn sweep_once(state: &AppState) -> Result<usize, String> {
    let directory = state.config.data_dir.join("fork");
    let contexts = AccountingReceiptContextStore::new(&state.config.data_dir);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.to_string()),
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| AccountingReceiptContextStore::is_context_path(path))
        .collect::<Vec<_>>();
    paths.sort();
    let mut committed = 0;
    for path in paths {
        match sweep_context(state, &contexts, &path).await {
            Ok(count) => committed += count,
            Err(error) => {
                let context = contexts.read(&path);
                let event = context.as_ref().map_or_else(
                    |_| hzr_core::AccountingGapEvent {
                        surface: hzr_core::AccountingGapSurface::ForkProducer,
                        workspace_hash: None,
                        session_hash: None,
                        operation_family: Some("invalid_accounting_context".into()),
                        at_unix: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs()
                            .max(1),
                    },
                    |context| context.gap_event(),
                );
                let recorded =
                    AccountingCoverageStore::new(&state.config.data_dir).ensure_missing(event);
                if let Err(recording_error) = &recorded {
                    tracing::error!(%recording_error, "accounting failure could not be recorded");
                }
                if context.is_err() && recorded.is_ok() {
                    // Invalid identity cannot be attributed safely. Preserve it for inspection.
                    let quarantine = path.with_extension("invalid");
                    if !quarantine.exists() {
                        if let Err(error) = fs::rename(&path, &quarantine) {
                            tracing::warn!(%error, "accounting context quarantine failed");
                        }
                    }
                }
                tracing::warn!(%error, context_path = %path.display(), "isolated accounting context failure");
            }
        }
    }
    committed += sweep_orphan_journals(state, &directory).await?; // 0.8.1
    Ok(committed)
}

/// 0.8.1: drain receipt journals that have no context file.
///
/// The correlation was never registered (daemon-free rewrite) or its context was retired, so
/// the receipts can only be recorded as `unattributed`. Recording them is still more honest
/// than an ever-growing undrained count: the token measurements are real, only the workspace
/// is unknown. Rejected batches are quarantined as `.rejected` and recorded as a producer gap.
async fn sweep_orphan_journals(state: &AppState, directory: &Path) -> Result<usize, String> {
    let now = std::time::SystemTime::now();
    let mut orphans = Vec::new();
    for entry in fs::read_dir(directory)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
    {
        let path = entry.path();
        let Some(correlation_id) = orphan_journal_correlation(&path) else {
            continue;
        };
        if directory
            .join(format!("accounting-context-{correlation_id}.json"))
            .is_file()
        {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() || metadata.len() == 0 {
            continue;
        }
        let age = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .map_or(0, |age| age.as_secs());
        if age < ORPHAN_JOURNAL_GRACE_SECS {
            continue;
        }
        orphans.push((correlation_id, path));
    }
    orphans.sort();
    orphans.dedup_by(|left, right| left.0 == right.0);
    let mut committed = 0;
    for (correlation_id, path) in orphans.into_iter().take(ORPHAN_JOURNALS_PER_SWEEP) {
        let runner = state.rtk.runner().map_err(|error| error.to_string())?;
        let handle = runner
            .accounting_handle(&correlation_id)
            .map_err(|error| error.to_string())?;
        // The producer may have been an earlier fork-core build (the journal predates an
        // upgrade); the receipt is validated against the identity it recorded.
        let drained =
            drain_accounting_with_policy(&handle, AccountingEngineIdentityPolicy::RecordedBuild)
                .map_err(|error| error.to_string())?;
        let batch_id = match &drained.status {
            AccountingDrainStatus::Empty => continue,
            AccountingDrainStatus::Ready { batch_id } if drained.failures.is_empty() => {
                batch_id.clone()
            }
            AccountingDrainStatus::Ready { .. } | AccountingDrainStatus::Rejected { .. } => {
                quarantine_orphan_journal(state, &path, &drained.status);
                continue;
            }
        };
        for receipt in drained.receipts {
            state
                .ledger
                .record_engine_receipt(
                    receipt,
                    UNATTRIBUTED_PROJECT_PATH.to_owned(),
                    None,
                    None,
                    AccountingChannel::HookCli,
                )
                .await
                .map_err(|error| error.to_string())?;
            committed += 1;
        }
        acknowledge_accounting(&handle, &batch_id).map_err(|error| error.to_string())?;
        // A successful recovery closes the gap an earlier rejected orphan batch may have opened.
        let _ = AccountingCoverageStore::new(&state.config.data_dir).recover(orphan_gap_event());
    }
    Ok(committed)
}

fn orphan_gap_event() -> hzr_core::AccountingGapEvent {
    hzr_core::AccountingGapEvent {
        surface: hzr_core::AccountingGapSurface::ForkProducer,
        workspace_hash: None,
        session_hash: None,
        operation_family: Some("orphan_receipt_rejected".into()),
        at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .max(1),
    }
}

fn orphan_journal_correlation(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix("accounting-receipts-")?;
    let correlation_id = rest
        .strip_suffix(".jsonl")
        .or_else(|| rest.strip_suffix(".jsonl.pending"))?;
    (correlation_id.len() == 32
        && correlation_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
    .then(|| correlation_id.to_owned())
}

fn quarantine_orphan_journal(state: &AppState, path: &Path, status: &AccountingDrainStatus) {
    let recorded =
        AccountingCoverageStore::new(&state.config.data_dir).ensure_missing(orphan_gap_event());
    if let Err(error) = &recorded {
        tracing::error!(%error, "orphan receipt rejection could not be recorded");
        return;
    }
    // The drain rotated the journal to `.pending`; quarantine every pending file of the batch.
    for candidate in [path.to_path_buf(), path.with_extension("jsonl.pending")] {
        if candidate.is_file() {
            let quarantine = candidate.with_extension("rejected");
            if let Err(error) = fs::rename(&candidate, &quarantine) {
                tracing::warn!(%error, journal = %candidate.display(), "orphan journal quarantine failed");
            }
        }
    }
    tracing::warn!(?status, journal = %path.display(), "orphan receipt journal rejected");
}

async fn sweep_context(
    state: &AppState,
    contexts: &AccountingReceiptContextStore,
    path: &Path,
) -> Result<usize, String> {
    let context = contexts.read(path).map_err(|error| error.to_string())?;
    let runner = state.rtk.runner().map_err(|error| error.to_string())?;
    let handle = runner
        .accounting_handle(&context.correlation_id)
        .map_err(|error| error.to_string())?;
    let drained = drain_accounting(&handle).map_err(|error| error.to_string())?;
    let batch_id = match &drained.status {
        AccountingDrainStatus::Empty if context.completed_at_unix.is_some() => {
            fs::remove_file(path).map_err(|error| error.to_string())?;
            return Ok(0);
        }
        AccountingDrainStatus::Empty => return Ok(0),
        AccountingDrainStatus::Ready { batch_id } if drained.failures.is_empty() => batch_id,
        AccountingDrainStatus::Ready { .. } | AccountingDrainStatus::Rejected { .. } => {
            return Err("accounting batch is rejected or contains producer failures".into());
        }
    };
    let mut committed = 0;
    for receipt in drained.receipts {
        state
            .ledger
            .record_engine_receipt(
                receipt,
                context.project_path.clone(),
                context.agent.clone(),
                context.session_id.clone(),
                context.channel,
            )
            .await
            .map_err(|error| error.to_string())?;
        committed += 1;
    }
    acknowledge_accounting(&handle, batch_id).map_err(|error| error.to_string())?;
    AccountingCoverageStore::new(&state.config.data_dir)
        .recover(context.gap_event())
        .map_err(|error| error.to_string())?;
    Ok(committed)
}

pub async fn run(state: AppState) {
    loop {
        if let Err(error) = sweep_once(&state).await {
            tracing::warn!(%error, "accounting receipt sweep failed");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use hzr_core::{AccountingCoverageStore, AccountingReceiptContextStore, Config};
    use hzr_exec::{ForkRuntimePaths, PINNED_RTK_VERSION, expected_engine_identity};
    use hzr_protocol::{
        AccountingAttribution, AccountingMeasurement, AccountingOperationKind,
        AccountingOperationMode, AccountingRoute, AccountingStage, ENGINE_CONTRACT_VERSION,
        EngineAccountingReceipt,
    };
    use tempfile::tempdir;

    use super::{ABANDONED_CONTEXT_TTL_SECS, register, sweep_once};
    use crate::AppState;

    #[cfg(unix)]
    #[tokio::test]
    async fn registered_hook_receipt_is_committed_and_acknowledged() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().expect("temporary directory");
        let engines = directory.path().join("engines");
        fs::create_dir_all(&engines).expect("engine directory");
        let binary = engines.join("rtk");
        let contract = serde_json::to_string(&expected_engine_identity().expect("engine identity"))
            .expect("contract JSON");
        fs::write(
            &binary,
            format!(
                r#"#!/bin/sh
if test "${{1:-}}" = --version; then
  printf 'rtk %s\n' '{PINNED_RTK_VERSION}'
  exit 0
fi
if test "${{1:-}}" = contract && test "${{2:-}}" = --json; then
  printf '%s\n' '{contract}'
  exit 0
fi
if test "${{1:-}}" = rewrite && test "${{2:-}}" = --help; then
  printf 'Usage: rtk rewrite [ARGS]... Raw command to rewrite\n'
  exit 0
fi
if test "${{1:-}}" = proxy && test "${{2:-}}" = --help; then
  printf 'Usage: rtk proxy [ARGS]... Execute command without filtering\n'
  exit 0
fi
exit 64
"#
            ),
        )
        .expect("fake rtk");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("permissions");
        let mut config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        config.engines.auto_start_icm = false;
        config.engines.directory = Some(engines);
        let state = AppState::initialize(config).await.expect("state");
        let project = directory.path().join("project");
        fs::create_dir(&project).expect("project");
        let correlation_id = "0123456789abcdef0123456789abcdef";
        register(
            &state,
            correlation_id,
            &project,
            Some("claude-code"),
            Some("s1"),
            hzr_protocol::AccountingChannel::Mcp,
        )
        .expect("registration");
        let paths = ForkRuntimePaths::from_data_root(&state.config.data_dir);
        let journal = paths
            .accounting_receipt_journal
            .parent()
            .expect("journal parent")
            .join(format!("accounting-receipts-{correlation_id}.jsonl"));
        let receipt = EngineAccountingReceipt {
            contract_version: ENGINE_CONTRACT_VERSION,
            engine: expected_engine_identity().expect("engine identity"),
            correlation_id: correlation_id.to_owned(),
            sequence: 1,
            occurred_at_unix_ms: 1,
            baseline_tokens: 20,
            delivered_tokens: 5,
            execution_ms: 2,
            measurement: AccountingMeasurement::Estimated,
            route: AccountingRoute::Optimized,
            attribution: AccountingAttribution {
                operation: AccountingOperationKind::Read,
                mode: AccountingOperationMode::ReadFiltered,
                stage: AccountingStage::FinalDelivery,
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
        };
        fs::write(
            &journal,
            format!(
                "{}\n",
                serde_json::to_string(&receipt).expect("receipt JSON")
            ),
        )
        .expect("receipt journal");

        let malformed = state
            .config
            .data_dir
            .join("fork/accounting-context-00000000000000000000000000000000.json");
        fs::write(&malformed, b"{broken").expect("bad context before valid context");
        assert_eq!(sweep_once(&state).await.expect("sweep"), 1);
        assert!(
            malformed.with_extension("invalid").exists(),
            "invalid context preserved"
        );
        assert!(!journal.exists());
        assert!(
            AccountingReceiptContextStore::new(&state.config.data_dir)
                .context_path(correlation_id)
                .exists()
        );
        assert_eq!(sweep_once(&state).await.expect("empty replay"), 0);
        let mut later = receipt.clone();
        later.sequence = 2;
        fs::write(
            &journal,
            format!("{}\n", serde_json::to_string(&later).expect("late receipt")),
        )
        .expect("late journal");
        assert_eq!(sweep_once(&state).await.expect("later producer batch"), 1);
        assert_eq!(sweep_once(&state).await.expect("second replay"), 0);
        let contexts = AccountingReceiptContextStore::new(&state.config.data_dir);
        assert_eq!(
            contexts
                .read(&contexts.context_path(correlation_id))
                .expect("valid receipt fixture")
                .channel,
            hzr_protocol::AccountingChannel::Mcp
        );
        contexts
            .complete(correlation_id)
            .expect("producer finished all batches");
        assert_eq!(
            sweep_once(&state)
                .await
                .expect("completed producer retirement"),
            0
        );
        assert!(!contexts.context_path(correlation_id).exists());
        assert!(
            !AccountingCoverageStore::new(&state.config.data_dir)
                .snapshot(2)
                .expect("coverage")
                .live_complete,
            "the invalid context remains an explicit unattributed gap"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unknown_empty_context_is_not_falsely_recovered_by_age() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().expect("temporary directory");
        let engines = directory.path().join("engines");
        fs::create_dir_all(&engines).expect("engine directory");
        let binary = engines.join("rtk");
        let contract = serde_json::to_string(&expected_engine_identity().expect("engine identity"))
            .expect("contract JSON");
        fs::write(
            &binary,
            format!(
                r#"#!/bin/sh
if test "${{1:-}}" = --version; then
  printf 'rtk %s\n' '{PINNED_RTK_VERSION}'
  exit 0
fi
if test "${{1:-}}" = contract && test "${{2:-}}" = --json; then
  printf '%s\n' '{contract}'
  exit 0
fi
if test "${{1:-}}" = rewrite && test "${{2:-}}" = --help; then
  printf 'Usage: rtk rewrite [ARGS]... Raw command to rewrite\n'
  exit 0
fi
if test "${{1:-}}" = proxy && test "${{2:-}}" = --help; then
  printf 'Usage: rtk proxy [ARGS]... Execute command without filtering\n'
  exit 0
fi
exit 64
"#
            ),
        )
        .expect("fake rtk");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("permissions");
        let mut config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        config.engines.auto_start_icm = false;
        config.engines.directory = Some(engines);
        let state = AppState::initialize(config).await.expect("state");
        let project = directory.path().join("project");
        fs::create_dir(&project).expect("project");
        let correlation_id = "fedcba9876543210fedcba9876543210";
        register(
            &state,
            correlation_id,
            &project,
            None,
            Some("denied"),
            hzr_protocol::AccountingChannel::HookCli,
        )
        .expect("registration");

        let contexts = AccountingReceiptContextStore::new(&state.config.data_dir);
        let path = contexts.context_path(correlation_id);
        let mut context = contexts.read(&path).expect("context");
        context.registered_at_unix = context
            .registered_at_unix
            .saturating_sub(ABANDONED_CONTEXT_TTL_SECS + 1);
        fs::write(&path, serde_json::to_vec(&context).expect("context JSON"))
            .expect("expired context");

        assert_eq!(sweep_once(&state).await.expect("sweep"), 0);
        assert!(path.exists(), "age does not prove producer completion");
        // 0.8.2: a registration is pending inside the producer grace; inspect it as settled.
        let settled = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + hzr_core::FORK_PRODUCER_PENDING_GRACE_SECS;
        assert!(
            !AccountingCoverageStore::new(&state.config.data_dir)
                .snapshot(settled)
                .expect("coverage")
                .live_complete
        );
        contexts
            .complete(correlation_id)
            .expect("producer completion");
        assert_eq!(sweep_once(&state).await.expect("completed empty sweep"), 0);
        assert!(!path.exists());
        assert!(
            !AccountingCoverageStore::new(&state.config.data_dir)
                .snapshot(settled)
                .expect("coverage")
                .live_complete,
            "missing receipts remain unresolved after cleanup"
        );
    }

    // 0.8.1: journals whose context never existed are recovered as `unattributed` once old.
    #[cfg(unix)]
    #[tokio::test]
    async fn orphan_receipt_journal_without_context_is_recovered_after_grace() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().expect("temporary directory");
        let engines = directory.path().join("engines");
        fs::create_dir_all(&engines).expect("engine directory");
        let binary = engines.join("rtk");
        let contract = serde_json::to_string(&expected_engine_identity().expect("engine identity"))
            .expect("contract JSON");
        fs::write(
            &binary,
            format!(
                r#"#!/bin/sh
if test "${{1:-}}" = --version; then
  printf 'rtk %s\n' '{PINNED_RTK_VERSION}'
  exit 0
fi
if test "${{1:-}}" = contract && test "${{2:-}}" = --json; then
  printf '%s\n' '{contract}'
  exit 0
fi
if test "${{1:-}}" = rewrite && test "${{2:-}}" = --help; then
  printf 'Usage: rtk rewrite [ARGS]... Raw command to rewrite\n'
  exit 0
fi
if test "${{1:-}}" = proxy && test "${{2:-}}" = --help; then
  printf 'Usage: rtk proxy [ARGS]... Execute command without filtering\n'
  exit 0
fi
exit 64
"#
            ),
        )
        .expect("fake rtk");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("permissions");
        let mut config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        config.engines.auto_start_icm = false;
        config.engines.directory = Some(engines);
        let state = AppState::initialize(config).await.expect("state");
        let paths = ForkRuntimePaths::from_data_root(&state.config.data_dir);
        paths.ensure_layout().expect("fork layout");
        let correlation_id = "abcdefabcdefabcdefabcdefabcdef01";
        let journal = paths
            .accounting_receipt_journal
            .parent()
            .expect("journal parent")
            .join(format!("accounting-receipts-{correlation_id}.jsonl"));
        // The journal predates an upgrade: the producer was an earlier build of the contract.
        let mut earlier_build = expected_engine_identity().expect("engine identity");
        earlier_build.manifest_sha256 = "0".repeat(64);
        earlier_build.content_manifest_sha256 = "1".repeat(64);
        let receipt = EngineAccountingReceipt {
            contract_version: ENGINE_CONTRACT_VERSION,
            engine: earlier_build,
            correlation_id: correlation_id.to_owned(),
            sequence: 1,
            occurred_at_unix_ms: 1,
            baseline_tokens: 1062,
            delivered_tokens: 56,
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
        };
        fs::write(
            &journal,
            format!(
                "{}\n",
                serde_json::to_string(&receipt).expect("receipt JSON")
            ),
        )
        .expect("orphan journal");
        // Fresh journals may still belong to a producer whose context is about to appear.
        assert_eq!(sweep_once(&state).await.expect("fresh orphan sweep"), 0);
        assert!(journal.exists(), "a fresh orphan journal is left alone");
        let old = std::time::SystemTime::now()
            - std::time::Duration::from_secs(super::ORPHAN_JOURNAL_GRACE_SECS + 60);
        fs::File::options()
            .write(true)
            .open(&journal)
            .expect("journal handle")
            .set_modified(old)
            .expect("age the journal");
        assert_eq!(sweep_once(&state).await.expect("aged orphan sweep"), 1);
        assert!(
            !journal.exists(),
            "drained journal is acknowledged and removed"
        );
        assert!(!journal.with_extension("jsonl.pending").exists());
        assert_eq!(
            hzr_exec::accounting_journal_inventory(&state.config.data_dir)
                .expect("inventory")
                .undrained_receipts,
            0
        );
        assert_eq!(sweep_once(&state).await.expect("replay"), 0);
    }
}
