//! HZR-owned instruction blocks inside agent configuration files.
//!
//! Claude reads `~/.claude/CLAUDE.md` and Codex reads `~/.codex/AGENTS.md`. Both
//! are user-authored files, so HZR never rewrites them wholesale: it owns exactly
//! one delimited block and retires the legacy RTK reference that block replaces.
//! Every mutation reuses the migration discipline from PRD §11 — full-SHA backup,
//! compare-and-swap under a filesystem lock, atomic replace, `--dry-run` preview.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use directories::BaseDirs;
use serde::Serialize;

use crate::adoption::{atomic_write, backup_path, commit, read_optional, sha256};

/// Opening delimiter of the HZR-owned region. Kept stable forever: it is the only
/// handle for idempotent reinstall and for clean removal.
const BEGIN: &str = "<!-- hzr:begin managed agent contract — do not edit inside -->";
const END: &str = "<!-- hzr:end managed agent contract -->";

/// Legacy references retired when the HZR block is installed. `@RTK.md` is the
/// import directive; the prose lines are matched separately so a user who wrote
/// their own RTK guidance keeps it and only the machine-managed import is removed.
const LEGACY_IMPORTS: [&str; 2] = ["@RTK.md", "@~/.claude/RTK.md"];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Claude,
    Codex,
}

