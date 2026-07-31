use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::task::JoinError;
use tokio::time::timeout;
use uuid::Uuid;

use crate::config::{ManagedAgentConfig, ResponseFormat};
use crate::preflight::{PreflightError, PreflightReport, preflight};
use crate::process::{ProcessGroupGuard, configure_process_group};

const PROCESS_TERMINATION_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentEvent {
    pub seq: u64,
    pub request_id: String,
    pub kind: String,
    pub data: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AgentRun {
    pub request_id: String,
    pub text: String,
    pub json: Option<Value>,
    pub events: Vec<AgentEvent>,
}

pub struct ManagedAgent {
    config: ManagedAgentConfig,
}

impl ManagedAgent {
    #[must_use]
    pub fn new(config: ManagedAgentConfig) -> Self {
        Self { config }
    }

    pub async fn preflight(&self) -> Result<PreflightReport, RunError> {
        preflight(&self.config.node, &self.config.integration)
            .await
            .map_err(RunError::Preflight)
    }

    pub async fn run(
        &self,
        prompt: &str,
        response_format: ResponseFormat,
        max_turns: u32,
    ) -> Result<AgentRun, RunError> {
        if prompt.trim().is_empty() {
            return Err(RunError::InvalidRequest("prompt must not be empty".into()));
        }
        if !(1..=100).contains(&max_turns) {
            return Err(RunError::InvalidRequest(
                "max_turns must be between 1 and 100".into(),
            ));
        }
        if self.config.max_capture_bytes == 0 {
            return Err(RunError::InvalidRequest(
                "max_capture_bytes must be positive".into(),
            ));
        }
        if self.config.timeout.is_zero() {
            return Err(RunError::InvalidRequest(
                "managed agent timeout must be positive".into(),
            ));
        }

        let report = self.preflight().await?;
        prepare_agent_data_dir(&self.config.agent_data_dir)?;
        let request_id = Uuid::now_v7().to_string();
        let request = BridgeRequest {
            request_id: &request_id,
            prompt,
            response_format,
            max_turns,
        };
        let payload = serde_json::to_vec(&request).map_err(RunError::Encode)?;
        if payload.len() > 4 * 1024 * 1024 {
            return Err(RunError::InvalidRequest(
                "encoded bridge request exceeds four MiB".into(),
            ));
        }

        let mut command = Command::new(&self.config.node);
        command
            .arg(&report.bridge)
            .current_dir(&self.config.workspace)
            .env("HZR_DAEMON_URL", self.config.hzr_api.endpoint())
            .env("HZR_DAEMON_TOKEN", self.config.hzr_api.token().expose())
            .env("HZR_AGENT_DIR", &self.config.agent_data_dir)
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        configure_process_group(&mut command);
        let mut child = command.spawn().map_err(RunError::Spawn)?;
        let mut process_group = ProcessGroupGuard::new(&child).map_err(RunError::ProcessControl)?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RunError::Protocol("bridge stdout is unavailable".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| RunError::Protocol("bridge stderr is unavailable".into()))?;
        let capture_limit = self.config.max_capture_bytes;
        let mut stdout_task = tokio::spawn(capture(stdout, capture_limit));
        let mut stderr_task = tokio::spawn(capture(stderr, capture_limit));

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| RunError::Protocol("bridge stdin is unavailable".into()))?;
        let completion = timeout(self.config.timeout, async {
            stdin.write_all(&payload).await.map_err(RunError::Io)?;
            stdin.write_all(b"\n").await.map_err(RunError::Io)?;
            stdin.shutdown().await.map_err(RunError::Io)?;
            child.wait().await.map_err(RunError::Io)
        })
        .await;

        let status = match completion {
            Ok(Ok(status)) => {
                if let Err(error) = process_group.finish() {
                    stop_capture_tasks(&mut stdout_task, &mut stderr_task).await;
                    return Err(RunError::ProcessControl(error));
                }
                status
            }
            Ok(Err(error)) => {
                let termination = process_group
                    .terminate(&mut child, PROCESS_TERMINATION_GRACE)
                    .await;
                stop_capture_tasks(&mut stdout_task, &mut stderr_task).await;
                termination.map_err(RunError::ProcessControl)?;
                return Err(error);
            }
            Err(_) => {
                let termination = process_group
                    .terminate(&mut child, PROCESS_TERMINATION_GRACE)
                    .await;
                stop_capture_tasks(&mut stdout_task, &mut stderr_task).await;
                termination.map_err(RunError::ProcessControl)?;
                return Err(RunError::Timeout);
            }
        };
        let drains = timeout(PROCESS_TERMINATION_GRACE, async {
            let stdout_capture = (&mut stdout_task).await.map_err(RunError::Join)??;
            let stderr_capture = (&mut stderr_task).await.map_err(RunError::Join)??;
            Ok::<_, RunError>((stdout_capture, stderr_capture))
        })
        .await;
        let (stdout_capture, stderr_capture) = match drains {
            Ok(result) => result?,
            Err(_) => {
                stop_capture_tasks(&mut stdout_task, &mut stderr_task).await;
                return Err(RunError::CaptureDrainTimeout);
            }
        };
        if stdout_capture.overflow || stderr_capture.overflow {
            return Err(RunError::CaptureLimit(capture_limit));
        }

        let stderr_text = redact_token(
            &String::from_utf8_lossy(&stderr_capture.bytes),
            self.config.hzr_api.token().expose(),
        );
        let stdout_text = std::str::from_utf8(&stdout_capture.bytes)
            .map_err(|error| RunError::Protocol(format!("bridge output is not UTF-8: {error}")))?;
        let redacted_stdout = redact_token(stdout_text, self.config.hzr_api.token().expose());
        let events = parse_events(redacted_stdout.as_bytes(), &request_id)?;
        if !status.success() {
            let diagnostic = if stderr_text.trim().is_empty() {
                events
                    .iter()
                    .rev()
                    .find(|event| event.kind == "error")
                    .and_then(|event| event.data.get("message"))
                    .and_then(Value::as_str)
                    .map_or_else(String::new, |message| {
                        redact_token(message, self.config.hzr_api.token().expose())
                    })
            } else {
                stderr_text
            };
            return Err(RunError::BridgeExit {
                status: status.code(),
                stderr: diagnostic,
            });
        }

        let result = events
            .iter()
            .find(|event| event.kind == "result")
            .ok_or_else(|| RunError::Protocol("bridge emitted no result event".into()))?;
        let text = result
            .data
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| RunError::Protocol("result event has no text field".into()))?
            .to_owned();
        let json = result.data.get("response").cloned();
        if response_format == ResponseFormat::Json && json.is_none() {
            return Err(RunError::Protocol(
                "JSON result event has no parsed response".into(),
            ));
        }

        Ok(AgentRun {
            request_id,
            text,
            json,
            events,
        })
    }
}

