use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hzr_core::{
    DetailedOperationAttribution, FidelityAllowance, FidelitySessionUsage, Ledger, LedgerError,
    LedgerRecord, OperationAttribution, OperationChannel, OperationMeasurement, OperationRoute,
    PrivacyPseudonymizer, PrivacySafeFidelityOperation, privacy_identity_hash,
};
use hzr_protocol::{FidelityReconcileReceipt, FidelityUnknownResolution, TraceId};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

const LEDGER_QUEUE_CAPACITY: usize = 256;
const RESERVED_FIDELITY_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Clone)]
pub struct LedgerWriter {
    sender: mpsc::Sender<WriteCommand>,
    privacy: PrivacyPseudonymizer,
    #[cfg(test)]
    inject_fidelity_failure: Arc<AtomicBool>,
}

enum WriteCommand {
    Usage {
        // Box: LedgerRecord с workspace identity раздувает enum — держим варианты компактными.
        record: Box<LedgerRecord>,
        reply: oneshot::Sender<Result<(), LedgerError>>,
    },
    /// An HZR-owned reduction, written to the same table the pinned engine uses so it is
    /// summarized by exactly the same queries.
    Operation {
        record: Box<OperationRecord>,
        reply: oneshot::Sender<Result<(), LedgerWriterError>>,
    },
    PolicyEvent {
        record: Box<PolicyEventRecord>,
        reply: oneshot::Sender<Result<(), LedgerError>>,
    },
    FidelityUsage {
        session_id: String,
        allowance: FidelityAllowance,
        reply: oneshot::Sender<Result<FidelitySessionUsage, LedgerError>>,
    },
    ReserveFidelity {
        reservation_id: String,
        session_id: String,
        session_hash: String,
        allowance: FidelityAllowance,
        output_tokens_upper_bound: u64,
        override_budget: bool,
        reply: oneshot::Sender<Result<Option<FidelityReservation>, LedgerWriterError>>,
    },
    CompleteFidelity {
        reservation: FidelityReservation,
        record: Box<OperationRecord>,
        reply: oneshot::Sender<Result<(), LedgerWriterError>>,
    },
    BeginFidelity {
        reservation: FidelityReservation,
        record: Box<OperationRecord>,
        reply: oneshot::Sender<Result<(), LedgerWriterError>>,
    },
    ReconcileFidelity {
        reservation_id: String,
        resolution: FidelityUnknownResolution,
        reply: oneshot::Sender<Result<FidelityReconcileReceipt, LedgerWriterError>>,
    },
    CancelFidelity {
        reservation: FidelityReservation,
        pre_spawn_proven: bool,
        reply: oneshot::Sender<Result<(), LedgerWriterError>>,
    },
}

#[derive(Clone, Debug)]
pub struct OperationRecord {
    pub original_command: String,
    pub recorded_command: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub execution_ms: u64,
    pub project_path: String,
    pub channel: OperationChannel,
    pub measurement: OperationMeasurement,
    pub route: OperationRoute,
    pub agent: Option<String>,
    pub session_id: Option<String>,
    pub attribution: Option<hzr_protocol::AccountingAttribution>,
    pub evasion: Option<hzr_protocol::EvasionAttribution>,
}

#[derive(Clone, Debug)]
pub struct FidelityReservation {
    id: String,
    session_hash: String,
}

#[derive(Clone, Debug)]
pub struct PolicyEventRecord {
    pub project_path: String,
    pub agent: Option<String>,
    pub session_id: Option<String>,
    pub evasion: hzr_protocol::EvasionAttribution,
    pub decision: hzr_protocol::PolicyDecision,
    pub replacement_family: Option<String>,
}

#[derive(Debug, Error)]
pub enum LedgerWriterError {
    #[error("failed to initialize usage ledger: {0}")]
    Ledger(#[from] LedgerError),
    #[error("failed to start usage ledger writer: {0}")]
    Thread(std::io::Error),
    #[error("usage ledger writer is unavailable")]
    Unavailable,
    #[error("durable fidelity reservation failed: {0}")]
    Durability(String),
    #[error(
        "executed fidelity operation was not written to the ledger: {detail}; durable_incident={incident_persisted}"
    )]
    AccountingIncomplete {
        detail: String,
        incident_persisted: bool,
    },
}

impl LedgerWriterError {
    pub const fn incident_persisted(&self) -> bool {
        matches!(
            self,
            Self::AccountingIncomplete {
                incident_persisted: true,
                ..
            }
        )
    }

    pub fn execution_unknown(detail: impl Into<String>) -> Self {
        Self::AccountingIncomplete {
            detail: detail.into(),
            incident_persisted: true,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum DurableFidelityRecord {
    Reserved {
        schema_version: u32,
        reservation_id: String,
        session_hash: String,
        output_tokens_upper_bound: u64,
        created_at_unix_ms: u64,
    },
    Executing {
        schema_version: u32,
        reservation_id: String,
        session_hash: String,
        output_tokens_upper_bound: u64,
        created_at_unix_ms: u64,
        execution_started_at_unix_ms: u64,
        operation: PrivacySafeFidelityOperation,
    },
    Executed {
        schema_version: u32,
        output_tokens_upper_bound: u64,
        operation: PrivacySafeFidelityOperation,
    },
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PendingFidelityState {
    Reserved,
    Executing,
}

#[derive(Clone)]
struct PendingFidelity {
    output_tokens_upper_bound: u64,
    created_at_unix_ms: u64,
    state: PendingFidelityState,
    operation: Option<PrivacySafeFidelityOperation>,
}

struct DurableLoad {
    pending: HashMap<String, HashMap<String, PendingFidelity>>,
    integrity_issues: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FidelityDurabilityStatus {
    pub reserved: usize,
    pub executing_unknown: usize,
    pub executed_pending_replay: usize,
    pub corrupt: usize,
    pub unknown_reservation_ids: Vec<String>,
}

impl FidelityDurabilityStatus {
    pub fn healthy(&self) -> bool {
        self.executing_unknown == 0 && self.executed_pending_replay == 0 && self.corrupt == 0
    }
}

pub fn inspect_fidelity_pending(directory: &Path) -> std::io::Result<FidelityDurabilityStatus> {
    if !directory.is_dir() {
        return Ok(FidelityDurabilityStatus::default());
    }
    let mut status = FidelityDurabilityStatus::default();
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            if path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.ends_with(".json.corrupt"))
            {
                status.corrupt += 1;
            }
            continue;
        }
        let record = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<DurableFidelityRecord>(&bytes).ok());
        match record {
            Some(DurableFidelityRecord::Reserved {
                schema_version: 1, ..
            }) => status.reserved += 1,
            Some(DurableFidelityRecord::Executing {
                schema_version: 1,
                reservation_id,
                ..
            }) => {
                status.executing_unknown += 1;
                status.unknown_reservation_ids.push(reservation_id);
            }
            Some(DurableFidelityRecord::Executed {
                schema_version: 1, ..
            }) => status.executed_pending_replay += 1,
            _ => status.corrupt += 1,
        }
    }
    Ok(status)
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn durable_record_path(directory: &Path, reservation_id: &str) -> PathBuf {
    directory.join(format!("{reservation_id}.json"))
}

fn resolution_receipt_path(directory: &Path, reservation_id: &str) -> PathBuf {
    directory.join(format!("{reservation_id}.receipt"))
}

fn remove_durable_record(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(path)?;
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_data()?;
    }
    Ok(())
}

fn persist_json(path: &Path, record: &impl serde::Serialize) -> bool {
    let Ok(bytes) = serde_json::to_vec(record) else {
        return false;
    };
    let temporary = path.with_extension("json.tmp");
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(&bytes)?;
        file.sync_data()?;
        std::fs::rename(&temporary, path)?;
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)?.sync_data()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result.is_ok()
}

