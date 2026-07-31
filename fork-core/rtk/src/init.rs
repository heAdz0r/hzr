use crate::grepai; // grepai integration
use crate::write_core::{AtomicWriter, WriteOptions};
use anyhow::{Context, Result};
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

// The shell hook delegates rewrite decisions to the Rust registry.
const REWRITE_HOOK: &str = include_str!("../hooks/rtk-rewrite.sh");

// Embedded slim RTK awareness instructions
const RTK_SLIM: &str = include_str!("../hooks/rtk-awareness.md");

/// Control flow for settings.json patching
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PatchMode {
    Ask,  // Default: prompt user [y/N]
    Auto, // --auto-patch: no prompt
    Skip, // --no-patch: manual instructions
}

#[derive(Clone, Copy)]
pub enum FilterTrust {
    Ask,
    Trust,
    Skip,
}

pub fn finalize_filter_trust(dry_run: bool, mode: FilterTrust) -> Result<()> {
    for path in crate::trust::gated_filter_paths() {
        if !path.exists() {
            continue;
        }
        let trusted = matches!(
            crate::trust::check_trust(&path).unwrap_or(crate::trust::TrustStatus::Untrusted),
            crate::trust::TrustStatus::Trusted | crate::trust::TrustStatus::EnvOverride
        );
        if trusted {
            continue;
        }

        let should_trust = match mode {
            FilterTrust::Trust => true,
            FilterTrust::Skip => false,
            FilterTrust::Ask => {
                if !io::stdin().is_terminal() {
                    false
                } else {
                    eprint!("Trust custom filters at {}? [y/N] ", path.display());
                    io::stderr().flush()?;
                    let mut answer = String::new();
                    io::stdin().read_line(&mut answer)?;
                    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
                }
            }
        };

        if should_trust {
            if dry_run {
                println!("[dry-run] would trust custom filters: {}", path.display());
            } else {
                let bytes = fs::read(&path)?;
                let hash = crate::integrity::compute_hash_bytes(&bytes);
                crate::trust::trust_filter_with_hash(&path, &hash)?;
                println!("Trusted custom filters: {}", path.display());
            }
        } else {
            println!("Custom filters remain disabled: {}", path.display());
        }
    }
    Ok(())
}

/// Result of settings.json patching operation
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PatchResult {
    Patched,        // Hook was added successfully
    AlreadyPresent, // Hook was already in settings.json
    Declined,       // User declined when prompted
    Skipped,        // --no-patch flag used
}

// Legacy full instructions for backward compatibility (--claude-md mode)
const RTK_INSTRUCTIONS: &str = include_str!("../hooks/rtk-instructions.md");
const RTK_SLIM_CODEX: &str = include_str!("../hooks/rtk-awareness-codex.md");

const LEGACY_HOOK_FILES: &[&str] = &[
    "rtk-block-native-grep.sh",
    "rtk-block-native-read.sh",
    "rtk-block-native-write.sh",
    "rtk-block-task.sh",
];

pub fn run(
    global: bool,
    claude_md: bool,
    hook_only: bool,
    codex: bool,
    patch_mode: PatchMode,
    dry_run: bool,
    verbose: u8,
) -> Result<()> {
    if codex {
        if claude_md || hook_only || patch_mode != PatchMode::Ask {
            anyhow::bail!(
                "--codex cannot be combined with --claude-md, --hook-only, --auto-patch, or --no-patch"
            );
        }
        if dry_run {
            return preview_codex_mode(global, verbose);
        }
        return run_codex_mode(global, verbose);
    }
    if dry_run {
        return preview_claude_mode(global, claude_md, hook_only, patch_mode, verbose);
    }
    // Mode selection
    match (claude_md, hook_only) {
        (true, _) => run_claude_md_mode(global, verbose),
        (false, true) => run_hook_only_mode(global, patch_mode, verbose),
        (false, false) => run_default_mode(global, patch_mode, verbose),
    }
}

fn prepare_hook_path() -> Result<PathBuf> {
    let claude_dir = resolve_claude_dir()?;
    let hook_dir = claude_dir.join("hooks");
    fs::create_dir_all(&hook_dir)
        .with_context(|| format!("Failed to create hook directory: {}", hook_dir.display()))?;
    Ok(hook_dir.join("rtk-rewrite.sh"))
}

/// Write a single hook file if missing or outdated, return true if changed
#[cfg(unix)]
fn install_single_hook(hook_path: &Path, content: &str, verbose: u8) -> Result<bool> {
    let changed = if hook_path.exists() {
        let existing = fs::read_to_string(hook_path)
            .with_context(|| format!("Failed to read existing hook: {}", hook_path.display()))?;

        if existing == content {
            if verbose > 0 {
                eprintln!("Hook already up to date: {}", hook_path.display());
            }
            false
        } else {
            fs::write(hook_path, content)
                .with_context(|| format!("Failed to write hook to {}", hook_path.display()))?;
            if verbose > 0 {
                eprintln!("Updated hook: {}", hook_path.display());
            }
            true
        }
    } else {
        fs::write(hook_path, content)
            .with_context(|| format!("Failed to write hook to {}", hook_path.display()))?;
        if verbose > 0 {
            eprintln!("Created hook: {}", hook_path.display());
        }
        true
    };

    // Set executable permissions
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(hook_path, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("Failed to set hook permissions: {}", hook_path.display()))?;

    Ok(changed)
}

#[cfg(unix)]
fn ensure_hook_installed(rewrite_path: &Path, verbose: u8) -> Result<bool> {
    let changed = install_single_hook(rewrite_path, REWRITE_HOOK, verbose)?;
    crate::integrity::store_hash(rewrite_path)?;
    Ok(changed)
}

fn remove_legacy_hook_files(rewrite_path: &Path, verbose: u8) -> Result<usize> {
    let hook_dir = rewrite_path
        .parent()
        .context("Rewrite hook path has no parent directory")?;
    let mut removed = 0;

    for name in LEGACY_HOOK_FILES {
        let path = hook_dir.join(name);
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove obsolete hook: {}", path.display()))?;
            removed += 1;
            if verbose > 0 {
                eprintln!("Removed obsolete hook: {}", path.display());
            }
        }
    }

    Ok(removed)
}

/// Idempotent file write: create or update if content differs
fn write_if_changed(path: &Path, content: &str, name: &str, verbose: u8) -> Result<bool> {
    let existed = path.exists();
    let writer = AtomicWriter::new(WriteOptions::durable());
    let stats = writer
        .write_str(path, content)
        .with_context(|| format!("Failed to write {}: {}", name, path.display()))?;

    if stats.skipped_unchanged {
        if verbose > 0 {
            eprintln!("{} already up to date: {}", name, path.display());
        }
        Ok(false)
    } else {
        if verbose > 0 {
            if existed {
                eprintln!("Updated {}: {}", name, path.display());
            } else {
                eprintln!("Created {}: {}", name, path.display());
            }
        }
        Ok(true)
    }
}

/// Atomic write using tempfile + rename
/// Prevents corruption on crash/interrupt
fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let writer = AtomicWriter::new(WriteOptions::durable());
    writer.write_str(&target, content).map(|_| ())?;
    Ok(())
}

/// Prompt user for consent to patch settings.json
/// Prints to stderr (stdout may be piped), reads from stdin
/// Default is No (capital N)
fn prompt_user_consent(settings_path: &Path) -> Result<bool> {
    use std::io::{self, BufRead, IsTerminal};

    eprintln!("\nPatch existing {}? [y/N] ", settings_path.display());

    // If stdin is not a terminal (piped), default to No
    if !io::stdin().is_terminal() {
        eprintln!("(non-interactive mode, defaulting to N)");
        return Ok(false);
    }

    let stdin = io::stdin();
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .context("Failed to read user input")?;

    let response = line.trim().to_lowercase();
    Ok(response == "y" || response == "yes")
}