#[derive(Serialize)]
struct BridgeRequest<'a> {
    request_id: &'a str,
    prompt: &'a str,
    response_format: ResponseFormat,
    max_turns: u32,
}

struct Capture {
    bytes: Vec<u8>,
    overflow: bool,
}

async fn capture(mut reader: impl AsyncRead + Unpin, limit: usize) -> Result<Capture, RunError> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    let mut chunk = [0_u8; 8 * 1024];
    let mut overflow = false;
    loop {
        let read = reader.read(&mut chunk).await.map_err(RunError::Io)?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        let retained = remaining.min(read);
        bytes.extend_from_slice(&chunk[..retained]);
        overflow |= retained < read;
    }
    Ok(Capture { bytes, overflow })
}

async fn stop_capture_tasks(
    stdout: &mut tokio::task::JoinHandle<Result<Capture, RunError>>,
    stderr: &mut tokio::task::JoinHandle<Result<Capture, RunError>>,
) {
    stdout.abort();
    stderr.abort();
    let _ = stdout.await;
    let _ = stderr.await;
}

fn parse_events(bytes: &[u8], request_id: &str) -> Result<Vec<AgentEvent>, RunError> {
    let rendered = std::str::from_utf8(bytes)
        .map_err(|error| RunError::Protocol(format!("bridge output is not UTF-8: {error}")))?;
    let mut events = Vec::new();
    let mut expected_sequence = 0_u64;
    let mut ready = false;
    let mut terminal = false;

    for line in rendered.lines().filter(|line| !line.trim().is_empty()) {
        let event = serde_json::from_str::<AgentEvent>(line)
            .map_err(|error| RunError::Protocol(format!("invalid bridge JSONL event: {error}")))?;
        if event.request_id != request_id {
            return Err(RunError::Protocol("bridge request id mismatch".into()));
        }
        if event.seq != expected_sequence {
            return Err(RunError::Protocol(
                "bridge event sequence must start at zero and remain contiguous".into(),
            ));
        }
        if terminal {
            return Err(RunError::Protocol(
                "bridge emitted an event after a terminal event".into(),
            ));
        }
        match event.kind.as_str() {
            "ready" if expected_sequence == 0 => ready = true,
            "ready" => {
                return Err(RunError::Protocol(
                    "bridge emitted a duplicate or misplaced ready event".into(),
                ));
            }
            "agent_event" if ready => {}
            "result" if ready => terminal = true,
            "error" => terminal = true,
            "agent_event" | "result" => {
                return Err(RunError::Protocol(
                    "bridge emitted an agent or result event before ready".into(),
                ));
            }
            _ => {
                return Err(RunError::Protocol(format!(
                    "bridge emitted unknown event kind {}",
                    event.kind
                )));
            }
        }
        expected_sequence = expected_sequence
            .checked_add(1)
            .ok_or_else(|| RunError::Protocol("bridge event sequence overflowed".into()))?;
        events.push(event);
    }
    Ok(events)
}