impl Surface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// Environment override first, so tests and non-standard layouts never touch
    /// the real user configuration.
    pub fn default_path(self) -> Result<PathBuf> {
        match self {
            Self::Claude => {
                if let Some(directory) = std::env::var_os("CLAUDE_CONFIG_DIR") {
                    return Ok(PathBuf::from(directory).join("CLAUDE.md"));
                }
                let base = BaseDirs::new().context("cannot determine the user home directory")?;
                Ok(base.home_dir().join(".claude/CLAUDE.md"))
            }
            Self::Codex => {
                if let Some(directory) = std::env::var_os("CODEX_HOME") {
                    return Ok(PathBuf::from(directory).join("AGENTS.md"));
                }
                let base = BaseDirs::new().context("cannot determine the user home directory")?;
                Ok(base.home_dir().join(".codex/AGENTS.md"))
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InstructionReport {
    pub surface: Surface,
    pub path: PathBuf,
    pub changed: bool,
    pub installed: bool,
    pub legacy_rtk_imports_removed: usize,
    pub legacy_directives_migrated: usize,
    pub backup_path: Option<PathBuf>,
    pub before_sha256: String,
    pub after_sha256: String,
}

/// The managed block. `contract_path` is the absolute `HZR.md` shipped with the
/// installation, referenced rather than inlined so a single file stays canonical.
fn managed_block(surface: Surface, contract_path: &Path) -> String {
    let import = match surface {
        // Claude resolves `@path` imports relative to the configuration file, so an
        // absolute path is the only form that survives a relocated bundle.
        Surface::Claude => format!("@{}\n\n", contract_path.display()),
        // Codex has no import directive; it reads AGENTS.md literally.
        Surface::Codex => format!(
            "Bootstrap by reading `{0}` with `hzr rtk -- read {0} --level none` before other tool use.\n\n",
            contract_path.display(),
        ),
    };

    format!(
        "{BEGIN}\n\n\
         # HZR tool contract (managed)\n\n\
         `hzr` — heAdz0r's Zero-Redundancy engine — is the only control plane. Do not\n\
         invoke a separately installed `rtk`, `grepai` or `icm` binary: HZR owns those\n\
         engines internally, and a direct call creates the duplicate scan, duplicate\n\
         store and unaccounted usage this engine exists to remove.\n\n\
         This managed region defines tool routing only. Keep repository-specific roles,\n\
         source paths and test commands in that repository's root instruction file, not\n\
         in a user-global instruction file.\n\n\
         {import}\
         | Instead of | Use |\n\
         |---|---|\n\
         | `Read` | `hzr rtk -- read <file>`; Markdown defaults to a digest, `--level none` is exact |\n\
         | `Grep` | `hzr rgai \"<intent>\"` (semantic) or `hzr search \"<pattern>\" --mode exact` |\n\
         | `Edit`/`Write` | `hzr rtk -- write patch\\|replace\\|set\\|create\\|batch ...` |\n\
         | memory | `hzr memory recall\\|store` |\n\
         | context | `hzr context plan \"<intent>\"` |\n\
         | exact/raw output | `hzr rtk -- raw <command...>` |\n\
         | optional TDD | `hzr tdd` only when user/repository policy or regression risk justifies test-first overhead |\n\
         | build this project | `hzr build <args>` (not `hzr release`, which rebuilds HZR) |\n\n\
         TDD is opt-in, not the default. When token or time efficiency matters, skip it\n\
         and use proportionate verification; repository-required quality gates still apply.\n\n\
         `read -n` defaults to exact content and preserves source coordinates, including\n\
         ranged and tail reads. `--max-lines N` is the exact head equivalent. `--outline`\n\
         returns Markdown headings or heuristic symbols for Rust, Python, TypeScript,\n\
         JavaScript, Go and Java.\n\n\
         Batch writes are atomic and idempotent per file; independent file groups can fail separately,\n\
         so inspect every operation result. Batch is not an all-files transaction.\n\n\
         ## Memory scopes\n\n\
         One store, two namespaces. `--scope project` (the store default) is for facts about\n\
         *this repository*. `--scope global` is for facts about the **user** — a preference or\n\
         a standing rule that should apply in every repository, so it does not have to be\n\
         restated per project. Recall defaults to project + global; another repository's\n\
         memory is never reachable from any scope.\n\n\
         ## MCP tools\n\n\
         If the `hzr` MCP server is registered, prefer its tools over the CLI — same\n\
         single store and index, and the calls are accounted:\n\n\
         | Tool | Use it for |\n\
         |---|---|\n\
         | `hzr_context_plan` | Build bounded graph-first evidence for unfamiliar or cross-cutting work. |\n\
         | `hzr_search` | Find code by intent (`mode: semantic`) or exactly (`mode: exact`). |\n\
         | `hzr_memory_recall` | Recall decisions, resolved errors and prior context before re-reading files. |\n\
         | `hzr_memory_store` | Persist a decision, resolved error or finished work. Not ephemeral state. |\n\
         | `hzr_codec` | Apply or shadow-measure protected response-density transforms. |\n\n\
         MCP inputs are strictly validated and results include typed `structuredContent`.\n\
         `isError: true` means no success was confirmed and no fallback engine or store\n\
         was used. If a store transport fails after dispatch, recall before retrying because\n\
         completion may be unknown.\n\
         MCP is client-managed stdio: `hzr init` never starts it. Run `hzr install --force`\n\
         once to register it, and `hzr mcp status` to audit native client launch state.\n\
         `hzr mcp config --client codex\\|claude-desktop --workspace <dir> --apply` writes a pinned registration; omit `--apply` to print a paste snippet. Never\n\
         register `icm`, `grepai` or `rtk` as your own MCP server: each direct launch adds\n\
         another writer to the store HZR supervises and leaks orphans when the session dies.\n\n\
         `hzr rtk -- raw <command> <args...>` directly spawns the first argument; it does\n\
         not interpret pipes, redirects or globs unless an explicit shell is the command.\n\n\
         The installed `PreToolUse` hook routes Bash through the managed daemon and\n\
         falls back to the same pinned fork-core when the daemon is down. A degraded\n\
         rewrite keeps command policy but is absent from the usage ledger; `hzr doctor`\n\
         and `hzr stats` report that incomplete accounting rather than hiding it.\n\n\
         The hook matches `Bash`, `Agent` and `Task` only. It does **not** see your host's\n\
         own `Read`, `Grep`, `Edit`, `Write` or `Glob`, so nothing redirects those calls and\n\
         nothing records them — they are absent from `hzr stats` entirely. The table above is\n\
         therefore yours to follow, not something the hook enforces: a native file tool is\n\
         allowed and sometimes right, but it is invisible, so prefer the `hzr` command\n\
         whenever one exists.\n\n\
         {END}"
    )
}

/// Remove a previously installed block, tolerating hand-edited whitespace around it.
fn strip_managed_block(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(BEGIN) {
        let after = &rest[start..];
        let Some(end_offset) = after.find(END) else {
            // Unterminated marker: leave the remainder untouched rather than
            // truncating user content.
            break;
        };
        out.push_str(&rest[..start]);
        rest = &after[end_offset + END.len()..];
        rest = rest.strip_prefix('\n').unwrap_or(rest);
        rest = rest.strip_prefix('\n').unwrap_or(rest);
    }
    out.push_str(rest);
    out
}

/// Drop legacy RTK import directives. Only whole-line matches are removed so
/// prose that merely mentions RTK.md survives.
fn strip_legacy_imports(text: &str) -> (String, usize) {
    let mut removed = 0;
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            if LEGACY_IMPORTS.contains(&trimmed) {
                removed += 1;
                return false;
            }
            true
        })
        .collect();
    let mut out = kept.join("\n");
    if text.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    (out, removed)
}

const LEGACY_COMMAND_MIGRATIONS: [(&str, &str); 25] = [
    ("`which rtk`", "`command -v hzr`"),
    ("`rtk read <file>`", "`hzr rtk -- read <file>`"),
    (
        "`rtk grep <pattern>`",
        "`hzr search \"<pattern>\" --mode exact`",
    ),
    ("`rtk rgai <query>`", "`hzr rgai \"<query>\"`"),
    (
        "`rtk write file <path> --content @/tmp/f`",
        "`hzr rtk -- write file <path> --content @/tmp/f`",
    ),
    (
        "`rtk write batch --plan '[...]'`",
        "`hzr rtk -- write batch --plan '[...]'`",
    ),
    (
        "`rtk write patch/replace/set`",
        "`hzr rtk -- write patch/replace/set`",
    ),
    (
        "`rtk write patch/replace`",
        "`hzr rtk -- write patch/replace`",
    ),
    ("`rtk write batch`", "`hzr rtk -- write batch`"),
    ("`rtk rgai`", "`hzr rgai`"),
    ("`rtk grep`", "`hzr search --mode exact`"),
    ("`rtk` commands", "`hzr` commands"),
    ("`rtk` CLI", "`hzr` CLI"),
    ("Use `rtk` directly via Bash", "Use `hzr` directly via Bash"),
    (
        "Native tools bypass RTK's token-saving filters",
        "Native tools bypass HZR's token-saving control plane",
    ),
    ("no rtk equivalent", "no hzr equivalent"),
    ("rtk commands run through it", "hzr commands run through it"),
    ("if rtk is unavailable", "if hzr is unavailable"),
    ("`icm_memory_recall`", "`hzr_memory_recall`"),
    ("`icm_memory_store`", "`hzr_memory_store`"),
    ("(icm_memory_recall)", "(hzr_memory_recall)"),
    ("(icm_memory_store)", "(hzr_memory_store)"),
    (
        "`hzr search --mode exact` (regex)",
        "`hzr search --mode exact` (literal, case-sensitive)",
    ),
    (
        "you MUST use Bash tool with `hzr` commands INSTEAD of native tools",
        "prefer Bash with `hzr` commands for HZR-covered operations",
    ),
    (
        "single Bash call, atomic, idempotent",
        "single Bash call; atomic and idempotent per file, with independent file results",
    ),
];

fn migrate_legacy_directives(text: &str) -> (String, usize) {
    let mut migrated = text.to_owned();
    let mut changes = 0;
    for (legacy, hzr) in LEGACY_COMMAND_MIGRATIONS {
        let occurrences = migrated.matches(legacy).count();
        if occurrences > 0 {
            migrated = migrated.replace(legacy, hzr);
            changes += occurrences;
        }
    }
    (migrated, changes)
}

fn compose(existing: &str, surface: Surface, contract_path: &Path) -> (String, usize, usize) {
    let without_block = strip_managed_block(existing);
    let (body, removed) = strip_legacy_imports(&without_block);
    let (body, migrated) = migrate_legacy_directives(&body);
    let block = managed_block(surface, contract_path);
    let trimmed = body.trim_end();
    let composed = if trimmed.is_empty() {
        format!("{block}\n")
    } else {
        // Managed block goes last: user intent stays at the top of their own file.
        format!("{trimmed}\n\n{block}\n")
    };
    (composed, removed, migrated)
}

pub fn is_installed(path: &Path) -> Result<bool> {
    let before = read_optional(path)?;
    Ok(String::from_utf8_lossy(&before).contains(BEGIN))
}

/// Legacy imperatives that contradict the HZR block when they survive next to it.
///
/// These are matched outside the managed region only, because the HZR block itself
/// legitimately mentions `rtk` (as `hzr rtk -- ...`) and names the engines it forbids
/// calling directly. Matching the whole file would flag HZR's own text.
const LEGACY_MANDATES: [(&str, &str); 6] = [
    (
        "rtk-instead-of-native-tools",
        "you MUST use Bash tool with `rtk`",
    ),
    ("rtk-read-mandate", "`rtk read <file>`"),
    ("rtk-write-mandate", "`rtk write patch/replace/set`"),
    ("rtk-search-mandate", "Always use `rtk rgai`"),
    ("icm-recall-mandate", "`icm_memory_recall`"),
    ("icm-store-mandate", "`icm_memory_store`"),
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InstructionAudit {
    pub path: PathBuf,
    pub installed: bool,
    /// The referenced canonical contract exists and is readable. A block pointing at a
    /// missing `HZR.md` teaches the agent nothing, so presence of the marker is not proof.
    pub contract_readable: bool,
    pub contract_path: Option<PathBuf>,
    /// Legacy directives still active *outside* the managed block.
    pub conflicting_mandates: Vec<String>,
}

impl InstructionAudit {
    /// Healthy means installed, contract reachable, and no surviving legacy mandate.
    /// A marker sitting next to a conflicting mandate is a failure, never a pass.
    pub fn healthy(&self) -> bool {
        self.installed && self.contract_readable && self.conflicting_mandates.is_empty()
    }
}

/// Extract the `@path` / literal contract reference the managed block points at, so the
/// audit can verify the target rather than trusting the marker.
fn referenced_contract(block: &str) -> Option<PathBuf> {
    for line in block.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('@') {
            return Some(PathBuf::from(rest));
        }
        if let Some(rest) = line.strip_prefix("Bootstrap by reading `") {
            if let Some(end) = rest.find('`') {
                return Some(PathBuf::from(&rest[..end]));
            }
        }
        if let Some(start) = line.find("Read the full contract at `") {
            let rest = &line[start + "Read the full contract at `".len()..];
            if let Some(end) = rest.find('`') {
                return Some(PathBuf::from(&rest[..end]));
            }
        }
    }
    None
}

pub fn audit(path: &Path) -> Result<InstructionAudit> {
    let bytes = read_optional(path)?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    let installed = text.contains(BEGIN);

    // Split managed region from user content so HZR's own prose is never flagged.
    let outside = strip_managed_block(&text);
    let conflicting_mandates = LEGACY_MANDATES
        .iter()
        .filter(|(_, needle)| outside.contains(needle))
        .map(|(id, _)| (*id).to_owned())
        .collect();

    let contract_path = if installed {
        text.find(BEGIN)
            .and_then(|start| {
                let region = &text[start..];
                region.find(END).map(|end| &region[..end])
            })
            .and_then(referenced_contract)
    } else {
        None
    };
    let contract_readable = contract_path
        .as_ref()
        .map(|contract| contract.is_file())
        .unwrap_or(false);

    Ok(InstructionAudit {
        path: path.to_path_buf(),
        installed,
        contract_readable,
        contract_path,
        conflicting_mandates,
    })
}

pub fn install(
    surface: Surface,
    path: &Path,
    contract_path: &Path,
    dry_run: bool,
    confirmed: bool,
) -> Result<InstructionReport> {
    let before = read_optional(path)?;
    let existing = String::from_utf8(before.clone())
        .with_context(|| format!("{} is not UTF-8; HZR will not rewrite it", path.display()))?;
    let (after, legacy_removed, directives_migrated) = compose(&existing, surface, contract_path);
    apply(
        surface,
        path,
        &before,
        after,
        legacy_removed,
        directives_migrated,
        dry_run,
        confirmed,
        "installation",
    )
}

pub fn uninstall(
    surface: Surface,
    path: &Path,
    dry_run: bool,
    confirmed: bool,
) -> Result<InstructionReport> {
    let before = read_optional(path)?;
    if before.is_empty() {
        return Ok(InstructionReport {
            surface,
            path: path.to_path_buf(),
            changed: false,
            installed: false,
            legacy_rtk_imports_removed: 0,
            legacy_directives_migrated: 0,
            before_sha256: sha256(&before),
            after_sha256: sha256(&before),
            backup_path: None,
        });
    }
    let existing = String::from_utf8(before.clone())
        .with_context(|| format!("{} is not UTF-8; HZR will not rewrite it", path.display()))?;
    let stripped = strip_managed_block(&existing);
    let after = if stripped.trim().is_empty() {
        String::new()
    } else {
        let mut text = stripped.trim_end().to_owned();
        text.push('\n');
        text
    };
    apply(
        surface,
        path,
        &before,
        after,
        0,
        0,
        dry_run,
        confirmed,
        "uninstallation",
    )
}

#[allow(clippy::too_many_arguments)]
fn apply(
    surface: Surface,
    path: &Path,
    before: &[u8],
    after: String,
    legacy_removed: usize,
    directives_migrated: usize,
    dry_run: bool,
    confirmed: bool,
    action: &str,
) -> Result<InstructionReport> {
    let changed = before != after.as_bytes();
    let backup = (changed && !before.is_empty()).then(|| backup_path(path, before));

    if changed && !dry_run {
        if !confirmed {
            bail!(
                "{action} changes {}; inspect `hzr install --dry-run`, then rerun with `--force` to confirm",
                path.display()
            );
        }
        match backup.as_ref() {
            // Instruction files have no default document, so "absent" is empty bytes.
            Some(backup) => commit(path, before, after.as_bytes(), backup, b"")?,
            // No prior file: nothing to preserve, so a plain atomic create is correct.
            None => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("cannot create instruction directory {}", parent.display())
                    })?;
                }
                atomic_write(path, after.as_bytes())?;
            }
        }
    }

    Ok(InstructionReport {
        surface,
        path: path.to_path_buf(),
        changed,
        installed: after.contains(BEGIN),
        legacy_rtk_imports_removed: legacy_removed,
        legacy_directives_migrated: directives_migrated,
        before_sha256: sha256(before),
        after_sha256: sha256(after.as_bytes()),
        backup_path: backup,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        BEGIN, END, Surface, compose, migrate_legacy_directives, strip_legacy_imports,
        strip_managed_block,
    };

    fn contract() -> &'static Path {
        Path::new("/opt/hzr/share/hzr/HZR.md")
    }

    #[test]
    fn test_install_preserves_user_content_and_appends_managed_block() {
        let (out, removed, migrated) =
            compose("# My rules\n\nBe careful.\n", Surface::Claude, contract());
        assert_eq!(removed, 0);
        assert_eq!(migrated, 0);
        assert!(out.starts_with("# My rules\n\nBe careful."));
        assert!(out.contains(BEGIN));
        assert!(out.contains(END));
        assert!(out.contains("@/opt/hzr/share/hzr/HZR.md"));
    }

    #[test]
    fn test_install_is_idempotent() {
        let first = compose("# Mine\n", Surface::Claude, contract()).0;
        let second = compose(&first, Surface::Claude, contract()).0;
        assert_eq!(first, second, "reinstall must not duplicate the block");
        assert_eq!(second.matches(BEGIN).count(), 1);
    }

    #[test]
    fn test_install_retires_legacy_rtk_import() {
        let (out, removed, _) = compose("# Mine\n\n@RTK.md\n", Surface::Claude, contract());
        assert_eq!(removed, 1);
        assert!(!out.contains("@RTK.md"));
        assert!(out.contains("# Mine"));
    }

    #[test]
    fn test_legacy_strip_keeps_prose_mentioning_rtk() {
        let (out, removed) = strip_legacy_imports("see @RTK.md for details\n");
        assert_eq!(removed, 0, "inline prose is not an import directive");
        assert!(out.contains("see @RTK.md for details"));
    }

    #[test]
    fn test_install_migrates_legacy_commands_without_touching_hzr_block() {
        let legacy = "Use `rtk read <file>` and `rtk rgai <query>`. Recall with \
                      `icm_memory_recall`.\n";
        let (out, _, migrated) = compose(legacy, Surface::Claude, contract());

        assert_eq!(migrated, 3);
        assert!(out.contains("`hzr rtk -- read <file>`"));
        assert!(out.contains("`hzr rgai \"<query>\"`"));
        assert!(out.contains("`hzr_memory_recall`"));
        assert_eq!(out.matches("`hzr rtk -- read <file>`").count(), 2);
        assert_eq!(compose(&out, Surface::Claude, contract()).0, out);
    }

    #[test]
    fn test_directive_migration_preserves_unrelated_rtk_prose() {
        let (out, migrated) = migrate_legacy_directives("RTK history remains attributable.\n");
        assert_eq!(migrated, 0);
        assert_eq!(out, "RTK history remains attributable.\n");
    }

    #[test]
    fn test_install_migrates_ambiguous_legacy_hzr_directives() {
        let legacy = "Use `hzr search --mode exact` (regex). Recall with \
                      (icm_memory_recall). For covered work you MUST use Bash tool with \
                      `hzr` commands INSTEAD of native tools. Apply the plan in a single \
                      Bash call, atomic, idempotent.\n";
        let (out, _, migrated) = compose(legacy, Surface::Codex, contract());

        assert_eq!(migrated, 4);
        assert!(out.contains("`hzr search --mode exact` (literal, case-sensitive)"));
        assert!(out.contains("(hzr_memory_recall)"));
        assert!(out.contains("prefer Bash with `hzr` commands for HZR-covered operations"));
        assert!(out.contains(
            "single Bash call; atomic and idempotent per file, with independent file results"
        ));
        assert_eq!(compose(&out, Surface::Codex, contract()).0, out);
    }

    #[test]
    fn test_uninstall_restores_original_body() {
        let original = "# Mine\n\nKeep this.\n";
        let installed = compose(original, Surface::Claude, contract()).0;
        let stripped = strip_managed_block(&installed);
        assert_eq!(stripped.trim_end(), original.trim_end());
        assert!(!stripped.contains(BEGIN));
    }

    #[test]
    fn test_codex_surface_uses_literal_reference_not_claude_import() {
        let out = compose("", Surface::Codex, contract()).0;
        assert!(out.contains(
            "Bootstrap by reading `/opt/hzr/share/hzr/HZR.md` with `hzr rtk -- read /opt/hzr/share/hzr/HZR.md --level none` before other tool use"
        ));
        assert!(
            !out.contains("\n@/opt/hzr"),
            "Codex has no @import directive"
        );
    }

    #[test]
    fn test_managed_block_describes_mcp_and_batch_semantics_exactly() {
        let out = compose("", Surface::Codex, contract()).0;
        assert!(out.contains("`hzr_codec`"));
        assert!(out.contains("| optional TDD | `hzr tdd` only when"));
        assert!(out.contains("TDD is opt-in, not the default"));
        assert!(!out.contains("`hzr tdd` before production changes"));
        assert!(
            out.contains("--apply` writes a pinned registration"),
            "managed block must document the apply path for pinned MCP registration"
        );
        assert!(out.contains("independent file groups can fail separately"));
        assert!(!out.contains("Register the server with `hzr mcp config"));
    }

    #[test]
    fn test_codex_audit_resolves_the_executable_bootstrap_contract() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let contract = directory.path().join("HZR.md");
        let instructions = directory.path().join("AGENTS.md");
        std::fs::write(&contract, "contract").expect("contract fixture");
        std::fs::write(&instructions, compose("", Surface::Codex, &contract).0)
            .expect("instruction fixture");

        let report = super::audit(&instructions).expect("instruction audit");
        assert_eq!(report.contract_path.as_deref(), Some(contract.as_path()));
        assert!(report.contract_readable);
        assert!(report.healthy());
    }

    #[test]
    fn test_unterminated_marker_never_truncates_user_content() {
        let text = format!("# Mine\n\n{BEGIN}\n\nhalf written");
        let stripped = strip_managed_block(&text);
        assert!(stripped.contains("# Mine"));
        assert!(stripped.contains("half written"));
    }
}
