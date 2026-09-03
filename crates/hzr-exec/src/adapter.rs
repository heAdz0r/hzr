use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use hzr_engine_contract::{
    ACCOUNTING_CORRELATION_ENV, ACCOUNTING_FAILURE_JOURNAL_ENV, ACCOUNTING_RECEIPT_JOURNAL_ENV,
    BYTE_FIDELITY_ENV, ENGINE_CONTRACT_VERSION, EngineContractIdentity, EvasionAttribution,
    RewritePlan, RewritePlanDecision, RewritePlanReason,
};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::{CanonicalCommand, ExecError, RewriteDecision, RewriteSource};

pub const PINNED_RTK_VERSION: &str = "0.44.1-fork.1";
pub use hzr_engine_contract::INTERNAL_EVASION_ENV;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForkRuntimePaths {
    pub memory_db: PathBuf,
    pub tee_dir: PathBuf,
    pub audit_dir: PathBuf,
    pub accounting_receipt_journal: PathBuf,
    pub accounting_failure_journal: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForkAccountingHandle {
    pub(crate) correlation_id: String,
    pub(crate) receipt_journal: PathBuf,
    pub(crate) failure_journal: PathBuf,
}

impl ForkAccountingHandle {
    #[must_use]
    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }
}

impl ForkRuntimePaths {
    #[must_use]
    pub fn from_data_root(data_root: &Path) -> Self {
        let root = data_root.join("fork");
        Self {
            memory_db: root.join("mem.db"),
            tee_dir: root.join("tee"),
            audit_dir: root.join("audit"),
            accounting_receipt_journal: root.join("accounting-receipts.jsonl"),
            accounting_failure_journal: root.join("accounting-failures.jsonl"),
        }
    }

    pub fn ensure_layout(&self) -> Result<(), ExecError> {
        let memory_parent = parent_or_error(&self.memory_db)?;
        let receipt_parent = parent_or_error(&self.accounting_receipt_journal)?;
        for directory in [
            memory_parent,
            receipt_parent,
            self.tee_dir.as_path(),
            self.audit_dir.as_path(),
        ] {
            fs::create_dir_all(directory).map_err(|source| ExecError::PrepareForkRuntime {
                path: directory.to_owned(),
                source,
            })?;
            set_private_directory_permissions(directory)?;
        }
        self.validate_shell_paths()
    }

    fn validate_shell_paths(&self) -> Result<(), ExecError> {
        for path in [
            &self.memory_db,
            &self.tee_dir,
            &self.audit_dir,
            &self.accounting_receipt_journal,
            &self.accounting_failure_journal,
        ] {
            if path.to_str().is_none() {
                return Err(ExecError::NonUtf8ForkRuntimePath { path: path.clone() });
            }
        }
        Ok(())
    }

    fn apply_to_command(&self, command: &mut Command, binary: &Path) -> Result<(), ExecError> {
        let accounting = self.new_accounting_context()?;
        let binary_directory = binary
            .parent()
            .ok_or_else(|| ExecError::InvalidForkBinaryPath {
                path: binary.to_owned(),
            })?;
        command
            .env("RTK_MEM_DB_PATH", &self.memory_db)
            .env_remove("RTK_DB_PATH")
            .env("RTK_TEE_DIR", &self.tee_dir)
            .env("RTK_AUDIT_DIR", &self.audit_dir)
            .env(ACCOUNTING_RECEIPT_JOURNAL_ENV, &accounting.receipt_journal)
            .env(ACCOUNTING_FAILURE_JOURNAL_ENV, &accounting.failure_journal)
            .env(ACCOUNTING_CORRELATION_ENV, &accounting.correlation_id)
            .env("RTK_TEE", "0")
            .env("RTK_HISTORY_DAYS", "0")
            .env("RTK_TRACKING_DISABLED", "0")
            .env("RTK_TELEMETRY_DISABLED", "1")
            .env("PATH", prefixed_path(binary_directory)?);
        Ok(())
    }

    fn apply_to_environment(
        &self,
        environment: &mut crate::Environment,
        binary: &Path,
        accounting: &ForkAccountingHandle,
    ) -> Result<(), ExecError> {
        let binary_directory = binary
            .parent()
            .ok_or_else(|| ExecError::InvalidForkBinaryPath {
                path: binary.to_owned(),
            })?;
        for (key, value) in [
            ("RTK_MEM_DB_PATH", path_text(&self.memory_db)?),
            ("RTK_TEE_DIR", path_text(&self.tee_dir)?),
            ("RTK_AUDIT_DIR", path_text(&self.audit_dir)?),
            (
                ACCOUNTING_RECEIPT_JOURNAL_ENV,
                path_text(&accounting.receipt_journal)?,
            ),
            (
                ACCOUNTING_FAILURE_JOURNAL_ENV,
                path_text(&accounting.failure_journal)?,
            ),
            (ACCOUNTING_CORRELATION_ENV, &accounting.correlation_id),
            ("RTK_TEE", "0"),
            ("RTK_HISTORY_DAYS", "0"),
            ("RTK_TRACKING_DISABLED", "0"),
            ("RTK_TELEMETRY_DISABLED", "1"),
        ] {
            environment.set.insert(key.to_owned(), value.to_owned());
        }
        environment.set.remove("RTK_DB_PATH");
        if !environment
            .remove
            .iter()
            .any(|variable| variable == "RTK_DB_PATH")
        {
            environment.remove.push("RTK_DB_PATH".to_owned());
        }
        let path = prefixed_path(binary_directory)?;
        let path = path
            .into_string()
            .map_err(|path| ExecError::NonUtf8ForkRuntimePath {
                path: PathBuf::from(path),
            })?;
        environment.set.insert("PATH".to_owned(), path);
        Ok(())
    }

