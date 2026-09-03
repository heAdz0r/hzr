use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hzr_core::{
    AccountingCoverageStore, AccountingGapEvent, AccountingGapSurface, privacy_identity_hash,
};
use hzr_exec::{AccountingDrainStatus, acknowledge_accounting, drain_accounting};
use hzr_protocol::AccountingChannel;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::AppState;

const CONTEXT_PREFIX: &str = "accounting-context-";
const CONTEXT_SUFFIX: &str = ".json";
const MAX_CONTEXT_BYTES: u64 = 16 * 1024;
const ABANDONED_CONTEXT_TTL_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AccountingContext {
    correlation_id: String,
    project_path: String,
    agent: Option<String>,
    session_id: Option<String>,
    registered_at_unix: u64,
}

pub fn register(
    state: &AppState,
    correlation_id: &str,
    project_path: &Path,
    agent: Option<&str>,
    session_id: Option<&str>,
) -> Result<(), String> {
    let runner = state.rtk.runner().map_err(|error| error.to_string())?;
    runner
        .accounting_handle(correlation_id)
        .map_err(|error| error.to_string())?;
    let context = AccountingContext {
        correlation_id: correlation_id.to_owned(),
        project_path: project_path.to_string_lossy().into_owned(),
        agent: agent.map(str::to_owned),
        session_id: session_id.map(str::to_owned),
        registered_at_unix: unix_now(),
    };
    let path = context_path(&state.config.data_dir, correlation_id);
    let parent = path
        .parent()
        .ok_or_else(|| "accounting context path has no parent".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    serde_json::to_writer(&mut temporary, &context).map_err(|error| error.to_string())?;
    temporary
        .write_all(b"\n")
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    temporary
        .persist(&path)
        .map_err(|error| error.to_string())?;
    if let Err(error) =
        AccountingCoverageStore::new(&state.config.data_dir).record_missing(gap_event(&context))
    {
        let _ = fs::remove_file(&path);
        return Err(error.to_string());
    }
    Ok(())
}

pub async fn sweep_once(state: &AppState) -> Result<usize, String> {
    let directory = state.config.data_dir.join("fork");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.to_string()),
    };
    let mut committed = 0;
    for path in entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_context_path(path))
    {
        let context = read_context(&path)?;
        let runner = state.rtk.runner().map_err(|error| error.to_string())?;
        let handle = runner
            .accounting_handle(&context.correlation_id)
            .map_err(|error| error.to_string())?;
        let drained = drain_accounting(&handle).map_err(|error| error.to_string())?;
        let batch_id = match &drained.status {
            AccountingDrainStatus::Empty
                if unix_now().saturating_sub(context.registered_at_unix)
                    >= ABANDONED_CONTEXT_TTL_SECS =>
            {
                fs::remove_file(&path).map_err(|error| error.to_string())?;
                AccountingCoverageStore::new(&state.config.data_dir)
                    .recover(gap_event(&context))
                    .map_err(|error| error.to_string())?;
                continue;
            }
            AccountingDrainStatus::Empty => continue,
            AccountingDrainStatus::Ready { batch_id } if drained.failures.is_empty() => batch_id,
            AccountingDrainStatus::Ready { .. } | AccountingDrainStatus::Rejected { .. } => {
                continue;
            }
        };
        for receipt in drained.receipts {
            state
                .ledger
                .record_engine_receipt(
                    receipt,
                    context.project_path.clone(),
                    context.agent.clone(),
                    context.session_id.clone(),
                    AccountingChannel::HookCli,
                )
                .await
                .map_err(|error| error.to_string())?;
            committed += 1;
        }
        acknowledge_accounting(&handle, batch_id).map_err(|error| error.to_string())?;
        fs::remove_file(&path).map_err(|error| error.to_string())?;
        AccountingCoverageStore::new(&state.config.data_dir)
            .recover(gap_event(&context))
            .map_err(|error| error.to_string())?;
    }
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

fn read_context(path: &Path) -> Result<AccountingContext, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_CONTEXT_BYTES {
        return Err(format!(
            "accounting context exceeds {MAX_CONTEXT_BYTES} bytes"
        ));
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn context_path(data_root: &Path, correlation_id: &str) -> PathBuf {
    data_root
        .join("fork")
        .join(format!("{CONTEXT_PREFIX}{correlation_id}{CONTEXT_SUFFIX}"))
}

fn is_context_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(CONTEXT_PREFIX) && name.ends_with(CONTEXT_SUFFIX))
}

fn gap_event(context: &AccountingContext) -> AccountingGapEvent {
    AccountingGapEvent {
        surface: AccountingGapSurface::ForkProducer,
        workspace_hash: Some(privacy_identity_hash("workspace", &context.project_path)),
        session_hash: context
            .session_id
            .as_deref()
            .map(|session| privacy_identity_hash("session", session)),
        operation_family: None,
        at_unix: unix_now(),
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .max(1)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use hzr_core::{AccountingCoverageStore, Config};
    use hzr_exec::{ForkRuntimePaths, PINNED_RTK_VERSION, expected_engine_identity};
    use hzr_protocol::{
        AccountingAttribution, AccountingMeasurement, AccountingOperationKind,
        AccountingOperationMode, AccountingRoute, AccountingStage, ENGINE_CONTRACT_VERSION,
        EngineAccountingReceipt,
    };
    use tempfile::tempdir;

    use super::{ABANDONED_CONTEXT_TTL_SECS, context_path, read_context, register, sweep_once};
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

        assert_eq!(sweep_once(&state).await.expect("sweep"), 1);
        assert!(!journal.exists());
        assert!(
            AccountingCoverageStore::new(&state.config.data_dir)
                .snapshot(2)
                .expect("coverage")
                .live_complete
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn abandoned_empty_context_is_retired_without_poisoning_live_coverage() {
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
        register(&state, correlation_id, &project, None, Some("denied")).expect("registration");

        let path = context_path(&state.config.data_dir, correlation_id);
        let mut context = read_context(&path).expect("context");
        context.registered_at_unix = context
            .registered_at_unix
            .saturating_sub(ABANDONED_CONTEXT_TTL_SECS + 1);
        fs::write(&path, serde_json::to_vec(&context).expect("context JSON"))
            .expect("expired context");

        assert_eq!(sweep_once(&state).await.expect("sweep"), 0);
        assert!(!path.exists());
        assert!(
            AccountingCoverageStore::new(&state.config.data_dir)
                .snapshot(2)
                .expect("coverage")
                .live_complete
        );
    }
}