/// Print manual instructions for settings.json patching
fn print_manual_instructions(rewrite_path: &Path) {
    println!("\n  MANUAL STEP: Add this to ~/.claude/settings.json:");
    println!("  {{");
    println!("    \"hooks\": {{ \"PreToolUse\": [");
    println!("      {{");
    println!("        \"matcher\": \"Bash\",");
    println!(
        "        \"hooks\": [{{ \"type\": \"command\", \"command\": \"{}\", \"timeout\": 5 }}]",
        rewrite_path.display()
    );
    println!("      }}");
    println!("    ]}}");
    println!("  }}");
    println!("\n  Then restart Claude Code. Test with: git status\n");
}

/// Remove RTK hook entry from settings.json
/// Returns true if hook was found and removed
fn remove_hook_from_json(root: &mut serde_json::Value) -> bool {
    let hooks = match root.get_mut("hooks").and_then(|h| h.get_mut("PreToolUse")) {
        Some(pre_tool_use) => pre_tool_use,
        None => return false,
    };

    let pre_tool_use_array = match hooks.as_array_mut() {
        Some(arr) => arr,
        None => return false,
    };

    // Remove the current rewrite hook and obsolete native-tool blockers.
    let original_len = pre_tool_use_array.len();
    pre_tool_use_array.retain(|entry| {
        if let Some(hooks_array) = entry.get("hooks").and_then(|h| h.as_array()) {
            for hook in hooks_array {
                if let Some(command) = hook.get("command").and_then(|c| c.as_str()) {
                    if command.contains("rtk-rewrite.sh")
                        || LEGACY_HOOK_FILES.iter().any(|name| command.contains(name))
                    {
                        return false; // Remove this RTK entry
                    }
                }
            }
        }
        true // Keep this entry
    });

    pre_tool_use_array.len() < original_len
}

/// Remove RTK hook from settings.json file
/// Backs up before modification, returns true if hook was found and removed
fn remove_hook_from_settings(verbose: u8) -> Result<bool> {
    let claude_dir = resolve_claude_dir()?;
    let settings_path = claude_dir.join("settings.json");

    if !settings_path.exists() {
        if verbose > 0 {
            eprintln!("settings.json not found, nothing to remove");
        }
        return Ok(false);
    }

    let content = fs::read_to_string(&settings_path)
        .with_context(|| format!("Failed to read {}", settings_path.display()))?;

    if content.trim().is_empty() {
        return Ok(false);
    }

    let mut root: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse {} as JSON", settings_path.display()))?;

    let removed = remove_hook_from_json(&mut root);

    if removed {
        // Backup original
        let backup_path = settings_path.with_extension("json.bak");
        fs::copy(&settings_path, &backup_path)
            .with_context(|| format!("Failed to backup to {}", backup_path.display()))?;

        // Atomic write
        let serialized =
            serde_json::to_string_pretty(&root).context("Failed to serialize settings.json")?;
        atomic_write(&settings_path, &serialized)?;

        if verbose > 0 {
            eprintln!("Removed RTK hook from settings.json");
        }
    }

    Ok(removed)
}

/// Full uninstall: remove hook, RTK.md, @RTK.md reference, settings.json entry
pub fn uninstall(global: bool, codex: bool, dry_run: bool, verbose: u8) -> Result<()> {
    if dry_run {
        println!(
            "[dry-run] would uninstall RTK artifacts for {}",
            if codex { "Codex CLI" } else { "Claude Code" }
        );
        println!("[dry-run] Nothing written.");
        return Ok(());
    }
    if codex {
        return uninstall_codex(global, verbose);
    }
    if !global {
        anyhow::bail!("Uninstall only works with --global flag. For local projects, manually remove RTK from CLAUDE.md");
    }

    let claude_dir = resolve_claude_dir()?;
    let mut removed = Vec::new();

    // 1. Remove the current hook and obsolete hook files from older installs.
    for hook_name in std::iter::once(&"rtk-rewrite.sh").chain(LEGACY_HOOK_FILES.iter()) {
        let hook_path = claude_dir.join("hooks").join(hook_name);
        if hook_path.exists() {
            fs::remove_file(&hook_path)
                .with_context(|| format!("Failed to remove hook: {}", hook_path.display()))?;
            removed.push(format!("Hook: {}", hook_path.display()));
        }
    }

    // 2. Remove RTK.md
    let rtk_md_path = claude_dir.join("RTK.md");
    if rtk_md_path.exists() {
        fs::remove_file(&rtk_md_path)
            .with_context(|| format!("Failed to remove RTK.md: {}", rtk_md_path.display()))?;
        removed.push(format!("RTK.md: {}", rtk_md_path.display()));
    }

    // 3. Remove @RTK.md reference from CLAUDE.md
    let claude_md_path = claude_dir.join("CLAUDE.md");
    if claude_md_path.exists() {
        let content = fs::read_to_string(&claude_md_path)
            .with_context(|| format!("Failed to read CLAUDE.md: {}", claude_md_path.display()))?;

        if content.contains("@RTK.md") {
            let new_content = content
                .lines()
                .filter(|line| !line.trim().starts_with("@RTK.md"))
                .collect::<Vec<_>>()
                .join("\n");

            // Clean up double blanks
            let cleaned = clean_double_blanks(&new_content);

            fs::write(&claude_md_path, cleaned).with_context(|| {
                format!("Failed to write CLAUDE.md: {}", claude_md_path.display())
            })?;
            removed.push("CLAUDE.md: removed @RTK.md reference".to_string());
        }
    }

    // 4. Remove hook entry from settings.json
    if remove_hook_from_settings(verbose)? {
        removed.push("settings.json: removed RTK hook entry".to_string());
    }

    // Report results
    if removed.is_empty() {
        println!("RTK was not installed (nothing to remove)");
    } else {
        println!("RTK uninstalled:");
        for item in removed {
            println!("  - {}", item);
        }
        println!("\nRestart Claude Code to apply changes.");
    }

    Ok(())
}

fn uninstall_codex(global: bool, verbose: u8) -> Result<()> {
    if !global {
        anyhow::bail!(
            "Uninstall only works with --global flag. For local projects, manually remove RTK from AGENTS.md"
        );
    }
    let codex_dir = resolve_codex_dir()?;
    let rtk_md_path = codex_dir.join("RTK.md");
    let agents_md_path = codex_dir.join("AGENTS.md");
    let mut removed = Vec::new();
    if rtk_md_path.exists() {
        fs::remove_file(&rtk_md_path)?;
        removed.push(format!("RTK.md: {}", rtk_md_path.display()));
    }
    if remove_rtk_reference_from_agents(&agents_md_path, verbose)? {
        removed.push("AGENTS.md: removed @RTK.md reference".to_string());
    }
    if removed.is_empty() {
        println!("RTK was not installed for Codex CLI (nothing to remove)");
    } else {
        println!("RTK uninstalled for Codex CLI:");
        for item in removed {
            println!("  - {}", item);
        }
    }
    Ok(())
}

