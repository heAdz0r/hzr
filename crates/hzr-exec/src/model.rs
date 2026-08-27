use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::ExecError;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanonicalCommand {
    Argv { program: String, args: Vec<String> },
    Shell { shell: String, command: String },
}

impl CanonicalCommand {
    pub fn argv(program: impl Into<String>, args: Vec<String>) -> Result<Self, ExecError> {
        let program = program.into();
        if program.is_empty() {
            return Err(ExecError::EmptyProgram);
        }
        Ok(Self::Argv { program, args })
    }

    pub fn shell(command: impl Into<String>) -> Self {
        Self::Shell {
            shell: default_shell().to_owned(),
            command: command.into(),
        }
    }

    pub fn with_shell(
        shell: impl Into<String>,
        command: impl Into<String>,
    ) -> Result<Self, ExecError> {
        let shell = shell.into();
        if shell.is_empty() {
            return Err(ExecError::EmptyShell);
        }
        Ok(Self::Shell {
            shell,
            command: command.into(),
        })
    }

    #[must_use]
    pub fn program(&self) -> &str {
        match self {
            Self::Argv { program, .. } => program,
            Self::Shell { shell, .. } => shell,
        }
    }
}

#[cfg(unix)]
const fn default_shell() -> &'static str {
    "/bin/sh"
}

