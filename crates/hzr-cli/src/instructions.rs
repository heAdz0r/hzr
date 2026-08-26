//! HZR-owned instruction blocks inside agent configuration files.
//!
//! Claude reads `~/.claude/CLAUDE.md` and Codex reads `~/.codex/AGENTS.md`. Both
//! are user-authored files, so HZR never rewrites them wholesale: it owns exactly
//! one delimited block and retires the legacy RTK reference that block replaces.
//! Every mutation reuses the migration discipline from PRD §11 — full-SHA backup,
//! compare-and-swap under a filesystem lock, atomic replace, `--dry-run` preview.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use directories::{BaseDirs, ProjectDirs};
use serde::{Deserialize, Serialize};

use crate::adoption::{atomic_write, commit_with_lock, read_optional, sha256};

/// Opening delimiter of the HZR-owned region. Kept stable forever: it is the only
/// handle for idempotent reinstall and for clean removal.
const BEGIN: &str = "<!-- hzr:begin managed agent contract — do not edit inside -->";
const END: &str = "<!-- hzr:end managed agent contract -->";
const LEGACY_RTK_BEGIN: &str = "<!-- rtk-instructions";
const LEGACY_RTK_END: &str = "<!-- /rtk-instructions -->";
const AGENT_CAPABILITIES_JSON: &str = include_str!("../../../contracts/agent-capabilities.json");

/// Column at which managed prose wraps. Author-written lines in this block already
/// respect it; paragraphs that interpolate an installation path or a capability value
/// are wrapped programmatically, because the value's length is not known here and a
/// long one would otherwise emit a 200-column line into a user's instruction file.
const MANAGED_PROSE_WIDTH: usize = 88;

#[derive(Debug, Deserialize)]
struct AgentCapabilities {
    schema_version: u32,
    product: String,
    control_plane: String,
    internal_engines: Vec<String>,
    routes: Vec<AgentRoute>,
    mcp_tools: Vec<AgentTool>,
    harnesses: AgentHarnesses,
}

#[derive(Debug, Deserialize)]
struct AgentRoute {
    instead_of: String,
    command: String,
    guidance: String,
}

#[derive(Debug, Deserialize)]
struct AgentTool {
    name: String,
    purpose: String,
}

#[derive(Debug, Deserialize)]
struct AgentHarnesses {
    claude: AgentHarness,
    codex: AgentHarness,
}

#[derive(Debug, Deserialize)]
struct AgentHarness {
    instruction_file: Option<String>,
    native_hook_routing: bool,
    response_codec: Option<ResponseCodecCapability>,
}

#[derive(Debug, Deserialize)]
struct ResponseCodecCapability {
    global_replacement: bool,
    coverage: String,
    mechanism: String,
    economic_credit: bool,
}

fn agent_capabilities() -> AgentCapabilities {
    let contract: AgentCapabilities = serde_json::from_str(AGENT_CAPABILITIES_JSON)
        .expect("embedded agent capability contract must be valid JSON");
    assert_eq!(
        contract.schema_version, 1,
        "unsupported agent contract schema"
    );
    contract
}

fn route_table(contract: &AgentCapabilities) -> String {
    let mut table = String::from("| Instead of | Use |\n|---|---|\n");
    for route in &contract.routes {
        table.push_str(&format!(
            "| {} | `{}`; {} |\n",
            route.instead_of.replace('|', "\\|"),
            route.command.replace('|', "\\|"),
            route.guidance.replace('|', "\\|")
        ));
    }
    table
}

fn mcp_table(contract: &AgentCapabilities) -> String {
    let mut table = String::from("| Tool | Use it for |\n|---|---|\n");
    for tool in &contract.mcp_tools {
        table.push_str(&format!("| `{}` | {} |\n", tool.name, tool.purpose));
    }
    table
}

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