fn patch_settings_json(rewrite_path: &Path, mode: PatchMode, verbose: u8) -> Result<PatchResult> {
    let claude_dir = resolve_claude_dir()?;
    let settings_path = claude_dir.join("settings.json");
    let rewrite_command = rewrite_path
        .to_str()
        .context("Rewrite hook path contains invalid UTF-8")?;

    // Read or create settings.json
    let mut root = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)
            .with_context(|| format!("Failed to read {}", settings_path.display()))?;

        if content.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse {} as JSON", settings_path.display()))?
        }
    } else {
        serde_json::json!({})
    };

    if hooks_already_present(&root, rewrite_command) {
        if verbose > 0 {
            eprintln!("settings.json: all hooks already present");
        }
        return Ok(PatchResult::AlreadyPresent);
    }

    // Handle mode
    match mode {
        PatchMode::Skip => {
            print_manual_instructions(rewrite_path);
            return Ok(PatchResult::Skipped);
        }
        PatchMode::Ask => {
            if !prompt_user_consent(&settings_path)? {
                print_manual_instructions(rewrite_path);
                return Ok(PatchResult::Declined);
            }
        }
        PatchMode::Auto => {
            // Proceed without prompting
        }
    }

    // Remove any existing RTK entries first (clean slate for idempotent re-insert)
    remove_hook_from_json(&mut root);

    insert_hook_entry(&mut root, rewrite_command);

    // Backup original
    if settings_path.exists() {
        let backup_path = settings_path.with_extension("json.bak");
        fs::copy(&settings_path, &backup_path)
            .with_context(|| format!("Failed to backup to {}", backup_path.display()))?;
        if verbose > 0 {
            eprintln!("Backup: {}", backup_path.display());
        }
    }

    // Atomic write
    let serialized =
        serde_json::to_string_pretty(&root).context("Failed to serialize settings.json")?;
    atomic_write(&settings_path, &serialized)?;

    println!("\n  settings.json: Bash rewrite hook registered");
    if settings_path.with_extension("json.bak").exists() {
        println!(
            "  Backup: {}",
            settings_path.with_extension("json.bak").display()
        );
    }
    println!("  Restart Claude Code. Test with: git status");

    Ok(PatchResult::Patched)
}

/// Clean up consecutive blank lines (collapse 3+ to 2)
/// Used when removing @RTK.md line from CLAUDE.md
fn clean_double_blanks(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut result = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        if line.trim().is_empty() {
            // Count consecutive blank lines
            let mut blank_count = 0;
            while i < lines.len() && lines[i].trim().is_empty() {
                blank_count += 1;
                i += 1;
            }

            // Keep at most 2 blank lines
            let keep = blank_count.min(2);
            for _ in 0..keep {
                result.push("");
            }
        } else {
            result.push(line);
            i += 1;
        }
    }

    result.join("\n")
}

/// Register only the Bash rewrite hook. Native agent tools stay under the host's
/// own policy and are not routed through shell subprocesses.
fn insert_hook_entry(root: &mut serde_json::Value, rewrite_command: &str) {
    if !root.is_object() {
        *root = serde_json::json!({});
    }

    let hooks = root
        .as_object_mut()
        .expect("root was normalized to an object")
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }

    let pre_tool_use = hooks
        .as_object_mut()
        .expect("hooks was normalized to an object")
        .entry("PreToolUse")
        .or_insert_with(|| serde_json::json!([]));
    if !pre_tool_use.is_array() {
        *pre_tool_use = serde_json::json!([]);
    }

    pre_tool_use
        .as_array_mut()
        .expect("PreToolUse was normalized to an array")
        .push(serde_json::json!({
            "matcher": "Bash",
            "hooks": [{
                "type": "command",
                "command": rewrite_command,
                "timeout": 5
            }]
        }));
}

fn rtk_hook_state(root: &serde_json::Value, rewrite_command: &str) -> (bool, bool) {
    let Some(entries) = root
        .get("hooks")
        .and_then(|hooks| hooks.get("PreToolUse"))
        .and_then(serde_json::Value::as_array)
    else {
        return (false, false);
    };

    let mut has_rewrite = false;
    let mut has_legacy = false;
    for command in entries
        .iter()
        .filter_map(|entry| entry.get("hooks"))
        .filter_map(serde_json::Value::as_array)
        .flatten()
        .filter_map(|hook| hook.get("command"))
        .filter_map(serde_json::Value::as_str)
    {
        has_rewrite |= command == rewrite_command || command.contains("rtk-rewrite.sh");
        has_legacy |= LEGACY_HOOK_FILES.iter().any(|name| command.contains(name));
    }

    (has_rewrite, has_legacy)
}

fn hooks_already_present(root: &serde_json::Value, rewrite_command: &str) -> bool {
    matches!(rtk_hook_state(root, rewrite_command), (true, false))
}
/// Default mode: hook + slim RTK.md + @RTK.md reference
#[cfg(not(unix))]
fn run_default_mode(_global: bool, _patch_mode: PatchMode, _verbose: u8) -> Result<()> {
    eprintln!("⚠️  Hook-based mode requires Unix (macOS/Linux).");
    eprintln!("    Windows: use --claude-md mode for full injection.");
    eprintln!("    Falling back to --claude-md mode.");
    run_claude_md_mode(_global, _verbose)
}

#[cfg(unix)]
fn run_default_mode(global: bool, patch_mode: PatchMode, verbose: u8) -> Result<()> {
    if !global {
        // Local init: unchanged behavior (full injection into ./CLAUDE.md)
        return run_claude_md_mode(false, verbose);
    }

    let claude_dir = resolve_claude_dir()?;
    let rtk_md_path = claude_dir.join("RTK.md");
    let claude_md_path = claude_dir.join("CLAUDE.md");

    let rewrite_path = prepare_hook_path()?;
    ensure_hook_installed(&rewrite_path, verbose)?;
    remove_legacy_hook_files(&rewrite_path, verbose)?;

    // 2. Write RTK.md
    write_if_changed(&rtk_md_path, RTK_SLIM, "RTK.md", verbose)?;

    // 3. Patch CLAUDE.md (add @RTK.md, migrate if needed)
    let migrated = patch_claude_md(&claude_md_path, verbose)?;

    // 4. Print success message
    println!("\nRTK hook installed (global).\n");
    println!("  Rewrite:   {}", rewrite_path.display());
    println!(
        "  RTK.md:    {} ({} lines)",
        rtk_md_path.display(),
        RTK_SLIM.lines().count()
    );
    println!("  CLAUDE.md:  @RTK.md reference added");

    if migrated {
        println!("\n  Migrated: replaced the inline RTK block with @RTK.md");
    }

    let patch_result = patch_settings_json(&rewrite_path, patch_mode, verbose)?;

    // Report result
    match patch_result {
        PatchResult::Patched => {
            // Already printed by patch_settings_json
        }
        PatchResult::AlreadyPresent => {
            println!("\n  settings.json: rewrite hook already present");
            println!("  Restart Claude Code. Test with: git status");
        }
        PatchResult::Declined | PatchResult::Skipped => {
            // Manual instructions already printed by patch_settings_json
        }
    }

    // Remove project-local duplicates after the global hook is registered.
    cleanup_project_local_hooks(verbose)?;

    setup_grepai(patch_mode, verbose)?;

    println!(); // Final newline

    Ok(())
}