#[cfg(windows)]
const fn default_shell() -> &'static str {
    "cmd.exe"
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RtkRewriteRoute {
    #[default]
    Optimized,
    Proxy,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum RewriteSource {
    HzrPolicy,
    Rtk {
        version: String,
        #[serde(default)]
        route: RtkRewriteRoute,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum RewriteDecision {
    AllowRaw {
        reason: String,
    },
    AllowRewrite {
        command: CanonicalCommand,
        source: RewriteSource,
        reason: String,
    },
    Ask {
        proposed: Option<CanonicalCommand>,
        reason: String,
    },
    Deny {
        reason: String,
    },
}

impl RewriteDecision {
    #[must_use]
    pub fn allow_raw(reason: impl Into<String>) -> Self {
        Self::AllowRaw {
            reason: reason.into(),
        }
    }
}

/// Reconcile a policy verdict with a host that has already granted execution.
///
/// This is the single authority for that reconciliation, and it lives beside `RewriteDecision`
/// rather than inside one caller on purpose. It used to exist only in the `PreToolUse` hook, so
/// HZR answered the same question twice with two different amounts of information: the hook saw
/// the host's permission mode and allowed, then the `hzr exec run` that the approval had just
/// launched re-derived the verdict without it and refused. One intent, two verdicts, and an
/// operator watching an approved command fail.
///
/// Three properties hold on every surface:
///
/// * **Routing survives.** An `Ask` carrying an executable proposal becomes that managed command,
///   not raw execution — a grant removes the prompt, never the route.
/// * **Deny survives.** An explicit deny is a rule, not an absent one.
/// * **Evidence survives.** The reason records that a grant was applied, so an auto-approved
///   bypass is still visible as a bypass instead of quietly becoming an ordinary allow.
#[must_use]
pub fn reconcile_host_grant(decision: RewriteDecision, granted: bool) -> RewriteDecision {
    if !granted {
        return decision;
    }
    let RewriteDecision::Ask { proposed, reason } = decision else {
        return decision;
    };
    match proposed {
        Some(command) => RewriteDecision::AllowRewrite {
            command,
            source: RewriteSource::HzrPolicy,
            reason: format!("{reason}; {GRANT_APPLIED_REASON}"),
        },
        None => RewriteDecision::allow_raw(format!("{reason}; {GRANT_APPLIED_REASON}")),
    }
}

#[must_use]
pub fn host_grant_applied(decision: &RewriteDecision) -> bool {
    match decision {
        RewriteDecision::AllowRaw { reason } | RewriteDecision::AllowRewrite { reason, .. } => {
            reason.ends_with(GRANT_APPLIED_REASON)
        }
        RewriteDecision::Ask { .. } | RewriteDecision::Deny { .. } => false,
    }
}

/// Stated on every decision a grant converted, so the ledger and the operator see the same cause.
pub const GRANT_APPLIED_REASON: &str =
    "host permission mode grants execution, so HZR recorded it instead of prompting";

/// Private process marker consumed by the ledger writer before it can reach child processes.
pub use hzr_engine_contract::HOST_GRANT_APPLIED_ENV;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    pub inherit: bool,
    pub set: BTreeMap<String, String>,
    pub remove: Vec<String>,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            inherit: true,
            set: BTreeMap::new(),
            remove: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StdinSpec {
    #[default]
    Null,
    Bytes {
        data: Vec<u8>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum CaptureOverflow {
    Spill { directory: PathBuf },
    Truncate,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CaptureConfig {
    pub memory_limit_bytes: usize,
    pub max_capture_bytes: u64,
    pub overflow: CaptureOverflow,
    pub event_buffer: usize,
}

impl CaptureConfig {
    pub fn validate(&self) -> Result<(), ExecError> {
        if self.memory_limit_bytes == 0 {
            return Err(ExecError::InvalidMemoryLimit);
        }
        if self.max_capture_bytes == 0 {
            return Err(ExecError::InvalidCaptureLimit);
        }
        if self.memory_limit_bytes as u64 > self.max_capture_bytes {
            return Err(ExecError::MemoryLimitExceedsCaptureLimit);
        }
        if self.event_buffer == 0 {
            return Err(ExecError::InvalidEventBuffer);
        }
        Ok(())
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            memory_limit_bytes: 256 * 1024,
            max_capture_bytes: 64 * 1024 * 1024,
            overflow: CaptureOverflow::Spill {
                directory: std::env::temp_dir().join("hzr-exec"),
            },
            event_buffer: 64,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutionEnvelope {
    pub command: CanonicalCommand,
    pub decision: RewriteDecision,
    pub cwd: Option<PathBuf>,
    pub environment: Environment,
    pub stdin: StdinSpec,
    pub timeout_ms: Option<u64>,
    pub termination_grace_ms: u64,
    pub capture: CaptureConfig,
}

impl ExecutionEnvelope {
    #[must_use]
    pub fn allow_raw(command: CanonicalCommand) -> Self {
        Self {
            command,
            decision: RewriteDecision::allow_raw("no rewrite requested"),
            cwd: None,
            environment: Environment::default(),
            stdin: StdinSpec::default(),
            timeout_ms: None,
            termination_grace_ms: 250,
            capture: CaptureConfig::default(),
        }
    }

    #[must_use]
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout_ms.map(Duration::from_millis)
    }

    #[must_use]
    pub fn termination_grace(&self) -> Duration {
        Duration::from_millis(self.termination_grace_ms)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "storage", rename_all = "snake_case")]
pub enum CapturedContent {
    Inline { bytes: Vec<u8> },
    Spilled { path: PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapturedStream {
    pub content: CapturedContent,
    pub total_bytes: u64,
    pub stored_bytes: u64,
    pub sha256: String,
    pub truncated: bool,
    pub dropped_event_bytes: u64,
}

impl CapturedStream {
    #[must_use]
    pub fn is_exact(&self) -> bool {
        !self.truncated && self.total_bytes == self.stored_bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationCause {
    Exited,
    Signaled,
    TimedOut,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Termination {
    pub cause: TerminationCause,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub requested: CanonicalCommand,
    pub executed: CanonicalCommand,
    pub decision: RewriteDecision,
    pub raw_fallback: bool,
    pub stdout: CapturedStream,
    pub stderr: CapturedStream,
    pub termination: Termination,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NotStarted {
    ApprovalRequired {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decision_id: Option<String>,
        requested: CanonicalCommand,
        proposed: Option<CanonicalCommand>,
        reason: String,
    },
    Denied {
        requested: CanonicalCommand,
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ExecutionOutcome {
    Completed {
        result: Box<ExecutionResult>,
    },
    ExecutedAccountingIncomplete {
        result: Box<ExecutionResult>,
        accounting: AccountingIncomplete,
    },
    NotStarted {
        disposition: NotStarted,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccountingIncomplete {
    pub code: String,
    pub retryable: bool,
    pub incident_persisted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionEvent {
    Started {
        pid: Option<u32>,
        command: CanonicalCommand,
    },
    Output {
        stream: ExecutionStream,
        bytes: Vec<u8>,
    },
    Finished {
        termination: Termination,
        duration_ms: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NeverWorseChoice {
    Raw,
    Candidate,
}
