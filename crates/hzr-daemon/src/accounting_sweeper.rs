use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hzr_core::{AccountingCoverageStore, AccountingReceiptContext, AccountingReceiptContextStore};
use hzr_exec::{
    AccountingDrainStatus, AccountingEngineIdentityPolicy, accounting_lock_correlation,
    acknowledge_accounting, drain_accounting, drain_accounting_with_policy,
    remove_accounting_locks,
};
use hzr_protocol::{AccountingChannel, EngineAccountingReceipt, EvasionAttribution};

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

/// Registration without a classification, for tests; production callers register attributed.
#[cfg(test)]
pub(crate) fn register(
    state: &AppState,
    correlation_id: &str,
    project_path: &Path,
    agent: Option<&str>,
    session_id: Option<&str>,
    channel: AccountingChannel,
) -> Result<(), String> {
    register_attributed(
        state,
        correlation_id,
        project_path,
        agent,
        session_id,
        channel,
        None,
    )
}

/// 0.8.3: register a producer with the classification of the command it will run, so the
/// sweeper can attribute receipts that arrive without one (see [`attribute_receipt`]).
pub fn register_attributed(
    state: &AppState,
    correlation_id: &str,
    project_path: &Path,
    agent: Option<&str>,
    session_id: Option<&str>,
    channel: AccountingChannel,
    evasion: Option<EvasionAttribution>,
) -> Result<(), String> {
    let runner = state.rtk.runner().map_err(|error| error.to_string())?;
    runner
        .accounting_handle(correlation_id)
        .map_err(|error| error.to_string())?;
    AccountingReceiptContextStore::new(&state.config.data_dir)
        .register_with_attribution(
            correlation_id,
            project_path,
            agent,
            session_id,
            channel,
            evasion,
        )
        .map_err(|error| error.to_string())
}

/// 0.8.3: a receipt written by an engine that was not told how its command was classified takes
/// the classification from the registration. The hook used to export the classification as JSON
/// into the approved command's environment; keeping it in the registration leaves the command
/// the host inspects free of policy internals. A receipt that carries its own attribution (the
/// `hzr exec run` path sets the environment programmatically) is left as it is.
fn attribute_receipt(receipt: &mut EngineAccountingReceipt, context: &AccountingReceiptContext) {
    if receipt.attribution.evasion.is_none() {
        receipt.attribution.evasion = context.evasion;
    }
}

/// Receipts committed by one sweep, for tests that only need the count.
#[cfg(test)]
pub(crate) async fn sweep_once(state: &AppState) -> Result<usize, String> {
    Ok(sweep(state).await?.committed)
}

/// 0.8.3: what one sweep did, so the loop can pick its cadence.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SweepOutcome {
    pub(crate) committed: usize,
    /// Registrations that drained receipts or were registered within the active window.
    pub(crate) active_producers: usize,
}

/// 0.8.3: one directory listing per sweep, classified by file name.
///
/// The sweeper used to read the directory twice and then run a full drain attempt — lock file,
/// two journal probes, a rotation probe — for every registered context every second, whether or
/// not the context had anything to drain. With 18 500 files in the directory that was the
/// measured 70 % CPU and the write storm.
#[derive(Default)]
struct ForkInventory {
    contexts: Vec<PathBuf>,
    context_ids: BTreeSet<String>,
    /// Correlations with a receipt or failure journal, active or pending.
    journals: BTreeSet<String>,
    /// Receipt journals with their correlation, for the orphan pass.
    receipt_journals: Vec<(String, PathBuf)>,
    locks: Vec<(String, PathBuf)>,
}

impl ForkInventory {
    fn read(directory: &Path) -> Result<Option<Self>, String> {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let mut inventory = Self::default();
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if AccountingReceiptContextStore::is_context_path(&path) {
                if let Some(correlation_id) = context_correlation(&path) {
                    inventory.context_ids.insert(correlation_id);
                }
                inventory.contexts.push(path);
            } else if let Some(correlation_id) = orphan_journal_correlation(&path) {
                inventory.journals.insert(correlation_id.clone());
                inventory.receipt_journals.push((correlation_id, path));
            } else if let Some(correlation_id) = journal_correlation(&path, "accounting-failures-")
            {
                inventory.journals.insert(correlation_id);
            } else if let Some(correlation_id) = accounting_lock_correlation(&path) {
                inventory.locks.push((correlation_id, path));
            }
        }
        inventory.contexts.sort();
        inventory.receipt_journals.sort();
        Ok(Some(inventory))
    }
}