/// Offer grepai installation during `rtk init --global`
fn setup_grepai(patch_mode: PatchMode, verbose: u8) -> Result<()> {
    if std::env::var("RTK_SKIP_GREPAI").ok().as_deref() == Some("1") {
        if verbose > 0 {
            eprintln!("grepai setup skipped (RTK_SKIP_GREPAI=1)");
        }
        return Ok(());
    }

    // Check if grepai is already installed
    if let Some(path) = grepai::find_grepai_binary() {
        println!("\n  grepai: already installed at {}", path.display());
        return Ok(());
    }

    // Not installed — decide based on patch_mode
    match patch_mode {
        PatchMode::Auto => {
            // Install without prompting
            println!("\n  Installing grepai...");
            match grepai::install_grepai(verbose) {
                Ok(path) => {
                    println!("  grepai installed: {}", path.display());
                    println!(
                        "  Run `grepai init` in any project, then `grepai watch --background`."
                    );
                }
                Err(e) => {
                    eprintln!("  grepai install failed: {}", e);
                    eprintln!("  Install manually: https://github.com/yoanbernabeu/grepai");
                }
            }
        }
        PatchMode::Skip => {
            // Print manual instructions only
            println!("\n  grepai: not installed (skipped)");
            println!("  Install manually: https://github.com/yoanbernabeu/grepai");
        }
        PatchMode::Ask => {
            // Prompt with Y as default (capital Y, unlike settings.json which defaults to N)
            if prompt_grepai_consent()? {
                println!("  Installing grepai...");
                match grepai::install_grepai(verbose) {
                    Ok(path) => {
                        println!("  grepai installed: {}", path.display());
                        println!(
                            "  Run `grepai init` in any project, then `grepai watch --background`."
                        );
                    }
                    Err(e) => {
                        eprintln!("  grepai install failed: {}", e);
                        eprintln!("  Install manually: https://github.com/yoanbernabeu/grepai");
                    }
                }
            } else {
                println!("  grepai: skipped");
                println!("  Install later: https://github.com/yoanbernabeu/grepai");
            }
        }
    }

    Ok(())
}

/// Prompt user for consent to install grepai
/// Default is Yes (capital Y) — unlike settings.json patch which defaults to No
fn prompt_grepai_consent() -> Result<bool> {
    use std::io::{self, BufRead, IsTerminal};

    eprintln!("\nInstall grepai for semantic code search? [Y/n] ");

    // If stdin is not a terminal (piped), default to Yes
    if !io::stdin().is_terminal() {
        eprintln!("(non-interactive mode, defaulting to Y)");
        return Ok(true);
    }

    let stdin = io::stdin();
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .context("Failed to read user input")?;

    let response = line.trim().to_lowercase();
    // Default is Yes: empty input or explicit y/yes
    Ok(response.is_empty() || response == "y" || response == "yes")
}

/// Hook-only mode: just the hook, no RTK.md
#[cfg(not(unix))]
fn run_hook_only_mode(_global: bool, _patch_mode: PatchMode, _verbose: u8) -> Result<()> {
    anyhow::bail!("Hook install requires Unix (macOS/Linux). Use WSL or --claude-md mode.")
}

#[cfg(unix)]
fn run_hook_only_mode(global: bool, patch_mode: PatchMode, verbose: u8) -> Result<()> {
    if !global {
        eprintln!("Warning: --hook-only only makes sense with --global");
        eprintln!("    For local projects, use default mode or --claude-md");
        return Ok(());
    }

    let rewrite_path = prepare_hook_path()?;
    ensure_hook_installed(&rewrite_path, verbose)?;
    remove_legacy_hook_files(&rewrite_path, verbose)?;

    println!("\nRTK hook installed (hook-only mode).\n");
    println!("  Rewrite: {}", rewrite_path.display());
    println!(
        "  Note: No RTK.md created. Claude won't know about meta commands (gain, discover, proxy)."
    );

    let patch_result = patch_settings_json(&rewrite_path, patch_mode, verbose)?;

    // Report result
    match patch_result {
        PatchResult::Patched => {
            // Already printed by patch_settings_json
        }
        PatchResult::AlreadyPresent => {
            println!("\n  settings.json: rewrite hook already present");
            println!("  Restart Claude Code. Test with: git status");
        }
        PatchResult::Declined | PatchResult::Skipped => {
            // Manual instructions already printed by patch_settings_json
        }
    }

    println!(); // Final newline

    Ok(())
}

/// Legacy mode: concise inline instructions in CLAUDE.md.
fn run_claude_md_mode(global: bool, verbose: u8) -> Result<()> {
    let path = if global {
        resolve_claude_dir()?.join("CLAUDE.md")
    } else {
        PathBuf::from("CLAUDE.md")
    };

    if global {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
    }

    if verbose > 0 {
        eprintln!("Writing rtk instructions to: {}", path.display());
    }

    if path.exists() {
        let existing = fs::read_to_string(&path)?;
        // upsert_rtk_block handles all 4 cases: add, update, unchanged, malformed
        let (new_content, action) = upsert_rtk_block(&existing, RTK_INSTRUCTIONS);

        match action {
            RtkBlockUpsert::Added => {
                atomic_write(&path, &new_content)?;
                println!("[ok] Added rtk instructions to {}", path.display());
            }
            RtkBlockUpsert::Updated => {
                atomic_write(&path, &new_content)?;
                println!("[ok] Updated rtk instructions in {}", path.display());
            }
            RtkBlockUpsert::Unchanged => {
                println!(
                    "[ok] {} already contains up-to-date rtk instructions",
                    path.display()
                );
                return Ok(());
            }
            RtkBlockUpsert::Malformed => {
                eprintln!(
                    "[warn] Found '<!-- rtk-instructions' without closing marker in {}",
                    path.display()
                );

                if let Some((line_num, _)) = existing
                    .lines()
                    .enumerate()
                    .find(|(_, line)| line.contains("<!-- rtk-instructions"))
                {
                    eprintln!("    Location: line {}", line_num + 1);
                }

                eprintln!("    Action: Manually remove the incomplete block, then re-run:");
                if global {
                    eprintln!("            rtk init -g --claude-md");
                } else {
                    eprintln!("            rtk init --claude-md");
                }
                return Ok(());
            }
        }
    } else {
        atomic_write(&path, RTK_INSTRUCTIONS)?;
        println!("[ok] Created {} with rtk instructions", path.display());
    }

    if global {
        println!("   Claude Code will now use rtk in all sessions");
    } else {
        println!("   Claude Code will use rtk in this project");
    }

    Ok(())
}

fn run_codex_mode(global: bool, verbose: u8) -> Result<()> {
    let (agents_md_path, rtk_md_path) = if global {
        let codex_dir = resolve_codex_dir()?;
        (codex_dir.join("AGENTS.md"), codex_dir.join("RTK.md"))
    } else {
        (PathBuf::from("AGENTS.md"), PathBuf::from("RTK.md"))
    };

    if let Some(parent) = agents_md_path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_if_changed(&rtk_md_path, RTK_SLIM_CODEX, "RTK.md", verbose)?;
    let added_ref = patch_agents_md(&agents_md_path, verbose)?;

    println!("\nRTK configured for Codex CLI.\n");
    println!("  RTK.md:    {}", rtk_md_path.display());
    println!(
        "  AGENTS.md: @RTK.md reference {}",
        if added_ref {
            "added"
        } else {
            "already present"
        }
    );
    Ok(())
}

fn preview_codex_mode(global: bool, verbose: u8) -> Result<()> {
    let base = if global {
        resolve_codex_dir()?
    } else {
        std::env::current_dir()?
    };
    println!(
        "[dry-run] would create or update RTK.md: {}",
        base.join("RTK.md").display()
    );
    println!(
        "[dry-run] would add @RTK.md to {}",
        base.join("AGENTS.md").display()
    );
    if verbose > 0 {
        println!("[dry-run] content:\n{}", RTK_SLIM_CODEX);
    }
    println!("[dry-run] Nothing written.");
    Ok(())
}

