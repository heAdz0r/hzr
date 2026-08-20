use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::{CanonicalCommand, ExecError, RewriteDecision, RewriteSource};

pub const PINNED_RTK_VERSION: &str = "0.44.1-fork.1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForkRuntimePaths {
    pub memory_db: PathBuf,
    pub history_db: PathBuf,
    pub tee_dir: PathBuf,
    pub audit_dir: PathBuf,
}

impl ForkRuntimePaths {
    #[must_use]
    pub fn from_data_root(data_root: &Path) -> Self {
        let root = data_root.join("fork");
        Self {
            memory_db: root.join("mem.db"),
            history_db: data_root.join("ledger/hzr.sqlite"),
            tee_dir: root.join("tee"),
            audit_dir: root.join("audit"),
        }
    }

    pub fn ensure_layout(&self) -> Result<(), ExecError> {
        let memory_parent = parent_or_error(&self.memory_db)?;
        let history_parent = parent_or_error(&self.history_db)?;
        for directory in [
            memory_parent,
            history_parent,
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
            &self.history_db,
            &self.tee_dir,
            &self.audit_dir,
        ] {
            if path.to_str().is_none() {
                return Err(ExecError::NonUtf8ForkRuntimePath { path: path.clone() });
            }
        }
        Ok(())
    }