    fn apply_to_std_command(
        &self,
        command: &mut std::process::Command,
        binary: &Path,
        accounting: &ForkAccountingHandle,
    ) -> Result<(), ExecError> {
        let binary_directory = binary
            .parent()
            .ok_or_else(|| ExecError::InvalidForkBinaryPath {
                path: binary.to_owned(),
            })?;
        command
            .env("RTK_MEM_DB_PATH", &self.memory_db)
            .env_remove("RTK_DB_PATH")
            .env("RTK_TEE_DIR", &self.tee_dir)
            .env("RTK_AUDIT_DIR", &self.audit_dir)
            .env(ACCOUNTING_RECEIPT_JOURNAL_ENV, &accounting.receipt_journal)
            .env(ACCOUNTING_FAILURE_JOURNAL_ENV, &accounting.failure_journal)
            .env(ACCOUNTING_CORRELATION_ENV, &accounting.correlation_id)
            .env("RTK_TEE", "0")
            .env("RTK_HISTORY_DAYS", "0")
            .env("RTK_TRACKING_DISABLED", "0")
            .env("RTK_TELEMETRY_DISABLED", "1")
            .env("PATH", prefixed_path(binary_directory)?);
        Ok(())
    }

    fn shell_exports(
        &self,
        binary_directory: &Path,
        accounting: &ForkAccountingHandle,
    ) -> Result<String, ExecError> {
        self.validate_shell_paths()?;
        let binary_directory =
            binary_directory
                .to_str()
                .ok_or_else(|| ExecError::NonUtf8ForkRuntimePath {
                    path: binary_directory.to_owned(),
                })?;
        Ok(format!(
            "unset RTK_DB_PATH\nRTK_MEM_DB_PATH={}\nRTK_TEE_DIR={}\nRTK_AUDIT_DIR={}\n{ACCOUNTING_RECEIPT_JOURNAL_ENV}={}\n{ACCOUNTING_FAILURE_JOURNAL_ENV}={}\n{ACCOUNTING_CORRELATION_ENV}={}\nRTK_TEE=0\nRTK_HISTORY_DAYS=0\nRTK_TRACKING_DISABLED=0\nRTK_TELEMETRY_DISABLED=1\nPATH={}${{PATH:+\":$PATH\"}}\nexport RTK_MEM_DB_PATH RTK_TEE_DIR RTK_AUDIT_DIR {ACCOUNTING_RECEIPT_JOURNAL_ENV} {ACCOUNTING_FAILURE_JOURNAL_ENV} {ACCOUNTING_CORRELATION_ENV} RTK_TEE RTK_HISTORY_DAYS RTK_TRACKING_DISABLED RTK_TELEMETRY_DISABLED PATH\n",
            shell_quote(path_text(&self.memory_db)?),
            shell_quote(path_text(&self.tee_dir)?),
            shell_quote(path_text(&self.audit_dir)?),
            shell_quote(path_text(&accounting.receipt_journal)?),
            shell_quote(path_text(&accounting.failure_journal)?),
            shell_quote(&accounting.correlation_id),
            shell_quote(binary_directory),
        ))
    }

    fn new_accounting_context(&self) -> Result<ForkAccountingHandle, ExecError> {
        self.accounting_context(uuid::Uuid::new_v4().simple().to_string())
    }

    fn accounting_context(
        &self,
        correlation_id: String,
    ) -> Result<ForkAccountingHandle, ExecError> {
        Ok(ForkAccountingHandle {
            receipt_journal: correlated_journal_path(
                &self.accounting_receipt_journal,
                &correlation_id,
            )?,
            failure_journal: correlated_journal_path(
                &self.accounting_failure_journal,
                &correlation_id,
            )?,
            correlation_id,
        })
    }
}

fn correlated_journal_path(base: &Path, correlation_id: &str) -> Result<PathBuf, ExecError> {
    let parent = parent_or_error(base)?;
    let stem = base.file_stem().and_then(OsStr::to_str).ok_or_else(|| {
        ExecError::NonUtf8ForkRuntimePath {
            path: base.to_owned(),
        }
    })?;
    Ok(parent.join(format!("{stem}-{correlation_id}.jsonl")))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForkCoreConfig {
    pub binary: PathBuf,
    pub runtime_paths: Option<ForkRuntimePaths>,
    pub probe_timeout_ms: u64,
    pub rewrite_timeout_ms: u64,
}

impl Default for ForkCoreConfig {
    fn default() -> Self {
        Self {
            binary: PathBuf::from("rtk"),
            runtime_paths: None,
            probe_timeout_ms: 5_000,
            rewrite_timeout_ms: 5_000,
        }
    }
}

pub type RtkAdapterConfig = ForkCoreConfig;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "interface", rename_all = "snake_case")]
pub enum RtkRewriteInterface {
    ForkCli,
    Unavailable { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RtkCapabilities {
    pub binary: PathBuf,
    pub detected_version: Option<String>,
    pub contract: Option<EngineContractIdentity>,
    pub rewrite: RtkRewriteInterface,
    pub proxy: bool,
}

#[derive(Deserialize)]
struct CurrentEngineMetadata {
    engine_version: String,
    manifest_sha256: String,
    content_manifest_sha256: String,
}

pub fn expected_engine_identity() -> Result<EngineContractIdentity, String> {
    let metadata: CurrentEngineMetadata =
        toml::from_str(include_str!("../../../fork-core/CURRENT_ENGINE.toml"))
            .map_err(|error| format!("embedded current-engine metadata is invalid: {error}"))?;
    Ok(EngineContractIdentity {
        contract_version: ENGINE_CONTRACT_VERSION,
        engine_version: metadata.engine_version,
        manifest_sha256: metadata.manifest_sha256,
        content_manifest_sha256: metadata.content_manifest_sha256,
    })
}

#[derive(Serialize)]
struct CapabilityBatchRequest<'a> {
    commands: &'a [String],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityBatchResponse {
    supported: Vec<bool>,
}

/// One rewrite decision together with the payload-free attribution that explains it.
///
/// The attribution travels with the decision because the process that records the operation is
/// not the process that classified it: the daemon knows the evasion class, the host runs the
/// approved command, and the engine writes the ledger row. Returning the decision alone left the
/// recorded row unattributed, so per-session avoidable figures could never move.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RtkRewriteOutcome {
    pub decision: RewriteDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evasion: Option<EvasionAttribution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accounting_correlation_id: Option<String>,
}