fn preview_claude_mode(
    global: bool,
    claude_md: bool,
    hook_only: bool,
    patch_mode: PatchMode,
    verbose: u8,
) -> Result<()> {
    if !global {
        let target = if claude_md {
            "CLAUDE.md"
        } else {
            ".rtk/filters.toml"
        };
        println!("[dry-run] would create or update {}", target);
    } else {
        let claude_dir = resolve_claude_dir()?;
        println!(
            "[dry-run] would install rewrite hook: {}",
            claude_dir.join("hooks/rtk-rewrite.sh").display()
        );
        if !hook_only {
            println!(
                "[dry-run] would create or update RTK.md: {}",
                claude_dir.join("RTK.md").display()
            );
        }
        if patch_mode != PatchMode::Skip {
            println!(
                "[dry-run] would patch settings.json: {}",
                claude_dir.join("settings.json").display()
            );
        }
    }
    if verbose > 0 && claude_md {
        println!("[dry-run] content:\n{}", RTK_INSTRUCTIONS);
    }
    println!("[dry-run] Nothing written.");
    Ok(())
}

fn patch_agents_md(path: &Path, verbose: u8) -> Result<bool> {
    let content = if path.exists() {
        fs::read_to_string(path)
            .with_context(|| format!("Failed to read AGENTS.md: {}", path.display()))?
    } else {
        String::new()
    };
    if content.lines().any(|line| line.trim() == "@RTK.md") {
        return Ok(false);
    }
    let new_content = if content.trim().is_empty() {
        "@RTK.md\n".to_string()
    } else {
        format!("{}\n\n@RTK.md\n", content.trim_end())
    };
    atomic_write(path, &new_content)
        .with_context(|| format!("Failed to write AGENTS.md: {}", path.display()))?;
    if verbose > 0 {
        eprintln!("Added @RTK.md reference to AGENTS.md");
    }
    Ok(true)
}

fn remove_rtk_reference_from_agents(path: &Path, verbose: u8) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = fs::read_to_string(path)?;
    if !content.lines().any(|line| line.trim() == "@RTK.md") {
        return Ok(false);
    }
    let new_content = content
        .lines()
        .filter(|line| line.trim() != "@RTK.md")
        .collect::<Vec<_>>()
        .join("\n");
    atomic_write(path, &clean_double_blanks(&new_content))?;
    if verbose > 0 {
        eprintln!(
            "Removed @RTK.md reference from AGENTS.md: {}",
            path.display()
        );
    }
    Ok(true)
}

// --- upsert_rtk_block: idempotent RTK block management ---

#[derive(Debug, Clone, Copy, PartialEq)]
enum RtkBlockUpsert {
    /// No existing block found — appended new block
    Added,
    /// Existing block found with different content — replaced
    Updated,
    /// Existing block found with identical content — no-op
    Unchanged,
    /// Opening marker found without closing marker — not safe to rewrite
    Malformed,
}

/// Insert or replace the RTK instructions block in `content`.
///
/// Returns `(new_content, action)` describing what happened.
/// The caller decides whether to write `new_content` based on `action`.
fn upsert_rtk_block(content: &str, block: &str) -> (String, RtkBlockUpsert) {
    let start_marker = "<!-- rtk-instructions";
    let end_marker = "<!-- /rtk-instructions -->";

    if let Some(start) = content.find(start_marker) {
        if let Some(relative_end) = content[start..].find(end_marker) {
            let end = start + relative_end;
            let end_pos = end + end_marker.len();
            let current_block = content[start..end_pos].trim();
            let desired_block = block.trim();

            if current_block == desired_block {
                return (content.to_string(), RtkBlockUpsert::Unchanged);
            }

            // Replace stale block with desired block
            let before = content[..start].trim_end();
            let after = content[end_pos..].trim_start();

            let result = match (before.is_empty(), after.is_empty()) {
                (true, true) => desired_block.to_string(),
                (true, false) => format!("{desired_block}\n\n{after}"),
                (false, true) => format!("{before}\n\n{desired_block}"),
                (false, false) => format!("{before}\n\n{desired_block}\n\n{after}"),
            };

            return (result, RtkBlockUpsert::Updated);
        }

        // Opening marker without closing marker — malformed
        return (content.to_string(), RtkBlockUpsert::Malformed);
    }

    // No existing block — append
    let trimmed = content.trim();
    if trimmed.is_empty() {
        (block.to_string(), RtkBlockUpsert::Added)
    } else {
        (
            format!("{trimmed}\n\n{}", block.trim()),
            RtkBlockUpsert::Added,
        )
    }
}

/// Patch CLAUDE.md: add @RTK.md, migrate if old block exists
fn patch_claude_md(path: &Path, verbose: u8) -> Result<bool> {
    let mut content = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    let mut migrated = false;

    // Check for old block and migrate
    if content.contains("<!-- rtk-instructions") {
        let (new_content, did_migrate) = remove_rtk_block(&content);
        if did_migrate {
            content = new_content;
            migrated = true;
            if verbose > 0 {
                eprintln!("Migrated: removed old RTK block from CLAUDE.md");
            }
        }
    }

    // Check if @RTK.md already present
    if content.contains("@RTK.md") {
        if verbose > 0 {
            eprintln!("@RTK.md reference already present in CLAUDE.md");
        }
        if migrated {
            fs::write(path, content)?;
        }
        return Ok(migrated);
    }

    // Add @RTK.md
    let new_content = if content.is_empty() {
        "@RTK.md\n".to_string()
    } else {
        format!("{}\n\n@RTK.md\n", content.trim())
    };

    fs::write(path, new_content)?;

    if verbose > 0 {
        eprintln!("Added @RTK.md reference to CLAUDE.md");
    }

    Ok(migrated)
}

/// Remove old RTK block from CLAUDE.md (migration helper)
fn remove_rtk_block(content: &str) -> (String, bool) {
    if let (Some(start), Some(end)) = (
        content.find("<!-- rtk-instructions"),
        content.find("<!-- /rtk-instructions -->"),
    ) {
        let end_pos = end + "<!-- /rtk-instructions -->".len();
        let before = content[..start].trim_end();
        let after = content[end_pos..].trim_start();

        let result = if after.is_empty() {
            before.to_string()
        } else {
            format!("{}\n\n{}", before, after)
        };

        (result, true) // migrated
    } else if content.contains("<!-- rtk-instructions") {
        eprintln!("⚠️  Warning: Found '<!-- rtk-instructions' without closing marker.");
        eprintln!("    This can happen if CLAUDE.md was manually edited.");

        // Find line number
        if let Some((line_num, _)) = content
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains("<!-- rtk-instructions"))
        {
            eprintln!("    Location: line {}", line_num + 1);
        }

        eprintln!("    Action: Manually remove the incomplete block, then re-run:");
        eprintln!("            rtk init -g");
        (content.to_string(), false)
    } else {
        (content.to_string(), false)
    }
}

fn resolve_claude_dir() -> Result<PathBuf> {
    resolve_claude_dir_from(
        std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
        dirs::home_dir(),
    )
}

fn resolve_codex_dir() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|home| home.join(".codex"))
        .context("Cannot determine home directory. Is $HOME set?")
}

fn resolve_claude_dir_from(
    config_dir: Option<PathBuf>,
    home_dir: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(path) = config_dir.filter(|path| !path.as_os_str().is_empty()) {
        return Ok(path);
    }

    home_dir
        .map(|home| home.join(".claude"))
        .context("Cannot determine Claude config directory. Set $CLAUDE_CONFIG_DIR or $HOME.")
}

