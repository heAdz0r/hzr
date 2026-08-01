use std::path::Path;

use hzr_core::{Ledger, LedgerError, LedgerRecord};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

const LEDGER_QUEUE_CAPACITY: usize = 256;

#[derive(Clone)]
pub struct LedgerWriter {
    sender: mpsc::Sender<WriteCommand>,
}

struct WriteCommand {
    record: LedgerRecord,
    reply: oneshot::Sender<Result<(), LedgerError>>,
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
                    let result = ledger.record(&command.record);
                    let _ = command.reply.send(result);
                }
            })
            .map_err(LedgerWriterError::Thread)?;
        Ok(Self { sender })
    }

    pub async fn record(&self, record: LedgerRecord) -> Result<(), LedgerWriterError> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(WriteCommand { record, reply })
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
