use std::path::Path;

use hzr_core::{
    Ledger, LedgerError, LedgerRecord, OperationAttribution, OperationChannel,
    OperationMeasurement, OperationRoute,
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
        record: LedgerRecord,
        reply: oneshot::Sender<Result<(), LedgerError>>,
    },
    /// An HZR-owned reduction, written to the same table the pinned engine uses so it is
    /// summarized by exactly the same queries.
    Operation {
        record: OperationRecord,
        reply: oneshot::Sender<Result<(), LedgerError>>,
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
                            let _ = reply.send(ledger.record_operation_attributed(
                                &record.original_command,
                                &record.recorded_command,
                                record.input_tokens,
                                record.output_tokens,
                                record.execution_ms,
                                OperationAttribution {
                                    project_path: &record.project_path,
                                    agent: None,
                                    session_id: None,
                                    channel: record.channel,
                                    measurement: record.measurement,
                                    route: record.route,
                                },
                            ));
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
            .send(WriteCommand::Usage { record, reply })
            .await
            .map_err(|_| LedgerWriterError::Unavailable)?;
        result.await.map_err(|_| LedgerWriterError::Unavailable)??;
        Ok(())
    }

    pub async fn record_operation(&self, record: OperationRecord) -> Result<(), LedgerWriterError> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(WriteCommand::Operation { record, reply })
            .await
            .map_err(|_| LedgerWriterError::Unavailable)?;
        result.await.map_err(|_| LedgerWriterError::Unavailable)??;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use hzr_core::{Ledger, LedgerRecord};
    use hzr_protocol::{TraceId, Usage};

    use super::LedgerWriter;

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
}