impl RtkRewriteOutcome {
    pub fn apply_evasion_environment(
        &self,
        environment: &mut crate::Environment,
    ) -> Result<(), ExecError> {
        environment.set.remove(INTERNAL_EVASION_ENV);
        environment
            .remove
            .retain(|variable| variable != INTERNAL_EVASION_ENV);
        match self.evasion {
            Some(evasion) => {
                environment.set.insert(
                    INTERNAL_EVASION_ENV.to_owned(),
                    serde_json::to_string(&evasion)?,
                );
            }
            None => environment.remove.push(INTERNAL_EVASION_ENV.to_owned()),
        }
        Ok(())
    }
}

/// Render an agent-facing reason for an Ask or Deny decision.
///
/// A bare "policy requires approval" costs the agent a turn and teaches it nothing: it names
/// neither the construct that was refused nor the route to reach for instead. Both are already
/// carried by the closed evasion taxonomy, so the message is assembled from that single
/// authority rather than restated at each call site. Commands without attribution keep the
/// original wording, because inventing a prescription for an unclassified construct would be a
/// guess presented as guidance.
fn policy_reason(
    decision: &str,
    plan_reason: Option<RewritePlanReason>,
    attribution: Option<&EvasionAttribution>,
    proposed: Option<&str>,
) -> String {
    let policy = match plan_reason {
        Some(RewritePlanReason::PermissionPolicy) => "fork-core permission policy",
        Some(RewritePlanReason::CanonicalPolicy) | None => "fork-core canonical policy",
    };
    let verb = if decision == "deny" {
        "denied the command"
    } else {
        "requires approval"
    };
    let Some(attribution) = attribution else {
        return format!("{policy} {verb}");
    };
    let mut reason = format!(
        "{tier} {decision} {class} ({construct}): {policy} {verb}; {prescription}",
        tier = attribution.tier.as_str(),
        class = attribution.class.as_str(),
        construct = attribution.class.construct(),
        prescription = attribution.class.prescription(),
    );
    if let Some(proposed) = proposed
        .map(str::trim)
        .filter(|proposed| !proposed.is_empty())
    {
        reason.push_str(&format!("; approving runs the managed form: {proposed}"));
    }
    reason
}

fn outcome_without_evasion(decision: RewriteDecision) -> RtkRewriteOutcome {
    RtkRewriteOutcome {
        decision,
        evasion: None,
        accounting_correlation_id: None,
    }
}

#[derive(Clone, Debug)]
pub struct PinnedRtkAdapter {
    config: RtkAdapterConfig,
    capabilities: RtkCapabilities,
}

#[derive(Clone, Debug)]
pub struct ForkCoreRunner {
    binary: PathBuf,
    runtime_paths: ForkRuntimePaths,
}

#[derive(Clone, Debug)]
pub struct ForkCoreInvocation {
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub timeout_ms: Option<u64>,
    pub capture: crate::CaptureConfig,
    pub stdin: crate::StdinSpec,
    account_usage: bool,
    accounting_correlation_id: String,
}

impl ForkCoreInvocation {
    #[must_use]
    pub fn new(args: Vec<String>) -> Self {
        Self {
            args,
            cwd: None,
            timeout_ms: None,
            capture: crate::CaptureConfig::default(),
            stdin: crate::StdinSpec::default(),
            account_usage: true,
            accounting_correlation_id: uuid::Uuid::new_v4().simple().to_string(),
        }
    }

    #[must_use]
    pub fn without_accounting(mut self) -> Self {
        self.account_usage = false;
        self
    }

    #[must_use]
    pub fn accounting_correlation_id(&self) -> &str {
        &self.accounting_correlation_id
    }
}

#[derive(Debug)]
pub struct AccountedForkExecution {
    pub outcome: crate::ExecutionOutcome,
    pub accounting: ForkAccountingHandle,
}

impl ForkCoreRunner {
    #[must_use]
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    #[must_use]
    pub fn runtime_paths(&self) -> &ForkRuntimePaths {
        &self.runtime_paths
    }

    pub fn accounting_handle(
        &self,
        correlation_id: &str,
    ) -> Result<ForkAccountingHandle, ExecError> {
        self.runtime_paths
            .accounting_context(correlation_id.to_owned())
    }

    pub fn managed_command(&self, args: &[String]) -> Result<CanonicalCommand, ExecError> {
        self.runtime_paths.ensure_layout()?;
        let binary = self
            .binary
            .to_str()
            .ok_or_else(|| ExecError::NonUtf8ForkBinaryPath {
                path: self.binary.clone(),
            })?;
        CanonicalCommand::argv(binary, args.to_vec())
    }

    pub fn envelope(&self, args: &[String]) -> Result<crate::ExecutionEnvelope, ExecError> {
        let accounting = self.runtime_paths.new_accounting_context()?;
        self.envelope_with_accounting(args, &accounting)
    }

    fn envelope_with_accounting(
        &self,
        args: &[String],
        accounting: &ForkAccountingHandle,
    ) -> Result<crate::ExecutionEnvelope, ExecError> {
        let command = self.managed_command(args)?;
        let mut envelope = crate::ExecutionEnvelope::allow_raw(command.clone());
        envelope.decision = RewriteDecision::AllowRewrite {
            command,
            source: RewriteSource::Rtk {
                version: PINNED_RTK_VERSION.to_owned(),
                route: crate::RtkRewriteRoute::Optimized,
            },
            reason: "direct managed fork-core invocation".to_owned(),
        };
        self.runtime_paths.apply_to_environment(
            &mut envelope.environment,
            &self.binary,
            accounting,
        )?;
        Ok(envelope)
    }