    fn apply_to_command(&self, command: &mut Command, binary: &Path) -> Result<(), ExecError> {
        let binary_directory = binary
            .parent()
            .ok_or_else(|| ExecError::InvalidForkBinaryPath {
                path: binary.to_owned(),
            })?;
        command
            .env("RTK_MEM_DB_PATH", &self.memory_db)
            .env("RTK_DB_PATH", &self.history_db)
            .env("RTK_TEE_DIR", &self.tee_dir)
            .env("RTK_AUDIT_DIR", &self.audit_dir)
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
    ) -> Result<(), ExecError> {
        let binary_directory = binary
            .parent()
            .ok_or_else(|| ExecError::InvalidForkBinaryPath {
                path: binary.to_owned(),
            })?;
        for (key, value) in [
            ("RTK_MEM_DB_PATH", path_text(&self.memory_db)?),
            ("RTK_DB_PATH", path_text(&self.history_db)?),
            ("RTK_TEE_DIR", path_text(&self.tee_dir)?),
            ("RTK_AUDIT_DIR", path_text(&self.audit_dir)?),
            ("RTK_TEE", "0"),
            ("RTK_HISTORY_DAYS", "0"),
            ("RTK_TRACKING_DISABLED", "0"),
            ("RTK_TELEMETRY_DISABLED", "1"),
        ] {
            environment.set.insert(key.to_owned(), value.to_owned());
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
    ) -> Result<(), ExecError> {
        let binary_directory = binary
            .parent()
            .ok_or_else(|| ExecError::InvalidForkBinaryPath {
                path: binary.to_owned(),
            })?;
        command
            .env("RTK_MEM_DB_PATH", &self.memory_db)
            .env("RTK_DB_PATH", &self.history_db)
            .env("RTK_TEE_DIR", &self.tee_dir)
            .env("RTK_AUDIT_DIR", &self.audit_dir)
            .env("RTK_TEE", "0")
            .env("RTK_HISTORY_DAYS", "0")
            .env("RTK_TRACKING_DISABLED", "0")
            .env("RTK_TELEMETRY_DISABLED", "1")
            .env("PATH", prefixed_path(binary_directory)?);
        Ok(())
    }

    fn shell_exports(&self, binary_directory: &Path) -> Result<String, ExecError> {
        self.validate_shell_paths()?;
        let binary_directory =
            binary_directory
                .to_str()
                .ok_or_else(|| ExecError::NonUtf8ForkRuntimePath {
                    path: binary_directory.to_owned(),
                })?;
        Ok(format!(
            "RTK_MEM_DB_PATH={}\nRTK_DB_PATH={}\nRTK_TEE_DIR={}\nRTK_AUDIT_DIR={}\nRTK_TEE=0\nRTK_HISTORY_DAYS=0\nRTK_TRACKING_DISABLED=0\nRTK_TELEMETRY_DISABLED=1\nPATH={}${{PATH:+\":$PATH\"}}\nexport RTK_MEM_DB_PATH RTK_DB_PATH RTK_TEE_DIR RTK_AUDIT_DIR RTK_TEE RTK_HISTORY_DAYS RTK_TRACKING_DISABLED RTK_TELEMETRY_DISABLED PATH\n",
            shell_quote(path_text(&self.memory_db)?),
            shell_quote(path_text(&self.history_db)?),
            shell_quote(path_text(&self.tee_dir)?),
            shell_quote(path_text(&self.audit_dir)?),
            shell_quote(binary_directory),
        ))
    }
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
    pub rewrite: RtkRewriteInterface,
    pub proxy: bool,
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
        }
    }

    #[must_use]
    pub fn without_accounting(mut self) -> Self {
        self.account_usage = false;
        self
    }
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
        self.runtime_paths
            .apply_to_environment(&mut envelope.environment, &self.binary)?;
        Ok(envelope)
    }

    pub fn std_command(&self, args: &[String]) -> Result<std::process::Command, ExecError> {
        self.std_command_inner(args)
    }

    pub fn std_command_os(&self, args: &[OsString]) -> Result<std::process::Command, ExecError> {
        self.std_command_inner(args)
    }

    fn std_command_inner<I, S>(&self, args: I) -> Result<std::process::Command, ExecError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.runtime_paths.ensure_layout()?;
        let mut command = std::process::Command::new(&self.binary);
        command.args(args);
        self.runtime_paths
            .apply_to_std_command(&mut command, &self.binary)?;
        Ok(command)
    }

    pub async fn execute(
        &self,
        invocation: ForkCoreInvocation,
    ) -> Result<crate::ExecutionOutcome, ExecError> {
        let mut envelope = self.envelope(&invocation.args)?;
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
        crate::ExecutionPipeline.execute(envelope).await
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

    pub async fn decide(&self, command: &CanonicalCommand) -> RewriteDecision {
        self.decide_in(command, None).await
    }

    pub async fn decide_in(
        &self,
        command: &CanonicalCommand,
        cwd: Option<&Path>,
    ) -> RewriteDecision {
        if let RtkRewriteInterface::Unavailable { reason } = &self.capabilities.rewrite {
            return RewriteDecision::Deny {
                reason: format!("managed fork-core unavailable: {reason}"),
            };
        }
        if !self.capabilities.proxy {
            return RewriteDecision::Deny {
                reason: "managed fork-core proxy capability is unavailable".to_owned(),
            };
        }
        let Some(runtime_paths) = self.config.runtime_paths.as_ref() else {
            return RewriteDecision::Deny {
                reason: "managed fork-core runtime paths are unavailable".to_owned(),
            };
        };
        if let Err(error) = runtime_paths.ensure_layout() {
            return RewriteDecision::Deny {
                reason: format!("managed fork-core runtime is unavailable: {error}"),
            };
        }

        let raw = render_command(command);
        let mut rewrite = Command::new(&self.config.binary);
        rewrite.arg("rewrite").arg(&raw);
        if let Err(error) = runtime_paths.apply_to_command(&mut rewrite, &self.config.binary) {
            return RewriteDecision::Deny {
                reason: format!("managed fork-core environment is unavailable: {error}"),
            };
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
                return RewriteDecision::Deny {
                    reason: format!("managed fork-core rewrite failed: {reason}"),
                };
            }
        };

        match output.status.code() {
            Some(0) => self.rewritten_decision(command, &output.stdout, false),
            Some(1) => self.proxy_decision(command),
            Some(2) => RewriteDecision::Deny {
                reason: "fork-core permission policy denied the command".to_owned(),
            },
            Some(3) => self.rewritten_decision(command, &output.stdout, true),
            status => RewriteDecision::Deny {
                reason: format!(
                    "managed fork-core rewrite returned unsupported status {}",
                    status.map_or_else(|| "signal".to_owned(), |code| code.to_string())
                ),
            },
        }
    }

    fn rewritten_decision(
        &self,
        original: &CanonicalCommand,
        stdout: &[u8],
        approval_required: bool,
    ) -> RewriteDecision {
        let rewritten = match std::str::from_utf8(stdout) {
            Ok(rewritten) if !rewritten.is_empty() => rewritten,
            Ok(_) => {
                return RewriteDecision::Deny {
                    reason: "fork-core returned an empty rewrite".to_owned(),
                };
            }
            Err(_) => {
                return RewriteDecision::Deny {
                    reason: "fork-core rewrite was not valid UTF-8".to_owned(),
                };
            }
        };
        let proposed = match self.managed_shell_command(original, rewritten) {
            Ok(command) => command,
            Err(error) => {
                return RewriteDecision::Deny {
                    reason: format!("fork-core rewrite could not be anchored: {error}"),
                };
            }
        };
        if approval_required {
            RewriteDecision::Ask {
                proposed: Some(proposed),
                reason: "fork-core permission policy requires approval".to_owned(),
            }
        } else {
            RewriteDecision::AllowRewrite {
                command: proposed,
                source: RewriteSource::Rtk {
                    version: PINNED_RTK_VERSION.to_owned(),
                    route: crate::RtkRewriteRoute::Optimized,
                },
                reason: "fork-core approved and produced the managed command".to_owned(),
            }
        }
    }

    fn proxy_decision(&self, original: &CanonicalCommand) -> RewriteDecision {
        let proxy = match self.managed_proxy_command(original) {
            Ok(command) => command,
            Err(error) => {
                return RewriteDecision::Deny {
                    reason: format!("fork-core proxy could not be anchored: {error}"),
                };
            }
        };
        RewriteDecision::AllowRewrite {
            command: proxy,
            source: RewriteSource::Rtk {
                version: PINNED_RTK_VERSION.to_owned(),
                route: crate::RtkRewriteRoute::Proxy,
            },
            reason: "fork-core selected tracked raw proxy execution".to_owned(),
        }
    }

    fn managed_shell_command(
        &self,
        original: &CanonicalCommand,
        rewritten: &str,
    ) -> Result<CanonicalCommand, ExecError> {
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
        let script = format!("{}{}", runtime.shell_exports(binary_directory)?, rewritten);
        CanonicalCommand::with_shell(shell, script)
    }

    fn managed_proxy_command(
        &self,
        original: &CanonicalCommand,
    ) -> Result<CanonicalCommand, ExecError> {
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
        let script = format!(
            "{}{}",
            runtime.shell_exports(binary_directory)?,
            render_argv(&argv)
        );
        CanonicalCommand::with_shell(execution_shell(original), script)
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
    use super::{PINNED_RTK_VERSION, parse_version, render_command};
    use crate::{CanonicalCommand, parse_simple_shell};

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
}
