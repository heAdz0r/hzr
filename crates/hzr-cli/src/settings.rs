use std::fs;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use hzr_core::Config;

#[derive(Debug, Subcommand)]
pub enum SettingsCommand {
    /// Inspect or update delegation; omitted fields keep their values
    Delegation {
        /// Provider preset; changing it selects its default model unless --model is supplied
        #[arg(long, value_parser = ["opencode-go", "openrouter", "deepseek"])]
        provider: Option<String>,
        /// Exact provider model ID; never changes the parent model
        #[arg(long)]
        model: Option<String>,
        /// Permit new delegated runs (true or false)
        #[arg(long, action = clap::ArgAction::Set)]
        enabled: Option<bool>,
        /// Maximum worker turns (1-100)
        #[arg(long)]
        max_turns: Option<u32>,
        /// Maximum worker duration in milliseconds (1000-1800000)
        #[arg(long)]
        timeout_ms: Option<u64>,
    },
    /// Store your provider key privately; never accepts a key in argv
    Login {
        #[arg(long, value_parser = ["opencode-go", "openrouter", "deepseek"])]
        provider: String,
        /// Import an existing private regular file instead of a hidden terminal prompt
        #[arg(long)]
        key_file: Option<PathBuf>,
    },
}

pub fn credential_path(config: &Config, provider: &str) -> PathBuf {
    config
        .data_dir
        .join("credentials")
        .join(format!("{provider}.secret"))
}

fn private_regular_key(path: &Path) -> Result<String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        bail!("credential must be a regular non-symlink file");
    }
    let file = options.open(path).context("cannot open credential file")?;
    // Inspect the opened file, not an earlier path snapshot that could be replaced.
    let metadata = file.metadata().context("cannot inspect credential file")?;
    if !metadata.is_file() {
        bail!("credential must be a regular non-symlink file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!("credential file must be private (chmod 600)");
        }
    }
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .context("cannot read credential file")?;
    if bytes.len() > 4096 {
        bail!("credential exceeds 4096 bytes");
    }
    validate_key(String::from_utf8(bytes).context("credential must be UTF-8")?)
}

fn validate_key(value: String) -> Result<String> {
    let key = value.trim().to_owned();
    if key.is_empty()
        || key.len() > 4096
        || key
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        bail!("credential must be one nonempty token of at most 4096 bytes");
    }
    Ok(key)
}

/// One delegated worker at a time per private HZR data root. A nonblocking
/// lock bounds quota concurrency and rejects normal recursive CLI delegation.
/// Explicitly unlock before closing: a concurrent fork can temporarily inherit
/// the file descriptor until exec, so close alone need not release flock yet.
pub struct FileLock(fs::File);

impl Drop for FileLock {
    fn drop(&mut self) {
        if fs2::FileExt::unlock(&self.0).is_err() {
            eprintln!("HZR could not release a configuration or delegation lock.");
        }
    }
}

pub fn acquire_delegation_slot(config: &Config) -> Result<FileLock> {
    use fs2::FileExt;
    config.ensure_layout()?;
    let path = config.data_dir.join("runtime/delegation.lock");
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    file.try_lock_exclusive().context(
        "a delegated worker is already running; recursive or concurrent delegation is not allowed",
    )?;
    Ok(FileLock(file))
}

pub fn require_credential(config: &Config) -> Result<PathBuf> {
    let path = credential_path(config, &config.delegation.provider);
    private_regular_key(&path).context("run hzr settings login for the selected provider")?;
    Ok(path)
}