fn persist_durable_record(path: &Path, record: &DurableFidelityRecord) -> bool {
    persist_json(path, record)
}

fn privacy_safe_fidelity_operation(
    privacy: &PrivacyPseudonymizer,
    reservation_id: String,
    record: &OperationRecord,
) -> Option<PrivacySafeFidelityOperation> {
    let evasion = record.evasion.or_else(|| {
        record
            .attribution
            .as_ref()
            .and_then(|detail| detail.evasion)
    })?;
    let project_hash = privacy_identity_hash("project", &record.project_path);
    let project_scope_hashes = Path::new(&record.project_path)
        .ancestors()
        .filter_map(Path::to_str)
        .filter(|value| !value.is_empty())
        .map(|value| privacy_identity_hash("project", value))
        .collect::<Vec<_>>()
        .join("|");
    let agent = record.agent.as_deref().map(|value| match value {
        "codex" | "claude-code" | "cursor" | "mcp" | "cli" | "hook" | "test" => value.to_owned(),
        _ => "other".into(),
    });
    Some(PrivacySafeFidelityOperation {
        reservation_id,
        input_tokens: record.input_tokens,
        output_tokens: record.output_tokens,
        execution_ms: record.execution_ms,
        measurement: record.measurement,
        project_hash,
        project_scope_hashes,
        session_hash: record
            .session_id
            .as_deref()
            .map(|value| privacy.hash("session", value)),
        agent_hash: record
            .agent
            .as_deref()
            .map(|value| privacy.hash("agent", value)),
        agent,
        evasion,
    })
}

fn load_and_reconcile_durable_records(
    directory: &Path,
    ledger: &Ledger,
) -> Result<DurableLoad, LedgerWriterError> {
    std::fs::create_dir_all(directory).map_err(|error| {
        LedgerWriterError::Durability(format!("create {}: {error}", directory.display()))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).map_err(
            |error| {
                LedgerWriterError::Durability(format!("protect {}: {error}", directory.display()))
            },
        )?;
    }
    let mut pending = HashMap::<String, HashMap<String, PendingFidelity>>::new();
    let mut integrity_issues = 0;
    let now = unix_time_ms();
    for entry in std::fs::read_dir(directory).map_err(|error| {
        LedgerWriterError::Durability(format!("read {}: {error}", directory.display()))
    })? {
        let entry = entry.map_err(|error| LedgerWriterError::Durability(error.to_string()))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let record = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<DurableFidelityRecord>(&bytes).ok());
        let Some(record) = record else {
            integrity_issues += 1;
            let _ = std::fs::rename(&path, path.with_extension("json.corrupt"));
            continue;
        };
        match record {
            DurableFidelityRecord::Reserved {
                schema_version: 1,
                reservation_id,
                session_hash,
                output_tokens_upper_bound,
                created_at_unix_ms,
            } => {
                if now.saturating_sub(created_at_unix_ms)
                    >= RESERVED_FIDELITY_TTL.as_millis() as u64
                {
                    if remove_durable_record(&path).is_err() {
                        integrity_issues += 1;
                    }
                    continue;
                }
                pending.entry(session_hash).or_default().insert(
                    reservation_id,
                    PendingFidelity {
                        output_tokens_upper_bound,
                        created_at_unix_ms,
                        state: PendingFidelityState::Reserved,
                        operation: None,
                    },
                );
            }
            DurableFidelityRecord::Executing {
                schema_version: 1,
                reservation_id,
                session_hash,
                output_tokens_upper_bound,
                created_at_unix_ms,
                operation,
                ..
            } => {
                let receipt = std::fs::read(resolution_receipt_path(directory, &reservation_id))
                    .ok()
                    .and_then(|bytes| {
                        serde_json::from_slice::<FidelityReconcileReceipt>(&bytes).ok()
                    });
                if let Some(receipt) = receipt {
                    let recorded = receipt.resolution
                        != FidelityUnknownResolution::AcknowledgeExecuted
                        || ledger
                            .record_privacy_safe_fidelity_operation(&operation)
                            .is_ok();
                    if !recorded || remove_durable_record(&path).is_err() {
                        integrity_issues += 1;
                    }
                    continue;
                }
                pending.entry(session_hash).or_default().insert(
                    reservation_id,
                    PendingFidelity {
                        output_tokens_upper_bound,
                        created_at_unix_ms,
                        state: PendingFidelityState::Executing,
                        operation: Some(operation),
                    },
                );
            }
            DurableFidelityRecord::Executed {
                schema_version: 1,
                operation,
                ..
            } => {
                if ledger
                    .record_privacy_safe_fidelity_operation(&operation)
                    .is_err()
                    || remove_durable_record(&path).is_err()
                {
                    integrity_issues += 1;
                }
            }
            _ => {
                integrity_issues += 1;
                let _ = std::fs::rename(&path, path.with_extension("json.corrupt"));
            }
        }
    }
    Ok(DurableLoad {
        pending,
        integrity_issues,
    })
}