    pub fn std_command(&self, args: &[String]) -> Result<std::process::Command, ExecError> {
        self.accounted_std_command(args).map(|(command, _)| command)
    }

    pub fn accounted_std_command(
        &self,
        args: &[String],
    ) -> Result<(std::process::Command, ForkAccountingHandle), ExecError> {
        self.accounted_std_command_inner(args)
    }

    pub fn std_command_os(&self, args: &[OsString]) -> Result<std::process::Command, ExecError> {
        self.accounted_std_command_os(args)
            .map(|(command, _)| command)
    }

    pub fn accounted_std_command_os(
        &self,
        args: &[OsString],
    ) -> Result<(std::process::Command, ForkAccountingHandle), ExecError> {
        self.accounted_std_command_inner(args)
    }

    fn accounted_std_command_inner<I, S>(
        &self,
        args: I,
    ) -> Result<(std::process::Command, ForkAccountingHandle), ExecError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.runtime_paths.ensure_layout()?;
        let accounting = self.runtime_paths.new_accounting_context()?;
        let mut command = std::process::Command::new(&self.binary);
        command.args(args);
        self.runtime_paths
            .apply_to_std_command(&mut command, &self.binary, &accounting)?;
        Ok((command, accounting))
    }

    pub async fn execute(
        &self,
        invocation: ForkCoreInvocation,
    ) -> Result<crate::ExecutionOutcome, ExecError> {
        Ok(self.execute_accounted(invocation).await?.outcome)
    }

    pub async fn execute_accounted(
        &self,
        invocation: ForkCoreInvocation,
    ) -> Result<AccountedForkExecution, ExecError> {
        let accounting = self
            .runtime_paths
            .accounting_context(invocation.accounting_correlation_id.clone())?;
        let mut envelope = self.envelope_with_accounting(&invocation.args, &accounting)?;
        if !invocation.account_usage {
            envelope
                .environment
                .set
                .insert("RTK_TRACKING_DISABLED".into(), "1".into());
        }
        envelope.cwd = invocation.cwd;
        envelope.timeout_ms = invocation.timeout_ms;
        envelope.capture = invocation.capture;
        envelope.stdin = invocation.stdin;
        let outcome = crate::ExecutionPipeline.execute(envelope).await?;
        Ok(AccountedForkExecution {
            outcome,
            accounting,
        })
    }
}

impl PinnedRtkAdapter {
    pub async fn detect(mut config: RtkAdapterConfig) -> Self {
        let capabilities = probe(&config).await;
        config.binary.clone_from(&capabilities.binary);
        Self {
            config,
            capabilities,
        }
    }

    #[must_use]
    pub fn capabilities(&self) -> &RtkCapabilities {
        &self.capabilities
    }

    pub fn runner(&self) -> Result<ForkCoreRunner, ExecError> {
        if !matches!(self.capabilities.rewrite, RtkRewriteInterface::ForkCli)
            || !self.capabilities.proxy
        {
            return Err(ExecError::ForkCoreUnavailable {
                reason: format!("{:?}", self.capabilities.rewrite),
            });
        }
        let runtime_paths = self
            .config
            .runtime_paths
            .clone()
            .ok_or(ExecError::MissingForkRuntimePaths)?;
        Ok(ForkCoreRunner {
            binary: self.config.binary.clone(),
            runtime_paths,
        })
    }

    /// Classify distinct commands through one canonical fork-core process.
    ///
    /// The response contains booleans only; command payloads never return through stdout.
    pub async fn classify_capabilities_once(
        config: RtkAdapterConfig,
        commands: &[String],
    ) -> Result<Vec<bool>, ExecError> {
        let binary = resolve_binary(&config.binary)
            .map_err(|reason| ExecError::ForkCoreUnavailable { reason })?;
        let runtime_paths = config
            .runtime_paths
            .ok_or(ExecError::MissingForkRuntimePaths)?;
        let payload = serde_json::to_vec(&CapabilityBatchRequest { commands })?;
        runtime_paths.ensure_layout()?;
        let mut command = Command::new(&binary);
        command.arg("capability-batch");
        runtime_paths.apply_to_command(&mut command, &binary)?;
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let program = binary.display().to_string();
        let mut child = command.spawn().map_err(|source| ExecError::Spawn {
            program: program.clone(),
            source,
        })?;
        let mut stdin = child.stdin.take().ok_or_else(|| ExecError::MissingPipe {
            program: program.clone(),
            stream: "stdin",
        })?;
        stdin
            .write_all(&payload)
            .await
            .map_err(|source| ExecError::WriteStdin {
                program: program.clone(),
                source,
            })?;
        drop(stdin);
        let output = tokio::time::timeout(
            Duration::from_millis(config.rewrite_timeout_ms),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| ExecError::ForkCoreUnavailable {
            reason: "capability batch timed out".to_owned(),
        })?
        .map_err(|source| ExecError::Wait {
            program: program.clone(),
            source,
        })?;
        if !output.status.success() {
            return Err(ExecError::ForkCoreUnavailable {
                reason: format!("capability batch exited with {}", output.status),
            });
        }
        let response: CapabilityBatchResponse = serde_json::from_slice(&output.stdout)?;
        if response.supported.len() != commands.len() {
            return Err(ExecError::ForkCoreUnavailable {
                reason: "capability batch cardinality mismatch".to_owned(),
            });
        }
        Ok(response.supported)
    }

    pub async fn decide(&self, command: &CanonicalCommand) -> RewriteDecision {
        self.decide_in(command, None).await
    }

