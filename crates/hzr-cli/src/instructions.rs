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
const LEGACY_RTK_BEGIN: &str = "<!-- rtk-instructions";
const LEGACY_RTK_END: &str = "<!-- /rtk-instructions -->";

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
    pub legacy_rtk_blocks_removed: usize,
    pub legacy_directives_migrated: usize,
    pub backup_path: Option<PathBuf>,
    pub before_sha256: String,
    pub after_sha256: String,
}

/// The managed block. `contract_path` is the absolute `HZR.md` shipped with the
/// installation, referenced rather than inlined so a single file stays canonical.
fn managed_block(_surface: Surface, contract_path: &Path) -> String {
    let contract_pointer = format!(
        concat!(
            "Read the full contract at `{0}` only when a bounded lookup cannot resolve ",
            "HZR-policy ambiguity.\n",
            "Ordinary tasks must not import or read it in full. Start with ",
            "`hzr read {0} --outline`, then read only the relevant ",
            "`--from`/`--to` range.\n\n",
        ),
        contract_path.display(),
    );

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
         {contract_pointer}\
         | Instead of | Use |\n\
         |---|---|\n\
         | `Read` | `hzr read <file>` uses the smart default; use `--outline` first for structure and ranges for exact evidence |\n\
         | `Grep` | `hzr rgai \"<intent>\"` or `hzr search \"<intent>\" --mode auto`; use `--mode exact` only for a known literal |\n\
         | `Edit`/`Write` | `hzr write patch\\|replace\\|set\\|create\\|batch ...` |\n\
         | memory | `hzr memory recall\\|store` |\n\
         | context | `hzr context plan \"<intent>\"` |\n\
         | shell command | `hzr exec run '<shell command>'`; canonical policy selects the filtered route and preserves shell grammar |\n\
         | explicit unfiltered recovery | `HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=<reason> hzr rtk -- raw <command...>`; reason must be `binary`, `checksum`, `machine_protocol`, `complete_log`, `full_patch`, or `verbatim_source` |\n\
         | optional TDD | `hzr tdd` only when user/repository policy or regression risk justifies test-first overhead |\n\
         | build this project | `hzr build <args>` (not `hzr release`, which rebuilds HZR) |\n\n\
         For agent-originated shell work, `hzr exec run` is the default. If\n\
         `hzr exec rewrite '<shell command>'` returns `allow_rewrite`, `raw` is forbidden.\n\
         Do not choose `raw` merely because a command uses SSH, JSON, pipes, redirects or\n\
         unfamiliar arguments. When no filter exists, `hzr exec run` performs the tracked\n\
         fallback without requiring the agent to select `raw`. POSIX shell launchers, env prefixes,\n\
         and simple Python file/JSON/subprocess wrappers do not bypass policy: HZR rewrites the\n\
         proven leaf or returns `Ask`. Opaque computation/migration remains tracked with zero\n\
         savings credit.\n\n\
         For a plain argv command whose output intent is known, the existing\n\
         `hzr rtk -- test`, `err`, `summary` and `log` routes provide bounded\n\
         generic filtering. Do not use them to reconstruct pipes, redirects or\n\
         other shell grammar; keep those commands on `hzr exec run`.\n\n\
        Unbounded `read --level none` defeats the smart default and is automatically reduced.\n\
         Prefer `--outline` for structure and `--from`/`--to` for exact evidence. Use\n\
         `HZR_EXACT_FIDELITY=1 hzr read <file> --level none` only when the whole file\n\
         is authoritative input that cannot be bounded. Search defaults to `--mode auto` for\n\
         discovery; `--mode exact` remains the escape hatch for a known symbol, error, key, or\n\
         audit literal.\n\n\
         TDD is opt-in, not the default. When token or time efficiency matters, skip it\n\
         and use proportionate verification; repository-required quality gates still apply.\n\n\
         `read -n` defaults to exact content and preserves source coordinates, including\n\
         ranged and tail reads. `--max-lines N` is the exact head equivalent. `--outline`\n\
         returns Markdown headings or heuristic symbols for Rust, Python, TypeScript,\n\
         JavaScript, Go and Java. For several files, use\n\
         `hzr read --batch --max-tokens N <files...>`; it preserves order and coordinates and\n\
         emits exact recovery ranges for omitted content.\n\n\
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
         | `hzr_memory_update` | Replace one superseded memory after namespace ownership is verified. |\n\
         | `hzr_memory_forget` | Delete one invalid memory after namespace ownership is verified. |\n\
         | `hzr_memory_prune` | Preview or remove low-weight memories in one namespace; preview is the default. |\n\
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
         `hzr rtk -- raw <command> <args...>` directly spawns the first argument and receives\n\
         zero savings credit. It is an explicit fidelity escape hatch, not the default shell\n\
         wrapper; normal agent shell work goes through `hzr exec run '<shell command>'`. The\n\
         fidelity marker without one of the closed reasons above returns `Ask`; even a valid\n\
         reason cannot override a managed equivalent, deny, or ambiguous-policy decision. The\n\
         per-session allowance is five operations or 100,000 estimated delivered tokens; an\n\
         oversized local read or unmeasurable remote exact stream asks before execution.\n\n\
         The installed `PreToolUse` hook routes Bash through the managed daemon and\n\
         falls back to the same pinned fork-core when the daemon is down. A degraded\n\
         rewrite keeps command policy but is absent from the usage ledger; `hzr doctor`\n\
         and `hzr stats` report that incomplete accounting rather than hiding it.\n\n\
        The failure-open `PreToolUse` hook sees native `Read`, `Grep`, `Glob`, `Edit` and\n\
         `Write`. In `steer` mode it prescribes `hzr read`/`hzr search`; `Glob` and native\n\
         edits remain allowed. `strict` additionally prescribes `hzr write`, while `observe`\n\
         retains measurement-only compatibility for existing installations. The `PostToolUse`\n\
         observer stores no tool content and grants no savings credit. In `steer`/`strict`,\n\
         policy-allowed native calls are accounted as typed E10 bypasses, not hidden as\n\
         `native_unaccounted`.\n\n\
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

/// Remove complete RTK v1/v2 instruction regions left by the predecessor installer.
/// Unterminated regions remain untouched so user content is never truncated.
fn strip_legacy_rtk_blocks(text: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut removed = 0;
    while let Some(start) = rest.find(LEGACY_RTK_BEGIN) {
        let after = &rest[start..];
        let Some(end_offset) = after.find(LEGACY_RTK_END) else {
            break;
        };
        out.push_str(&rest[..start]);
        rest = &after[end_offset + LEGACY_RTK_END.len()..];
        rest = rest.strip_prefix('\n').unwrap_or(rest);
        rest = rest.strip_prefix('\n').unwrap_or(rest);
        removed += 1;
    }
    out.push_str(rest);
    (out, removed)
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
    ("`rtk read <file>`", "`hzr read <file>`"),
    (
        "`rtk grep <pattern>`",
        "`hzr search \"<pattern>\" --mode exact`",
    ),
    ("`rtk rgai <query>`", "`hzr rgai \"<query>\"`"),
    (
        "`rtk write file <path> --content @/tmp/f`",
        "`hzr write file <path> --content @/tmp/f`",
    ),
    (
        "`rtk write batch --plan '[...]'`",
        "`hzr write batch --plan '[...]'`",
    ),
    (
        "`rtk write patch/replace/set`",
        "`hzr write patch/replace/set`",
    ),
    ("`rtk write patch/replace`", "`hzr write patch/replace`"),
    ("`rtk write batch`", "`hzr write batch`"),
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

fn compose(
    existing: &str,
    surface: Surface,
    contract_path: &Path,
) -> (String, usize, usize, usize) {
    let without_block = strip_managed_block(existing);
    let (without_legacy_blocks, legacy_blocks_removed) = strip_legacy_rtk_blocks(&without_block);
    let (body, removed) = strip_legacy_imports(&without_legacy_blocks);
    let (body, migrated) = migrate_legacy_directives(&body);
    let block = managed_block(surface, contract_path);
    let trimmed = body.trim_end();
    let composed = if trimmed.is_empty() {
        format!("{block}\n")
    } else {
        // Managed block goes last: user intent stays at the top of their own file.
        format!("{trimmed}\n\n{block}\n")
    };
    (composed, removed, legacy_blocks_removed, migrated)
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
const LEGACY_MANDATES: [(&str, &str); 7] = [
    ("rtk-managed-block", LEGACY_RTK_BEGIN),
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
    /// The delimited region exactly matches the block generated by this HZR build.
    /// Marker presence alone cannot prove current routing policy.
    pub current: bool,
    /// The referenced canonical contract exists and is readable. A block pointing at a
    /// missing `HZR.md` teaches the agent nothing, so presence of the marker is not proof.
    pub contract_readable: bool,
    pub contract_path: Option<PathBuf>,
    /// Legacy directives still active *outside* the managed block.
    pub conflicting_mandates: Vec<String>,
}

impl InstructionAudit {
    /// Healthy means current, contract reachable, and no surviving legacy mandate.
    /// A marker sitting next to a conflicting mandate is a failure, never a pass.
    pub fn healthy(&self) -> bool {
        self.installed
            && self.current
            && self.contract_readable
            && self.conflicting_mandates.is_empty()
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

pub fn audit(surface: Surface, path: &Path) -> Result<InstructionAudit> {
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

    let managed_region = if installed {
        text.find(BEGIN).and_then(|start| {
            let region = &text[start..];
            region
                .find(END)
                .map(|end| &region[..end.saturating_add(END.len())])
        })
    } else {
        None
    };
    let contract_path = managed_region.and_then(referenced_contract);
    let contract_readable = contract_path
        .as_ref()
        .map(|contract| contract.is_file())
        .unwrap_or(false);
    let current = managed_region
        .zip(contract_path.as_deref())
        .map(|(actual, contract)| actual == managed_block(surface, contract))
        .unwrap_or(false);

    Ok(InstructionAudit {
        path: path.to_path_buf(),
        installed,
        current,
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
    let (after, legacy_removed, legacy_blocks_removed, directives_migrated) =
        compose(&existing, surface, contract_path);
    apply(
        surface,
        path,
        &before,
        after,
        legacy_removed,
        legacy_blocks_removed,
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
            legacy_rtk_blocks_removed: 0,
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
    legacy_blocks_removed: usize,
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
        legacy_rtk_blocks_removed: legacy_blocks_removed,
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
        let (out, removed, blocks_removed, migrated) =
            compose("# My rules\n\nBe careful.\n", Surface::Claude, contract());
        assert_eq!(removed, 0);
        assert_eq!(blocks_removed, 0);
        assert_eq!(migrated, 0);
        assert!(out.starts_with("# My rules\n\nBe careful."));
        assert!(out.contains(BEGIN));
        assert!(out.contains(END));
        assert!(out.contains("`/opt/hzr/share/hzr/HZR.md`"));
        assert!(!out.contains("@/opt/hzr/share/hzr/HZR.md"));
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
        let (out, removed, _, _) = compose("# Mine\n\n@RTK.md\n", Surface::Claude, contract());
        assert_eq!(removed, 1);
        assert!(!out.contains("@RTK.md"));
        assert!(out.contains("# Mine"));
    }

    #[test]
    fn acceptance_gate_install_retires_complete_legacy_rtk_block() {
        let legacy = "# Project rules\n\nKeep this.\n\n<!-- rtk-instructions v2 -->\n\
# RTK\n\nAlways use `rtk rgai` and `rtk read <file>`.\n\
<!-- /rtk-instructions -->\n";
        let (out, imports_removed, blocks_removed, migrated) =
            compose(legacy, Surface::Claude, contract());

        assert_eq!(imports_removed, 0);
        assert_eq!(blocks_removed, 1);
        assert_eq!(migrated, 0, "removed block must not be rewritten in place");
        assert!(out.contains("# Project rules"));
        assert!(out.contains("Keep this."));
        assert!(!out.contains("rtk-instructions"));
        assert!(!out.contains("Always use `rtk rgai`"));
        assert!(out.contains(BEGIN));
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
        let (out, _, _, migrated) = compose(legacy, Surface::Claude, contract());

        assert_eq!(migrated, 3);
        assert!(out.contains("`hzr read <file>`"));
        assert!(out.contains("`hzr rgai \"<query>\"`"));
        assert!(out.contains("`hzr_memory_recall`"));
        assert_eq!(out.matches("`hzr read <file>`").count(), 2);
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
        let (out, _, _, migrated) = compose(legacy, Surface::Codex, contract());

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
    fn test_managed_surface_uses_bounded_contract_pointer_not_an_import() {
        let out = compose("", Surface::Codex, contract()).0;
        assert!(out.contains("Read the full contract at `/opt/hzr/share/hzr/HZR.md` only when"));
        assert!(out.contains("hzr read /opt/hzr/share/hzr/HZR.md --outline"));
        assert!(out.contains("relevant `--from`/`--to` range"));
        assert!(!out.contains("\n@/opt/hzr"));
        assert!(!out.contains("Bootstrap by reading"));
        assert!(!out.contains("HZR.md --level none` before other tool use"));
    }

    #[test]
    fn test_managed_block_describes_mcp_and_batch_semantics_exactly() {
        let out = compose("", Surface::Codex, contract()).0;
        assert!(out.contains("`hzr_codec`"));
        assert!(out.contains("`hzr_memory_update`"));
        assert!(out.contains("`hzr_memory_forget`"));
        assert!(out.contains("`hzr_memory_prune`"));
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
    fn test_managed_block_forbids_raw_when_policy_can_rewrite() {
        let out = compose("", Surface::Codex, contract()).0;

        assert!(out.contains("shell command | `hzr exec run '<shell command>'`"));
        assert!(out.contains("returns `allow_rewrite`, `raw` is forbidden"));
        assert!(out.contains("When no filter exists, `hzr exec run` performs the tracked"));
        assert!(out.contains("`hzr rtk -- test`, `err`, `summary` and `log`"));
        assert!(out.contains("`HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=<reason>"));
        assert!(out.contains("simple Python file/JSON/subprocess wrappers do not bypass policy"));
        assert!(out.contains("fidelity marker without one of the closed reasons"));
        assert!(!out.contains("| exact/raw output |"));
    }

    #[test]
    fn acceptance_gate_managed_contract_matches_native_observer_and_mcp_surface() {
        for surface in [Surface::Claude, Surface::Codex] {
            let out = compose("", surface, contract()).0;

            for tool in [
                "hzr_context_plan",
                "hzr_search",
                "hzr_memory_recall",
                "hzr_memory_store",
                "hzr_memory_update",
                "hzr_memory_forget",
                "hzr_memory_prune",
                "hzr_codec",
            ] {
                assert!(
                    out.contains(&format!("`{tool}`")),
                    "missing MCP tool {tool}"
                );
            }
            assert!(out.contains("failure-open `PreToolUse` hook sees native"));
            assert!(out.contains("`Glob` and native"));
            assert!(out.contains("`strict` additionally prescribes `hzr write`"));
            assert!(out.contains("`PostToolUse`"));
            assert!(out.contains("stores no tool content"));
            assert!(out.contains("grants no"));
            assert!(out.contains("savings credit"));
            assert!(out.contains("typed E10 bypasses"));
            assert!(!out.contains("nothing records them"));
            assert!(!out.contains("absent from `hzr stats` entirely"));
        }
    }

    #[test]
    fn acceptance_gate_no_unbounded_exact_defaults_in_managed_contract() {
        for surface in [Surface::Claude, Surface::Codex] {
            let out = compose("", surface, contract()).0;

            assert!(out.contains("`hzr search \"<intent>\" --mode auto`"));
            assert!(out.contains("use `--mode exact` only for a known literal"));
            assert!(out.contains("Prefer `--outline` for structure"));
            assert!(out.contains("`--from`/`--to` for exact evidence"));
            assert!(out.contains("`HZR_EXACT_FIDELITY=1 hzr read"));
            assert!(out.contains("`hzr read --batch --max-tokens N <files...>`"));
            assert!(!out.contains("Markdown defaults to a digest, `--level none` is exact"));
            assert!(!out.contains("\n@/opt/hzr"));
            assert!(!out.contains("Bootstrap by reading"));
            assert!(out.contains("HZR.md --outline"));
        }
    }

    #[test]
    fn test_codex_audit_resolves_the_executable_bootstrap_contract() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let contract = directory.path().join("HZR.md");
        let instructions = directory.path().join("AGENTS.md");
        std::fs::write(&contract, "contract").expect("contract fixture");
        std::fs::write(&instructions, compose("", Surface::Codex, &contract).0)
            .expect("instruction fixture");

        let report = super::audit(Surface::Codex, &instructions).expect("instruction audit");
        assert_eq!(report.contract_path.as_deref(), Some(contract.as_path()));
        assert!(report.contract_readable);
        assert!(report.current);
        assert!(report.healthy());
    }

    #[test]
    fn acceptance_gate_instruction_audit_rejects_stale_managed_policy() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let contract = directory.path().join("HZR.md");
        let instructions = directory.path().join("AGENTS.md");
        std::fs::write(&contract, "contract").expect("contract fixture");
        let stale = compose("", Surface::Codex, &contract)
            .0
            .replace("raw` is forbidden", "raw` is preferred");
        std::fs::write(&instructions, stale).expect("stale instruction fixture");

        let report = super::audit(Surface::Codex, &instructions).expect("instruction audit");
        assert!(report.installed);
        assert!(report.contract_readable);
        assert!(!report.current);
        assert!(!report.healthy());
    }

    #[test]
    fn test_unterminated_marker_never_truncates_user_content() {
        let text = format!("# Mine\n\n{BEGIN}\n\nhalf written");
        let stripped = strip_managed_block(&text);
        assert!(stripped.contains("# Mine"));
        assert!(stripped.contains("half written"));
    }
}