fn write_operation(ledger: &Ledger, record: &OperationRecord) -> Result<(), LedgerError> {
    ledger.record_operation_attributed_with_detail(
        &record.original_command,
        &record.recorded_command,
        record.input_tokens,
        record.output_tokens,
        record.execution_ms,
        DetailedOperationAttribution {
            attribution: OperationAttribution {
                project_path: &record.project_path,
                agent: record.agent.as_deref(),
                session_id: record.session_id.as_deref(),
                channel: record.channel,
                measurement: record.measurement,
                route: record.route,
            },
            detail: record.attribution.as_ref(),
            evasion: record.evasion.as_ref().or_else(|| {
                record
                    .attribution
                    .as_ref()
                    .and_then(|detail| detail.evasion.as_ref())
            }),
        },
    )
}

impl LedgerWriter {
    pub fn open(path: &Path) -> Result<Self, LedgerWriterError> {
        let ledger = Ledger::open(path)?;
        let privacy = ledger.privacy_pseudonymizer()?;
        let pending_directory = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("fidelity-pending");
        let initial_pending = load_and_reconcile_durable_records(&pending_directory, &ledger)?;
        #[cfg(test)]
        let inject_fidelity_failure = Arc::new(AtomicBool::new(false));
        #[cfg(test)]
        let actor_failure = Arc::clone(&inject_fidelity_failure);
        let actor_privacy = privacy.clone();
        let actor_pending_directory = pending_directory.clone();
        let (sender, mut receiver) = mpsc::channel::<WriteCommand>(LEDGER_QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("hzr-ledger-writer".into())
            .spawn(move || {
                let mut pending_fidelity = initial_pending.pending;
                let fidelity_blocked = initial_pending.integrity_issues > 0;
                while let Some(command) = receiver.blocking_recv() {
                    match command {
                        WriteCommand::Usage { record, reply } => {
                            let _ = reply.send(ledger.record(&record));
                        }
                        WriteCommand::Operation {
                            record,
                            reply,
                        } => {
                            let result =
                                write_operation(&ledger, &record).map_err(LedgerWriterError::Ledger);
                            let _ = reply.send(result);
                        }
                        WriteCommand::PolicyEvent { record, reply } => {
                            let _ = reply.send(ledger.record_policy_event(hzr_core::PolicyEvent {
                                project_path: &record.project_path,
                                agent: record.agent.as_deref(),
                                session_id: record.session_id.as_deref(),
                                evasion: record.evasion,
                                decision: record.decision,
                                replacement_family: record.replacement_family.as_deref(),
                            }));
                        }
                        WriteCommand::FidelityUsage {
                            session_id,
                            allowance,
                            reply,
                        } => {
                            let _ =
                                reply.send(ledger.fidelity_session_usage(&session_id, allowance));
                        }
                        WriteCommand::ReserveFidelity {
                            reservation_id,
                            session_id,
                            session_hash,
                            allowance,
                            output_tokens_upper_bound,
                            override_budget,
                            reply,
                        } => {
                            let result = if fidelity_blocked {
                                Err(LedgerWriterError::Durability(
                                    "fidelity pending records require repair; run `hzr doctor`"
                                        .into(),
                                ))
                            } else {
                                ledger
                                .fidelity_session_usage(&session_id, allowance)
                                .map_err(LedgerWriterError::Ledger)
                                .and_then(|usage| {
                                    let session = pending_fidelity
                                        .entry(session_hash.clone())
                                        .or_default();
                                    let reserved_operations = session.len() as u64;
                                    let reserved_tokens = session
                                        .values()
                                        .map(|pending| pending.output_tokens_upper_bound)
                                        .sum::<u64>();
                                    if !override_budget
                                        && (usage.remaining_operations <= reserved_operations
                                            || output_tokens_upper_bound
                                                > usage
                                                    .remaining_tokens
                                                    .saturating_sub(reserved_tokens))
                                    {
                                        return Ok(None);
                                    }
                                    let reservation = FidelityReservation {
                                        id: reservation_id,
                                        session_hash,
                                    };
                                    let created_at_unix_ms = unix_time_ms();
                                    let durable = DurableFidelityRecord::Reserved {
                                        schema_version: 1,
                                        reservation_id: reservation.id.clone(),
                                        session_hash: reservation.session_hash.clone(),
                                        output_tokens_upper_bound,
                                        created_at_unix_ms,
                                    };
                                    if !persist_durable_record(
                                        &durable_record_path(
                                            &actor_pending_directory,
                                            &reservation.id,
                                        ),
                                        &durable,
                                    ) {
                                        return Err(LedgerWriterError::Durability(
                                            "persist pre-execution reservation".into(),
                                        ));
                                    }
                                    session.insert(
                                        reservation.id.clone(),
                                        PendingFidelity {
                                            output_tokens_upper_bound,
                                            created_at_unix_ms,
                                            state: PendingFidelityState::Reserved,
                                            operation: None,
                                        },
                                    );
                                    Ok(Some(reservation))
                                })
                            };
                            let _ = reply.send(result);
                        }
                        WriteCommand::BeginFidelity {
                            reservation,
                            record,
                            reply,
                        } => {
                            let operation = privacy_safe_fidelity_operation(
                                &actor_privacy,
                                reservation.id.clone(),
                                &record,
                            );
                            let result = pending_fidelity
                                .get_mut(&reservation.session_hash)
                                .and_then(|session| session.get_mut(&reservation.id))
                                .ok_or_else(|| {
                                    LedgerWriterError::Durability(
                                        "fidelity reservation is unavailable".into(),
                                    )
                                })
                                .and_then(|pending| {
                                    let operation = operation.ok_or_else(|| {
                                        LedgerWriterError::Durability(
                                            "execution boundary requires E7 attribution".into(),
                                        )
                                    })?;
                                    let durable = DurableFidelityRecord::Executing {
                                        schema_version: 1,
                                        reservation_id: reservation.id.clone(),
                                        session_hash: reservation.session_hash.clone(),
                                        output_tokens_upper_bound: pending
                                            .output_tokens_upper_bound,
                                        created_at_unix_ms: pending.created_at_unix_ms,
                                        execution_started_at_unix_ms: unix_time_ms(),
                                        operation: operation.clone(),
                                    };
                                    if !persist_durable_record(
                                        &durable_record_path(
                                            &actor_pending_directory,
                                            &reservation.id,
                                        ),
                                        &durable,
                                    ) {
                                        return Err(LedgerWriterError::Durability(
                                            "persist execution boundary".into(),
                                        ));
                                    }
                                    pending.state = PendingFidelityState::Executing;
                                    pending.operation = Some(operation);
                                    Ok(())
                                });
                            let _ = reply.send(result);
                        }
                        WriteCommand::CompleteFidelity {
                            reservation,
                            record,
                            reply,
                        } => {
                            let is_executing = pending_fidelity
                                .get(&reservation.session_hash)
                                .and_then(|session| session.get(&reservation.id))
                                .is_some_and(|pending| {
                                    pending.state == PendingFidelityState::Executing
                                });
                            if !is_executing {
                                let _ = reply.send(Err(LedgerWriterError::Durability(
                                    "fidelity completion requires an executing reservation"
                                        .into(),
                                )));
                                continue;
                            }
                            #[cfg(test)]
                            let inject_failure = actor_failure.load(Ordering::Relaxed);
                            #[cfg(not(test))]
                            let inject_failure = false;
                            let durable_operation = privacy_safe_fidelity_operation(
                                &actor_privacy,
                                reservation.id.clone(),
                                &record,
                            );
                            let durable = durable_operation.as_ref().map(|operation| {
                                DurableFidelityRecord::Executed {
                                    schema_version: 1,
                                    output_tokens_upper_bound: pending_fidelity
                                        .get(&reservation.session_hash)
                                        .and_then(|session| session.get(&reservation.id))
                                        .map(|pending| pending.output_tokens_upper_bound)
                                        .unwrap_or(record.output_tokens),
                                    operation: operation.clone(),
                                }
                            });
                            let durable_path = durable_record_path(
                                &actor_pending_directory,
                                &reservation.id,
                            );
                            let persisted = durable
                                .as_ref()
                                .is_some_and(|record| persist_durable_record(&durable_path, record));
                            let result = if !persisted {
                                Err(LedgerWriterError::AccountingIncomplete {
                                    detail: "persist executed fidelity outbox record".into(),
                                    incident_persisted: false,
                                })
                            } else if inject_failure {
                                Err(LedgerWriterError::AccountingIncomplete {
                                    detail: "injected fidelity accounting failure".into(),
                                    incident_persisted: true,
                                })
                            } else {
                                ledger
                                    .record_privacy_safe_fidelity_operation(
                                        durable_operation.as_ref().expect("persisted operation"),
                                    )
                                    .map_err(|error| LedgerWriterError::AccountingIncomplete {
                                        detail: error.to_string(),
                                        incident_persisted: true,
                                    })
                            };
                            let result = result.and_then(|()| {
                                remove_durable_record(&durable_path).map_err(|error| {
                                    LedgerWriterError::AccountingIncomplete {
                                        detail: format!(
                                            "ledger row committed but durable record cleanup failed: {error}"
                                        ),
                                        incident_persisted: true,
                                    }
                                })
                            });
                            if result.is_ok() {
                                if let Some(session) = pending_fidelity
                                    .get_mut(&reservation.session_hash)
                                {
                                    session.remove(&reservation.id);
                                    if session.is_empty() {
                                        pending_fidelity.remove(&reservation.session_hash);
                                    }
                                }
                            }
                            let _ = reply.send(result);
                        }
                        WriteCommand::ReconcileFidelity {
                            reservation_id,
                            resolution,
                            reply,
                        } => {
                            let receipt_path = resolution_receipt_path(
                                &actor_pending_directory,
                                &reservation_id,
                            );
                            if let Some(mut receipt) = std::fs::read(&receipt_path)
                                .ok()
                                .and_then(|bytes| {
                                    serde_json::from_slice::<FidelityReconcileReceipt>(&bytes).ok()
                                })
                            {
                                receipt.idempotent_replay = true;
                                let _ = reply.send(Ok(receipt));
                                continue;
                            }
                            let pending = pending_fidelity.iter().find_map(|(session_hash, session)| {
                                session
                                    .get(&reservation_id)
                                    .cloned()
                                    .map(|pending| (session_hash.clone(), pending))
                            });
                            let Some((session_hash, pending)) = pending else {
                                let _ = reply.send(Err(LedgerWriterError::Durability(
                                    "unknown fidelity reservation".into(),
                                )));
                                continue;
                            };
                            if pending.state != PendingFidelityState::Executing {
                                let _ = reply.send(Err(LedgerWriterError::Durability(
                                    "only an unknown executing reservation can be reconciled"
                                        .into(),
                                )));
                                continue;
                            }
                            let operation_recorded =
                                resolution == FidelityUnknownResolution::AcknowledgeExecuted;
                            if operation_recorded
                                && pending.operation.as_ref().is_none_or(|operation| {
                                    ledger
                                        .record_privacy_safe_fidelity_operation(operation)
                                        .is_err()
                                })
                            {
                                let _ = reply.send(Err(LedgerWriterError::AccountingIncomplete {
                                    detail: "failed to record acknowledged unknown execution"
                                        .into(),
                                    incident_persisted: true,
                                }));
                                continue;
                            }
                            let mut receipt = FidelityReconcileReceipt {
                                schema_version: 1,
                                reservation_id: reservation_id.clone(),
                                resolution,
                                operation_recorded,
                                allowance_released: true,
                                cleanup_complete: false,
                                idempotent_replay: false,
                            };
                            if !persist_json(&receipt_path, &receipt) {
                                let _ = reply.send(Err(LedgerWriterError::AccountingIncomplete {
                                    detail: "persist fidelity reconciliation receipt".into(),
                                    incident_persisted: true,
                                }));
                                continue;
                            }
                            let durable_path = durable_record_path(
                                &actor_pending_directory,
                                &reservation_id,
                            );
                            receipt.cleanup_complete = remove_durable_record(&durable_path).is_ok();
                            if receipt.cleanup_complete {
                                let _ = persist_json(&receipt_path, &receipt);
                            }
                            if let Some(session) = pending_fidelity.get_mut(&session_hash) {
                                session.remove(&reservation_id);
                                if session.is_empty() {
                                    pending_fidelity.remove(&session_hash);
                                }
                            }
                            let _ = reply.send(Ok(receipt));
                        }
                        WriteCommand::CancelFidelity {
                            reservation,
                            pre_spawn_proven,
                            reply,
                        } => {
                            let state = pending_fidelity
                                .get(&reservation.session_hash)
                                .and_then(|session| session.get(&reservation.id))
                                .map(|pending| pending.state);
                            if state == Some(PendingFidelityState::Executing)
                                && !pre_spawn_proven
                            {
                                let _ = reply.send(Err(LedgerWriterError::Durability(
                                    "executing fidelity reservation is an unknown-execution state and cannot be cancelled"
                                        .into(),
                                )));
                                continue;
                            }
                            let path = durable_record_path(
                                &actor_pending_directory,
                                &reservation.id,
                            );
                            let result = match remove_durable_record(&path) {
                                Ok(()) => Ok(()),
                                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                                Err(error) => Err(LedgerWriterError::Durability(format!(
                                    "remove cancelled reservation {}: {error}",
                                    path.display()
                                ))),
                            };
                            if result.is_ok() {
                                if let Some(session) =
                                    pending_fidelity.get_mut(&reservation.session_hash)
                                {
                                    session.remove(&reservation.id);
                                    if session.is_empty() {
                                        pending_fidelity.remove(&reservation.session_hash);
                                    }
                                }
                            }
                            let _ = reply.send(result);
                        }
                    }
                }
            })
            .map_err(LedgerWriterError::Thread)?;
        Ok(Self {
            sender,
            privacy,
            #[cfg(test)]
            inject_fidelity_failure,
        })
    }

    pub fn privacy_pseudonymizer(&self) -> PrivacyPseudonymizer {
        self.privacy.clone()
    }

    pub async fn record(&self, record: LedgerRecord) -> Result<(), LedgerWriterError> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(WriteCommand::Usage {
                record: Box::new(record),
                reply,
            })
            .await
            .map_err(|_| LedgerWriterError::Unavailable)?;
        result.await.map_err(|_| LedgerWriterError::Unavailable)??;
        Ok(())
    }