    pub async fn decide_in(
        &self,
        command: &CanonicalCommand,
        cwd: Option<&Path>,
    ) -> RewriteDecision {
        self.decide_with_plan_in(command, cwd).await.decision
    }

    pub async fn decide_with_plan_in(
        &self,
        command: &CanonicalCommand,
        cwd: Option<&Path>,
    ) -> RtkRewriteOutcome {
        self.decide_in_mode(command, cwd, false).await
    }

    pub async fn decide_byte_fidelity_in(
        &self,
        command: &CanonicalCommand,
        cwd: Option<&Path>,
    ) -> RewriteDecision {
        self.decide_byte_fidelity_with_plan_in(command, cwd)
            .await
            .decision
    }

    pub async fn decide_byte_fidelity_with_plan_in(
        &self,
        command: &CanonicalCommand,
        cwd: Option<&Path>,
    ) -> RtkRewriteOutcome {
        self.decide_in_mode(command, cwd, true).await
    }

    async fn decide_in_mode(
        &self,
        command: &CanonicalCommand,
        cwd: Option<&Path>,
        byte_fidelity: bool,
    ) -> RtkRewriteOutcome {
        if let RtkRewriteInterface::Unavailable { reason } = &self.capabilities.rewrite {
            return outcome_without_evasion(RewriteDecision::Deny {
                reason: format!("managed fork-core unavailable: {reason}"),
            });
        }
        if !self.capabilities.proxy {
            return outcome_without_evasion(RewriteDecision::Deny {
                reason: "managed fork-core proxy capability is unavailable".to_owned(),
            });
        }
        let Some(runtime_paths) = self.config.runtime_paths.as_ref() else {
            return outcome_without_evasion(RewriteDecision::Deny {
                reason: "managed fork-core runtime paths are unavailable".to_owned(),
            });
        };
        if let Err(error) = runtime_paths.ensure_layout() {
            return outcome_without_evasion(RewriteDecision::Deny {
                reason: format!("managed fork-core runtime is unavailable: {error}"),
            });
        }

        let raw = render_command(command);
        let mut rewrite = Command::new(&self.config.binary);
        rewrite.arg("rewrite-plan").arg(&raw);
        if byte_fidelity {
            rewrite.env(BYTE_FIDELITY_ENV, "1");
        } else {
            rewrite.env_remove(BYTE_FIDELITY_ENV);
        }
        if let Err(error) = runtime_paths.apply_to_command(&mut rewrite, &self.config.binary) {
            return outcome_without_evasion(RewriteDecision::Deny {
                reason: format!("managed fork-core environment is unavailable: {error}"),
            });
        }
        if let Some(cwd) = cwd {
            rewrite.current_dir(cwd);
        }
        let output = run_with_timeout(
            &mut rewrite,
            Duration::from_millis(self.config.rewrite_timeout_ms),
        )
        .await;
        let output = match output {
            Ok(output) => output,
            Err(reason) => {
                return outcome_without_evasion(RewriteDecision::Deny {
                    reason: format!("managed fork-core rewrite failed: {reason}"),
                });
            }
        };
        if !output.status.success() {
            return outcome_without_evasion(RewriteDecision::Deny {
                reason: format!(
                    "managed fork-core rewrite plan exited with {}",
                    output.status
                ),
            });
        }
        let plan: RewritePlan = match serde_json::from_slice(&output.stdout) {
            Ok(plan) => plan,
            Err(error) => {
                return outcome_without_evasion(RewriteDecision::Deny {
                    reason: format!("managed fork-core rewrite plan was invalid JSON: {error}"),
                });
            }
        };
        if !plan.is_consistent() {
            return outcome_without_evasion(RewriteDecision::Deny {
                reason: "managed fork-core rewrite plan had inconsistent policy metadata"
                    .to_owned(),
            });
        }
        let (decision, accounting_correlation_id) = match plan.decision {
            RewritePlanDecision::Rewrite => match plan.proposed {
                Some(proposed) => self.rewritten_decision(command, proposed.as_bytes(), false),
                None => (
                    RewriteDecision::Deny {
                        reason: "fork-core rewrite plan omitted its proposed command".to_owned(),
                    },
                    None,
                ),
            },
            RewritePlanDecision::Proxy if plan.proposed.is_none() => self.proxy_decision(command),
            RewritePlanDecision::Proxy => (
                RewriteDecision::Deny {
                    reason: "fork-core proxy plan unexpectedly included a command".to_owned(),
                },
                None,
            ),
            RewritePlanDecision::Ask => {
                let prescribed = plan.proposed.clone();
                let managed = plan
                    .proposed
                    .and_then(|proposed| self.managed_shell_command(command, &proposed).ok());
                let correlation_id = managed
                    .as_ref()
                    .map(|(_, accounting)| accounting.correlation_id().to_owned());
                (
                    RewriteDecision::Ask {
                        proposed: managed.map(|(command, _)| command),
                        reason: policy_reason(
                            "ask",
                            plan.reason,
                            plan.attribution.as_ref(),
                            prescribed.as_deref(),
                        ),
                    },
                    correlation_id,
                )
            }
            RewritePlanDecision::Deny => (
                RewriteDecision::Deny {
                    reason: policy_reason("deny", plan.reason, plan.attribution.as_ref(), None),
                },
                None,
            ),
        };
        RtkRewriteOutcome {
            decision,
            evasion: plan.attribution,
            accounting_correlation_id,
        }
    }