pub(crate) async fn sweep(state: &AppState) -> Result<SweepOutcome, String> {
    let directory = state.config.data_dir.join("fork");
    let Some(inventory) = ForkInventory::read(&directory)? else {
        return Ok(SweepOutcome::default());
    };
    let contexts = AccountingReceiptContextStore::new(&state.config.data_dir);
    let now_unix = unix_now();
    let mut outcome = SweepOutcome::default();
    for path in &inventory.contexts {
        let has_journal = context_correlation(path)
            .is_some_and(|correlation_id| inventory.journals.contains(&correlation_id));
        if has_journal {
            outcome.active_producers += 1;
            match sweep_context(state, &contexts, path, &directory).await {
                Ok(count) => outcome.committed += count,
                Err(error) => record_context_failure(state, &contexts, path, &error),
            }
            continue;
        }
        // 0.8.3: nothing to drain, so the context alone decides; no lock, probe or rotation.
        match contexts.read(path) {
            Ok(context) if context.completed_at_unix.is_some() => {
                retire_context(state, path, &context.correlation_id);
            }
            Ok(context)
                if now_unix.saturating_sub(context.registered_at_unix)
                    >= ABANDONED_CONTEXT_TTL_SECS =>
            {
                tracing::info!(
                    correlation_id = %context.correlation_id,
                    "abandoned accounting context retired; its gap stays recorded"
                );
                retire_context(state, path, &context.correlation_id);
            }
            Ok(context) => {
                if now_unix.saturating_sub(context.registered_at_unix) < ACTIVE_CONTEXT_WINDOW_SECS
                {
                    outcome.active_producers += 1;
                }
            }
            Err(error) => record_context_failure(state, &contexts, path, &error.to_string()),
        }
    }
    outcome.committed += sweep_orphan_journals(state, &inventory).await?; // 0.8.1
    remove_stale_locks(&inventory); // 0.8.3
    Ok(outcome)
}

fn record_context_failure(
    state: &AppState,
    contexts: &AccountingReceiptContextStore,
    path: &Path,
    error: &str,
) {
    let context = contexts.read(path);
    let event = context.as_ref().map_or_else(
        |_| hzr_core::AccountingGapEvent {
            surface: hzr_core::AccountingGapSurface::ForkProducer,
            workspace_hash: None,
            session_hash: None,
            operation_family: Some("invalid_accounting_context".into()),
            at_unix: unix_now(),
        },
        |context| context.gap_event(),
    );
    let recorded = AccountingCoverageStore::new(&state.config.data_dir).ensure_missing(event);
    if let Err(recording_error) = &recorded {
        tracing::error!(%recording_error, "accounting failure could not be recorded");
    }
    if context.is_err() && recorded.is_ok() {
        // Invalid identity cannot be attributed safely. Preserve it for inspection.
        let quarantine = path.with_extension("invalid");
        if !quarantine.exists() {
            if let Err(error) = fs::rename(path, &quarantine) {
                tracing::warn!(%error, "accounting context quarantine failed");
            }
        }
    }
    tracing::warn!(%error, context_path = %path.display(), "isolated accounting context failure");
}

/// 0.8.3: remove a finished registration together with the lock files its producer and the
/// drains left behind. The producer is gone by construction: it completed, or it registered a
/// day ago and never wrote a receipt.
fn retire_context(state: &AppState, path: &Path, correlation_id: &str) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(%error, context_path = %path.display(), "accounting context retirement failed");
            return;
        }
    }
    if let Ok(runner) = state.rtk.runner() {
        if let Ok(handle) = runner.accounting_handle(correlation_id) {
            if let Err(error) = remove_accounting_locks(&handle) {
                tracing::warn!(%error, %correlation_id, "accounting lock cleanup failed");
            }
        }
    }
}

/// 0.8.3: lock files whose correlation has neither a context nor a journal and that nobody
/// touched for the orphan grace belong to a producer that is gone.
fn remove_stale_locks(inventory: &ForkInventory) {
    let now = SystemTime::now();
    let mut removed = 0_usize;
    for (correlation_id, path) in &inventory.locks {
        if removed >= STALE_LOCKS_PER_SWEEP {
            break;
        }
        if inventory.context_ids.contains(correlation_id)
            || inventory.journals.contains(correlation_id)
        {
            continue;
        }
        let stale = fs::metadata(path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age.as_secs() >= ORPHAN_JOURNAL_GRACE_SECS);
        if !stale {
            continue;
        }
        match fs::remove_file(path) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(%error, lock = %path.display(), "stale accounting lock removal failed");
            }
        }
    }
    if removed > 0 {
        tracing::info!(removed, "stale accounting lock files removed");
    }
}

