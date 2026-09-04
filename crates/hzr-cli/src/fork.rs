use std::ffi::OsString;
use std::path::Path;
use std::process::{ExitCode, Stdio};

use anyhow::{Context, Result, bail};
use hzr_core::{AccountingReceiptContextStore, Config, ambient_session_id};
use hzr_exec::{ForkRuntimePaths, PinnedRtkAdapter, RtkAdapterConfig};

pub async fn passthrough(config: &Config, args: &[OsString]) -> Result<ExitCode> {
    if is_contract_bootstrap(args) {
        if let Some(message) = crate::update::startup_notice(&config.data_dir).await {
            eprintln!("{}", crate::update::agent_notice(&message));
        }
    }
    let binary = config.engines.binary("rtk");
    reject_compatibility_cycle(&binary)?;
    let adapter = PinnedRtkAdapter::detect(RtkAdapterConfig {
        binary,
        runtime_paths: Some(ForkRuntimePaths::from_data_root(&config.data_dir)),
        ..RtkAdapterConfig::default()
    })
    .await;
    let runner = adapter.runner().with_context(|| {
        format!(
            "managed fork-core is unavailable: {:?}",
            adapter.capabilities().rewrite
        )
    })?;
    let (mut command, accounting) = runner.accounted_std_command_os(args)?;
    let project =
        std::fs::canonicalize(std::env::current_dir()?).context("resolve direct fork workspace")?;
    let agent = std::env::var("HZR_CLIENT").ok();
    AccountingReceiptContextStore::new(&config.data_dir).register(
        accounting.correlation_id(),
        &project,
        agent.as_deref(),
        ambient_session_id().as_deref(),
    )?;
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = supervised_status(command)
        .await
        .with_context(|| format!("failed to run {}", runner.binary().display()))?;
    if let Err(error) =
        AccountingReceiptContextStore::new(&config.data_dir).complete(accounting.correlation_id())
    {
        eprintln!("HZR accounting completion remains unresolved: {error}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            // exec resets caught signal handlers. The static shell only re-raises the numeric
            // child signal in its own process, preserving the original wait status without FFI.
            use std::os::unix::process::CommandExt;
            let error = std::process::Command::new("/bin/sh")
                .args([
                    "-c",
                    "kill -s \"$1\" \"$$\"",
                    "hzr-signal",
                    &signal.to_string(),
                ])
                .exec();
            return Err(error).context("failed to preserve fork termination signal");
        }
    }
    std::process::exit(status.code().unwrap_or(1))
}

async fn supervised_status(
    command: std::process::Command,
) -> std::io::Result<std::process::ExitStatus> {
    #[cfg(unix)]
    {
        use nix::sys::signal::{Signal, kill, killpg};
        use nix::unistd::Pid;
        use std::io::IsTerminal;
        use std::os::unix::process::CommandExt;
        use tokio::signal::unix::{SignalKind, signal};
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        let owns_group = !std::io::stdin().is_terminal();
        let mut command = command;
        if owns_group {
            command.process_group(0);
        }
        let mut command = tokio::process::Command::from(command);
        command.kill_on_drop(true);
        let mut child = command.spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| std::io::Error::other("owned fork has no process ID"))?;
        let signal = tokio::select! {
            status = child.wait() => return status,
            _ = interrupt.recv() => Signal::SIGINT,
            _ = terminate.recv() => Signal::SIGTERM,
        };
        let group = Pid::from_raw(i32::try_from(pid).map_err(std::io::Error::other)?);
        // The owned child remains unreaped, preventing PID reuse.
        if owns_group {
            let _ = killpg(group, signal);
        } else {
            // Keep terminal foreground semantics; never signal a group shared with the user's shell.
            let _ = kill(group, signal);
        }
        match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
            Ok(status) => status,
            Err(_) => {
                if owns_group {
                    let _ = killpg(group, Signal::SIGKILL);
                } else {
                    let _ = kill(group, Signal::SIGKILL);
                }
                child.wait().await
            }
        }
    }
    #[cfg(not(unix))]
    {
        let shutdown = hzr_daemon::shutdown_signal()?;
        let mut command = tokio::process::Command::from(command);
        command.kill_on_drop(true);
        let mut child = command.spawn()?;
        tokio::select! {
            status = child.wait() => status,
            _ = shutdown => {
                child.kill().await?;
                child.wait().await
            }
        }
    }
}

fn is_contract_bootstrap(args: &[OsString]) -> bool {
    args.first().is_some_and(|argument| argument == "read")
        && args.iter().skip(1).any(|argument| {
            Path::new(argument)
                .file_name()
                .is_some_and(|name| name == "HZR.md")
        })
}

fn reject_compatibility_cycle(binary: &Path) -> Result<()> {
    let candidate = if binary.components().count() > 1 {
        binary.is_file().then(|| binary.to_owned())
    } else {
        std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|directory| directory.join(binary))
                .find(|candidate| {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;

                        std::fs::metadata(candidate)
                            .map(|metadata| {
                                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                            })
                            .unwrap_or(false)
                    }
                    #[cfg(not(unix))]
                    {
                        candidate.is_file()
                    }
                })
        })
    };
    let Some(candidate) = candidate else {
        return Ok(());
    };
    let current = std::env::current_exe().context("failed to resolve the HZR executable")?;
    #[cfg(unix)]
    let same_file = {
        use std::os::unix::fs::MetadataExt;

        let candidate = std::fs::metadata(&candidate).with_context(|| {
            format!(
                "failed to inspect managed fork-core {}",
                candidate.display()
            )
        })?;
        let current = std::fs::metadata(&current)
            .with_context(|| format!("failed to inspect HZR executable {}", current.display()))?;
        candidate.dev() == current.dev() && candidate.ino() == current.ino()
    };
    #[cfg(not(unix))]
    let same_file = {
        let candidate = std::fs::canonicalize(&candidate).with_context(|| {
            format!(
                "failed to resolve managed fork-core {}",
                candidate.display()
            )
        })?;
        let current = std::fs::canonicalize(&current)
            .with_context(|| format!("failed to resolve HZR executable {}", current.display()))?;
        candidate == current
    };
    if same_file {
        bail!(
            "managed fork-core {} resolves to the HZR compatibility executable; configure engines.directory to the private fork-core binary",
            binary.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::is_contract_bootstrap;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn installed_contract_read_is_an_agent_update_checkpoint() {
        assert!(is_contract_bootstrap(&args(&[
            "read",
            "/opt/hzr/current/share/hzr/HZR.md",
            "--level",
            "none",
        ])));
        assert!(!is_contract_bootstrap(&args(&[
            "read",
            "/opt/hzr/current/share/hzr/README.md",
            "--level",
            "none",
        ])));
        assert!(!is_contract_bootstrap(&args(&["raw", "sh", "HZR.md",])));
    }
}