    pub async fn record_operation(&self, record: OperationRecord) -> Result<(), LedgerWriterError> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(WriteCommand::Operation {
                record: Box::new(record),
                reply,
            })
            .await
            .map_err(|_| LedgerWriterError::Unavailable)?;
        result.await.map_err(|_| LedgerWriterError::Unavailable)??;
        Ok(())
    }

    #[cfg(test)]
    pub fn inject_fidelity_failure(&self, enabled: bool) {
        self.inject_fidelity_failure
            .store(enabled, Ordering::Relaxed);
    }

    pub async fn record_policy_event(
        &self,
        record: PolicyEventRecord,
    ) -> Result<(), LedgerWriterError> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(WriteCommand::PolicyEvent {
                record: Box::new(record),
                reply,
            })
            .await
            .map_err(|_| LedgerWriterError::Unavailable)?;
        result.await.map_err(|_| LedgerWriterError::Unavailable)??;
        Ok(())
    }

    pub async fn fidelity_session_usage(
        &self,
        session_id: String,
        allowance: FidelityAllowance,
    ) -> Result<FidelitySessionUsage, LedgerWriterError> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(WriteCommand::FidelityUsage {
                session_id,
                allowance,
                reply,
            })
            .await
            .map_err(|_| LedgerWriterError::Unavailable)?;
        Ok(result.await.map_err(|_| LedgerWriterError::Unavailable)??)
    }

    pub async fn reserve_fidelity(
        &self,
        session_id: String,
        allowance: FidelityAllowance,
        output_tokens_upper_bound: u64,
    ) -> Result<Option<FidelityReservation>, LedgerWriterError> {
        self.reserve_fidelity_with_policy(session_id, allowance, output_tokens_upper_bound, false)
            .await
    }

    pub async fn reserve_fidelity_override(
        &self,
        session_id: String,
        allowance: FidelityAllowance,
        output_tokens_upper_bound: u64,
    ) -> Result<FidelityReservation, LedgerWriterError> {
        self.reserve_fidelity_with_policy(session_id, allowance, output_tokens_upper_bound, true)
            .await?
            .ok_or_else(|| {
                LedgerWriterError::Durability("approved reservation was rejected".into())
            })
    }

    async fn reserve_fidelity_with_policy(
        &self,
        session_id: String,
        allowance: FidelityAllowance,
        output_tokens_upper_bound: u64,
        override_budget: bool,
    ) -> Result<Option<FidelityReservation>, LedgerWriterError> {
        let reservation_id = TraceId::new().to_string();
        let session_hash = self.privacy.hash("session", &session_id);
        let (reply, result) = oneshot::channel();
        self.sender
            .send(WriteCommand::ReserveFidelity {
                reservation_id,
                session_id,
                session_hash,
                allowance,
                output_tokens_upper_bound,
                override_budget,
                reply,
            })
            .await
            .map_err(|_| LedgerWriterError::Unavailable)?;
        Ok(result.await.map_err(|_| LedgerWriterError::Unavailable)??)
    }

    pub async fn complete_fidelity(
        &self,
        reservation: FidelityReservation,
        record: OperationRecord,
    ) -> Result<(), LedgerWriterError> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(WriteCommand::CompleteFidelity {
                reservation,
                record: Box::new(record),
                reply,
            })
            .await
            .map_err(|_| LedgerWriterError::Unavailable)?;
        result.await.map_err(|_| LedgerWriterError::Unavailable)??;
        Ok(())
    }

    pub async fn begin_fidelity(
        &self,
        reservation: &FidelityReservation,
        record: OperationRecord,
    ) -> Result<(), LedgerWriterError> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(WriteCommand::BeginFidelity {
                reservation: reservation.clone(),
                record: Box::new(record),
                reply,
            })
            .await
            .map_err(|_| LedgerWriterError::Unavailable)?;
        result.await.map_err(|_| LedgerWriterError::Unavailable)??;
        Ok(())
    }

    pub async fn reconcile_fidelity(
        &self,
        reservation_id: String,
        resolution: FidelityUnknownResolution,
    ) -> Result<FidelityReconcileReceipt, LedgerWriterError> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(WriteCommand::ReconcileFidelity {
                reservation_id,
                resolution,
                reply,
            })
            .await
            .map_err(|_| LedgerWriterError::Unavailable)?;
        result.await.map_err(|_| LedgerWriterError::Unavailable)?
    }

    pub async fn cancel_fidelity(
        &self,
        reservation: FidelityReservation,
    ) -> Result<(), LedgerWriterError> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(WriteCommand::CancelFidelity {
                reservation,
                pre_spawn_proven: false,
                reply,
            })
            .await
            .map_err(|_| LedgerWriterError::Unavailable)?;
        result.await.map_err(|_| LedgerWriterError::Unavailable)??;
        Ok(())
    }

    pub async fn recover_fidelity_pre_spawn(
        &self,
        reservation: FidelityReservation,
    ) -> Result<(), LedgerWriterError> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(WriteCommand::CancelFidelity {
                reservation,
                pre_spawn_proven: true,
                reply,
            })
            .await
            .map_err(|_| LedgerWriterError::Unavailable)?;
        result.await.map_err(|_| LedgerWriterError::Unavailable)??;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use hzr_core::{
        FidelityAllowance, Ledger, LedgerRecord, OperationChannel, OperationMeasurement,
        OperationRoute,
    };
    use hzr_exec::{
        CanonicalCommand, CaptureConfig, CaptureOverflow, ExecutionEnvelope, ExecutionPipeline,
    };
    use hzr_protocol::{
        EnforcementTier, EvasionAttribution, EvasionClass, EvasionPathForm, FidelityReason,
        FidelityValidation, PolicyDecision, TraceId, Usage,
    };

    use super::{
        DurableFidelityRecord, LedgerWriter, LedgerWriterError, OperationRecord, PolicyEventRecord,
        durable_record_path, inspect_fidelity_pending, persist_durable_record,
    };

    fn pending_record(session_id: &str) -> OperationRecord {
        OperationRecord {
            original_command: "hzr exec fidelity".into(),
            recorded_command: "hzr exec fidelity".into(),
            input_tokens: 0,
            output_tokens: 0,
            execution_ms: 0,
            project_path: "/work".into(),
            channel: OperationChannel::HookCli,
            measurement: OperationMeasurement::Unmeasured,
            route: OperationRoute::Bypassed,
            agent: Some("test".into()),
            session_id: Some(session_id.into()),
            attribution: None,
            evasion: Some(EvasionAttribution {
                class: EvasionClass::E7FidelityHatch,
                wrapper_depth: 1,
                interpreter: None,
                path_form: EvasionPathForm::Bare,
                stage_count: 1,
                hatch_marker: true,
                avoidable: false,
                tier: EnforcementTier::T4HatchQuarantine,
                fidelity_reason: Some(FidelityReason::Checksum),
                fidelity_validation: FidelityValidation::Valid,
            }),
        }
    }

    #[tokio::test]
    async fn concurrent_writes_share_one_initialized_connection() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("ledger.sqlite");
        let writer = LedgerWriter::open(&path).expect("ledger writer");
        assert!(
            writer
                .reserve_fidelity(
                    "oversized-session".into(),
                    FidelityAllowance::default(),
                    100_001,
                )
                .await
                .expect("oversized reservation")
                .is_none()
        );
        let mut tasks = Vec::new();
        for _ in 0..100 {
            let writer = writer.clone();
            tasks.push(tokio::spawn(async move {
                writer
                    .record(LedgerRecord {
                        trace_id: TraceId::new(),
                        provider: None,
                        model: None,
                        usage: Usage::default(),
                        turns: 1,
                        retries: 0,
                        latency_ms: 1,
                        outcome: "accepted".into(),
                        policy_version: env!("CARGO_PKG_VERSION").into(),
                        cost_microusd: None,
                        project_path: String::new(),
                    })
                    .await
            }));
        }
        for task in tasks {
            task.await.expect("writer task").expect("ledger write");
        }

        let summary = Ledger::open(&path)
            .expect("reader")
            .summary()
            .expect("summary");
        assert_eq!(summary.tasks, 100);
    }

    #[tokio::test]
    async fn fidelity_reservations_are_atomic_and_complete_exactly_once() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("ledger.sqlite");
        let writer = LedgerWriter::open(&path).expect("ledger writer");
        let mut tasks = Vec::new();
        for _ in 0..32 {
            let writer = writer.clone();
            tasks.push(tokio::spawn(async move {
                writer
                    .reserve_fidelity("shared-session".into(), FidelityAllowance::default(), 10)
                    .await
                    .expect("reservation")
            }));
        }
        let mut accepted = Vec::new();
        for task in tasks {
            if let Some(reservation) = task.await.expect("reservation task") {
                accepted.push(reservation);
            }
        }
        assert_eq!(accepted.len(), 5);

        let evasion = EvasionAttribution {
            class: EvasionClass::E7FidelityHatch,
            wrapper_depth: 1,
            interpreter: None,
            path_form: EvasionPathForm::Bare,
            stage_count: 1,
            hatch_marker: true,
            avoidable: false,
            tier: EnforcementTier::T4HatchQuarantine,
            fidelity_reason: Some(FidelityReason::Checksum),
            fidelity_validation: FidelityValidation::Valid,
        };
        for reservation in accepted {
            writer
                .begin_fidelity(&reservation, pending_record("shared-session"))
                .await
                .expect("execution boundary");
            writer
                .complete_fidelity(
                    reservation,
                    OperationRecord {
                        original_command: "fidelity".into(),
                        recorded_command: "fidelity".into(),
                        input_tokens: 10,
                        output_tokens: 10,
                        execution_ms: 1,
                        project_path: "/work".into(),
                        channel: OperationChannel::HookCli,
                        measurement: OperationMeasurement::Estimated,
                        route: OperationRoute::Bypassed,
                        agent: Some("cli".into()),
                        session_id: Some("shared-session".into()),
                        attribution: None,
                        evasion: Some(evasion),
                    },
                )
                .await
                .expect("complete fidelity");
        }

        assert!(
            writer
                .reserve_fidelity("shared-session".into(), FidelityAllowance::default(), 1)
                .await
                .expect("sixth reservation")
                .is_none()
        );
        let summary = Ledger::open(&path)
            .expect("reader")
            .evasion_summary(hzr_core::StatsQuery::default())
            .expect("evasion summary");
        assert_eq!(summary.fidelity_operations, 5);
        assert_eq!(summary.fidelity_delivered_tokens, 50);
    }

    #[tokio::test]
    async fn executing_reservation_cannot_be_completed_early_or_cancelled() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("ledger.sqlite");
        let writer = LedgerWriter::open(&path).expect("ledger writer");
        let reservation = writer
            .reserve_fidelity("phase-session".into(), FidelityAllowance::default(), 1)
            .await
            .expect("reservation")
            .expect("allowed reservation");
        let record = OperationRecord {
            original_command: "fidelity".into(),
            recorded_command: "fidelity".into(),
            input_tokens: 1,
            output_tokens: 1,
            execution_ms: 1,
            project_path: "/work".into(),
            channel: OperationChannel::HookCli,
            measurement: OperationMeasurement::Estimated,
            route: OperationRoute::Bypassed,
            agent: Some("cli".into()),
            session_id: Some("phase-session".into()),
            attribution: None,
            evasion: Some(EvasionAttribution {
                class: EvasionClass::E7FidelityHatch,
                wrapper_depth: 1,
                interpreter: None,
                path_form: EvasionPathForm::Bare,
                stage_count: 1,
                hatch_marker: true,
                avoidable: false,
                tier: EnforcementTier::T4HatchQuarantine,
                fidelity_reason: Some(FidelityReason::Checksum),
                fidelity_validation: FidelityValidation::Valid,
            }),
        };
        assert!(matches!(
            writer.complete_fidelity(reservation.clone(), record).await,
            Err(LedgerWriterError::Durability(_))
        ));
        writer
            .begin_fidelity(&reservation, pending_record("phase-session"))
            .await
            .expect("execution boundary");
        assert!(matches!(
            writer.cancel_fidelity(reservation.clone()).await,
            Err(LedgerWriterError::Durability(_))
        ));
        assert!(
            durable_record_path(&directory.path().join("fidelity-pending"), &reservation.id)
                .is_file()
        );
    }

    #[tokio::test]
    async fn fidelity_write_failure_reconciles_exactly_once_after_restart() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("ledger.sqlite");
        let writer = LedgerWriter::open(&path).expect("ledger writer");
        let reservation = writer
            .reserve_fidelity("secret-session".into(), FidelityAllowance::default(), 7)
            .await
            .expect("reservation")
            .expect("allowed reservation");
        writer
            .begin_fidelity(&reservation, pending_record("secret-session"))
            .await
            .expect("execution boundary");
        writer.inject_fidelity_failure(true);
        let error = writer
            .complete_fidelity(
                reservation,
                OperationRecord {
                    original_command: "secret-command --private".into(),
                    recorded_command: "rtk raw secret-command".into(),
                    input_tokens: 12,
                    output_tokens: 7,
                    execution_ms: 3,
                    project_path: "/secret/workspace".into(),
                    channel: OperationChannel::HookCli,
                    measurement: OperationMeasurement::Estimated,
                    route: OperationRoute::Bypassed,
                    agent: Some("cli".into()),
                    session_id: Some("secret-session".into()),
                    attribution: None,
                    evasion: Some(EvasionAttribution {
                        class: EvasionClass::E7FidelityHatch,
                        wrapper_depth: 1,
                        interpreter: None,
                        path_form: EvasionPathForm::Bare,
                        stage_count: 1,
                        hatch_marker: true,
                        avoidable: false,
                        tier: EnforcementTier::T4HatchQuarantine,
                        fidelity_reason: Some(FidelityReason::Checksum),
                        fidelity_validation: FidelityValidation::Valid,
                    }),
                },
            )
            .await
            .expect_err("injected write must fail");
        assert!(matches!(
            error,
            LedgerWriterError::AccountingIncomplete {
                incident_persisted: true,
                ..
            }
        ));
        let pending_directory = directory.path().join("fidelity-pending");
        let pending_path = std::fs::read_dir(&pending_directory)
            .expect("pending directory")
            .next()
            .expect("pending record")
            .expect("pending entry")
            .path();
        let incident = std::fs::read_to_string(&pending_path).expect("durable incident");
        assert!(incident.contains("\"state\":\"executed\""));
        assert!(incident.contains("\"output_tokens\":7"));
        for secret in [
            "secret-command",
            "--private",
            "/secret/workspace",
            "secret-session",
        ] {
            assert!(!incident.contains(secret), "incident leaked {secret}");
        }
        assert_eq!(
            Ledger::open(&path)
                .expect("ledger")
                .evasion_summary(hzr_core::StatsQuery::default())
                .expect("stats")
                .fidelity_operations,
            0
        );

        drop(writer);
        let restarted = LedgerWriter::open(&path).expect("restart reconciles durable record");
        let ledger = Ledger::open(&path).expect("reconciled ledger");
        assert_eq!(
            ledger
                .evasion_summary(hzr_core::StatsQuery::default())
                .expect("global stats")
                .fidelity_operations,
            1
        );
        assert_eq!(
            ledger
                .evasion_summary(hzr_core::StatsQuery {
                    project_path: Some("/secret/workspace"),
                    ..hzr_core::StatsQuery::default()
                })
                .expect("project stats")
                .fidelity_operations,
            1
        );
        drop(restarted);
        let _restarted_again = LedgerWriter::open(&path).expect("idempotent second restart");
        assert_eq!(
            Ledger::open(&path)
                .expect("ledger after second restart")
                .evasion_summary(hzr_core::StatsQuery::default())
                .expect("stats after second restart")
                .fidelity_operations,
            1
        );
        assert_eq!(
            std::fs::read_dir(pending_directory)
                .expect("pending directory after replay")
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn reserved_and_executing_restart_states_are_bounded_and_visible() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("ledger.sqlite");
        let writer = LedgerWriter::open(&path).expect("ledger writer");
        let reservation = writer
            .reserve_fidelity("restart-session".into(), FidelityAllowance::default(), 10)
            .await
            .expect("reservation")
            .expect("allowed reservation");
        writer
            .begin_fidelity(&reservation, pending_record("restart-session"))
            .await
            .expect("execution boundary");
        drop(writer);

        let restarted = LedgerWriter::open(&path).expect("restart with unknown execution");
        let status = inspect_fidelity_pending(&directory.path().join("fidelity-pending"))
            .expect("durability status");
        assert_eq!(status.executing_unknown, 1);
        let mut accepted = 0;
        for _ in 0..5 {
            if restarted
                .reserve_fidelity("restart-session".into(), FidelityAllowance::default(), 1)
                .await
                .expect("post-restart reservation")
                .is_some()
            {
                accepted += 1;
            }
        }
        assert_eq!(accepted, 4);

        drop(restarted);
        let ledger = Ledger::open(&path).expect("ledger");
        let privacy = ledger.privacy_pseudonymizer().expect("privacy mapper");
        let pending_directory = directory.path().join("fidelity-pending");
        let expired_id = "expired-pre-execution";
        assert!(persist_durable_record(
            &durable_record_path(&pending_directory, expired_id),
            &DurableFidelityRecord::Reserved {
                schema_version: 1,
                reservation_id: expired_id.into(),
                session_hash: privacy.hash("session", "expired-session"),
                output_tokens_upper_bound: 10,
                created_at_unix_ms: 0,
            },
        ));
        drop(ledger);
        let _restarted = LedgerWriter::open(&path).expect("expired reservation is removable");
        assert!(!durable_record_path(&pending_directory, expired_id).exists());
    }

    #[tokio::test]
    async fn post_spawn_executor_failure_preserves_unknown_execution_across_restart() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("ledger.sqlite");
        let writer = LedgerWriter::open(&path).expect("ledger writer");
        let reservation = writer
            .reserve_fidelity(
                "post-spawn-session".into(),
                FidelityAllowance::default(),
                10,
            )
            .await
            .expect("reservation")
            .expect("allowed reservation");
        writer
            .begin_fidelity(&reservation, pending_record("post-spawn-session"))
            .await
            .expect("execution boundary");

        let invalid_spill_directory = directory.path().join("spill-is-a-file");
        std::fs::write(&invalid_spill_directory, b"not a directory")
            .expect("invalid spill fixture");
        let marker = directory.path().join("process-started");
        let mut envelope = ExecutionEnvelope::allow_raw(CanonicalCommand::shell(format!(
            "printf started > '{}'; printf 0123456789",
            marker.display()
        )));
        envelope.capture = CaptureConfig {
            memory_limit_bytes: 1,
            max_capture_bytes: 1024,
            overflow: CaptureOverflow::Spill {
                directory: invalid_spill_directory,
            },
            event_buffer: 8,
        };
        let handle = ExecutionPipeline.start(envelope).expect("process spawned");
        handle
            .wait()
            .await
            .expect_err("capture fails after process spawn");
        assert_eq!(
            std::fs::read_to_string(marker).expect("side effect marker"),
            "started"
        );

        assert!(matches!(
            writer.cancel_fidelity(reservation).await,
            Err(LedgerWriterError::Durability(_))
        ));
        drop(writer);
        let _restarted = LedgerWriter::open(&path).expect("restart with unknown execution");
        assert_eq!(
            inspect_fidelity_pending(&directory.path().join("fidelity-pending"))
                .expect("durability status")
                .executing_unknown,
            1
        );
    }

    #[tokio::test]
    async fn corrupt_pending_record_is_quarantined_without_killing_daemon() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("ledger.sqlite");
        drop(Ledger::open(&path).expect("initialize ledger"));
        let pending_directory = directory.path().join("fidelity-pending");
        std::fs::create_dir_all(&pending_directory).expect("pending directory");
        std::fs::write(pending_directory.join("broken.json"), b"{truncated")
            .expect("corrupt fixture");

        let writer = LedgerWriter::open(&path).expect("control plane remains available");
        let status = inspect_fidelity_pending(&pending_directory).expect("durability status");
        assert_eq!(status.corrupt, 1);
        assert!(
            writer
                .reserve_fidelity("blocked-session".into(), FidelityAllowance::default(), 1)
                .await
                .expect_err("new fidelity is blocked")
                .to_string()
                .contains("require repair")
        );
    }

    #[tokio::test]
    async fn cancellation_failure_is_typed_and_keeps_durable_state() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("ledger.sqlite");
        let writer = LedgerWriter::open(&path).expect("ledger writer");
        let reservation = writer
            .reserve_fidelity("cancel-session".into(), FidelityAllowance::default(), 1)
            .await
            .expect("reservation")
            .expect("allowed reservation");
        let pending_path =
            durable_record_path(&directory.path().join("fidelity-pending"), &reservation.id);
        std::fs::remove_file(&pending_path).expect("remove fixture file");
        std::fs::create_dir(&pending_path).expect("unremovable fixture path");
        let error = writer
            .cancel_fidelity(reservation)
            .await
            .expect_err("cancellation must surface removal failure");
        assert!(matches!(error, LedgerWriterError::Durability(_)));
        assert!(pending_path.is_dir());
    }

    #[tokio::test]
    async fn denied_policy_event_is_audited_without_an_execution_row() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("ledger.sqlite");
        let writer = LedgerWriter::open(&path).expect("ledger writer");
        writer
            .record_policy_event(PolicyEventRecord {
                project_path: "/private/work".into(),
                agent: Some("claude-code:private-agent".into()),
                session_id: Some("private-session".into()),
                evasion: EvasionAttribution {
                    class: EvasionClass::E7FidelityHatch,
                    wrapper_depth: 1,
                    interpreter: None,
                    path_form: EvasionPathForm::Bare,
                    stage_count: 1,
                    hatch_marker: true,
                    avoidable: true,
                    tier: EnforcementTier::T4HatchQuarantine,
                    fidelity_reason: None,
                    fidelity_validation: FidelityValidation::MissingReason,
                },
                decision: PolicyDecision::Deny,
                replacement_family: Some("read".into()),
            })
            .await
            .expect("policy event write");
        let ledger = Ledger::open(&path).expect("reader");
        assert_eq!(
            ledger.efficiency_summary().expect("efficiency").operations,
            0
        );
        let summary = ledger
            .evasion_summary(hzr_core::StatsQuery::default())
            .expect("evasion summary");
        assert_eq!(summary.policy_attempts, 1);
        let encoded = serde_json::to_string(&summary).expect("JSON");
        assert!(!encoded.contains("private"));
    }
}