fn prepare_agent_data_dir(path: &Path) -> Result<(), RunError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(RunError::Io)?;
        }
        Err(error) => return Err(RunError::Io(error)),
    }
    let metadata = fs::symlink_metadata(path).map_err(RunError::Io)?;
    if metadata.file_type().is_symlink() {
        return Err(RunError::InvalidAgentDataDirectory {
            path: path.into(),
            reason: "symbolic links are not allowed",
        });
    }
    if !metadata.is_dir() {
        return Err(RunError::InvalidAgentDataDirectory {
            path: path.into(),
            reason: "expected a directory",
        });
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(RunError::Io)?;
    Ok(())
}

fn redact_token(message: &str, token: &str) -> String {
    message.replace(token, "[REDACTED]")
}

#[derive(Debug, Error)]
pub enum RunError {
    #[error(transparent)]
    Preflight(PreflightError),
    #[error("invalid managed agent request: {0}")]
    InvalidRequest(String),
    #[error("failed to encode bridge request: {0}")]
    Encode(serde_json::Error),
    #[error("failed to launch Caveman bridge: {0}")]
    Spawn(std::io::Error),
    #[error("managed agent I/O failed: {0}")]
    Io(std::io::Error),
    #[error("invalid managed agent data directory {path}: {reason}")]
    InvalidAgentDataDirectory { path: PathBuf, reason: &'static str },
    #[error("managed agent process-group control failed: {0}")]
    ProcessControl(std::io::Error),
    #[error("managed agent bridge timed out")]
    Timeout,
    #[error("managed agent output exceeded the {0}-byte capture limit")]
    CaptureLimit(usize),
    #[error("managed agent output pipes remained open after process termination")]
    CaptureDrainTimeout,
    #[error("managed agent bridge exited with status {status:?}: {stderr}")]
    BridgeExit { status: Option<i32>, stderr: String },
    #[error("managed agent protocol error: {0}")]
    Protocol(String),
    #[error("managed agent capture task failed: {0}")]
    Join(JoinError),
}

#[cfg(test)]
mod tests {
    use super::{parse_events, prepare_agent_data_dir, redact_token};

    #[test]
    fn test_parse_events_requires_contiguous_sequence() {
        let events = b"{\"seq\":0,\"request_id\":\"r\",\"kind\":\"ready\",\"data\":{}}\n{\"seq\":2,\"request_id\":\"r\",\"kind\":\"result\",\"data\":{\"text\":\"ok\"}}\n";
        assert!(parse_events(events, "r").is_err());
    }

    #[test]
    fn test_parse_events_rejects_nonzero_initial_sequence() {
        let events = b"{\"seq\":1,\"request_id\":\"r\",\"kind\":\"ready\",\"data\":{}}\n";
        assert!(parse_events(events, "r").is_err());
    }

    #[test]
    fn test_parse_events_rejects_events_after_terminal() {
        let events = b"{\"seq\":0,\"request_id\":\"r\",\"kind\":\"ready\",\"data\":{}}\n{\"seq\":1,\"request_id\":\"r\",\"kind\":\"result\",\"data\":{\"text\":\"ok\"}}\n{\"seq\":2,\"request_id\":\"r\",\"kind\":\"agent_event\",\"data\":{}}\n";
        assert!(parse_events(events, "r").is_err());
    }

    #[test]
    fn test_parse_events_accepts_startup_error_without_ready() {
        let events = b"{\"seq\":0,\"request_id\":\"r\",\"kind\":\"error\",\"data\":{\"message\":\"failed\"}}\n";
        let parsed = parse_events(events, "r").expect("startup error is a valid terminal event");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].kind, "error");
    }

    #[test]
    fn test_redact_token_removes_every_occurrence() {
        assert_eq!(
            redact_token("token secret secret", "secret"),
            "token [REDACTED] [REDACTED]"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_prepare_agent_data_dir_rejects_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary directory");
        let target = directory.path().join("target");
        let link = directory.path().join("agent-data");
        std::fs::create_dir(&target).expect("target directory");
        symlink(target, &link).expect("agent data symlink");

        assert!(prepare_agent_data_dir(&link).is_err());
    }
}
