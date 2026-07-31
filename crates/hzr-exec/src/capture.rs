use std::path::PathBuf;

use sha2::{Digest, Sha256};
use tokio::fs::{File, OpenOptions};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::{
    CaptureConfig, CaptureOverflow, CapturedContent, CapturedStream, ExecError, ExecutionStream,
};

pub(crate) struct CaptureWriter {
    config: CaptureConfig,
    stream: ExecutionStream,
    inline: Vec<u8>,
    spill: Option<Spill>,
    hasher: Sha256,
    total_bytes: u64,
    stored_bytes: u64,
    dropped_event_bytes: u64,
}

struct Spill {
    path: PathBuf,
    file: File,
}

impl CaptureWriter {
    pub(crate) fn new(config: CaptureConfig, stream: ExecutionStream) -> Self {
        Self {
            inline: Vec::with_capacity(config.memory_limit_bytes),
            config,
            stream,
            spill: None,
            hasher: Sha256::new(),
            total_bytes: 0,
            stored_bytes: 0,
            dropped_event_bytes: 0,
        }
    }

    pub(crate) fn record_dropped_event(&mut self, bytes: usize) {
        self.dropped_event_bytes = self.dropped_event_bytes.saturating_add(bytes as u64);
    }

    pub(crate) async fn push(&mut self, bytes: &[u8]) -> Result<(), ExecError> {
        self.hasher.update(bytes);
        self.total_bytes = self.total_bytes.saturating_add(bytes.len() as u64);

        let storage_limit = match self.config.overflow {
            CaptureOverflow::Spill { .. } => self.config.max_capture_bytes,
            CaptureOverflow::Truncate => {
                (self.config.memory_limit_bytes as u64).min(self.config.max_capture_bytes)
            }
        };
        let remaining = storage_limit.saturating_sub(self.stored_bytes) as usize;
        let storable = bytes.len().min(remaining);
        if storable == 0 {
            return Ok(());
        }

        let bytes = &bytes[..storable];
        match &self.config.overflow {
            CaptureOverflow::Spill { directory }
                if self.spill.is_some()
                    || self.inline.len().saturating_add(bytes.len())
                        > self.config.memory_limit_bytes =>
            {
                let spill = self.ensure_spill(directory.clone()).await?;
                spill
                    .file
                    .write_all(bytes)
                    .await
                    .map_err(|source| ExecError::WriteSpill {
                        path: spill.path.clone(),
                        source,
                    })?;
            }
            CaptureOverflow::Spill { .. } | CaptureOverflow::Truncate => {
                self.inline.extend_from_slice(bytes);
            }
        }
        self.stored_bytes = self.stored_bytes.saturating_add(storable as u64);
        Ok(())
    }

    pub(crate) async fn finish(self) -> Result<CapturedStream, ExecError> {
        let content = if let Some(mut spill) = self.spill {
            spill
                .file
                .flush()
                .await
                .map_err(|source| ExecError::FlushSpill {
                    path: spill.path.clone(),
                    source,
                })?;
            CapturedContent::Spilled { path: spill.path }
        } else {
            CapturedContent::Inline { bytes: self.inline }
        };

        Ok(CapturedStream {
            content,
            total_bytes: self.total_bytes,
            stored_bytes: self.stored_bytes,
            sha256: format!("{:x}", self.hasher.finalize()),
            truncated: self.total_bytes != self.stored_bytes,
            dropped_event_bytes: self.dropped_event_bytes,
        })
    }

    async fn ensure_spill(&mut self, directory: PathBuf) -> Result<&mut Spill, ExecError> {
        if self.spill.is_none() {
            tokio::fs::create_dir_all(&directory)
                .await
                .map_err(|source| ExecError::CreateSpillDirectory {
                    path: directory.clone(),
                    source,
                })?;
            let name = match self.stream {
                ExecutionStream::Stdout => "stdout",
                ExecutionStream::Stderr => "stderr",
            };
            let path = directory.join(format!("hzr-exec-{}-{name}.bin", Uuid::now_v7()));
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .await
                .map_err(|source| ExecError::OpenSpill {
                    path: path.clone(),
                    source,
                })?;
            file.write_all(&self.inline)
                .await
                .map_err(|source| ExecError::WriteSpill {
                    path: path.clone(),
                    source,
                })?;
            self.inline.clear();
            self.spill = Some(Spill { path, file });
        }
        self.spill.as_mut().ok_or(ExecError::MissingSpillState)
    }
}

#[cfg(test)]
mod tests {
    use anyhow::{Result, anyhow};
    use tempfile::TempDir;

    use super::CaptureWriter;
    use crate::{CaptureConfig, CaptureOverflow, CapturedContent, ExecutionStream};

    #[tokio::test]
    async fn test_capture_writer_spills_without_losing_bytes() -> Result<()> {
        let directory = TempDir::new()?;
        let config = CaptureConfig {
            memory_limit_bytes: 4,
            max_capture_bytes: 32,
            overflow: CaptureOverflow::Spill {
                directory: directory.path().to_owned(),
            },
            event_buffer: 1,
        };
        let mut writer = CaptureWriter::new(config, ExecutionStream::Stdout);
        writer.push(b"hello").await?;
        writer.push(b" world").await?;
        let captured = writer.finish().await?;

        let CapturedContent::Spilled { ref path } = captured.content else {
            return Err(anyhow!("capture should spill"));
        };
        assert_eq!(std::fs::read(path)?, b"hello world");
        assert_eq!(captured.total_bytes, 11);
        assert!(captured.is_exact());
        Ok(())
    }

    #[tokio::test]
    async fn test_capture_writer_truncates_at_safe_memory_cap() -> Result<()> {
        let config = CaptureConfig {
            memory_limit_bytes: 4,
            max_capture_bytes: 32,
            overflow: CaptureOverflow::Truncate,
            event_buffer: 1,
        };
        let mut writer = CaptureWriter::new(config, ExecutionStream::Stderr);
        writer.push(b"abcdefgh").await?;
        let captured = writer.finish().await?;

        assert_eq!(captured.total_bytes, 8);
        assert_eq!(captured.stored_bytes, 4);
        assert!(captured.truncated);
        assert_eq!(
            captured.content,
            CapturedContent::Inline {
                bytes: b"abcd".to_vec()
            }
        );
        Ok(())
    }
}