fn save_key(path: &Path, key: &str) -> Result<()> {
    let parent = path.parent().context("credential directory missing")?;
    for ancestor in parent.ancestors() {
        if ancestor.exists() && fs::symlink_metadata(ancestor)?.file_type().is_symlink() {
            bail!("credential directory cannot contain symlinks");
        }
    }
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    if path
        .symlink_metadata()
        .is_ok_and(|metadata| !metadata.is_file())
    {
        bail!("credential destination must be a regular file");
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temporary.write_all(key.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .context("cannot persist private credential")?;
    Ok(())
}

fn lock_settings_update(path: &Path) -> Result<FileLock> {
    use fs2::FileExt;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut name = path
        .file_name()
        .context("configuration filename missing")?
        .to_os_string();
    name.push(".delegation.lock");
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    let lock = options.open(parent.join(name))?;
    if !lock.metadata()?.is_file() {
        bail!("settings lock must be a regular file");
    }
    lock.lock_exclusive()
        .context("cannot lock delegation settings")?;
    Ok(FileLock(lock))
}

pub fn execute(
    mut config: Config,
    path: &Path,
    command: Option<SettingsCommand>,
    json: bool,
) -> Result<ExitCode> {
    match command {
        Some(SettingsCommand::Delegation {
            provider,
            model,
            enabled,
            max_turns,
            timeout_ms,
        }) => {
            let changed = provider.is_some()
                || model.is_some()
                || enabled.is_some()
                || max_turns.is_some()
                || timeout_ms.is_some();
            // Serialize read-modify-write updates. Reload after locking so two
            // settings processes changing different fields cannot lose an update.
            let _update_lock = if changed {
                Some(lock_settings_update(path)?)
            } else {
                None
            };
            if changed {
                config = Config::load_or_default(path)?;
            }
            if let Some(provider) = provider {
                if provider != config.delegation.provider && model.is_none() {
                    config.delegation.model = match provider.as_str() {
                        "openrouter" => "deepseek/deepseek-v4.1-flash",
                        "deepseek" => "deepseek-flash",
                        _ => "deepseek-v4.1-flash",
                    }
                    .into();
                }
                config.delegation.provider = provider;
            }
            if let Some(model) = model {
                config.delegation.model = model;
            }
            if let Some(enabled) = enabled {
                config.delegation.enabled = enabled;
            }
            if let Some(turns) = max_turns {
                config.delegation.max_turns = turns;
            }
            if let Some(timeout) = timeout_ms {
                config.delegation.timeout_ms = timeout;
            }
            config.validate()?;
            if changed {
                config.write(path)?;
            }
        }
        Some(SettingsCommand::Login { provider, key_file }) => {
            let key = if let Some(source) = key_file {
                private_regular_key(&source)?
            } else {
                if !std::io::stdin().is_terminal() {
                    bail!("login requires a terminal or --key-file pointing to a private file");
                }
                validate_key(
                    rpassword::prompt_password("Provider API key (hidden): ")
                        .context("credential prompt failed")?,
                )?
            };
            save_key(&credential_path(&config, &provider), &key)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({"provider": provider, "credential_saved": true})
                );
            } else {
                println!("Private credential saved for {provider}.");
            }
            return Ok(ExitCode::SUCCESS);
        }
        None => {}
    }
    let ready = require_credential(&config).is_ok();
    if json {
        println!(
            "{}",
            serde_json::json!({"delegation": config.delegation, "credential_configured": ready, "parent_model": "unchanged"})
        );
    } else {
        let d = &config.delegation;
        println!(
            "Delegation: {}\nWorker: {} / {}\nLimits: {} turns, {} ms\nCredential: {}\nParent model: unchanged",
            if d.enabled { "enabled" } else { "disabled" },
            d.provider,
            d.model,
            d.max_turns,
            d.timeout_ms,
            if ready {
                "configured"
            } else {
                "missing or unsafe"
            }
        );
        println!(
            "\nConfigure: hzr settings delegation --provider opencode-go --model deepseek-v4.1-flash --enabled true\nLogin: hzr settings login --provider opencode-go\nRun: hzr delegate --file task.md"
        );
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegation_slot_rejects_concurrency_and_releases_on_drop() {
        let directory = tempfile::tempdir().expect("fixture");
        let config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        let first = acquire_delegation_slot(&config).expect("first worker");
        // A fork inherits this same open file description before exec closes it.
        let inherited = first.0.try_clone().expect("inherited descriptor");
        assert!(acquire_delegation_slot(&config).is_err());
        drop(first);
        let next = acquire_delegation_slot(&config).expect("released worker slot");
        drop(next);
        drop(inherited);
    }

    #[test]
    fn settings_reload_under_lock_preserves_previous_writer() {
        let directory = tempfile::tempdir().expect("fixture");
        let path = directory.path().join("config.toml");
        let initial = Config::default();
        initial.write(&path).expect("fixture");
        let mut concurrent = initial.clone();
        concurrent.delegation.max_turns = 23;
        concurrent.write(&path).expect("other writer");
        execute(
            initial,
            &path,
            Some(SettingsCommand::Delegation {
                provider: None,
                model: None,
                enabled: Some(true),
                max_turns: None,
                timeout_ms: None,
            }),
            true,
        )
        .expect("update from stale caller snapshot");
        let saved = Config::load(&path).expect("reload");
        assert_eq!(saved.delegation.max_turns, 23);
        assert!(saved.delegation.enabled);
    }
    #[test]
    fn credentials_roundtrip_without_entering_config() {
        let directory = tempfile::tempdir().expect("test fixture");
        let path = directory
            .path()
            .canonicalize()
            .expect("test fixture")
            .join("credentials/test.secret");
        save_key(&path, "fixture-not-a-real-key").expect("test fixture");
        assert_eq!(
            private_regular_key(&path).expect("test fixture"),
            "fixture-not-a-real-key"
        );
        assert!(validate_key("bad\nkey".into()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_and_public_credential() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = tempfile::tempdir().expect("test fixture");
        let path = directory
            .path()
            .canonicalize()
            .expect("test fixture")
            .join("private.secret");
        save_key(&path, "fixture").expect("test fixture");
        let link = directory
            .path()
            .canonicalize()
            .expect("test fixture")
            .join("link");
        symlink(&path, &link).expect("test fixture");
        assert!(private_regular_key(&link).is_err());
        assert!(save_key(&link, "replacement").is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("test fixture");
        assert!(private_regular_key(&path).is_err());
    }
}
