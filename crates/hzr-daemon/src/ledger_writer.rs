use std::path::Path;

use hzr_core::{
    DetailedOperationAttribution, FidelityAllowance, FidelitySessionUsage, Ledger, LedgerError,
    LedgerRecord, OperationAttribution, OperationChannel, OperationMeasurement, OperationRoute,
};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

const LEDGER_QUEUE_CAPACITY: usize = 256;

#[derive(Clone)]
pub struct LedgerWriter {
    sender: mpsc::Sender<WriteCommand>,
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
        reply: oneshot::Sender<Result<(), LedgerError>>,
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
}

impl LedgerWriter {
    pub fn open(path: &Path) -> Result<Self, LedgerWriterError> {
        let ledger = Ledger::open(path)?;
        let (sender, mut receiver) = mpsc::channel::<WriteCommand>(LEDGER_QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("hzr-ledger-writer".into())
            .spawn(move || {
                while let Some(command) = receiver.blocking_recv() {
                    match command {
                        WriteCommand::Usage { record, reply } => {
                            let _ = reply.send(ledger.record(&record));
                        }
                        WriteCommand::Operation { record, reply } => {
                            let _ = reply.send(
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
                                        evasion: record
                                            .attribution
                                            .as_ref()
                                            .and_then(|detail| detail.evasion.as_ref()),
                                    },
                                ),
                            );
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
                    }
                }
            })
            .map_err(LedgerWriterError::Thread)?;
        Ok(Self { sender })
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
}

#[cfg(test)]
mod tests {
    use hzr_core::{Ledger, LedgerRecord};
    use hzr_protocol::{
        EnforcementTier, EvasionAttribution, EvasionClass, EvasionPathForm, FidelityValidation,
        PolicyDecision, TraceId, Usage,
    };

    use super::{LedgerWriter, PolicyEventRecord};

    #[tokio::test]
    async fn concurrent_writes_share_one_initialized_connection() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("ledger.sqlite");
        let writer = LedgerWriter::open(&path).expect("ledger writer");
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