    fn rewritten_decision(
        &self,
        original: &CanonicalCommand,
        stdout: &[u8],
        approval_required: bool,
    ) -> (RewriteDecision, Option<String>) {
        let rewritten = match std::str::from_utf8(stdout) {
            Ok(rewritten) if !rewritten.is_empty() => rewritten,
            Ok(_) => {
                return (
                    RewriteDecision::Deny {
                        reason: "fork-core returned an empty rewrite".to_owned(),
                    },
                    None,
                );
            }
            Err(_) => {
                return (
                    RewriteDecision::Deny {
                        reason: "fork-core rewrite was not valid UTF-8".to_owned(),
                    },
                    None,
                );
            }
        };
        let proposed = match self.managed_shell_command(original, rewritten) {
            Ok(command) => command,
            Err(error) => {
                return (
                    RewriteDecision::Deny {
                        reason: format!("fork-core rewrite could not be anchored: {error}"),
                    },
                    None,
                );
            }
        };
        let correlation_id = proposed.1.correlation_id().to_owned();
        if approval_required {
            (
                RewriteDecision::Ask {
                    proposed: Some(proposed.0),
                    reason: "fork-core permission policy requires approval".to_owned(),
                },
                Some(correlation_id),
            )
        } else {
            (
                RewriteDecision::AllowRewrite {
                    command: proposed.0,
                    source: RewriteSource::Rtk {
                        version: PINNED_RTK_VERSION.to_owned(),
                        route: crate::RtkRewriteRoute::Optimized,
                    },
                    reason: "fork-core approved and produced the managed command".to_owned(),
                },
                Some(correlation_id),
            )
        }
    }

    fn proxy_decision(&self, original: &CanonicalCommand) -> (RewriteDecision, Option<String>) {
        let proxy = match self.managed_proxy_command(original) {
            Ok(command) => command,
            Err(error) => {
                return (
                    RewriteDecision::Deny {
                        reason: format!("fork-core proxy could not be anchored: {error}"),
                    },
                    None,
                );
            }
        };
        (
            RewriteDecision::AllowRewrite {
                command: proxy.0,
                source: RewriteSource::Rtk {
                    version: PINNED_RTK_VERSION.to_owned(),
                    route: crate::RtkRewriteRoute::Proxy,
                },
                reason: "fork-core selected tracked raw proxy execution".to_owned(),
            },
            Some(proxy.1.correlation_id().to_owned()),
        )
    }

    fn managed_shell_command(
        &self,
        original: &CanonicalCommand,
        rewritten: &str,
    ) -> Result<(CanonicalCommand, ForkAccountingHandle), ExecError> {
        let shell = execution_shell(original);
        let runtime = self
            .config
            .runtime_paths
            .as_ref()
            .ok_or(ExecError::MissingForkRuntimePaths)?;
        let binary_directory =
            self.config
                .binary
                .parent()
                .ok_or_else(|| ExecError::InvalidForkBinaryPath {
                    path: self.config.binary.clone(),
                })?;
        let accounting = runtime.new_accounting_context()?;
        let script = format!(
            "{}{}",
            runtime.shell_exports(binary_directory, &accounting)?,
            rewritten
        );
        Ok((CanonicalCommand::with_shell(shell, script)?, accounting))
    }

    fn managed_proxy_command(
        &self,
        original: &CanonicalCommand,
    ) -> Result<(CanonicalCommand, ForkAccountingHandle), ExecError> {
        let proxy_args = match original {
            CanonicalCommand::Argv { program, args } => {
                let mut proxy = Vec::with_capacity(args.len() + 2);
                proxy.push("proxy".to_owned());
                proxy.push(program.clone());
                proxy.extend(args.iter().cloned());
                proxy
            }
            CanonicalCommand::Shell { shell, command } => vec![
                "proxy".to_owned(),
                shell.clone(),
                shell_command_flag().to_owned(),
                command.clone(),
            ],
        };
        let runtime = self
            .config
            .runtime_paths
            .as_ref()
            .ok_or(ExecError::MissingForkRuntimePaths)?;
        let binary_directory =
            self.config
                .binary
                .parent()
                .ok_or_else(|| ExecError::InvalidForkBinaryPath {
                    path: self.config.binary.clone(),
                })?;
        let binary =
            self.config
                .binary
                .to_str()
                .ok_or_else(|| ExecError::NonUtf8ForkBinaryPath {
                    path: self.config.binary.clone(),
                })?;
        let mut argv = Vec::with_capacity(proxy_args.len() + 1);
        argv.push(binary.to_owned());
        argv.extend(proxy_args);
        let accounting = runtime.new_accounting_context()?;
        let script = format!(
            "{}{}",
            runtime.shell_exports(binary_directory, &accounting)?,
            render_argv(&argv)
        );
        Ok((
            CanonicalCommand::with_shell(execution_shell(original), script)?,
            accounting,
        ))
    }
}