/// Clean up project-local RTK hook duplicates when running `rtk init -g`
/// Removes .claude/hooks/rtk-*.sh files and hook entries from local settings
fn cleanup_project_local_hooks(verbose: u8) -> Result<bool> {
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    let local_claude = cwd.join(".claude");

    if !local_claude.exists() {
        return Ok(false);
    }

    let mut cleaned = Vec::new();

    // 1. Remove local hook script files
    let local_hooks_dir = local_claude.join("hooks");
    if local_hooks_dir.exists() {
        for hook_name in std::iter::once(&"rtk-rewrite.sh").chain(LEGACY_HOOK_FILES.iter()) {
            let local_hook = local_hooks_dir.join(hook_name);
            if local_hook.exists() {
                fs::remove_file(&local_hook).with_context(|| {
                    format!("Failed to remove local hook: {}", local_hook.display())
                })?;
                cleaned.push(format!("removed {}", local_hook.display()));
            }
        }
    }

    // 2. Remove hook entries from local settings files
    for settings_name in &["settings.json", "settings.local.json"] {
        let settings_path = local_claude.join(settings_name);
        if !settings_path.exists() {
            continue;
        }

        let content = fs::read_to_string(&settings_path)
            .with_context(|| format!("Failed to read local {}", settings_path.display()))?;

        if content.trim().is_empty() {
            continue;
        }

        let mut root: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue, // skip malformed JSON
        };

        if remove_hook_from_json(&mut root) {
            // Backup before modifying
            let backup_path = settings_path.with_extension("json.bak");
            fs::copy(&settings_path, &backup_path).ok(); // best-effort backup

            let serialized = serde_json::to_string_pretty(&root)
                .context("Failed to serialize local settings")?;
            fs::write(&settings_path, serialized)
                .with_context(|| format!("Failed to write local {}", settings_path.display()))?;
            cleaned.push(format!("cleaned RTK hooks from {}", settings_name));
        }
    }

    if !cleaned.is_empty() {
        println!("\n  Project-local cleanup:");
        for item in &cleaned {
            println!("    - {}", item);
        }
        if verbose > 0 {
            eprintln!("Cleaned {} project-local RTK artifacts", cleaned.len());
        }
    }

    Ok(!cleaned.is_empty())
}

/// Show current rtk configuration
pub fn show_config(codex: bool) -> Result<()> {
    if codex {
        return show_codex_config();
    }
    let claude_dir = resolve_claude_dir()?;
    let rewrite_path = claude_dir.join("hooks").join("rtk-rewrite.sh");
    let rtk_md_path = claude_dir.join("RTK.md");
    let global_claude_md = claude_dir.join("CLAUDE.md");
    let local_claude_md = PathBuf::from("CLAUDE.md");

    println!("rtk Configuration:\n");

    // Check rewrite hook
    if rewrite_path.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = fs::metadata(&rewrite_path)?;
            let perms = metadata.permissions();
            let is_executable = perms.mode() & 0o111 != 0;

            let hook_content = fs::read_to_string(&rewrite_path)?;
            let has_guards =
                hook_content.contains("command -v rtk") && hook_content.contains("command -v jq");

            if is_executable && has_guards {
                println!(
                    "  [ok] Rewrite hook: {} (executable, with guards)",
                    rewrite_path.display()
                );
            } else if !is_executable {
                println!(
                    "  [!]  Rewrite hook: {} (NOT executable - run: chmod +x)",
                    rewrite_path.display()
                );
            } else {
                println!(
                    "  [!]  Rewrite hook: {} (no guards - outdated)",
                    rewrite_path.display()
                );
            }
        }

        #[cfg(not(unix))]
        {
            println!("  [ok] Rewrite hook: {} (exists)", rewrite_path.display());
        }
    } else {
        println!("  [--] Rewrite hook: not found");
    }

    // Check RTK.md
    if rtk_md_path.exists() {
        println!("  [ok] RTK.md: {} (slim mode)", rtk_md_path.display());
    } else {
        println!("  [--] RTK.md: not found");
    }

    // Check global CLAUDE.md
    if global_claude_md.exists() {
        let content = fs::read_to_string(&global_claude_md)?;
        if content.contains("@RTK.md") {
            println!("  [ok] Global (~/.claude/CLAUDE.md): @RTK.md reference");
        } else if content.contains("<!-- rtk-instructions") {
            println!(
                "  [!]  Global (~/.claude/CLAUDE.md): old RTK block (run: rtk init -g to migrate)"
            );
        } else {
            println!("  [--] Global (~/.claude/CLAUDE.md): exists but rtk not configured");
        }
    } else {
        println!("  [--] Global (~/.claude/CLAUDE.md): not found");
    }

    // Check local CLAUDE.md
    if local_claude_md.exists() {
        let content = fs::read_to_string(&local_claude_md)?;
        if content.contains("rtk") {
            println!("  [ok] Local (./CLAUDE.md): rtk enabled");
        } else {
            println!("  [--] Local (./CLAUDE.md): exists but rtk not configured");
        }
    } else {
        println!("  [--] Local (./CLAUDE.md): not found");
    }

    // Check settings.json
    let settings_path = claude_dir.join("settings.json");
    if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)?;
        if !content.trim().is_empty() {
            if let Ok(root) = serde_json::from_str::<serde_json::Value>(&content) {
                let rewrite_command = rewrite_path.to_string_lossy();
                match rtk_hook_state(&root, &rewrite_command) {
                    (true, false) => println!("  [ok] settings.json: Bash rewrite hook configured"),
                    (true, true) => {
                        println!("  [!]  settings.json: obsolete native-tool blockers found");
                        println!("       Run: rtk init -g --auto-patch");
                    }
                    (false, _) => {
                        println!("  [!]  settings.json: rewrite hook missing");
                        println!("       Run: rtk init -g --auto-patch");
                    }
                }
            } else {
                println!("  [!]  settings.json: exists but invalid JSON");
            }
        } else {
            println!("  [--] settings.json: empty");
        }
    } else {
        println!("  [--] settings.json: not found");
    }

    // Check for project-local hook duplicates
    let cwd = std::env::current_dir().ok();
    if let Some(ref cwd) = cwd {
        let local_hooks_dir = cwd.join(".claude").join("hooks");
        let mut local_dupes = Vec::new();
        for hook_name in std::iter::once(&"rtk-rewrite.sh").chain(LEGACY_HOOK_FILES.iter()) {
            if local_hooks_dir.join(hook_name).exists() {
                local_dupes.push(*hook_name);
            }
        }
        if !local_dupes.is_empty() {
            println!(
                "\n  [!]  Project-local hook duplicates found: {}",
                local_dupes.join(", ")
            );
            println!("       Run `rtk init -g` to clean up (global hooks take precedence)");
        }
    }

    println!("\nSearch: rtk rgai for intent; rtk rg or rtk grep for exact matching.");
    println!("  rg and grep retain their own flags and regex semantics.\n");
    println!("Usage:");
    println!("  rtk init              # Full injection into local CLAUDE.md");
    println!("  rtk init -g           # Hook + RTK.md + @RTK.md + settings.json (recommended)");
    println!("  rtk init -g --auto-patch    # Same as above but no prompt");
    println!("  rtk init -g --no-patch      # Skip settings.json (manual setup)");
    println!("  rtk init -g --uninstall     # Remove all RTK artifacts");
    println!("  rtk init -g --claude-md     # Legacy: full injection into ~/.claude/CLAUDE.md");
    println!("  rtk init -g --hook-only     # Hook only, no RTK.md");

    Ok(())
}

