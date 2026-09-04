use std::fs;
use std::path::Path;
use std::time::Duration;

use hzr_core::{AccountingCoverageStore, AccountingReceiptContextStore};
use hzr_exec::{AccountingDrainStatus, acknowledge_accounting, drain_accounting};
use hzr_protocol::AccountingChannel;

use crate::AppState;

#[cfg(test)]
const ABANDONED_CONTEXT_TTL_SECS: u64 = 24 * 60 * 60;

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
    Ok(committed)
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
        assert!(
            !AccountingCoverageStore::new(&state.config.data_dir)
                .snapshot(2)
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
                .snapshot(2)
                .expect("coverage")
                .live_complete,
            "missing receipts remain unresolved after cleanup"
        );
    }
}