async fn probe(config: &RtkAdapterConfig) -> RtkCapabilities {
    let Some(runtime_paths) = config.runtime_paths.as_ref() else {
        return unavailable(
            config.binary.clone(),
            None,
            "managed runtime paths were not configured",
        );
    };
    if let Err(error) = runtime_paths.ensure_layout() {
        return unavailable(
            config.binary.clone(),
            None,
            &format!("runtime layout failed: {error}"),
        );
    }
    let binary = match resolve_binary(&config.binary) {
        Ok(binary) => binary,
        Err(reason) => return unavailable(config.binary.clone(), None, &reason),
    };
    if binary.file_name().and_then(|name| name.to_str()) != Some("rtk") {
        return unavailable(binary, None, "managed fork binary must be named rtk");
    }

    let version_output = run_probe(&binary, &["--version"], runtime_paths, config).await;
    let version_output = match version_output {
        Ok(output) if output.status.success() => output,
        Ok(_) => return unavailable(binary, None, "version probe returned failure"),
        Err(reason) => return unavailable(binary, None, &format!("version probe {reason}")),
    };
    let detected_version = parse_version(&version_output.stdout);
    if detected_version.as_deref() != Some(PINNED_RTK_VERSION) {
        return unavailable(
            binary,
            detected_version,
            &format!("expected fork-core {PINNED_RTK_VERSION}"),
        );
    }

    let expected_contract = match expected_engine_identity() {
        Ok(identity) => identity,
        Err(reason) => return unavailable(binary, detected_version, &reason),
    };
    let contract_output = run_probe(&binary, &["contract", "--json"], runtime_paths, config).await;
    let contract_output = match contract_output {
        Ok(output) if output.status.success() => output,
        Ok(_) => return unavailable(binary, detected_version, "contract probe returned failure"),
        Err(reason) => {
            return unavailable(
                binary,
                detected_version,
                &format!("contract probe {reason}"),
            );
        }
    };
    let contract: EngineContractIdentity = match serde_json::from_slice(&contract_output.stdout) {
        Ok(identity) => identity,
        Err(error) => {
            return unavailable(
                binary,
                detected_version,
                &format!("contract probe returned invalid JSON: {error}"),
            );
        }
    };
    if contract != expected_contract {
        return unavailable(
            binary,
            detected_version,
            "fork contract identity did not match CURRENT_ENGINE.toml",
        );
    }

    let rewrite_help = run_probe(&binary, &["rewrite", "--help"], runtime_paths, config).await;
    let rewrite_help = match rewrite_help {
        Ok(output) if output.status.success() => output,
        Ok(_) => {
            return unavailable(binary, detected_version, "rewrite probe returned failure");
        }
        Err(reason) => {
            return unavailable(binary, detected_version, &format!("rewrite probe {reason}"));
        }
    };
    let rewrite_text = output_text(&rewrite_help);
    if !rewrite_text.contains("rtk rewrite") || !rewrite_text.contains("Raw command to rewrite") {
        return unavailable(
            binary,
            detected_version,
            "fork rewrite contract was not detected",
        );
    }

    let proxy_help = run_probe(&binary, &["proxy", "--help"], runtime_paths, config).await;
    let proxy_help = match proxy_help {
        Ok(output) if output.status.success() => output,
        Ok(_) => {
            return unavailable(binary, detected_version, "proxy probe returned failure");
        }
        Err(reason) => {
            return unavailable(binary, detected_version, &format!("proxy probe {reason}"));
        }
    };
    let proxy_text = output_text(&proxy_help);
    if !proxy_text.contains("rtk proxy") || !proxy_text.contains("without filtering") {
        return unavailable(
            binary,
            detected_version,
            "fork proxy contract was not detected",
        );
    }

    RtkCapabilities {
        binary,
        detected_version,
        contract: Some(contract),
        rewrite: RtkRewriteInterface::ForkCli,
        proxy: true,
    }
}

async fn run_probe(
    binary: &Path,
    args: &[&str],
    runtime_paths: &ForkRuntimePaths,
    config: &RtkAdapterConfig,
) -> Result<Output, String> {
    let mut command = Command::new(binary);
    command.args(args);
    runtime_paths
        .apply_to_command(&mut command, binary)
        .map_err(|error| error.to_string())?;
    run_with_timeout(&mut command, Duration::from_millis(config.probe_timeout_ms)).await
}

fn unavailable(binary: PathBuf, detected_version: Option<String>, reason: &str) -> RtkCapabilities {
    RtkCapabilities {
        binary,
        detected_version,
        contract: None,
        rewrite: RtkRewriteInterface::Unavailable {
            reason: reason.to_owned(),
        },
        proxy: false,
    }
}

fn resolve_binary(binary: &Path) -> Result<PathBuf, String> {
    let candidates: Vec<PathBuf> = if binary.components().count() > 1 || binary.is_absolute() {
        vec![binary.to_owned()]
    } else {
        std::env::var_os("PATH")
            .map(|path| {
                std::env::split_paths(&path)
                    .map(|directory| directory.join(binary))
                    .collect()
            })
            .unwrap_or_default()
    };
    candidates
        .into_iter()
        .find_map(|candidate| {
            is_executable(&candidate)
                .then(|| fs::canonicalize(&candidate).ok())
                .flatten()
        })
        .ok_or_else(|| format!("binary {} was not found or executable", binary.display()))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), ExecError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
        ExecError::PrepareForkRuntime {
            path: path.to_owned(),
            source,
        }
    })
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), ExecError> {
    Ok(())
}

fn parse_version(stdout: &[u8]) -> Option<String> {
    String::from_utf8_lossy(stdout)
        .split_whitespace()
        .map(|part| part.trim_start_matches('v'))
        .find(|part| part.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
        .map(str::to_owned)
}

fn render_command(command: &CanonicalCommand) -> String {
    match command {
        CanonicalCommand::Argv { program, args } => {
            let mut argv = Vec::with_capacity(args.len() + 1);
            argv.push(program.clone());
            argv.extend(args.iter().cloned());
            render_argv(&argv)
        }
        CanonicalCommand::Shell { command, .. } => command.clone(),
    }
}

fn render_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|word| shell_quote(word))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_+-./:=,@%".contains(ch))
    {
        return word.to_owned();
    }
    format!("'{}'", word.replace('\'', "'\"'\"'"))
}

fn execution_shell(command: &CanonicalCommand) -> String {
    match command {
        CanonicalCommand::Shell { shell, .. } => shell.clone(),
        CanonicalCommand::Argv { .. } => default_shell().to_owned(),
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

#[cfg(unix)]
const fn shell_command_flag() -> &'static str {
    "-c"
}

#[cfg(windows)]
const fn shell_command_flag() -> &'static str {
    "/C"
}

fn parent_or_error(path: &Path) -> Result<&Path, ExecError> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| ExecError::InvalidForkRuntimePath {
            path: path.to_owned(),
        })
}

fn path_text(path: &Path) -> Result<&str, ExecError> {
    path.to_str()
        .ok_or_else(|| ExecError::NonUtf8ForkRuntimePath {
            path: path.to_owned(),
        })
}