fn context_correlation(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let correlation_id = name
        .strip_prefix("accounting-context-")?
        .strip_suffix(".json")?;
    (correlation_id.len() == 32
        && correlation_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
    .then(|| correlation_id.to_owned())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .max(1)
}

/// 0.8.1: drain receipt journals that have no context file.
///
/// The correlation was never registered (daemon-free rewrite) or its context was retired, so
/// the receipts can only be recorded as `unattributed`. Recording them is still more honest
/// than an ever-growing undrained count: the token measurements are real, only the workspace
/// is unknown. Rejected batches are quarantined as `.rejected` and recorded as a producer gap.
async fn sweep_orphan_journals(
    state: &AppState,
    inventory: &ForkInventory, // 0.8.3: reuse the sweep's single directory listing
) -> Result<usize, String> {
    let now = SystemTime::now();
    let mut orphans = Vec::new();
    for (correlation_id, path) in &inventory.receipt_journals {
        if inventory.context_ids.contains(correlation_id) {
            continue;
        }
        let Ok(metadata) = fs::metadata(path) else {
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
        orphans.push((correlation_id.clone(), path.clone()));
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
        // 0.8.3: the orphan's producer is gone (no context, journal older than the grace).
        if let Err(error) = remove_accounting_locks(&handle) {
            tracing::warn!(%error, %correlation_id, "orphan accounting lock cleanup failed");
        }
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
    journal_correlation(path, "accounting-receipts-")
}

/// 0.8.3: the correlation of an active or pending journal with the given name prefix.
fn journal_correlation(path: &Path, prefix: &str) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix(prefix)?;
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
    directory: &Path,
) -> Result<usize, String> {
    let context = contexts.read(path).map_err(|error| error.to_string())?;
    let runner = state.rtk.runner().map_err(|error| error.to_string())?;
    let handle = runner
        .accounting_handle(&context.correlation_id)
        .map_err(|error| error.to_string())?;
    let drained = drain_accounting(&handle).map_err(|error| error.to_string())?;
    let batch_id = match &drained.status {
        AccountingDrainStatus::Empty if context.completed_at_unix.is_some() => {
            retire_context(state, path, &context.correlation_id); // 0.8.3: locks go with it
            return Ok(0);
        }
        AccountingDrainStatus::Empty => return Ok(0),
        AccountingDrainStatus::Ready { batch_id } if drained.failures.is_empty() => batch_id,
        AccountingDrainStatus::Ready { .. } | AccountingDrainStatus::Rejected { .. } => {
            // 0.8.3: a rejected batch was retried every second forever and every retry rewrote
            // the coverage state. Quarantine the evidence once, record one closed gap, move on.
            quarantine_pending_journals(directory, &context.correlation_id);
            let mut event = context.gap_event();
            event.operation_family = Some("rejected_receipt_batch".into());
            let store = AccountingCoverageStore::new(&state.config.data_dir);
            if let Err(error) = store
                .record_missing(event.clone())
                .and_then(|_| store.recover(event))
            {
                tracing::error!(%error, "rejected receipt batch could not be recorded");
            }
            tracing::warn!(
                status = ?drained.status,
                correlation_id = %context.correlation_id,
                "registered receipt batch rejected and quarantined"
            );
            return Ok(0);
        }
    };
    let mut committed = 0;
    for mut receipt in drained.receipts {
        attribute_receipt(&mut receipt, &context); // 0.8.3
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

/// 0.8.3: rename the pending journals of a rejected registered batch to `.rejected`, the same
/// quarantine the orphan pass uses, so the next sweep finds nothing to retry.
fn quarantine_pending_journals(directory: &Path, correlation_id: &str) {
    for stem in ["accounting-receipts", "accounting-failures"] {
        let pending = directory.join(format!("{stem}-{correlation_id}.jsonl.pending"));
        if !pending.is_file() {
            continue;
        }
        let quarantine = directory.join(format!("{stem}-{correlation_id}.jsonl.rejected"));
        if let Err(error) = fs::rename(&pending, &quarantine) {
            tracing::warn!(%error, journal = %pending.display(), "rejected journal quarantine failed");
        }
    }
}

pub async fn run(state: AppState) {
    loop {
        // 0.8.3: one second while producers are active or receipts drained, otherwise idle.
        let delay = match sweep(&state).await {
            Ok(outcome) if outcome.committed > 0 || outcome.active_producers > 0 => {
                Duration::from_secs(1)
            }
            Ok(_) => Duration::from_secs(IDLE_SWEEP_SECS),
            Err(error) => {
                tracing::warn!(%error, "accounting receipt sweep failed");
                Duration::from_secs(1)
            }
        };
        tokio::time::sleep(delay).await;
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
    // 0.8.3: age retires the registration file, never the gap it opened.
    async fn abandoned_registration_is_retired_with_its_locks_without_faking_recovery() {
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
        let fork = state.config.data_dir.join("fork");
        // Locks the producer and earlier drains leave behind; nothing removed them before 0.8.3.
        let journal_lock = fork.join(format!("accounting-receipts-{correlation_id}.jsonl.lock"));
        let drain_lock = fork.join(format!(
            "accounting-receipts-{correlation_id}.jsonl.drain.lock"
        ));
        fs::write(&journal_lock, b"").expect("journal lock");
        fs::write(&drain_lock, b"").expect("drain lock");

        // Fresh and without receipts: kept, and no drain is attempted to find that out.
        assert_eq!(sweep_once(&state).await.expect("fresh sweep"), 0);
        assert!(path.exists(), "a live registration waits for its receipts");
        assert!(
            journal_lock.exists() && drain_lock.exists(),
            "a live producer keeps its locks"
        );
        // 0.8.2: a registration is pending inside the producer grace; inspect it as settled.
        let settled = super::unix_now() + hzr_core::FORK_PRODUCER_PENDING_GRACE_SECS;
        assert!(
            !AccountingCoverageStore::new(&state.config.data_dir)
                .snapshot(settled)
                .expect("coverage")
                .live_complete
        );

        let mut context = contexts.read(&path).expect("context");
        context.registered_at_unix = context
            .registered_at_unix
            .saturating_sub(ABANDONED_CONTEXT_TTL_SECS + 1);
        fs::write(&path, serde_json::to_vec(&context).expect("context JSON"))
            .expect("expired context");

        assert_eq!(sweep_once(&state).await.expect("aged sweep"), 0);
        assert!(!path.exists(), "an abandoned registration is retired");
        assert!(
            !journal_lock.exists() && !drain_lock.exists(),
            "its lock files go with it"
        );
        // Retirement records nothing and recovers nothing: the gap the registration opened is
        // still open once the pending grace has elapsed.
        assert!(
            !AccountingCoverageStore::new(&state.config.data_dir)
                .snapshot(settled)
                .expect("coverage")
                .live_complete,
            "retiring the file does not fake a recovery"
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

    // 0.8.3: the fake engine fixture the tests above build inline, for the tests below.
    #[cfg(unix)]
    async fn engine_state(directory: &std::path::Path) -> AppState {
        use std::os::unix::fs::PermissionsExt;

        let engines = directory.join("engines");
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
            data_dir: directory.join("data"),
            ..Config::default()
        };
        config.engines.auto_start_icm = false;
        config.engines.directory = Some(engines);
        AppState::initialize(config).await.expect("state")
    }

    // 0.8.3: locks whose producer left neither a context nor a journal are swept after the grace.
    #[cfg(unix)]
    #[tokio::test]
    async fn stale_lock_files_without_context_or_journal_are_removed() {
        let directory = tempdir().expect("temporary directory");
        let state = engine_state(directory.path()).await;
        let fork = state.config.data_dir.join("fork");
        fs::create_dir_all(&fork).expect("fork directory");
        let stale =
            fork.join("accounting-receipts-00000000000000000000000000000001.jsonl.drain.lock");
        let fresh = fork.join("accounting-receipts-00000000000000000000000000000002.jsonl.lock");
        let with_journal =
            fork.join("accounting-receipts-00000000000000000000000000000003.jsonl.lock");
        for path in [&stale, &fresh, &with_journal] {
            fs::write(path, b"").expect("lock file");
        }
        fs::write(
            fork.join("accounting-receipts-00000000000000000000000000000003.jsonl"),
            b"{}\n",
        )
        .expect("journal");
        let old = std::time::SystemTime::now()
            - std::time::Duration::from_secs(super::ORPHAN_JOURNAL_GRACE_SECS + 60);
        for path in [&stale, &with_journal] {
            fs::File::options()
                .write(true)
                .open(path)
                .expect("lock handle")
                .set_modified(old)
                .expect("age the lock");
        }

        assert_eq!(sweep_once(&state).await.expect("sweep"), 0);
        assert!(
            !stale.exists(),
            "a stale lock without context or journal is removed"
        );
        assert!(
            fresh.exists(),
            "a fresh lock may belong to a producer that is about to register"
        );
        assert!(
            with_journal.exists(),
            "a lock whose journal still exists is kept"
        );
    }

    // 0.8.3: a rejected registered batch is quarantined and recorded once, not retried forever.
    #[cfg(unix)]
    #[tokio::test]
    async fn rejected_registered_batch_is_quarantined_once() {
        let directory = tempdir().expect("temporary directory");
        let state = engine_state(directory.path()).await;
        let project = directory.path().join("project");
        fs::create_dir(&project).expect("project");
        let correlation_id = "0123456789abcdef0123456789abcdef";
        register(
            &state,
            correlation_id,
            &project,
            Some("claude-code"),
            Some("s2"),
            hzr_protocol::AccountingChannel::HookCli,
        )
        .expect("registration");
        let fork = state.config.data_dir.join("fork");
        let journal = fork.join(format!("accounting-receipts-{correlation_id}.jsonl"));
        let mut receipt = fixture_receipt(correlation_id);
        receipt.engine.manifest_sha256 = "f".repeat(64); // a build the daemon does not accept
        fs::write(
            &journal,
            format!(
                "{}\n",
                serde_json::to_string(&receipt).expect("receipt JSON")
            ),
        )
        .expect("journal");

        assert_eq!(sweep_once(&state).await.expect("rejecting sweep"), 0);
        assert!(!journal.exists());
        assert!(
            !journal.with_extension("jsonl.pending").exists(),
            "the pending batch is not left for a retry"
        );
        assert!(
            journal.with_extension("jsonl.rejected").exists(),
            "the evidence is quarantined"
        );
        let coverage_path = state
            .config
            .data_dir
            .join("ledger/accounting-coverage.json");
        let settled = super::unix_now() + 3_600;
        let snapshot = AccountingCoverageStore::new(&state.config.data_dir)
            .snapshot(settled)
            .expect("coverage");
        assert_eq!(
            snapshot.closed_intervals, 1,
            "the rejection is one closed gap"
        );
        assert!(!snapshot.historical_complete);

        let written = fs::read(&coverage_path).expect("coverage state");
        assert_eq!(sweep_once(&state).await.expect("quiet sweep"), 0);
        assert!(journal.with_extension("jsonl.rejected").exists());
        assert_eq!(
            fs::read(&coverage_path).expect("coverage state"),
            written,
            "a quarantined batch is not recorded again"
        );
    }

    // 0.8.3: a receipt from the pinned engine build without a classification of its own.
    fn fixture_receipt(correlation_id: &str) -> EngineAccountingReceipt {
        EngineAccountingReceipt {
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
        }
    }

    // 0.8.3: the classification stored with the registration reaches receipts that arrive
    // without one and never overrides one the engine recorded itself.
    #[cfg(unix)]
    #[tokio::test]
    async fn registration_classification_is_attached_to_unclassified_receipts() {
        use hzr_protocol::{
            EnforcementTier, EvasionAttribution, EvasionClass, EvasionPathForm, FidelityValidation,
        };

        let directory = tempdir().expect("temporary directory");
        let state = engine_state(directory.path()).await;
        let project = directory.path().join("project");
        fs::create_dir(&project).expect("project");
        let correlation_id = "0123456789abcdef0123456789abcdef";
        let registered = EvasionAttribution {
            class: EvasionClass::E5PipelineOrRedirect,
            wrapper_depth: 0,
            interpreter: None,
            path_form: EvasionPathForm::Bare,
            stage_count: 3,
            hatch_marker: false,
            avoidable: true,
            tier: EnforcementTier::T1NamedCorrection,
            fidelity_reason: None,
            fidelity_validation: FidelityValidation::NotRequested,
        };
        super::register_attributed(
            &state,
            correlation_id,
            &project,
            Some("claude-code"),
            Some("s3"),
            hzr_protocol::AccountingChannel::HookCli,
            Some(registered),
        )
        .expect("attributed registration");
        let contexts = AccountingReceiptContextStore::new(&state.config.data_dir);
        let context = contexts
            .read(&contexts.context_path(correlation_id))
            .expect("context");
        assert_eq!(context.evasion, Some(registered));

        let mut unclassified = fixture_receipt(correlation_id);
        super::attribute_receipt(&mut unclassified, &context);
        assert_eq!(unclassified.attribution.evasion, Some(registered));

        let mut engine_recorded = registered;
        engine_recorded.class = EvasionClass::E2ShellWrapper;
        engine_recorded.wrapper_depth = 1;
        let mut classified = fixture_receipt(correlation_id);
        classified.attribution.evasion = Some(engine_recorded);
        super::attribute_receipt(&mut classified, &context);
        assert_eq!(
            classified.attribution.evasion,
            Some(engine_recorded),
            "a receipt that carries its own classification keeps it"
        );
    }
}
