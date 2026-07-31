//! M1: Hook management functions extracted from mod.rs.
//! Handles installation/uninstallation of Claude Code hooks for memory layer.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

use super::{BLOCK_EXPLORE_SCRIPT, MEM_HOOK_SCRIPT};

pub(super) fn is_block_explore_entry(entry: &serde_json::Value) -> bool {
    entry.get("matcher").and_then(|m| m.as_str()) == Some("Task")
        && entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .map(|hooks| {
                hooks.iter().any(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .map(|c| c.contains("rtk-block-native-explore"))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
}

pub(super) fn is_mem_hook_entry(entry: &serde_json::Value) -> bool {
    entry.get("matcher").and_then(|m| m.as_str()) == Some("Task")
        && entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .map(|hooks| {
                hooks.iter().any(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .map(|c| c.contains("rtk-mem-context"))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
}

pub(super) fn installed_mem_hook_command(pre: &[serde_json::Value]) -> Option<String> {
    pre.iter()
        .find(|entry| is_mem_hook_entry(entry))
        .and_then(|entry| entry.get("hooks"))
        .and_then(|hooks| hooks.as_array())
        .and_then(|hooks| hooks.first())
        .and_then(|hook| hook.get("command"))
        .and_then(|command| command.as_str())
        .map(|s| s.to_string())
}

fn materialize_mem_hook_script() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Cannot find home directory")?;
    let hooks_dir = home.join(".claude").join("hooks");
    fs::create_dir_all(&hooks_dir)
        .with_context(|| format!("Failed to create hooks directory {}", hooks_dir.display()))?;

    let hook_path = hooks_dir.join("rtk-mem-context.sh");
    fs::write(&hook_path, MEM_HOOK_SCRIPT)
        .with_context(|| format!("Failed to write {}", hook_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o755);
        fs::set_permissions(&hook_path, perms).with_context(|| {
            format!(
                "Failed to set executable permissions on {}",
                hook_path.display()
            )
        })?;
    }

    Ok(hook_path)
}

fn materialize_block_explore_script() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Cannot find home directory")?;
    let hooks_dir = home.join(".claude").join("hooks");
    fs::create_dir_all(&hooks_dir)
        .with_context(|| format!("Failed to create hooks directory {}", hooks_dir.display()))?;

    let hook_path = hooks_dir.join("rtk-block-native-explore.sh");
    fs::write(&hook_path, BLOCK_EXPLORE_SCRIPT)
        .with_context(|| format!("Failed to write {}", hook_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o755);
        fs::set_permissions(&hook_path, perms).with_context(|| {
            format!(
                "Failed to set executable permissions on {}",
                hook_path.display()
            )
        })?;
    }

    Ok(hook_path)
}

/// Install (or uninstall) the rtk-mem-context.sh PreToolUse:Task hook in ~/.claude/settings.json
pub fn run_install_hook(uninstall: bool, status_only: bool, verbose: u8) -> Result<()> {
    let settings_path = dirs::home_dir()
        .context("Cannot find home directory")?
        .join(".claude")
        .join("settings.json");

    let raw = if settings_path.exists() {
        fs::read_to_string(&settings_path).context("Failed to read settings.json")?
    } else {
        "{}".to_string()
    };

    let mut settings: serde_json::Value =
        serde_json::from_str(&raw).context("Failed to parse settings.json")?;

    let pre = settings
        .get("hooks")
        .and_then(|h| h.get("PreToolUse"))
        .and_then(|a| a.as_array())
        .cloned()
        .unwrap_or_default();
    let existing_hook = installed_mem_hook_command(&pre);
    let already_installed = existing_hook.is_some();

    if status_only {
        println!(
            "memory.hook status={} path={} command={}",
            if already_installed {
                "installed"
            } else {
                "not_installed"
            },
            settings_path.display(),
            existing_hook.unwrap_or_else(|| "-".to_string())
        );
        return Ok(());
    }

    if uninstall {
        if !already_installed {
            println!("memory.hook uninstall: nothing to remove");
            return Ok(());
        }
        let filtered: Vec<serde_json::Value> = pre
            .into_iter()
            .filter(|entry| !is_mem_hook_entry(entry) && !is_block_explore_entry(entry))
            .collect();
        settings["hooks"]["PreToolUse"] = serde_json::json!(filtered);
        if settings_path.exists() {
            let backup = settings_path.with_extension("json.bak");
            let _ = fs::copy(&settings_path, &backup);
        }
        let json = serde_json::to_string_pretty(&settings)?;
        fs::write(&settings_path, json)?;
        println!("memory.hook uninstall ok path={}", settings_path.display());
        return Ok(());
    }

    let hook_bin = materialize_mem_hook_script()?;
    let block_explore_bin = materialize_block_explore_script()?;
    let mem_hook_entry = serde_json::json!({
        "matcher": "Task",
        "hooks": [{"type": "command", "command": hook_bin.to_string_lossy().to_string(), "timeout": 10}]
    });
    let block_explore_entry = serde_json::json!({
        "matcher": "Task",
        "hooks": [{"type": "command", "command": block_explore_bin.to_string_lossy().to_string(), "timeout": 10}]
    });

    let mut new_pre: Vec<serde_json::Value> = pre
        .into_iter()
        .filter(|entry| !is_mem_hook_entry(entry) && !is_block_explore_entry(entry))
        .collect();
    new_pre.push(block_explore_entry);
    new_pre.push(mem_hook_entry);
    settings["hooks"]["PreToolUse"] = serde_json::json!(new_pre);

    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent)?;
    }

    if settings_path.exists() {
        let backup = settings_path.with_extension("json.bak");
        if let Err(e) = fs::copy(&settings_path, &backup) {
            eprintln!(
                "memory.hook WARNING: failed to create backup {}: {}",
                backup.display(),
                e
            );
        }
    }

    let json = serde_json::to_string_pretty(&settings)?;
    fs::write(&settings_path, &json)
        .with_context(|| format!("Failed to write {}", settings_path.display()))?;

    println!(
        "memory.hook {} path={}",
        if already_installed {
            "updated ok"
        } else {
            "installed ok"
        },
        settings_path.display()
    );
    if verbose > 0 {
        println!("  mem_hook:          {}", hook_bin.display());
        println!("  block_explore:     {}", block_explore_bin.display());
        println!("  fires on: PreToolUse:Task (all subagent types)");
    }
    Ok(())
}