fn show_codex_config() -> Result<()> {
    let codex_dir = resolve_codex_dir()?;
    let global_agents = codex_dir.join("AGENTS.md");
    let global_rtk = codex_dir.join("RTK.md");
    let local_agents = PathBuf::from("AGENTS.md");
    let local_rtk = PathBuf::from("RTK.md");
    println!("rtk Configuration (Codex CLI):\n");
    for (label, path) in [("Global RTK.md", global_rtk), ("Local RTK.md", local_rtk)] {
        println!(
            "  [{}] {}: {}",
            if path.exists() { "ok" } else { "--" },
            label,
            path.display()
        );
    }
    for (label, path) in [
        ("Global AGENTS.md", global_agents),
        ("Local AGENTS.md", local_agents),
    ] {
        let configured = fs::read_to_string(&path)
            .map(|content| content.lines().any(|line| line.trim() == "@RTK.md"))
            .unwrap_or(false);
        println!(
            "  [{}] {}: {}",
            if configured { "ok" } else { "--" },
            label,
            path.display()
        );
    }
    println!("\nUsage:");
    println!("  rtk init --codex");
    println!("  rtk init -g --codex");
    println!("  rtk init -g --codex --uninstall");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_templates_are_concise_and_current() {
        assert!(RTK_INSTRUCTIONS.contains("<!-- rtk-instructions v4 -->"));
        assert!(RTK_INSTRUCTIONS.contains("rtk rg <pattern>"));
        assert!(RTK_INSTRUCTIONS.contains("rtk grep <pattern>"));
        assert!(RTK_INSTRUCTIONS.contains("--from <N> --to <M>"));
        assert!(RTK_INSTRUCTIONS.contains("native editing tools are still valid"));
        assert!(RTK_INSTRUCTIONS.lines().count() <= 60);
        assert!(RTK_SLIM.lines().count() <= 50);
        assert!(!RTK_SLIM.contains("internal rg -> grep fallback"));
        assert!(!RTK_SLIM.contains("0 tokens overhead"));
    }

    #[test]
    fn test_claude_config_dir_takes_precedence() {
        let configured = PathBuf::from("/tmp/custom-claude");
        let home = PathBuf::from("/tmp/home");
        assert_eq!(
            resolve_claude_dir_from(Some(configured.clone()), Some(home)).unwrap(),
            configured
        );
    }

    #[test]
    fn test_claude_config_dir_falls_back_to_home() {
        let home = PathBuf::from("/tmp/home");
        assert_eq!(
            resolve_claude_dir_from(None, Some(home.clone())).unwrap(),
            home.join(".claude")
        );
    }

    #[test]
    fn test_hook_delegates_without_bypassing_host_permissions() {
        assert!(REWRITE_HOOK.contains("command -v rtk"));
        assert!(REWRITE_HOOK.contains("command -v jq"));
        assert!(REWRITE_HOOK.contains("rtk rewrite \"$CMD\""));
        assert!(REWRITE_HOOK.contains("updatedInput"));
        assert!(!REWRITE_HOOK.contains("permissionDecision\""));
        assert!(!REWRITE_HOOK.contains("^ssh"));
    }

    #[test]
    fn test_migration_removes_old_block() {
        let input =
            "# My Config\n\n<!-- rtk-instructions v2 -->\nOLD\n<!-- /rtk-instructions -->\n\nMore";
        let (result, migrated) = remove_rtk_block(input);
        assert!(migrated);
        assert!(!result.contains("OLD"));
        assert!(result.contains("# My Config"));
        assert!(result.contains("More"));
    }

    #[test]
    fn test_migration_rejects_missing_end_marker() {
        let input = "<!-- rtk-instructions v2 -->\npartial";
        let (result, migrated) = remove_rtk_block(input);
        assert!(!migrated);
        assert_eq!(result, input);
    }

    #[test]
    fn test_upsert_rtk_block_updates_stale_content() {
        let input =
            "# Team\n\n<!-- rtk-instructions v1 -->\nOLD\n<!-- /rtk-instructions -->\n\nMore";
        let (content, action) = upsert_rtk_block(input, RTK_INSTRUCTIONS);
        assert_eq!(action, RtkBlockUpsert::Updated);
        assert!(!content.contains("OLD"));
        assert!(content.contains("<!-- rtk-instructions v4 -->"));
        assert!(content.contains("# Team"));
        assert!(content.contains("More"));
    }

    #[test]
    fn test_upsert_rtk_block_is_idempotent() {
        let input = format!("# Team\n\n{}\n\nMore\n", RTK_INSTRUCTIONS);
        let (content, action) = upsert_rtk_block(&input, RTK_INSTRUCTIONS);
        assert_eq!(action, RtkBlockUpsert::Unchanged);
        assert_eq!(content, input);
    }

    #[test]
    fn test_hook_state_requires_rewrite_without_legacy_blockers() {
        let rewrite = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        let current = serde_json::json!({
            "hooks": {"PreToolUse": [{
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": rewrite}]
            }]}
        });
        assert_eq!(rtk_hook_state(&current, rewrite), (true, false));
        assert!(hooks_already_present(&current, rewrite));

        let legacy = serde_json::json!({
            "hooks": {"PreToolUse": [
                {"matcher": "Bash", "hooks": [{"command": rewrite}]},
                {"matcher": "Read", "hooks": [{"command": "/x/rtk-block-native-read.sh"}]}
            ]}
        });
        assert_eq!(rtk_hook_state(&legacy, rewrite), (true, true));
        assert!(!hooks_already_present(&legacy, rewrite));
    }

    #[test]
    fn test_insert_hook_entry_preserves_existing_hooks() {
        let mut root = serde_json::json!({
            "hooks": {"PreToolUse": [{
                "matcher": "Task",
                "hooks": [{"command": "/x/rtk-mem-context.sh"}]
            }]},
            "model": "claude"
        });
        insert_hook_entry(&mut root, "/x/rtk-rewrite.sh");

        let entries = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["hooks"][0]["command"], "/x/rtk-mem-context.sh");
        assert_eq!(entries[1]["matcher"], "Bash");
        assert_eq!(root["model"], "claude");
    }

    #[test]
    fn test_remove_hook_entries_preserves_non_blocking_rtk_hooks() {
        let mut root = serde_json::json!({
            "hooks": {"PreToolUse": [
                {"matcher": "Bash", "hooks": [{"command": "/x/rtk-rewrite.sh"}]},
                {"matcher": "Task", "hooks": [{"command": "/x/rtk-block-task.sh"}]},
                {"matcher": "Task", "hooks": [{"command": "/x/rtk-mem-context.sh"}]},
                {"matcher": "Bash", "hooks": [{"command": "/x/custom.sh"}]}
            ]}
        });
        assert!(remove_hook_from_json(&mut root));

        let serialized = serde_json::to_string(&root).unwrap();
        assert!(!serialized.contains("rtk-rewrite.sh"));
        assert!(!serialized.contains("rtk-block-task.sh"));
        assert!(serialized.contains("rtk-mem-context.sh"));
        assert!(serialized.contains("custom.sh"));
    }

    #[cfg(unix)]
    #[test]
    fn test_install_single_hook_is_executable_and_idempotent() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let hook = temp.path().join("rtk-rewrite.sh");
        let legacy = temp.path().join("rtk-block-native-read.sh");
        fs::write(&legacy, "legacy").unwrap();
        assert!(install_single_hook(&hook, REWRITE_HOOK, 0).unwrap());
        assert!(!install_single_hook(&hook, REWRITE_HOOK, 0).unwrap());
        assert_eq!(remove_legacy_hook_files(&hook, 0).unwrap(), 1);
        assert!(!legacy.exists());
        assert_ne!(fs::metadata(&hook).unwrap().permissions().mode() & 0o111, 0);
    }

    // Tests for atomic_write()
    #[test]
    fn test_atomic_write() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.json");

        let content = r#"{"key": "value"}"#;
        atomic_write(&file_path, content).unwrap();

        assert!(file_path.exists());
        let written = fs::read_to_string(&file_path).unwrap();
        assert_eq!(written, content);
    }

    #[cfg(unix)]
    #[test]
    fn test_atomic_write_preserves_symlink() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let target_path = temp.path().join("real-settings.json");
        let link_path = temp.path().join("settings.json");

        fs::write(&target_path, "{}").expect("seed target file");
        symlink(&target_path, &link_path).expect("create symlink");

        atomic_write(&link_path, "{\"hooks\":{}}").unwrap();

        assert!(fs::symlink_metadata(&link_path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&target_path).unwrap(), "{\"hooks\":{}}");
    }

    #[cfg(unix)]
    #[test]
    fn test_atomic_write_preserves_relative_symlink() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let target_dir = temp.path().join("real");
        fs::create_dir(&target_dir).unwrap();
        let target_path = target_dir.join("settings.json");
        let link_path = temp.path().join("settings.json");

        fs::write(&target_path, "{}").expect("seed target file");
        symlink(Path::new("real/settings.json"), &link_path).expect("create relative symlink");

        atomic_write(&link_path, "{\"patched\":true}").unwrap();

        assert!(fs::symlink_metadata(&link_path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(&target_path).unwrap(),
            "{\"patched\":true}"
        );
    }

    #[test]
    fn test_write_if_changed_idempotent() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("idempotent.txt");

        let created = write_if_changed(&file_path, "v1", "test", 0).unwrap();
        assert!(created);

        let unchanged = write_if_changed(&file_path, "v1", "test", 0).unwrap();
        assert!(!unchanged);

        let updated = write_if_changed(&file_path, "v2", "test", 0).unwrap();
        assert!(updated);
    }

    // Test for preserve_order round-trip
    #[test]
    fn test_preserve_order_round_trip() {
        let original = r#"{"env": {"PATH": "/usr/bin"}, "permissions": {"allowAll": true}, "model": "claude-sonnet-4"}"#;
        let parsed: serde_json::Value = serde_json::from_str(original).unwrap();
        let serialized = serde_json::to_string(&parsed).unwrap();

        // Just check that keys exist (preserve_order doesn't guarantee exact order in nested objects)
        assert!(serialized.contains("\"env\""));
        assert!(serialized.contains("\"permissions\""));
        assert!(serialized.contains("\"model\""));
    }

    // Tests for clean_double_blanks()
    #[test]
    fn test_clean_double_blanks() {
        // Input: line1, 2 blank lines, line2, 1 blank line, line3, 3 blank lines, line4
        // Expected: line1, 2 blank lines (kept), line2, 1 blank line, line3, 2 blank lines (max), line4
        let input = "line1\n\n\nline2\n\nline3\n\n\n\nline4";
        // That's: line1 \n \n \n line2 \n \n line3 \n \n \n \n line4
        // Which is: line1, blank, blank, line2, blank, line3, blank, blank, blank, line4
        // So 2 blanks after line1 (keep both), 1 blank after line2 (keep), 3 blanks after line3 (keep 2)
        let expected = "line1\n\n\nline2\n\nline3\n\n\nline4";
        assert_eq!(clean_double_blanks(input), expected);
    }

    #[test]
    fn test_clean_double_blanks_preserves_single() {
        let input = "line1\n\nline2\n\nline3";
        assert_eq!(clean_double_blanks(input), input); // No change
    }

    // Tests for remove_hook_from_json()
    #[test]
    fn test_remove_hook_from_json_removes_all_rtk() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {"matcher": "Bash", "hooks": [{"type": "command", "command": "/some/other/hook.sh"}]},
                    {"matcher": "Bash", "hooks": [{"type": "command", "command": "/x/rtk-rewrite.sh"}]},
                    {"matcher": "Grep", "hooks": [{"type": "command", "command": "/x/rtk-block-native-grep.sh"}]},
                    {"matcher": "Read", "hooks": [{"type": "command", "command": "/x/rtk-block-native-read.sh"}]},
                    {"matcher": "Edit", "hooks": [{"type": "command", "command": "/x/rtk-block-native-write.sh"}]},
                    {"matcher": "Write", "hooks": [{"type": "command", "command": "/x/rtk-block-native-write.sh"}]},
                    {"matcher": "Task", "hooks": [{"type": "command", "command": "/x/rtk-block-task.sh"}]}
                ]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(removed);

        // Should have only the non-RTK hook left
        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 1);
        assert_eq!(
            pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap(),
            "/some/other/hook.sh"
        );
    }

    #[test]
    fn test_remove_hook_removes_only_rewrite() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {"matcher": "Bash", "hooks": [{"type": "command", "command": "/x/rtk-rewrite.sh"}]}
                ]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(removed);

        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 0);
    }

    #[test]
    fn test_remove_hook_when_not_present() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{"type": "command", "command": "/some/other/hook.sh"}]
                }]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(!removed);
    }

    // Tests for cleanup_project_local_hooks()
    #[test]
    fn test_cleanup_removes_local_hook_files() {
        let temp = TempDir::new().unwrap();
        let local_hooks = temp.path().join(".claude").join("hooks");
        fs::create_dir_all(&local_hooks).unwrap();

        // Create local hook duplicates
        fs::write(local_hooks.join("rtk-rewrite.sh"), "#!/bin/bash\nold").unwrap();
        fs::write(
            local_hooks.join("rtk-block-native-grep.sh"),
            "#!/bin/bash\nold",
        )
        .unwrap();

        // Verify they exist
        assert!(local_hooks.join("rtk-rewrite.sh").exists());
        assert!(local_hooks.join("rtk-block-native-grep.sh").exists());

        // We can't easily call cleanup_project_local_hooks() because it uses CWD,
        // but we can test the removal logic directly
        for hook_name in &[
            "rtk-rewrite.sh",
            "rtk-block-native-grep.sh",
            "rtk-block-native-read.sh",
            "rtk-block-native-write.sh",
            "rtk-block-task.sh",
        ] {
            let path = local_hooks.join(hook_name);
            if path.exists() {
                fs::remove_file(&path).unwrap();
            }
        }
        assert!(!local_hooks.join("rtk-rewrite.sh").exists());
        assert!(!local_hooks.join("rtk-block-native-grep.sh").exists());
    }

    #[test]
    fn test_cleanup_removes_hook_entries_from_local_settings() {
        // Test that remove_hook_from_json handles all RTK entries in local settings
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {"matcher": "Bash", "hooks": [{"type": "command", "command": ".claude/hooks/rtk-rewrite.sh"}]},
                    {"matcher": "Grep", "hooks": [{"type": "command", "command": ".claude/hooks/rtk-block-native-grep.sh"}]},
                    {"matcher": "Read", "hooks": [{"type": "command", "command": ".claude/hooks/rtk-block-native-read.sh"}]},
                    {"matcher": "Task", "hooks": [{"type": "command", "command": ".claude/hooks/rtk-block-task.sh"}]},
                    {"matcher": "Bash", "hooks": [{"type": "command", "command": "/other/project-hook.sh"}]}
                ]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(removed);

        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 1); // Only non-RTK hook remains
        assert_eq!(
            pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap(),
            "/other/project-hook.sh"
        );
    }
}