/// Wrappable units of managed prose: whitespace separated, except that a `code span`
/// stays whole. An installation path may contain spaces and has to stay copy-pasteable,
/// so it keeps its own line instead of being broken in half.
fn prose_words(text: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut span: Option<String> = None;
    for token in text.split_whitespace() {
        let closes = token.matches('`').count() % 2 == 1;
        match span.as_mut() {
            Some(open) => {
                open.push(' ');
                open.push_str(token);
                if closes {
                    words.push(span.take().unwrap_or_default());
                }
            }
            None if closes => span = Some(token.to_string()),
            None => words.push(token.to_string()),
        }
    }
    if let Some(unterminated) = span.take() {
        words.push(unterminated);
    }
    words
}

/// Greedy word wrap for one managed prose paragraph, applied *after* interpolation, so
/// the rendered width never depends on how long a path or capability value happens to be.
/// A unit wider than `width` owns its line rather than being split.
fn wrap_prose(text: &str, width: usize) -> String {
    let words = prose_words(text);
    let mut wrapped = String::new();
    let mut line = String::new();
    for word in words {
        let fits = line.is_empty() || line.chars().count() + 1 + word.chars().count() <= width;
        if !fits {
            wrapped.push_str(&line);
            wrapped.push('\n');
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    if !line.is_empty() {
        wrapped.push_str(&line);
        wrapped.push('\n');
    }
    wrapped
}

/// The managed block. `contract_path` is the absolute `HZR.md` shipped with the
/// installation, referenced rather than inlined so a single file stays canonical.
fn managed_block(surface: Surface, contract_path: &Path) -> String {
    let capabilities = agent_capabilities();
    let harness = match surface {
        Surface::Claude => &capabilities.harnesses.claude,
        Surface::Codex => &capabilities.harnesses.codex,
    };
    let expected_instruction_file = match surface {
        Surface::Claude => "CLAUDE.md",
        Surface::Codex => "AGENTS.md",
    };
    assert_eq!(
        harness.instruction_file.as_deref(),
        Some(expected_instruction_file),
        "agent contract harness file must match the installation surface"
    );
    let route_table = route_table(&capabilities);
    let mcp_table = mcp_table(&capabilities);
    let engines = capabilities
        .internal_engines
        .iter()
        .map(|engine| format!("`{engine}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let harness_guidance = if harness.native_hook_routing {
        concat!(
            "The Claude Code `PreToolUse` hook routes Bash through the managed daemon and\n",
            "falls back to the same pinned fork-core when the daemon is down. A degraded\n",
            "rewrite keeps command policy but is absent from the usage ledger; `hzr doctor`\n",
            "and `hzr stats` report that incomplete accounting rather than hiding it.\n\n",
            "The failure-open `PreToolUse` hook sees native `Read`, `Grep`, `Glob`, `Edit` and\n",
            "`Write`. In `steer` mode it prescribes `hzr read`/`hzr search`; `Glob` and native\n",
            "edits remain allowed. `strict` additionally prescribes `hzr write`, while `observe`\n",
            "retains measurement-only compatibility. The `PostToolUse` observer stores no tool\n",
            "content and grants no savings credit. In `steer`/`strict`, policy-allowed native\n",
            "calls are accounted as typed E10 bypasses, not hidden as `native_unaccounted`.\n"
        )
    } else {
        concat!(
            "Codex does not run HZR's Claude `PreToolUse` or `PostToolUse` hooks. Follow the\n",
            "routes above explicitly, and prefer registered HZR MCP tools when they are available.\n",
            "Native operations not routed through HZR are outside HZR accounting.\n"
        )
    };
    let codec = harness
        .response_codec
        .as_ref()
        .expect("interactive harness must declare response codec coverage");
    assert!(!codec.global_replacement);
    assert_eq!(codec.coverage, "instructed");
    assert!(!codec.economic_credit);
    let codec_guidance = format!(
        "## Response codec coverage\n\n{}\n",
        wrap_prose(
            &format!(
                "This host cannot let HZR replace every final response. Coverage is \
                 `{coverage}` via `{mechanism}`. Before delivering long low- or medium-risk \
                 prose where compression is useful, call `hzr_codec` once and use its \
                 returned `content`. If the tool is not available, keep the response concise \
                 and report codec coverage as unavailable. An explicit tool result is not \
                 proof that the host delivered it: HZR grants zero economic credit unless a \
                 trusted host confirms replacement. Shadow results are counterfactual \
                 measurements only.",
                coverage = codec.coverage,
                mechanism = codec.mechanism,
            ),
            MANAGED_PROSE_WIDTH,
        ),
    );
    let contract_pointer = format!(
        "{}\n",
        wrap_prose(
            &format!(
                "Read the full contract at `{0}` only when a bounded lookup cannot resolve \
                 HZR-policy ambiguity. Ordinary tasks must not import or read it in full. \
                 Start with `hzr read {0} --outline`, then read only the relevant \
                 `--from`/`--to` range.",
                contract_path.display(),
            ),
            MANAGED_PROSE_WIDTH,
        ),
    );

    format!(
        "{BEGIN}\n\n\
         # HZR tool contract (managed)\n\n\
         `{control_plane}` — {product}'s Zero-Redundancy control plane — is the only control plane. Do not\n\
         invoke separately installed {engines} binaries: HZR owns those\n\
         engines internally, and a direct call creates the duplicate scan, duplicate\n\
         store and unaccounted usage this engine exists to remove.\n\n\
         This managed region defines tool routing only. Keep repository-specific roles,\n\
         source paths and test commands in that repository's root instruction file, not\n\
         in a user-global instruction file.\n\n\
         {contract_pointer}\
         {route_table}\n\
         ## Execution invariants\n\n\
         For agent-originated shell work, `hzr exec run` is the default. If\n\
         `hzr exec rewrite '<shell command>'` returns `allow_rewrite`, `raw` is forbidden.\n\
         When no filter exists, it performs a tracked fallback; policy ambiguity returns `Ask`.\n\
         For plain argv commands with known output intent,\n\
         `hzr rtk -- test`, `err`, `summary` and `log` routes provide bounded\n\
         filtering. Keep pipes, redirects and other shell grammar on `hzr exec run`.\n\n\
         Unbounded `read --level none` defeats the smart default and is automatically reduced.\n\
         Prefer `--outline` for structure and `--from`/`--to` for exact evidence. Use\n\
         `HZR_EXACT_FIDELITY=1 hzr read <file> --level none` only when the whole file\n\
         is authoritative input that cannot be bounded. Multi-file reads use\n\
         `hzr read --batch --max-tokens N <files...>`.\n\n\
         TDD is opt-in, not the default. When token or time efficiency matters, skip it\n\
         and use proportionate verification; repository-required quality gates still apply.\n\n\
         ## Memory scopes\n\n\
         One store, two namespaces. `--scope project` (the store default) is for facts about\n\
         *this repository*. `--scope global` is for facts about the **user** — a preference or\n\
         standing rule that applies in every repository. Recall may combine project and global;\n\
         another repository's memory is never reachable.\n\n\
         ## MCP tools\n\n\
         Use a registered `hzr` MCP server only after its initialize result reports\n\
         `serverInfo.workspace.bound = true` and `serverInfo.workspace.project` exactly matches\n\
         the canonical current worktree. Otherwise use the CLI routes and repair the project pin;\n\
         never recommend or use an MCP session bound to another workspace:\n\n\
         {mcp_table}\n\
         MCP is client-managed stdio; `hzr init` writes the trusted-project Codex registration\n\
         but never starts it. `isError: true` confirms no\n\
         success and no fallback store. Recall before retrying an ambiguously completed write.\n\
         Never register {engines} as separate MCP servers.\n\n\
         {harness_guidance}\n\
         {codec_guidance}\
         {END}",
        control_plane = capabilities.control_plane,
        product = capabilities.product,
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

fn conflicting_mandates(text: &str) -> Vec<String> {
    let mut conflicts = BTreeSet::new();
    let mut managed = false;
    for (index, line) in text.lines().enumerate() {
        if line.contains(BEGIN) {
            managed = true;
            continue;
        }
        if line.contains(END) {
            managed = false;
            continue;
        }
        if managed {
            continue;
        }
        let lower = line.to_lowercase();
        for (id, needle) in LEGACY_MANDATES {
            if line.contains(needle) {
                conflicts.insert(format_instruction_conflict(
                    id,
                    index + 1,
                    line,
                    "remove the obsolete directive; the managed HZR contract is authoritative",
                ));
            }
        }
        let direct = if contains_direct_engine_command(&lower, "grepai") {
            Some((
                "direct-grepai",
                "replace it with `hzr search` or `hzr context plan`",
            ))
        } else if contains_direct_engine_command(&lower, "rtk") {
            Some((
                "direct-rtk",
                "route it through `hzr exec run` or the matching first-class HZR command",
            ))
        } else if contains_unnegated(&lower, "icm_memory_")
            || contains_direct_engine_command(&lower, "icm")
        {
            Some((
                "direct-icm",
                "replace it with the corresponding `hzr memory` command or HZR MCP tool",
            ))
        } else if imperative_native_rg(&lower) {
            Some((
                "native-rg-mandate",
                "replace the mandate with `hzr search` and keep native `rg` only as an explicitly allowed repository fallback",
            ))
        } else if imperative_native_edit(&lower) {
            Some((
                "native-edit-mandate",
                "route managed edits through `hzr write`; keep repository-specific fallback wording non-mandatory",
            ))
        } else {
            None
        };
        if let Some((id, remediation)) = direct {
            conflicts.insert(format_instruction_conflict(
                id,
                index + 1,
                line,
                remediation,
            ));
        }
    }
    conflicts.into_iter().collect()
}

fn contains_direct_engine_command(line: &str, engine: &str) -> bool {
    if engine == "rtk" && line.contains("hzr rtk") {
        return false;
    }
    let commands: &[&str] = match engine {
        "grepai" => &[
            "grepai search",
            "grepai callers",
            "grepai callees",
            "grepai graph",
            "grepai trace",
        ],
        "rtk" => &[
            "rtk read",
            "rtk rgai",
            "rtk grep",
            "rtk write",
            "rtk cargo",
            "rtk test",
            "rtk git",
            "rtk log",
            "rtk summary",
            "rtk err",
            "rtk raw",
            "rtk proxy",
            "rtk --",
        ],
        "icm" => &["icm serve", "icm recall", "icm store", "icm search"],
        _ => return false,
    };
    commands.iter().any(|command| {
        line.match_indices(command).any(|(position, _)| {
            !command_is_negated(line, position) && command_is_directive(line, position)
        })
    })
}

fn contains_unnegated(line: &str, needle: &str) -> bool {
    line.match_indices(needle)
        .any(|(position, _)| !command_is_negated(line, position))
}

fn command_is_negated(line: &str, position: usize) -> bool {
    let prefix = &line[..position];
    let clause = prefix
        .rsplit_once([';', '.'])
        .map_or(prefix, |(_, clause)| clause);
    ["do not", "don't", "never", "не использ", "запрещ"]
        .iter()
        .any(|negative| clause.contains(negative))
}

fn command_is_directive(line: &str, position: usize) -> bool {
    let trimmed = line.trim_start_matches(|character: char| {
        character.is_whitespace() || matches!(character, '-' | '*' | '`' | '|' | '$' | '>')
    });
    position <= line.len().saturating_sub(trimmed.len()) + 2
        || line.contains('`')
        || [
            "use ",
            "run ",
            "bash",
            "must",
            "always",
            "используй",
            "запусти",
            "обязательно",
            "should show",
        ]
        .iter()
        .any(|imperative| line.contains(imperative))
}

fn imperative_native_rg(line: &str) -> bool {
    (line.contains("must use")
        || line.contains("always use")
        || line.contains("используй")
        || line.contains("обязательно"))
        && (line.contains("`rg") || line.contains(" rg "))
}

fn imperative_native_edit(line: &str) -> bool {
    (line.contains("must use")
        || line.contains("always use")
        || line.contains("используй")
        || line.contains("обязательно"))
        && (line.contains("`edit`") || line.contains(" edit tool"))
}

fn format_instruction_conflict(
    id: &str,
    line_number: usize,
    line: &str,
    remediation: &str,
) -> String {
    let excerpt = line.trim().chars().take(180).collect::<String>();
    format!("{id} at line {line_number}: {excerpt:?}; remediation: {remediation}")
}

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

    let conflicting_mandates = conflicting_mandates(&text);

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
    let state = (changed && !before.is_empty())
        .then(|| instruction_state_paths(path, before))
        .transpose()?;
    let backup = state.as_ref().map(|(backup, _)| backup.clone());

    if changed && !dry_run {
        if !confirmed {
            bail!(
                "{action} changes {}; inspect `hzr install --dry-run`, then rerun with `--force` to confirm",
                path.display()
            );
        }
        match backup.as_ref() {
            // Instruction files have no default document, so "absent" is empty bytes.
            Some(backup) => commit_with_lock(
                path,
                before,
                after.as_bytes(),
                backup,
                b"",
                &state.as_ref().context("instruction state")?.1,
            )?,
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

fn instruction_state_paths(path: &Path, before: &[u8]) -> Result<(PathBuf, PathBuf)> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let project = ProjectDirs::from("dev", "headz0r", "hzr")
        .context("cannot determine HZR instruction state directory")?;
    let identity = sha256(absolute.as_os_str().as_encoded_bytes());
    let directory = project.data_dir().join("instruction-state").join(identity);
    Ok((
        directory.join(format!("backup-{}.md", sha256(before))),
        directory.join("write.lock"),
    ))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        BEGIN, END, MANAGED_PROSE_WIDTH, Surface, compose, managed_block,
        migrate_legacy_directives, prose_words, strip_legacy_imports, strip_managed_block,
    };

    fn contract() -> &'static Path {
        Path::new("/opt/hzr/share/hzr/HZR.md")
    }

    #[test]
    fn test_managed_prose_never_exceeds_the_wrap_width() {
        // A realistic long installation root: the block must not widen because of it.
        let long_contract =
            Path::new("/Users/a-very-long-account-name/.local/share/hzr/current/share/hzr/HZR.md");
        for surface in [Surface::Claude, Surface::Codex] {
            for line in managed_block(surface, long_contract).lines() {
                // Table rows are laid out by column, and a single unbreakable unit
                // (a path, or a command inside one code span) may own its line.
                if line.starts_with('|') || prose_words(line).len() <= 1 {
                    continue;
                }
                assert!(
                    line.chars().count() <= MANAGED_PROSE_WIDTH,
                    "{} managed line is {} columns: {line}",
                    surface.as_str(),
                    line.chars().count()
                );
            }
        }
    }

    #[test]
    fn test_managed_prose_keeps_a_path_with_spaces_on_one_line() {
        let spaced = Path::new("/Users/someone/Library/Application Support/hzr/share/hzr/HZR.md");
        for surface in [Surface::Claude, Surface::Codex] {
            let block = managed_block(surface, spaced);
            assert!(
                block.contains("`/Users/someone/Library/Application Support/hzr/share/hzr/HZR.md`"),
                "{} split a code span across lines",
                surface.as_str()
            );
        }
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
        // Managed prose is wrapped after interpolation, so assert the sentence, not the
        // line breaks it happens to land on.
        let flat = out.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains("Read the full contract at `/opt/hzr/share/hzr/HZR.md` only when"));
        assert!(flat.contains("hzr read /opt/hzr/share/hzr/HZR.md --outline"));
        assert!(flat.contains("relevant `--from`/`--to` range"));
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
        assert!(out.contains("| optional TDD | `hzr tdd`;"));
        assert!(out.contains("TDD is opt-in, not the default"));
        assert!(!out.contains("`hzr tdd` before production changes"));
        assert!(out.contains("batch is not an all-files transaction"));
        assert!(out.contains("MCP is client-managed stdio"));
        assert!(out.contains("`serverInfo.workspace.bound = true`"));
        assert!(out.contains("exactly matches\nthe canonical current worktree"));
        assert!(out.contains("never recommend or use an MCP session bound to another workspace"));
        assert!(!out.contains("Register the server with `hzr mcp config"));
    }

    #[test]
    fn acceptance_gate_managed_contract_routes_project_builds_through_exec_policy() {
        let out = compose("", Surface::Codex, contract()).0;

        assert!(out.contains("build this project | `hzr exec run '<project build command>'`"));
        assert!(!out.contains("`hzr build <args>`"));
    }

    #[test]
    fn acceptance_gate_all_instruction_surfaces_project_the_capability_ssot() {
        let capabilities = super::agent_capabilities();
        let canonical = include_str!("../../../HZR.md");
        let readme = include_str!("../../../README.md");
        let claude_awareness = include_str!("../../../integrations/claude-code/hzr-awareness.md");
        let codex_awareness =
            include_str!("../../../integrations/claude-code/hzr-awareness-codex.md");

        for surface in [Surface::Claude, Surface::Codex] {
            let rendered = compose("", surface, contract()).0;
            for route in &capabilities.routes {
                let markdown_command = route.command.replace('|', "\\|");
                assert!(
                    rendered.contains(&format!("`{markdown_command}`")),
                    "{} projection is missing route {}",
                    surface.as_str(),
                    route.command
                );
            }
        }
        for tool in &capabilities.mcp_tools {
            for (name, document) in [
                ("HZR.md", canonical),
                ("README.md", readme),
                ("Claude awareness", claude_awareness),
                ("Codex awareness", codex_awareness),
            ] {
                assert!(
                    document.contains(&format!("`{}`", tool.name)),
                    "{name} is missing SSOT MCP tool {}",
                    tool.name
                );
            }
        }
        assert!(!canonical.contains("Project build -> hzr build <args>"));
        assert!(!readme.contains("hzr build <args>"));
    }

    #[test]
    fn acceptance_gate_managed_projection_stays_bounded_and_defers_mutable_detail() {
        for surface in [Surface::Claude, Surface::Codex] {
            let rendered = compose("", surface, contract()).0;
            assert!(
                rendered.len() < 8 * 1024,
                "{} managed projection grew to {} bytes",
                surface.as_str(),
                rendered.len()
            );
            assert_eq!(rendered.matches(BEGIN).count(), 1);
            assert!(!rendered.contains("100,000 estimated delivered tokens"));
            assert!(rendered.contains("HZR.md --outline"));
        }
    }

    #[test]
    fn test_managed_block_forbids_raw_when_policy_can_rewrite() {
        let out = compose("", Surface::Codex, contract()).0;

        assert!(out.contains("shell command | `hzr exec run '<shell command>'`"));
        assert!(out.contains("returns `allow_rewrite`, `raw` is forbidden"));
        assert!(out.contains("When no filter exists, it performs a tracked fallback"));
        assert!(out.contains("`hzr rtk -- test`, `err`, `summary` and `log`"));
        assert!(out.contains("`HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=<reason>"));
        assert!(out.contains("Allowed reasons: binary, checksum, machine_protocol"));
        assert!(!out.contains("| exact/raw output |"));
    }

    #[test]
    fn acceptance_gate_managed_contract_matches_native_observer_and_mcp_surface() {
        let capabilities = super::agent_capabilities();
        for surface in [Surface::Claude, Surface::Codex] {
            let out = compose("", surface, contract()).0;

            for tool in &capabilities.mcp_tools {
                assert!(
                    out.contains(&format!("`{}`", tool.name)),
                    "missing MCP tool {}",
                    tool.name
                );
            }
            assert!(!out.contains("nothing records them"));
            assert!(!out.contains("absent from `hzr stats` entirely"));
        }

        let claude = compose("", Surface::Claude, contract()).0;
        assert!(claude.contains("failure-open `PreToolUse` hook sees native"));
        assert!(claude.contains("`Glob` and native"));
        assert!(claude.contains("`strict` additionally prescribes `hzr write`"));
        assert!(claude.contains("typed E10 bypasses"));

        let codex = compose("", Surface::Codex, contract()).0;
        assert!(codex.contains("Codex does not run HZR's Claude `PreToolUse`"));
        assert!(!codex.contains("failure-open `PreToolUse` hook sees native"));
        assert!(
            codex.contains("Native operations not routed through HZR are outside HZR accounting")
        );
    }

    #[test]
    fn acceptance_gate_no_unbounded_exact_defaults_in_managed_contract() {
        for surface in [Surface::Claude, Surface::Codex] {
            let out = compose("", surface, contract()).0;

            assert!(out.contains("`hzr search \"<intent>\" --mode auto`"));
            assert!(out.contains("--mode exact only for a known literal"));
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

    #[test]
    fn acceptance_gate_audit_reports_direct_engine_and_native_mandates_with_lines() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let contract = directory.path().join("HZR.md");
        let instructions = directory.path().join("AGENTS.md");
        std::fs::write(&contract, "contract").expect("contract fixture");
        let user = concat!(
            "# Project\n",
            "Use Bash to run grepai search and grepai callers.\n",
            "Always use `rtk read src/lib.rs`.\n",
            "Используй `icm serve` для памяти.\n",
            "Always use `rg` for repository discovery.\n",
            "You MUST use Edit tool for changes.\n",
            "Do not run grepai graph directly.\n",
        );
        std::fs::write(&instructions, compose(user, Surface::Codex, &contract).0)
            .expect("instruction fixture");

        let report = super::audit(Surface::Codex, &instructions).expect("instruction audit");
        assert!(!report.healthy());
        assert_eq!(report.conflicting_mandates.len(), 5);
        for expected in [
            "direct-grepai at line 2",
            "direct-rtk at line 3",
            "direct-icm at line 4",
            "native-rg-mandate at line 5",
            "native-edit-mandate at line 6",
        ] {
            assert!(
                report
                    .conflicting_mandates
                    .iter()
                    .any(|finding| finding.contains(expected)),
                "missing {expected}: {:?}",
                report.conflicting_mandates
            );
        }
        assert!(
            report
                .conflicting_mandates
                .iter()
                .all(|finding| finding.contains("remediation:"))
        );
        let rendered = std::fs::read_to_string(&instructions).expect("unchanged user text");
        assert!(rendered.contains("grepai search and grepai callers"));
    }

    #[test]
    fn acceptance_gate_direct_command_matrix_handles_negation_and_descriptive_prose() {
        for command in [
            "rtk cargo test",
            "rtk test",
            "rtk git status",
            "rtk log build.log",
            "rtk summary command",
            "rtk err command",
            "rtk raw command",
            "rtk proxy command",
            "grepai trace symbol",
        ] {
            assert!(
                super::contains_direct_engine_command(
                    command,
                    if command.starts_with("rtk") {
                        "rtk"
                    } else {
                        "grepai"
                    }
                ),
                "direct command not detected: {command}"
            );
        }
        assert!(!super::contains_direct_engine_command(
            "The docs describe grepai search conceptually.",
            "grepai"
        ));
        assert!(!super::contains_direct_engine_command(
            "do not run grepai trace directly.",
            "grepai"
        ));
        let mixed = super::conflicting_mandates(
            "Do not run grepai trace; run rtk cargo test through the old wrapper.\n",
        );
        assert_eq!(mixed.len(), 1);
        assert!(mixed[0].contains("direct-rtk at line 1"));
    }
}
