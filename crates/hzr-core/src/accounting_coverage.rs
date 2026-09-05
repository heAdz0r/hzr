use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use thiserror::Error;

const COVERAGE_SCHEMA_VERSION: u32 = 1;
const MAX_IDENTITY_BYTES: usize = 256;
const MAX_INTERVALS: usize = 100_000;
/// 0.8.3: a repeated inspection of one unresolved condition refreshes its timestamp at most this
/// often. The daemon sweeper re-inspects every second; rewriting and fsyncing the whole state
/// file for each look was a measured write storm.
const REPEATED_FAILURE_REWRITE_SECS: u64 = 60;
const ACCOUNTING_CONTEXT_PREFIX: &str = "accounting-context-";
const ACCOUNTING_CONTEXT_SUFFIX: &str = ".json";
const MAX_ACCOUNTING_CONTEXT_BYTES: u64 = 16 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingGapSurface {
    RewriteDaemon,
    Hook,
    Cli,
    Mcp,
    ForkProducer,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AccountingGapEvent {
    pub surface: AccountingGapSurface,
    pub workspace_hash: Option<String>,
    pub session_hash: Option<String>,
    pub operation_family: Option<String>,
    pub at_unix: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AccountingGapInterval {
    pub surface: AccountingGapSurface,
    #[serde(default)]
    pub workspace_hash: Option<String>,
    pub session_hash: Option<String>,
    pub operation_family: Option<String>,
    pub started_at_unix: u64,
    pub last_failure_at_unix: u64,
    pub recovered_at_unix: Option<u64>,
    pub missing_operations: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct AccountingCoverageSnapshot {
    pub open_intervals: usize,
    /// 0.8.2: fork-producer registrations younger than
    /// [`FORK_PRODUCER_PENDING_GRACE_SECS`] whose receipts have not drained yet. A command that
    /// is still running (or whose receipts the sweeper has not reached) is in flight, not a
    /// gap; counting it as one made every prompt and status line report `DEGRADED` while the
    /// previous command was still finishing.
    #[serde(default)]
    pub pending_producer_operations: u64,
    pub closed_intervals: usize,
    pub open_missing_operations: u64,
    pub lifetime_missing_operations: u64,
    pub legacy_missing_operations: u64,
    pub rewrite_missing_operations: u64,
    pub hook_missing_operations: u64,
    pub cli_missing_operations: u64,
    pub mcp_missing_operations: u64,
    pub fork_producer_missing_operations: u64,
    pub live_complete: bool,
    pub historical_complete: bool,
    pub gap_started_at_unix: Option<u64>,
    pub last_failure_at_unix: Option<u64>,
    pub last_recovered_at_unix: Option<u64>,
    pub open_gap_seconds: Option<u64>,
    pub closed_gap_seconds: u64,
}

#[derive(Debug, Error)]
pub enum AccountingCoverageError {
    #[error("accounting coverage I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("accounting coverage state is invalid: {0}")]
    Invalid(String),
    #[error("accounting coverage serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Deserialize, Serialize)]
struct AccountingCoverageState {
    schema_version: u32,
    #[serde(default)]
    legacy_missing_operations: u64,
    intervals: Vec<AccountingGapInterval>,
}

impl Default for AccountingCoverageState {
    fn default() -> Self {
        Self {
            schema_version: COVERAGE_SCHEMA_VERSION,
            legacy_missing_operations: 0,
            intervals: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AccountingCoverageStore {
    state_path: PathBuf,
    lock_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccountingReceiptContext {
    pub correlation_id: String,
    pub project_path: String,
    pub agent: Option<String>,
    pub session_id: Option<String>,
    pub registered_at_unix: u64,
    #[serde(default)]
    pub completed_at_unix: Option<u64>,
    #[serde(default = "default_accounting_channel")]
    pub channel: hzr_protocol::AccountingChannel,
    /// 0.8.3: how the command was classified when it was approved. The hook used to export this
    /// as JSON into the approved command's environment; it now travels with the registration and
    /// the daemon attaches it to the receipts it drains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evasion: Option<hzr_protocol::EvasionAttribution>,
}

fn default_accounting_channel() -> hzr_protocol::AccountingChannel {
    hzr_protocol::AccountingChannel::HookCli
}

impl AccountingReceiptContext {
    #[must_use]
    pub fn gap_event(&self) -> AccountingGapEvent {
        AccountingGapEvent {
            surface: AccountingGapSurface::ForkProducer,
            workspace_hash: Some(crate::privacy_identity_hash(
                "workspace",
                &self.project_path,
            )),
            session_hash: self
                .session_id
                .as_deref()
                .map(|session| crate::privacy_identity_hash("session", session)),
            operation_family: None,
            at_unix: unix_now(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AccountingReceiptContextStore {
    data_root: PathBuf,
}

impl AccountingReceiptContextStore {
    #[must_use]
    pub fn new(data_root: &Path) -> Self {
        Self {
            data_root: data_root.to_owned(),
        }
    }

    pub fn register(
        &self,
        correlation_id: &str,
        project_path: &Path,
        agent: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<(), AccountingCoverageError> {
        self.register_with_channel(
            correlation_id,
            project_path,
            agent,
            session_id,
            default_accounting_channel(),
        )
    }

    pub fn register_with_channel(
        &self,
        correlation_id: &str,
        project_path: &Path,
        agent: Option<&str>,
        session_id: Option<&str>,
        channel: hzr_protocol::AccountingChannel,
    ) -> Result<(), AccountingCoverageError> {
        self.register_with_attribution(
            correlation_id,
            project_path,
            agent,
            session_id,
            channel,
            None,
        )
    }

    /// 0.8.3: register a producer together with the classification of the command it will run.
    /// The daemon copies `evasion` onto every receipt of this correlation that arrives without
    /// one, so the approved command no longer has to carry the classification in its environment.
    pub fn register_with_attribution(
        &self,
        correlation_id: &str,
        project_path: &Path,
        agent: Option<&str>,
        session_id: Option<&str>,
        channel: hzr_protocol::AccountingChannel,
        evasion: Option<hzr_protocol::EvasionAttribution>,
    ) -> Result<(), AccountingCoverageError> {
        validate_correlation_id(correlation_id)?;
        let context = AccountingReceiptContext {
            correlation_id: correlation_id.to_owned(),
            project_path: project_path.to_string_lossy().into_owned(),
            agent: agent.map(str::to_owned),
            session_id: session_id.map(str::to_owned),
            registered_at_unix: unix_now(),
            completed_at_unix: None,
            channel,
            evasion,
        };
        let path = self.context_path(correlation_id);
        let parent = path.parent().ok_or_else(|| {
            AccountingCoverageError::Invalid("accounting context path has no parent".into())
        })?;
        fs::create_dir_all(parent).map_err(|source| context_io(&path, source))?;
        let existing = fs::read_dir(parent)
            .map_err(|source| context_io(&path, source))?
            .filter_map(Result::ok)
            .filter(|entry| Self::is_context_path(&entry.path()))
            .take(20_001)
            .count();
        if existing >= 20_000 {
            return Err(AccountingCoverageError::Invalid(
                "unresolved accounting context capacity reached; pending attribution was retained"
                    .into(),
            ));
        }
        write_context(&path, &context)?;
        if let Err(error) =
            AccountingCoverageStore::new(&self.data_root).record_missing(context.gap_event())
        {
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        Ok(())
    }

    /// Called only after the owned producer has exited and its final writes are closed.
    pub fn complete(&self, correlation_id: &str) -> Result<(), AccountingCoverageError> {
        validate_correlation_id(correlation_id)?;
        let path = self.context_path(correlation_id);
        let mut context = self.read(&path)?;
        context.completed_at_unix = Some(unix_now());
        write_context(&path, &context) // 0.8.3: one atomic writer for every context update
    }

    #[must_use]
    pub fn context_path(&self, correlation_id: &str) -> PathBuf {
        self.data_root.join("fork").join(format!(
            "{ACCOUNTING_CONTEXT_PREFIX}{correlation_id}{ACCOUNTING_CONTEXT_SUFFIX}"
        ))
    }

    #[must_use]
    pub fn is_context_path(path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.starts_with(ACCOUNTING_CONTEXT_PREFIX)
                    && name.ends_with(ACCOUNTING_CONTEXT_SUFFIX)
            })
    }

    pub fn read(&self, path: &Path) -> Result<AccountingReceiptContext, AccountingCoverageError> {
        let metadata = fs::symlink_metadata(path).map_err(|source| context_io(path, source))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(AccountingCoverageError::Invalid(
                "accounting context must be a regular file".into(),
            ));
        }
        if metadata.len() > MAX_ACCOUNTING_CONTEXT_BYTES {
            return Err(AccountingCoverageError::Invalid(format!(
                "accounting context exceeds {MAX_ACCOUNTING_CONTEXT_BYTES} bytes"
            )));
        }
        let bytes = fs::read(path).map_err(|source| context_io(path, source))?;
        let context: AccountingReceiptContext = serde_json::from_slice(&bytes)?;
        validate_correlation_id(&context.correlation_id)?;
        if self.context_path(&context.correlation_id) != path {
            return Err(AccountingCoverageError::Invalid(
                "accounting context correlation does not match its path".into(),
            ));
        }
        Ok(context)
    }
}

impl AccountingCoverageStore {
    #[must_use]
    pub fn new(data_root: &Path) -> Self {
        let ledger = data_root.join("ledger");
        Self {
            state_path: ledger.join("accounting-coverage.json"),
            lock_path: ledger.join("accounting-coverage.lock"),
        }
    }

    #[must_use]
    pub fn exists(&self) -> bool {
        self.state_path.is_file()
    }

    pub fn record_missing(
        &self,
        event: AccountingGapEvent,
    ) -> Result<AccountingCoverageSnapshot, AccountingCoverageError> {
        self.record_missing_event(event, true)
    }

    /// Repeated inspection of one unresolved condition is not another missing operation.
    pub fn ensure_missing(
        &self,
        event: AccountingGapEvent,
    ) -> Result<AccountingCoverageSnapshot, AccountingCoverageError> {
        self.record_missing_event(event, false)
    }

    fn record_missing_event(
        &self,
        event: AccountingGapEvent,
        count_repetition: bool,
    ) -> Result<AccountingCoverageSnapshot, AccountingCoverageError> {
        validate_event(&event)?;
        self.with_exclusive_state(|state| {
            let interval = state.intervals.iter_mut().rev().find(|interval| {
                interval.recovered_at_unix.is_none()
                    && interval.surface == event.surface
                    && interval.workspace_hash == event.workspace_hash
                    && interval.session_hash == event.session_hash
                    && interval.operation_family == event.operation_family
            });
            match interval {
                Some(interval) => {
                    let mut changed = false; // 0.8.3: report whether the state needs a rewrite
                    if count_repetition {
                        interval.missing_operations = interval.missing_operations.saturating_add(1);
                        changed = true;
                    }
                    // 0.8.3: a bare re-inspection moves the timestamp at most once a minute.
                    let advanced = event.at_unix.saturating_sub(interval.last_failure_at_unix);
                    if advanced > 0
                        && (count_repetition || advanced >= REPEATED_FAILURE_REWRITE_SECS)
                    {
                        interval.last_failure_at_unix = event.at_unix;
                        changed = true;
                    }
                    Ok(changed)
                }
                None => {
                    if state.intervals.len() >= MAX_INTERVALS {
                        return Err(AccountingCoverageError::Invalid(format!(
                            "interval limit {MAX_INTERVALS} exceeded"
                        )));
                    }
                    state.intervals.push(AccountingGapInterval {
                        surface: event.surface,
                        workspace_hash: event.workspace_hash,
                        session_hash: event.session_hash,
                        operation_family: event.operation_family,
                        started_at_unix: event.at_unix,
                        last_failure_at_unix: event.at_unix,
                        recovered_at_unix: None,
                        missing_operations: 1,
                    });
                    Ok(true)
                }
            }
        })?;
        self.snapshot(event.at_unix)
    }

    pub fn recover(&self, event: AccountingGapEvent) -> Result<usize, AccountingCoverageError> {
        validate_event(&event)?;
        if !self.exists() {
            return Ok(0);
        }
        let mut recovered = 0;
        self.with_exclusive_state(|state| {
            state.intervals.retain_mut(|interval| {
                let matches = interval.recovered_at_unix.is_none()
                    && interval.surface == event.surface
                    && interval.workspace_hash == event.workspace_hash
                    && interval.session_hash == event.session_hash
                    && interval.operation_family == event.operation_family;
                if !matches {
                    return true;
                }
                recovered += 1;
                // 0.8.3: a producer registration recovered inside its grace was in flight, never
                // a gap. Closing it instead made every successful command a "historical gap":
                // 2 067 of 2 071 intervals on one workstation were such commands, the state
                // file grew to 714 KB and was rewritten twice per command.
                if is_settled_registration(interval, event.at_unix) {
                    return false;
                }
                interval.recovered_at_unix = Some(event.at_unix.max(interval.last_failure_at_unix));
                true
            });
            Ok(recovered > 0)
        })?;
        Ok(recovered)
    }

    pub fn import_legacy(
        &self,
        legacy_missing_operations: u64,
    ) -> Result<(), AccountingCoverageError> {
        if legacy_missing_operations == 0 || self.exists() {
            return Ok(());
        }
        self.with_exclusive_state(|state| {
            state.legacy_missing_operations = legacy_missing_operations;
            Ok(true) // 0.8.3: the closure reports whether the state changed
        })
    }

    pub fn snapshot(
        &self,
        now_unix: u64,
    ) -> Result<AccountingCoverageSnapshot, AccountingCoverageError> {
        self.snapshot_filtered(None, now_unix)
    }

    pub fn snapshot_for_context(
        &self,
        session_hash: &str,
        workspace_hash: &str,
        now_unix: u64,
    ) -> Result<AccountingCoverageSnapshot, AccountingCoverageError> {
        for (field, value) in [
            ("session_hash", session_hash),
            ("workspace_hash", workspace_hash),
        ] {
            if value.is_empty() || value.len() > MAX_IDENTITY_BYTES {
                return Err(AccountingCoverageError::Invalid(format!(
                    "{field} must contain 1..={MAX_IDENTITY_BYTES} bytes"
                )));
            }
        }
        self.snapshot_filtered(Some((session_hash, workspace_hash)), now_unix)
    }

    fn snapshot_filtered(
        &self,
        context: Option<(&str, &str)>,
        now_unix: u64,
    ) -> Result<AccountingCoverageSnapshot, AccountingCoverageError> {
        let lock = self.open_lock()?;
        FileExt::lock_shared(&lock).map_err(|source| self.io(source))?;
        let state = self.read_state()?;
        FileExt::unlock(&lock).map_err(|source| self.io(source))?;
        Ok(snapshot(&state, context, now_unix))
    }

    /// Runs `update` under the exclusive lock and rewrites the state only when it reports a change.
    fn with_exclusive_state(
        &self,
        update: impl FnOnce(&mut AccountingCoverageState) -> Result<bool, AccountingCoverageError>,
    ) -> Result<(), AccountingCoverageError> {
        let lock = self.open_lock()?;
        lock.lock_exclusive().map_err(|source| self.io(source))?;
        let mut state = self.read_state()?;
        // 0.8.3: an unchanged state is not rewritten; a changed one is pruned of settled
        // registrations first, which also migrates files written by earlier versions.
        if update(&mut state)? {
            prune_settled_registrations(&mut state);
            self.write_state(&state)?;
        }
        FileExt::unlock(&lock).map_err(|source| self.io(source))
    }

    fn open_lock(&self) -> Result<std::fs::File, AccountingCoverageError> {
        let parent = self
            .state_path
            .parent()
            .ok_or_else(|| AccountingCoverageError::Invalid("state path has no parent".into()))?;
        fs::create_dir_all(parent).map_err(|source| self.io(source))?;
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.lock_path)
            .map_err(|source| self.io(source))
    }

    fn read_state(&self) -> Result<AccountingCoverageState, AccountingCoverageError> {
        let bytes = match fs::read(&self.state_path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(AccountingCoverageState::default());
            }
            Err(source) => return Err(self.io(source)),
        };
        let state: AccountingCoverageState = serde_json::from_slice(&bytes)?;
        validate_state(&state)?;
        Ok(state)
    }

    fn write_state(&self, state: &AccountingCoverageState) -> Result<(), AccountingCoverageError> {
        validate_state(state)?;
        let parent = self
            .state_path
            .parent()
            .ok_or_else(|| AccountingCoverageError::Invalid("state path has no parent".into()))?;
        let mut temporary = NamedTempFile::new_in(parent).map_err(|source| self.io(source))?;
        serde_json::to_writer(&mut temporary, state)?;
        temporary
            .write_all(b"\n")
            .map_err(|source| self.io(source))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|source| self.io(source))?;
        temporary
            .persist(&self.state_path)
            .map_err(|error| self.io(error.error))?;
        Ok(())
    }

    fn io(&self, source: std::io::Error) -> AccountingCoverageError {
        AccountingCoverageError::Io {
            path: self.state_path.clone(),
            source,
        }
    }
}

fn validate_correlation_id(correlation_id: &str) -> Result<(), AccountingCoverageError> {
    if correlation_id.len() != 32
        || !correlation_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(AccountingCoverageError::Invalid(
            "accounting correlation id must be 32 lowercase hexadecimal characters".into(),
        ));
    }
    Ok(())
}

fn context_io(path: &Path, source: std::io::Error) -> AccountingCoverageError {
    AccountingCoverageError::Io {
        path: path.to_owned(),
        source,
    }
}

/// 0.8.3: atomically replace a registration context (temporary file, fsync, rename).
fn write_context(
    path: &Path,
    context: &AccountingReceiptContext,
) -> Result<(), AccountingCoverageError> {
    let parent = path
        .parent()
        .ok_or_else(|| AccountingCoverageError::Invalid("context parent missing".into()))?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|source| context_io(path, source))?;
    serde_json::to_writer(&mut temporary, context)?;
    temporary
        .write_all(b"\n")
        .map_err(|source| context_io(path, source))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| context_io(path, source))?;
    temporary
        .persist(path)
        .map_err(|error| context_io(path, error.error))?;
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .max(1)
}

fn validate_event(event: &AccountingGapEvent) -> Result<(), AccountingCoverageError> {
    if event.at_unix == 0 {
        return Err(AccountingCoverageError::Invalid(
            "accounting event timestamp must be non-zero".into(),
        ));
    }
    for (field, value) in [
        ("workspace_hash", event.workspace_hash.as_deref()),
        ("session_hash", event.session_hash.as_deref()),
        ("operation_family", event.operation_family.as_deref()),
    ] {
        if value.is_some_and(|value| value.is_empty() || value.len() > MAX_IDENTITY_BYTES) {
            return Err(AccountingCoverageError::Invalid(format!(
                "{field} must contain 1..={MAX_IDENTITY_BYTES} bytes"
            )));
        }
    }
    Ok(())
}

fn validate_state(state: &AccountingCoverageState) -> Result<(), AccountingCoverageError> {
    if state.schema_version != COVERAGE_SCHEMA_VERSION {
        return Err(AccountingCoverageError::Invalid(format!(
            "unsupported schema version {}",
            state.schema_version
        )));
    }
    if state.intervals.len() > MAX_INTERVALS {
        return Err(AccountingCoverageError::Invalid(format!(
            "interval limit {MAX_INTERVALS} exceeded"
        )));
    }
    if state.intervals.iter().any(|interval| {
        interval.missing_operations == 0
            || interval.started_at_unix == 0
            || interval.last_failure_at_unix < interval.started_at_unix
            || interval
                .recovered_at_unix
                .is_some_and(|recovered| recovered < interval.last_failure_at_unix)
    }) {
        return Err(AccountingCoverageError::Invalid(
            "interval ordering or count is invalid".into(),
        ));
    }
    Ok(())
}

/// How long a fork-producer registration may wait for its receipts before it counts as a gap.
/// Matches the daemon sweeper's orphan-journal grace so the two views agree.
pub const FORK_PRODUCER_PENDING_GRACE_SECS: u64 = 600;

/// 0.8.3: a fork-producer registration whose receipts drained inside the pending grace. It was
/// in flight the whole time and is removed on recovery instead of being kept as a closed gap.
fn is_settled_registration(interval: &AccountingGapInterval, recovered_at_unix: u64) -> bool {
    interval.surface == AccountingGapSurface::ForkProducer
        && interval.operation_family.is_none()
        && recovered_at_unix.saturating_sub(interval.last_failure_at_unix)
            < FORK_PRODUCER_PENDING_GRACE_SECS
}

/// 0.8.3: drop closed intervals that were settled registrations. Earlier versions closed them
/// instead of removing them, so a state file could hold thousands of successful commands as
/// "historical gaps"; pruning on write migrates such a file the first time it changes.
fn prune_settled_registrations(state: &mut AccountingCoverageState) {
    state.intervals.retain(|interval| {
        !interval
            .recovered_at_unix
            .is_some_and(|recovered| is_settled_registration(interval, recovered))
    });
}

fn snapshot(
    state: &AccountingCoverageState,
    context: Option<(&str, &str)>,
    now_unix: u64,
) -> AccountingCoverageSnapshot {
    let legacy_missing_operations = if context.is_none() {
        state.legacy_missing_operations
    } else {
        0
    };
    let mut result = AccountingCoverageSnapshot {
        legacy_missing_operations,
        lifetime_missing_operations: legacy_missing_operations,
        historical_complete: legacy_missing_operations == 0,
        ..AccountingCoverageSnapshot::default()
    };
    for interval in &state.intervals {
        if let Some((session, workspace)) = context {
            if interval.workspace_hash.as_deref() != Some(workspace)
                || interval
                    .session_hash
                    .as_deref()
                    .is_some_and(|value| value != session)
            {
                continue;
            }
        }
        // 0.8.2: an open producer registration inside the grace window is in flight, not
        // evidence of a gap; it becomes one only when receipts fail to arrive in time.
        if interval.recovered_at_unix.is_none()
            && interval.surface == AccountingGapSurface::ForkProducer
            && interval.operation_family.is_none()
            && now_unix.saturating_sub(interval.last_failure_at_unix)
                < FORK_PRODUCER_PENDING_GRACE_SECS
        {
            result.pending_producer_operations = result
                .pending_producer_operations
                .saturating_add(interval.missing_operations);
            continue;
        }
        result.historical_complete = false;
        result.lifetime_missing_operations = result
            .lifetime_missing_operations
            .saturating_add(interval.missing_operations);
        result.last_failure_at_unix = Some(
            result
                .last_failure_at_unix
                .unwrap_or_default()
                .max(interval.last_failure_at_unix),
        );
        let surface_total = match interval.surface {
            AccountingGapSurface::RewriteDaemon => &mut result.rewrite_missing_operations,
            AccountingGapSurface::Hook => &mut result.hook_missing_operations,
            AccountingGapSurface::Cli => &mut result.cli_missing_operations,
            AccountingGapSurface::Mcp => &mut result.mcp_missing_operations,
            AccountingGapSurface::ForkProducer => &mut result.fork_producer_missing_operations,
        };
        *surface_total = surface_total.saturating_add(interval.missing_operations);
        match interval.recovered_at_unix {
            Some(recovered) => {
                result.closed_intervals = result.closed_intervals.saturating_add(1);
                result.last_recovered_at_unix = Some(
                    result
                        .last_recovered_at_unix
                        .unwrap_or_default()
                        .max(recovered),
                );
                result.closed_gap_seconds = result
                    .closed_gap_seconds
                    .saturating_add(recovered.saturating_sub(interval.started_at_unix));
            }
            None => {
                result.open_intervals = result.open_intervals.saturating_add(1);
                result.open_missing_operations = result
                    .open_missing_operations
                    .saturating_add(interval.missing_operations);
                result.gap_started_at_unix = Some(
                    result
                        .gap_started_at_unix
                        .map_or(interval.started_at_unix, |started| {
                            started.min(interval.started_at_unix)
                        }),
                );
            }
        }
    }
    result.live_complete = result.open_intervals == 0;
    result.open_gap_seconds = result
        .gap_started_at_unix
        .map(|started| now_unix.saturating_sub(started));
    result
}

#[cfg(test)]
mod pending_producer_tests {
    use super::*;

    // 0.8.2: a registration whose receipts have not drained yet is pending, not a gap.
    #[test]
    fn in_flight_producer_registration_is_pending_until_the_grace_elapses() {
        let directory = tempfile::tempdir().expect("directory");
        let store = AccountingCoverageStore::new(directory.path());
        let event = AccountingGapEvent {
            surface: AccountingGapSurface::ForkProducer,
            workspace_hash: Some("sha256:workspace".into()),
            session_hash: Some("sha256:session".into()),
            operation_family: None,
            at_unix: 1_000,
        };
        store.record_missing(event.clone()).expect("registration");
        let fresh = store.snapshot(1_000 + 5).expect("snapshot");
        assert!(fresh.live_complete);
        assert!(fresh.historical_complete);
        assert_eq!(fresh.pending_producer_operations, 1);
        assert_eq!(fresh.open_missing_operations, 0);
        let late = store
            .snapshot(1_000 + FORK_PRODUCER_PENDING_GRACE_SECS)
            .expect("snapshot");
        assert!(!late.live_complete);
        assert_eq!(late.pending_producer_operations, 0);
        assert_eq!(late.open_missing_operations, 1);
        // 0.8.3: receipts that arrive after the grace close a real gap.
        let mut late_recovery = event;
        late_recovery.at_unix = 1_000 + FORK_PRODUCER_PENDING_GRACE_SECS + 1;
        store.recover(late_recovery).expect("recover");
        let recovered = store
            .snapshot(1_000 + FORK_PRODUCER_PENDING_GRACE_SECS + 1)
            .expect("snapshot");
        assert!(recovered.live_complete);
        assert!(!recovered.historical_complete);
        assert_eq!(recovered.closed_intervals, 1);
    }

    // 0.8.3: a registration recovered inside the grace never was a gap and leaves nothing behind.
    #[test]
    fn recovery_inside_the_grace_removes_the_registration_instead_of_closing_a_gap() {
        let directory = tempfile::tempdir().expect("directory");
        let store = AccountingCoverageStore::new(directory.path());
        let event = AccountingGapEvent {
            surface: AccountingGapSurface::ForkProducer,
            workspace_hash: Some("sha256:workspace".into()),
            session_hash: Some("sha256:session".into()),
            operation_family: None,
            at_unix: 1_000,
        };
        store.record_missing(event.clone()).expect("registration");
        let mut prompt_recovery = event;
        prompt_recovery.at_unix = 1_005;
        assert_eq!(store.recover(prompt_recovery).expect("recover"), 1);
        let settled = store
            .snapshot(1_000 + FORK_PRODUCER_PENDING_GRACE_SECS + 1)
            .expect("snapshot");
        assert!(settled.live_complete);
        assert!(settled.historical_complete);
        assert_eq!(settled.closed_intervals, 0);
        assert_eq!(settled.lifetime_missing_operations, 0);
        assert_eq!(settled.closed_gap_seconds, 0);
    }

    // 0.8.3: state files written by earlier versions lose their settled registrations on the
    // next write; real gaps and their totals are kept.
    #[test]
    fn settled_registrations_written_by_earlier_versions_are_pruned_on_the_next_write() {
        let directory = tempfile::tempdir().expect("directory");
        let ledger = directory.path().join("ledger");
        std::fs::create_dir_all(&ledger).expect("ledger directory");
        std::fs::write(
            ledger.join("accounting-coverage.json"),
            concat!(
                r#"{"schema_version":1,"legacy_missing_operations":0,"intervals":["#,
                r#"{"surface":"fork_producer","workspace_hash":"w","session_hash":"s","#,
                r#""operation_family":null,"started_at_unix":100,"last_failure_at_unix":100,"#,
                r#""recovered_at_unix":103,"missing_operations":1},"#,
                r#"{"surface":"mcp","workspace_hash":"w","session_hash":"s","#,
                r#""operation_family":"search","started_at_unix":200,"last_failure_at_unix":200,"#,
                r#""recovered_at_unix":260,"missing_operations":2}]}"#,
                "\n"
            ),
        )
        .expect("legacy state");
        let store = AccountingCoverageStore::new(directory.path());
        assert_eq!(
            store
                .snapshot(10_000)
                .expect("legacy read")
                .closed_intervals,
            2
        );

        store
            .record_missing(AccountingGapEvent {
                surface: AccountingGapSurface::Cli,
                workspace_hash: Some("w".into()),
                session_hash: Some("s".into()),
                operation_family: Some("read".into()),
                at_unix: 10_000,
            })
            .expect("first write after upgrade");
        let migrated = store.snapshot(10_000).expect("migrated read");
        assert_eq!(migrated.closed_intervals, 1);
        assert_eq!(migrated.open_intervals, 1);
        assert_eq!(migrated.lifetime_missing_operations, 3);
        assert_eq!(migrated.mcp_missing_operations, 2);
        assert_eq!(migrated.closed_gap_seconds, 60);
    }

    // 0.8.3: inspecting one open condition again rewrites the state at most once a minute.
    #[test]
    fn repeated_inspection_refreshes_the_timestamp_at_most_once_a_minute() {
        let directory = tempfile::tempdir().expect("directory");
        let store = AccountingCoverageStore::new(directory.path());
        let state_path = directory.path().join("ledger/accounting-coverage.json");
        let event = |at_unix| AccountingGapEvent {
            surface: AccountingGapSurface::ForkProducer,
            workspace_hash: None,
            session_hash: None,
            operation_family: Some("invalid_accounting_context".into()),
            at_unix,
        };
        store.record_missing(event(1_000)).expect("first failure");
        let written = std::fs::read(&state_path).expect("state");

        store.ensure_missing(event(1_030)).expect("inspection");
        assert_eq!(
            std::fs::read(&state_path).expect("state"),
            written,
            "an inspection inside the minute leaves the file untouched"
        );
        assert_eq!(
            store
                .snapshot(1_030)
                .expect("snapshot")
                .last_failure_at_unix,
            Some(1_000)
        );

        store
            .ensure_missing(event(1_060))
            .expect("later inspection");
        assert_ne!(std::fs::read(&state_path).expect("state"), written);
        let refreshed = store.snapshot(1_060).expect("snapshot");
        assert_eq!(refreshed.last_failure_at_unix, Some(1_060));
        assert_eq!(refreshed.open_missing_operations, 1);
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn repeated_condition_does_not_inflate_missing_operation_count() {
        let directory = tempfile::tempdir().expect("valid accounting fixture");
        let store = AccountingCoverageStore::new(directory.path());
        let event = super::AccountingGapEvent {
            surface: super::AccountingGapSurface::ForkProducer,
            workspace_hash: None,
            session_hash: None,
            operation_family: Some("invalid_context".into()),
            at_unix: 1,
        };
        for _ in 0..5 {
            store
                .ensure_missing(event.clone())
                .expect("valid accounting fixture");
        }
        assert_eq!(
            store
                .snapshot(1)
                .expect("valid accounting fixture")
                .open_missing_operations,
            1
        );
        store
            .record_missing(event)
            .expect("valid accounting fixture");
        assert_eq!(
            store
                .snapshot(1)
                .expect("valid accounting fixture")
                .open_missing_operations,
            2
        );
    }
    use std::sync::{Arc, Barrier};
    use std::thread;

    use tempfile::tempdir;

    use super::{
        AccountingCoverageStore, AccountingGapEvent, AccountingGapSurface,
        FORK_PRODUCER_PENDING_GRACE_SECS,
    };

    fn event(at_unix: u64) -> AccountingGapEvent {
        AccountingGapEvent {
            surface: AccountingGapSurface::ForkProducer,
            workspace_hash: Some("workspace-hash".into()),
            session_hash: Some("session-hash".into()),
            operation_family: Some("read".into()),
            at_unix,
        }
    }

    #[test]
    fn gap_recovery_preserves_closed_duration_and_history() {
        let directory = tempdir().expect("coverage root");
        let store = AccountingCoverageStore::new(directory.path());
        store.record_missing(event(10)).expect("first failure");
        store.record_missing(event(15)).expect("second failure");

        assert_eq!(store.recover(event(22)).expect("recover"), 1);
        assert_eq!(store.recover(event(30)).expect("idempotent"), 0);
        let snapshot = store.snapshot(40).expect("snapshot");
        assert!(snapshot.live_complete);
        assert!(!snapshot.historical_complete);
        assert_eq!(snapshot.lifetime_missing_operations, 2);
        assert_eq!(snapshot.closed_intervals, 1);
        assert_eq!(snapshot.closed_gap_seconds, 12);
        assert_eq!(snapshot.last_recovered_at_unix, Some(22));
    }

    #[test]
    fn concurrent_failure_and_recovery_never_erase_an_event() {
        let directory = tempdir().expect("coverage root");
        let store = Arc::new(AccountingCoverageStore::new(directory.path()));
        store.record_missing(event(10)).expect("initial failure");
        let barrier = Arc::new(Barrier::new(3));
        let writer = {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                store.record_missing(event(20)).expect("racing failure");
            })
        };
        let recovery = {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                store.recover(event(30)).expect("racing recovery");
            })
        };
        barrier.wait();
        writer.join().expect("writer thread");
        recovery.join().expect("recovery thread");

        // Registrations are pending inside the grace window; inspect them as settled gaps.
        let snapshot = store
            .snapshot(40 + FORK_PRODUCER_PENDING_GRACE_SECS)
            .expect("snapshot");
        assert_eq!(snapshot.lifetime_missing_operations, 2);
        assert!(snapshot.open_intervals <= 1);
        assert!(snapshot.closed_intervals <= 1);
        assert!((1..=2).contains(&(snapshot.open_intervals + snapshot.closed_intervals)));
    }

    #[test]
    fn corrupt_state_is_unknown_instead_of_defaulting_to_complete() {
        let directory = tempdir().expect("coverage root");
        let store = AccountingCoverageStore::new(directory.path());
        store.record_missing(event(10)).expect("failure");
        std::fs::write(
            directory.path().join("ledger/accounting-coverage.json"),
            b"{not-json",
        )
        .expect("corrupt state");
        assert!(store.snapshot(20).is_err());
        assert!(store.record_missing(event(20)).is_err());
    }

    #[test]
    fn context_snapshot_excludes_other_sessions_and_workspaces() {
        let directory = tempdir().expect("coverage root");
        let store = AccountingCoverageStore::new(directory.path());
        store.record_missing(event(10)).expect("failure");

        let settled = 20 + FORK_PRODUCER_PENDING_GRACE_SECS;
        let matching = store
            .snapshot_for_context("session-hash", "workspace-hash", settled)
            .expect("matching session");
        let other = store
            .snapshot_for_context("other-session", "workspace-hash", settled)
            .expect("other session");
        let other_workspace = store
            .snapshot_for_context("session-hash", "other-workspace", settled)
            .expect("other workspace");
        assert!(!matching.live_complete);
        assert!(other.live_complete);
        assert!(other.historical_complete);
        assert!(other_workspace.live_complete);
        assert!(other_workspace.historical_complete);
    }

    #[test]
    fn recovery_closes_only_the_exact_surface_and_operation_family() {
        let directory = tempdir().expect("coverage root");
        let store = AccountingCoverageStore::new(directory.path());
        store.record_missing(event(10)).expect("fork failure");
        let mut mcp = event(11);
        mcp.surface = AccountingGapSurface::Mcp;
        mcp.operation_family = Some("search".into());
        store.record_missing(mcp.clone()).expect("MCP failure");

        assert_eq!(store.recover(mcp).expect("MCP recovery"), 1);
        let snapshot = store
            .snapshot(20 + FORK_PRODUCER_PENDING_GRACE_SECS)
            .expect("snapshot");
        assert_eq!(snapshot.open_intervals, 1);
        assert_eq!(snapshot.fork_producer_missing_operations, 1);
        assert_eq!(snapshot.mcp_missing_operations, 1);
    }
}

// 0.8.3: the command classification is stored with the registration.
#[cfg(test)]
mod attribution_tests {
    use hzr_protocol::{
        AccountingChannel, EnforcementTier, EvasionAttribution, EvasionClass, EvasionPathForm,
        FidelityValidation,
    };

    use super::AccountingReceiptContextStore;

    fn classification() -> EvasionAttribution {
        EvasionAttribution {
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
        }
    }

    #[test]
    fn registration_keeps_the_classification_and_reads_contexts_written_without_one() {
        let directory = tempfile::tempdir().expect("directory");
        let store = AccountingReceiptContextStore::new(directory.path());
        let project = directory.path().join("project");
        let correlation_id = "0123456789abcdef0123456789abcdef";
        store
            .register_with_attribution(
                correlation_id,
                &project,
                Some("claude-code"),
                Some("s1"),
                AccountingChannel::HookCli,
                Some(classification()),
            )
            .expect("attributed registration");
        let path = store.context_path(correlation_id);
        let context = store.read(&path).expect("context");
        assert_eq!(context.evasion, Some(classification()));
        store.complete(correlation_id).expect("completion");
        let completed = store.read(&path).expect("completed context");
        assert_eq!(
            completed.evasion,
            Some(classification()),
            "completion keeps the classification"
        );
        assert!(completed.completed_at_unix.is_some());

        // A context written by 0.8.2 has no `evasion` field at all.
        let legacy_id = "fedcba9876543210fedcba9876543210";
        std::fs::write(
            store.context_path(legacy_id),
            format!(
                r#"{{"correlation_id":"{legacy_id}","project_path":"{}","agent":null,"session_id":null,"registered_at_unix":1,"channel":"hook_cli"}}"#,
                project.display()
            ),
        )
        .expect("legacy context");
        let legacy = store
            .read(&store.context_path(legacy_id))
            .expect("legacy context parses");
        assert_eq!(legacy.evasion, None);
        let plain_id = "00000000000000000000000000000001";
        store
            .register_with_channel(plain_id, &project, None, None, AccountingChannel::Mcp)
            .expect("plain registration");
        let plain = std::fs::read_to_string(store.context_path(plain_id)).expect("plain context");
        assert!(
            !plain.contains("evasion"),
            "an unclassified registration does not serialize the field: {plain}"
        );
    }
}