fn prefixed_path(binary_directory: &Path) -> Result<OsString, ExecError> {
    let mut paths = vec![binary_directory.to_owned()];
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path));
    }
    std::env::join_paths(paths).map_err(|error| ExecError::InvalidForkPathEnvironment {
        reason: error.to_string(),
    })
}

fn output_text(output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

async fn run_with_timeout(command: &mut Command, timeout: Duration) -> Result<Output, String> {
    command.kill_on_drop(true);
    match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(format!("could not start: {error}")),
        Err(_) => Err(format!("timed out after {} ms", timeout.as_millis())),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EvasionAttribution, PINNED_RTK_VERSION, PinnedRtkAdapter, RewritePlanReason,
        RtkAdapterConfig, parse_version, policy_reason, render_command,
    };
    use crate::{CanonicalCommand, parse_simple_shell};
    use hzr_protocol::{EnforcementTier, EvasionClass, EvasionPathForm, FidelityValidation};

    /// Every closed evasion class must produce a message an agent can act on.
    ///
    /// The 0.4.4 shipping behavior was a single generic string for all 25 Ask cases in the
    /// acceptance matrix: no class, no construct, no route. This gate fails if any class
    /// regresses to a reason the agent cannot act on.
    #[test]
    fn acceptance_gate_every_ask_names_its_class_and_prescribes_a_route() {
        const CLASSES: [EvasionClass; 10] = [
            EvasionClass::E1QuotedCoveredCommand,
            EvasionClass::E2ShellWrapper,
            EvasionClass::E3InterpreterRead,
            EvasionClass::E4ExecutablePath,
            EvasionClass::E5PipelineOrRedirect,
            EvasionClass::E6NestedUnboundedReader,
            EvasionClass::E7FidelityHatch,
            EvasionClass::E8NativeTool,
            EvasionClass::E9DiagnosticBypass,
            EvasionClass::E10CapabilityGap,
        ];
        for class in CLASSES {
            let attribution = EvasionAttribution {
                class,
                wrapper_depth: 0,
                interpreter: None,
                path_form: EvasionPathForm::Bare,
                stage_count: 1,
                hatch_marker: false,
                avoidable: true,
                tier: EnforcementTier::T2DenyWithPrescription,
                fidelity_reason: None,
                fidelity_validation: FidelityValidation::NotRequested,
            };
            let ask = policy_reason(
                "ask",
                Some(RewritePlanReason::CanonicalPolicy),
                Some(&attribution),
                None,
            );
            assert!(
                ask.contains(class.as_str()),
                "{ask} omits the class token for {class:?}"
            );
            assert!(
                ask.contains(class.construct()),
                "{ask} omits the construct for {class:?}"
            );
            assert!(
                ask.contains(class.prescription()),
                "{ask} omits the prescribed route for {class:?}"
            );
            assert!(ask.contains("requires approval"), "{ask} omits the verdict");

            let deny = policy_reason(
                "deny",
                Some(RewritePlanReason::PermissionPolicy),
                Some(&attribution),
                None,
            );
            assert!(
                deny.contains("denied the command") && deny.contains(class.prescription()),
                "{deny} is not actionable for {class:?}"
            );
        }
    }

    /// An approved Ask must show the exact command approval will run.
    #[test]
    fn acceptance_gate_a_proposed_ask_shows_the_managed_form() {
        let attribution = EvasionAttribution {
            class: EvasionClass::E2ShellWrapper,
            wrapper_depth: 1,
            interpreter: None,
            path_form: EvasionPathForm::Bare,
            stage_count: 1,
            hatch_marker: false,
            avoidable: true,
            tier: EnforcementTier::T2DenyWithPrescription,
            fidelity_reason: None,
            fidelity_validation: FidelityValidation::NotRequested,
        };
        let reason = policy_reason(
            "ask",
            Some(RewritePlanReason::CanonicalPolicy),
            Some(&attribution),
            Some("rtk read README.md"),
        );
        assert!(reason.contains("approving runs the managed form: rtk read README.md"));
    }

    /// Without attribution the message stays exactly as it was: a prescription invented for an
    /// unclassified construct would be a guess presented to the agent as guidance.
    #[test]
    fn an_unclassified_command_keeps_the_original_wording() {
        assert_eq!(
            policy_reason("ask", None, None, None),
            "fork-core canonical policy requires approval"
        );
        assert_eq!(
            policy_reason(
                "deny",
                Some(RewritePlanReason::PermissionPolicy),
                None,
                None
            ),
            "fork-core permission policy denied the command"
        );
    }

    #[test]
    fn test_parse_version_finds_fork_semver_token() {
        assert_eq!(
            parse_version(format!("rtk {PINNED_RTK_VERSION}\n").as_bytes()),
            Some(PINNED_RTK_VERSION.to_owned())
        );
    }

    #[test]
    fn test_render_argv_round_trips_quotes() {
        let command = CanonicalCommand::argv(
            "git",
            vec![
                "log".to_owned(),
                "--format=%h %s".to_owned(),
                "apostrophe's".to_owned(),
            ],
        )
        .expect("valid argv");
        assert_eq!(
            parse_simple_shell(&render_command(&command)),
            Ok(vec![
                "git".to_owned(),
                "log".to_owned(),
                "--format=%h %s".to_owned(),
                "apostrophe's".to_owned(),
            ])
        );
    }

    #[tokio::test]
    async fn capability_batch_fails_closed_when_the_pinned_engine_is_absent() {
        let config = RtkAdapterConfig {
            binary: std::path::PathBuf::from("/definitely/missing/hzr-fork-core"),
            ..RtkAdapterConfig::default()
        };

        assert!(
            PinnedRtkAdapter::classify_capabilities_once(config, &["cargo test".to_owned()])
                .await
                .is_err()
        );
    }
}
