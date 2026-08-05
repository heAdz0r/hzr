use std::ffi::OsString;
use std::path::Path;
use std::process::{ExitCode, Stdio};

use anyhow::{Context, Result, bail};
use hzr_core::Config;
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
    let mut command = runner.std_command_os(args)?;
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        let error = command.exec();
        Err(error).with_context(|| format!("failed to exec {}", runner.binary().display()))
    }

    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .with_context(|| format!("failed to run {}", runner.binary().display()))?;
        std::process::exit(status.code().unwrap_or(1));
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
