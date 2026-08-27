use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use hzr_agent::ResponseFormat;
use hzr_protocol::{
    CodecProfile, FidelityClass, MemoryImportance, MemoryScopeSelector, MemoryWriteScope,
    RiskClass, SearchMode,
};

use crate::cli_help::{
    HZR_CLI_STYLES, HZR_COMMAND_GROUPS, HZR_EXAMPLES_FOOTER, HZR_ROOT_HELP_TEMPLATE,
};
use crate::cli_subcommand_help::{
    AGENT_AFTER_HELP, AGENT_LONG_ABOUT, AGENT_RUN_AFTER_HELP, AGENT_RUN_LONG_ABOUT,
    DISABLE_AFTER_HELP, DISABLE_LONG_ABOUT, DOCTOR_AFTER_HELP, DOCTOR_LONG_ABOUT,
    ENABLE_AFTER_HELP, ENABLE_LONG_ABOUT, MCP_CONFIG_AFTER_HELP, MCP_CONFIG_LONG_ABOUT,
    MCP_SERVE_AFTER_HELP, MCP_SERVE_LONG_ABOUT, MCP_STATUS_AFTER_HELP, MCP_STATUS_LONG_ABOUT,
    MEMORY_FORGET_AFTER_HELP, MEMORY_FORGET_LONG_ABOUT, MEMORY_PRUNE_AFTER_HELP,
    MEMORY_PRUNE_LONG_ABOUT, MEMORY_RECALL_AFTER_HELP, MEMORY_RECALL_LONG_ABOUT,
    MEMORY_STATUS_AFTER_HELP, MEMORY_STATUS_LONG_ABOUT, MEMORY_STORE_AFTER_HELP,
    MEMORY_STORE_LONG_ABOUT, MEMORY_UPDATE_AFTER_HELP, MEMORY_UPDATE_LONG_ABOUT, STATS_AFTER_HELP,
    STATS_LONG_ABOUT, UPDATE_AFTER_HELP, UPDATE_LONG_ABOUT,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatsDuration {
    seconds: u64,
    label: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum AccountingVersion {
    #[default]
    Current,
    All,
}

impl StatsDuration {
    pub const fn seconds(&self) -> u64 {
        self.seconds
    }

    pub fn label(&self) -> &str {
        &self.label
    }
}

fn parse_stats_duration(value: &str) -> Result<StatsDuration, String> {
    let (amount, unit) = value
        .split_at_checked(value.len().saturating_sub(1))
        .ok_or_else(|| "duration must be a positive integer followed by h, d, or w".to_owned())?;
    if amount.is_empty() || !amount.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("duration must be a positive integer followed by h, d, or w".to_owned());
    }
    let amount = amount
        .parse::<u64>()
        .map_err(|_| "duration is too large".to_owned())?;
    if amount == 0 {
        return Err("duration must be greater than zero".to_owned());
    }
    let unit_seconds = match unit {
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        "w" => 7 * 24 * 60 * 60,
        _ => return Err("duration unit must be h, d, or w".to_owned()),
    };
    let seconds = amount
        .checked_mul(unit_seconds)
        .ok_or_else(|| "duration is too large".to_owned())?;
    Ok(StatsDuration {
        seconds,
        label: format!("{amount}{unit}"),
    })
}

#[derive(Debug, Parser)]
#[command(
    name = "hzr",
    version,
    about = "Unified agent efficiency platform",
    styles = HZR_CLI_STYLES,
    help_template = HZR_ROOT_HELP_TEMPLATE,
    before_help = HZR_COMMAND_GROUPS,
    after_help = HZR_EXAMPLES_FOOTER
)]
pub struct Cli {
    /// Path to an HZR config file (defaults to the platform config)
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Emit machine-readable JSON instead of human text
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(
        about = "Register workspace data and refresh agent instructions",
        long_about = "Initialize the workspace registry, private data layout, visualizer service, and the current managed agent contract for the configured activation scope."
    )]
    Init {
        /// Re-run initialization even when the workspace is already registered
        #[arg(long)]
        force: bool,
        /// Replace the existing configuration with defaults; requires --force and creates a backup
        #[arg(long, requires = "force")]
        reset: bool,
        /// Print the initialization plan without changing config, registry, instructions, or services
        ///
        /// Combines with `--if-needed`, which is what fleet reconciliation needs: the plan
        /// carries `changes_required`, so a caller can see which workspaces would be written
        /// before writing to any of them.
        #[arg(long, conflicts_with = "if_enabled")]
        dry_run: bool,
        /// Override the private HZR data directory for this workspace
        #[arg(long, value_name = "DIR")]
        data_dir: Option<PathBuf>,
        /// No-op when the workspace is already initialized
        #[arg(long, conflicts_with = "force")]
        if_needed: bool,
        /// Initialize only when the current workspace was explicitly enabled
        #[arg(long, conflicts_with_all = ["force", "if_needed"])]
        if_enabled: bool,
        /// Suppress non-essential status output
        #[arg(long)]
        quiet: bool,
        /// Emit Claude SessionStart-compatible JSON when an update is available
        #[arg(long, hide = true, requires = "quiet")]
        session_start_hook: bool,
        /// Register the workspace without installing or starting the production daemon
        #[arg(long)]
        skip_service: bool,
    },
    #[command(
        about = "Enable HZR for one workspace",
        long_about = ENABLE_LONG_ABOUT,
        after_help = ENABLE_AFTER_HELP
    )]
    Enable {
        /// Workspace root to enable (defaults to the current directory)
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
    },
    #[command(
        about = "Disable HZR for one workspace; keep data",
        long_about = DISABLE_LONG_ABOUT,
        after_help = DISABLE_AFTER_HELP
    )]
    Disable {
        /// Workspace root to disable (defaults to the current directory)
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
    },
    #[command(about = "Inspect project-only activation mode and enabled workspaces")]
    Activation {
        #[command(subcommand)]
        command: ActivationCommand,
    },
    #[command(
        about = "Adopt HZR binaries, hooks, and instructions",
        long_about = "Adopt HZR for this machine: install PATH binaries, the one hook dispatcher, agent instructions, and the visualizer service."
    )]
    Install {
        /// Show the adoption plan without changing the system
        #[arg(long)]
        dry_run: bool,
        /// Apply adoption changes that otherwise require confirmation
        #[arg(long)]
        force: bool,
        /// Directory that receives durable `hzr`/`hzrd` binaries and must be on PATH
        #[arg(long, value_name = "DIR")]
        prefix: Option<PathBuf>,
        /// Explicit binary the hooks should invoke instead of the resolved current executable
        #[arg(long, value_name = "PATH")]
        binary: Option<PathBuf>,
        /// Allow hooks to point at a `target/debug` or `target/release` build (development only)
        #[arg(long)]
        allow_dev_path: bool,
        /// Keep external `icm hook` entries instead of centralizing memory ownership in HZR
        #[arg(long)]
        keep_external_icm: bool,
        /// Skip `CLAUDE.md`/`AGENTS.md` instruction wiring and install hooks only
        #[arg(long)]
        skip_instructions: bool,
        /// Skip service startup and keep the installed SessionStart hook from starting it
        #[arg(long)]
        skip_service: bool,
        /// Enable HZR only for the current workspace; hooks become no-ops elsewhere
        #[arg(long)]
        project_only: bool,
        /// Native file-tool policy. New installs default to steer; upgrades retain observe.
        #[arg(long, value_enum, value_name = "MODE")]
        native_tool_mode: Option<crate::adoption::NativeToolMode>,
        /// Select the global fallback/Claude Desktop workspace (default: install cwd).
        /// `hzr init` writes an exact project-scoped Codex pin; Claude Desktop remains a
        /// singleton client and must be explicitly retargeted before another workspace uses it.
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
    },
    #[command(about = "Remove HZR adoption hooks")]
    Uninstall {
        /// Leave indexes and memory on disk after removing hooks
        #[arg(long)]
        keep_data: bool,
        /// Show the uninstall plan without changing the system
        #[arg(long)]
        dry_run: bool,
        /// Apply uninstall changes that otherwise require confirmation
        #[arg(long)]
        force: bool,
    },
    #[command(about = "Inspect or run the HZR hook dispatcher")]
    Hooks {
        #[command(subcommand)]
        command: HooksCommand,
    },
    #[command(
        about = "Verify instructions, pins, ownership, and daemon health",
        long_about = DOCTOR_LONG_ABOUT,
        after_help = DOCTOR_AFTER_HELP
    )]
    Doctor {
        /// Workspace root to diagnose (defaults to the current directory)
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
        /// Safely migrate one unambiguous legacy .grepai before diagnosing
        #[arg(long)]
        fix: bool,
        /// Refresh the managed contract block in every registered workspace reporting a stale one
        #[arg(long)]
        reconcile_fleet: bool,
        /// Show what --reconcile-fleet would rewrite, without writing
        #[arg(long, requires = "reconcile_fleet")]
        dry_run: bool,
        /// Transactionally migrate each registered workspace with one unambiguous root legacy index
        #[arg(long, requires = "reconcile_fleet")]
        migrate_legacy_indexes: bool,
        /// Resolve one doctor-reported unknown fidelity reservation through hzrd
        #[arg(long, value_name = "RESERVATION_ID")]
        resolve_fidelity: Option<String>,
        /// Conservatively record the unknown execution with zero unmeasured tokens
        #[arg(
            long,
            requires = "resolve_fidelity",
            conflicts_with = "prove_not_executed"
        )]
        acknowledge_executed: bool,
        /// Release the allowance only after the operator proved the process never executed
        #[arg(
            long,
            requires = "resolve_fidelity",
            conflicts_with = "acknowledge_executed"
        )]
        prove_not_executed: bool,
    },
    #[command(about = "Operate the authenticated loopback daemon")]
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    #[command(about = "Inspect the engine manifest from hzrd")]
    Engines {
        #[command(subcommand)]
        command: EnginesCommand,
    },
    #[command(about = "Inspect or init the canonical grepai index")]
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },
    #[command(
        about = "Search via the exact/semantic router",
        long_about = "Search through the canonical exact/semantic router over the one HZR-owned index."
    )]
    Search(SearchArgs),
    #[command(about = "rgai-compatible search over the same index")]
    Rgai(SearchArgs),
    #[command(about = "Plan bounded code and ICM context")]
    Context {
        #[command(subcommand)]
        command: ContextCommand,
    },
    #[command(about = "Recall, store, or inspect ICM memory")]
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    #[command(about = "Rewrite or run a command through policy")]
    Exec {
        #[command(subcommand)]
        command: ExecCommand,
    },
    #[command(about = "Compile protected response-density transforms")]
    Codec {
        #[command(subcommand)]
        command: CodecCommand,
    },
    #[command(about = "Inspect pricing evidence or ingest provider receipts")]
    Billing {
        #[command(subcommand)]
        command: BillingCommand,
    },
    #[command(
        about = "Run the managed caveman-code agent",
        long_about = AGENT_LONG_ABOUT,
        after_help = AGENT_AFTER_HELP
    )]
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    #[command(
        about = "Serve HZR tools over stdio MCP",
        long_about = "Serve HZR-owned tools to external agents over stdio MCP. Routes to the one HZR store and does not spawn orphan engines."
    )]
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    #[command(
        about = "Build, install, and verify an HZR release",
        long_about = "Build this source tree into a bundle, install it globally, switch `current`, and verify every engine reports the expected version."
    )]
    Release {
        /// Synchronize every current version surface before building the release
        #[arg(value_name = "VERSION")]
        version: Option<String>,
        /// Show the release plan without building or installing
        #[arg(long)]
        dry_run: bool,
        /// Apply the release even when confirmation would otherwise be required
        #[arg(long)]
        force: bool,
        /// Keep the running daemon on the previous bundle instead of restarting it
        #[arg(long)]
        skip_service: bool,
        /// Override the versioned install root for this release
        #[arg(long, value_name = "DIR")]
        install_root: Option<PathBuf>,
    },
    #[command(
        about = "Install a newer GitHub release when available",
        long_about = UPDATE_LONG_ABOUT,
        after_help = UPDATE_AFTER_HELP
    )]
    Update {
        /// Report whether a newer GitHub release is available without downloading or installing.
        ///
        /// Exit status is 0 when the check succeeds (already current or update available).
        /// Non-zero only when the check fails (network, parse, or unsupported platform).
        #[arg(long)]
        check: bool,
    },
    #[command(about = "Print the optional HZR Red-Green-Refactor contract")]
    Tdd,
    #[command(
        about = "Run the inherited fork-core self-build pipeline",
        long_about = "Compatibility alias for the inherited fork-core `build` command. This is not a generic project-build wrapper; run project builds through `hzr exec run '<project build command>'`. Building the HZR distribution itself is `hzr release`."
    )]
    Build(ForkForwardArgs),
    #[command(about = "Run tests through the inherited failure-first filter")]
    Test(ForkForwardArgs),
    #[command(about = "Read files through bounded HZR filtering")]
    Read(ForkForwardArgs),
    #[command(about = "Write files atomically through HZR")]
    Write(ForkForwardArgs),
    #[command(
        about = "Show cumulative efficiency gains",
        long_about = STATS_LONG_ABOUT,
        after_help = STATS_AFTER_HELP
    )]
    Stats {
        /// Limit the ledger view to one workspace root
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
        /// Emit every per-command row. Default JSON is bounded to protect agent context.
        #[arg(long)]
        all: bool,
        /// Show privacy-safe evasion and fidelity-budget aggregates
        #[arg(long)]
        evasion: bool,
        /// Limit all ledger summaries to records observed within this window (for example 24h, 7d, or 4w)
        #[arg(long, value_name = "DURATION", value_parser = parse_stats_duration)]
        since: Option<StatsDuration>,
        /// Select current privacy-typed accounting or an explicitly labeled compatibility view
        #[arg(long, value_enum, default_value_t)]
        accounting_version: AccountingVersion,
    },
    #[command(hide = true)]
    Savings,
    #[command(about = "Inspect or centralize legacy state")]
    Migrate {
        #[command(subcommand)]
        command: MigrateCommand,
    },
    #[command(about = "Run the managed fork-core CLI unchanged")]
    Rtk(RtkArgs),
}

#[derive(Debug, Subcommand)]
pub enum McpCommand {
    #[command(
        about = "Serve stdio MCP until the parent agent closes stdin; routes to the one HZR store",
        long_about = MCP_SERVE_LONG_ABOUT,
        after_help = MCP_SERVE_AFTER_HELP
    )]
    Serve {
        /// Workspace root that scopes MCP memory and search
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
    },
    #[command(
        about = "Print or apply an MCP server registration; pin --workspace so memory is not bound to the client cwd",
        long_about = MCP_CONFIG_LONG_ABOUT,
        after_help = MCP_CONFIG_AFTER_HELP
    )]
    Config {
        /// Target agent client for the registration snippet
        #[arg(long, value_enum, default_value_t = McpClientArg::Codex)]
        client: McpClientArg,
        /// Pin the project the server's memory is scoped to. Without it (print mode) the
        /// namespace comes from whatever directory the client launched from, which is `/` for
        /// the Claude desktop app and a per-session directory for Codex. With `--apply`,
        /// defaults to the current directory.
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
        /// Write the registration into the client's config instead of printing a paste snippet.
        /// Claude Desktop stores one singleton selected workspace. Codex also has a global
        /// fallback, while `hzr init` writes the preferred exact project-scoped registration.
        #[arg(long)]
        apply: bool,
    },
    #[command(
        about = "Report native client registrations and the client-managed stdio lifecycle",
        long_about = MCP_STATUS_LONG_ABOUT,
        after_help = MCP_STATUS_AFTER_HELP
    )]
    Status,
}

#[derive(Debug, Subcommand)]
pub enum BillingCommand {
    #[command(about = "Show the embedded and optional user-overridden public pricing catalog")]
    Catalog,
    #[command(about = "Ingest one strict JSON provider receipt through the authenticated daemon")]
    Receipt {
        /// JSON file containing one paired provider receipt; stdin is not accepted implicitly
        #[arg(long, value_name = "FILE")]
        file: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum McpClientArg {
    Codex,
    ClaudeDesktop,
    ClaudeCode,
}

impl From<McpClientArg> for crate::client_config::Client {
    fn from(value: McpClientArg) -> Self {
        match value {
            McpClientArg::Codex => Self::Codex,
            McpClientArg::ClaudeDesktop => Self::ClaudeDesktop,
            McpClientArg::ClaudeCode => Self::ClaudeCode,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum ActivationCommand {
    #[command(about = "List activation mode and enabled workspaces")]
    Status,
}

#[derive(Debug, Subcommand)]
pub enum HooksCommand {
    #[command(about = "Report HZR, legacy RTK, and external ICM hook ownership")]
    Status,
    #[command(hide = true)]
    Dispatch {
        #[arg(long, value_enum, default_value = "observe")]
        native_mode: crate::adoption::NativeToolMode,
    },
    #[command(hide = true)]
    Observe {
        #[arg(long, value_enum, default_value = "observe")]
        native_mode: crate::adoption::NativeToolMode,
    },
    #[command(hide = true)]
    Feedback,
    #[command(hide = true)]
    Statusline,
}

#[derive(Debug, Subcommand)]
pub enum DaemonCommand {
    #[command(about = "Serve hzrd in the foreground until interrupted")]
    Serve,
    #[command(about = "Read typed health from the authenticated daemon")]
    Status,
    #[command(about = "Read the pinned engine manifest from the daemon")]
    Engines,
    #[command(about = "Manage the production user service that owns the single hzrd")]
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Subcommand)]
pub enum ServiceCommand {
    #[command(about = "Install and start the platform user service idempotently")]
    Install,
    #[command(about = "Start the installed platform user service")]
    Start,
    #[command(about = "Stop the platform user service without deleting its definition")]
    Stop,
    #[command(about = "Restart the platform user service on the active bundle")]
    Restart,
    #[command(about = "Report whether the platform user service is active")]
    Status,
}

#[derive(Debug, Subcommand)]
pub enum EnginesCommand {
    #[command(about = "Read the pinned engine manifest from the daemon")]
    Status,
}

#[derive(Debug, Subcommand)]
pub enum IndexCommand {
    #[command(about = "Inspect placement and artifacts without starting a watcher")]
    Status {
        /// Workspace root whose index placement is reported
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
    },
    #[command(about = "Initialize the canonical index; hzrd remains the watcher owner")]
    Init {
        /// Workspace root that receives the canonical index
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ContextCommand {
    #[command(about = "Fuse the fork memory planner and centralized ICM under a hard budget")]
    Plan(ContextPlanArgs),
}

#[derive(Clone, Debug, Args)]
pub struct ContextPlanArgs {
    pub intent: String,
    /// Workspace root used for code and memory planning
    #[arg(long, value_name = "DIR")]
    pub workspace: Option<PathBuf>,
    /// Optional path hint that scopes the planner
    #[arg(long, value_name = "PATH")]
    pub path: Option<PathBuf>,
    /// Optional ICM topic hint for memory retrieval
    #[arg(long)]
    pub topic: Option<String>,
    /// Maximum search hits to include in the plan
    #[arg(long, default_value_t = 10, value_parser = parse_limit)]
    pub search_limit: usize,
    /// Maximum memories to include in the plan
    #[arg(long, default_value_t = 5, value_parser = parse_limit)]
    pub memory_limit: usize,
}

/// Trailing arguments forwarded verbatim to one inherited fork-core subcommand.
#[derive(Clone, Debug, Args)]
pub struct ForkForwardArgs {
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "ARG"
    )]
    pub args: Vec<OsString>,
}

#[derive(Clone, Debug, Args)]
pub struct RtkArgs {
    #[arg(last = true, value_name = "ARG", num_args = 0.., allow_hyphen_values = true)]
    pub args: Vec<OsString>,
}

#[derive(Clone, Debug, Args)]
pub struct SearchArgs {
    pub query: String,
    /// Workspace root that owns the canonical index
    #[arg(long, value_name = "DIR")]
    pub workspace: Option<PathBuf>,
    /// Restrict the search to one or more subtrees.
    ///
    /// Accepts several values because `--path crates fork-core/src` is the natural way to
    /// write it, and rejecting that with clap's "unexpected argument" sent agents straight
    /// to `raw rg` and kept them there.
    #[arg(long, value_name = "PATH", num_args = 1..)]
    pub path: Vec<PathBuf>,
    /// Maximum number of hits to return (1-100)
    #[arg(long, default_value_t = 10, value_parser = parse_limit)]
    pub limit: usize,
    /// Search mode: exact, semantic, or auto
    #[arg(long, value_enum)]
    pub mode: Option<SearchModeArg>,
    /// Include matching file content snippets in the response
    #[arg(long)]
    pub include_content: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum RecallScopeArg {
    /// Only this repository.
    Project,
    /// Only your user-global memory.
    Global,
    /// This repository plus your user-global memory.
    ProjectAndGlobal,
}

impl From<RecallScopeArg> for MemoryScopeSelector {
    fn from(value: RecallScopeArg) -> Self {
        match value {
            RecallScopeArg::Project => Self::Project,
            RecallScopeArg::Global => Self::Global,
            RecallScopeArg::ProjectAndGlobal => Self::ProjectAndGlobal,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum StoreScopeArg {
    Project,
    Global,
}

impl From<StoreScopeArg> for MemoryWriteScope {
    fn from(value: StoreScopeArg) -> Self {
        match value {
            StoreScopeArg::Project => Self::Project,
            StoreScopeArg::Global => Self::Global,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum MemoryCommand {
    #[command(
        about = "Recall relevant memories with full ICM semantics",
        long_about = MEMORY_RECALL_LONG_ABOUT,
        after_help = MEMORY_RECALL_AFTER_HELP
    )]
    Recall {
        query: String,
        /// Workspace root that selects the project memory namespace
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
        /// Restrict recall to one ICM topic
        #[arg(long)]
        topic: Option<String>,
        /// Restrict recall to memories tagged with this keyword
        #[arg(long)]
        keyword: Option<String>,
        /// Maximum number of memories to return (1-100)
        #[arg(long, default_value_t = 10, value_parser = parse_limit)]
        limit: usize,
        /// Which namespaces to reach. Defaults to this project plus your global memory
        #[arg(long, value_enum, default_value_t = RecallScopeArg::ProjectAndGlobal)]
        scope: RecallScopeArg,
    },
    #[command(
        about = "Store or update a centralized ICM memory",
        long_about = MEMORY_STORE_LONG_ABOUT,
        after_help = MEMORY_STORE_AFTER_HELP
    )]
    Store {
        topic: String,
        /// Workspace root that selects the project memory namespace
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
        /// Memory body text (mutually exclusive with `--file`)
        #[arg(value_name = "CONTENT", conflicts_with = "file")]
        content: Option<String>,
        /// Read memory body from a file instead of CONTENT
        #[arg(long, value_name = "PATH", conflicts_with = "content")]
        file: Option<PathBuf>,
        /// Retention importance for the stored memory
        #[arg(long, value_enum, default_value_t = ImportanceArg::Medium)]
        importance: ImportanceArg,
        /// Keyword tags attached to the memory (repeatable)
        #[arg(long = "keyword")]
        keywords: Vec<String>,
        /// Optional raw sidecar payload stored beside the memory body
        #[arg(long)]
        raw: Option<String>,
        /// `global` for a user-wide preference or rule; `project` (default) for a fact
        /// about this repository only
        #[arg(long, value_enum, default_value_t = StoreScopeArg::Project)]
        scope: StoreScopeArg,
    },
    #[command(
        about = "Delete one memory after namespace ownership is verified",
        long_about = MEMORY_FORGET_LONG_ABOUT,
        after_help = MEMORY_FORGET_AFTER_HELP
    )]
    Forget {
        id: String,
        /// Workspace root that selects the project memory namespace
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
        /// Namespace that must own the memory id
        #[arg(long, value_enum, default_value_t = StoreScopeArg::Project)]
        scope: StoreScopeArg,
    },
    #[command(
        about = "Replace one memory after namespace ownership is verified",
        long_about = MEMORY_UPDATE_LONG_ABOUT,
        after_help = MEMORY_UPDATE_AFTER_HELP
    )]
    Update {
        id: String,
        /// Workspace root that selects the project memory namespace
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
        /// Replacement body text (mutually exclusive with `--file`)
        #[arg(value_name = "CONTENT", conflicts_with = "file")]
        content: Option<String>,
        /// Read the replacement body from a file instead of CONTENT
        #[arg(long, value_name = "PATH", conflicts_with = "content")]
        file: Option<PathBuf>,
        /// Optional new retention importance
        #[arg(long, value_enum)]
        importance: Option<ImportanceArg>,
        /// Replace keyword tags (repeatable)
        #[arg(long = "keyword")]
        keywords: Option<Vec<String>>,
        /// Namespace that must own the memory id
        #[arg(long, value_enum, default_value_t = StoreScopeArg::Project)]
        scope: StoreScopeArg,
    },
    #[command(
        about = "Delete low-weight memories only in the selected namespace",
        long_about = MEMORY_PRUNE_LONG_ABOUT,
        after_help = MEMORY_PRUNE_AFTER_HELP
    )]
    Prune {
        /// Workspace root that selects the project memory namespace
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
        /// Delete or preview memories at or below this weight
        #[arg(long, default_value_t = 0.1)]
        threshold: f32,
        /// Delete selected records; without this flag prune only previews
        #[arg(long)]
        apply: bool,
        /// Namespace to prune
        #[arg(long, value_enum, default_value_t = StoreScopeArg::Project)]
        scope: StoreScopeArg,
    },
    #[command(
        about = "Show ICM state reported by hzrd",
        long_about = MEMORY_STATUS_LONG_ABOUT,
        after_help = MEMORY_STATUS_AFTER_HELP
    )]
    Status,
}

#[derive(Debug, Subcommand)]
pub enum ExecCommand {
    #[command(about = "Show the RTK/HZR rewrite decision without executing")]
    Rewrite(ExecArgs),
    #[command(about = "Execute through the canonical policy and capture pipeline")]
    Run(ExecArgs),
    #[command(about = "Approve and execute one pending fork-core decision")]
    Approve { decision_id: String },
    #[command(about = "Deny and consume one pending fork-core decision")]
    Deny { decision_id: String },
}

#[derive(Clone, Debug, Args)]
pub struct ExecArgs {
    #[arg(
        value_name = "SHELL_COMMAND",
        help = "One shell string; quote it to preserve pipes, redirects, and spaces"
    )]
    pub command: String,
    /// Working directory for the rewritten or executed command
    #[arg(long, value_name = "DIR")]
    pub cwd: Option<PathBuf>,
    /// Kill the command after this many milliseconds
    #[arg(long)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Subcommand)]
pub enum CodecCommand {
    #[command(about = "Apply a safe transform; reads stdin when TEXT and --file are absent")]
    Compile {
        /// Input text to transform (mutually exclusive with `--file`)
        #[arg(value_name = "TEXT", conflicts_with = "file")]
        text: Option<String>,
        /// Read input text from a file instead of TEXT or stdin
        #[arg(long, value_name = "PATH", conflicts_with = "text")]
        file: Option<PathBuf>,
        /// Fidelity class that bounds how lossy the transform may be
        #[arg(long, value_enum, default_value_t = FidelityArg::Semantic)]
        fidelity: FidelityArg,
        /// Optional codec profile override
        #[arg(long, value_enum)]
        profile: Option<ProfileArg>,
        /// Risk class that gates irreversible transforms
        #[arg(long, value_enum, default_value_t = RiskArg::Low)]
        risk: RiskArg,
    },
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    #[command(
        about = "Run one managed prompt; reads stdin when PROMPT and --file are absent",
        long_about = AGENT_RUN_LONG_ABOUT,
        after_help = AGENT_RUN_AFTER_HELP
    )]
    Run {
        /// Prompt text (mutually exclusive with `--file`)
        #[arg(value_name = "PROMPT", conflicts_with = "file")]
        prompt: Option<String>,
        /// Read the prompt from a file instead of PROMPT or stdin
        #[arg(long, value_name = "PATH", conflicts_with = "prompt")]
        file: Option<PathBuf>,
        /// Workspace root the managed agent is bound to
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
        /// Maximum agent turns before the run stops
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=100))]
        max_turns: u32,
        /// Response shape returned by the managed agent
        #[arg(long, value_enum, default_value_t = ResponseFormatArg::Text)]
        response_format: ResponseFormatArg,
        /// Abort the managed run after this many milliseconds
        #[arg(long, default_value_t = 1_800_000)]
        timeout_ms: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum MigrateCommand {
    #[command(about = "Report legacy indexes, memory, settings, wrappers, and process markers")]
    Scan {
        /// Workspace root to scan for legacy markers
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
    },
    #[command(about = "Move one legacy .grepai into HZR with a retained backup and manifest")]
    Apply {
        /// Workspace root whose legacy `.grepai` should be moved
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
    },
    #[command(about = "Archive one explicitly selected nested .grepai with a hash manifest")]
    ArchiveIndex {
        /// Parent workspace that currently reports the duplicate
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
        /// Exact nested `.grepai` directory to archive
        #[arg(long, value_name = "PATH")]
        source: PathBuf,
        /// Preview the hash, backup, and manifest without writing
        #[arg(long)]
        dry_run: bool,
        /// Apply the archive after reviewing the dry-run receipt
        #[arg(long, conflicts_with = "dry_run")]
        force: bool,
    },
    #[command(
        about = "Snapshot and idempotently import platform RTK history into the canonical ledger"
    )]
    History {
        /// Preview the history import without writing the ledger
        #[arg(long)]
        dry_run: bool,
        /// Perform the history import
        #[arg(long, conflicts_with = "dry_run")]
        force: bool,
    },
    #[command(about = "Snapshot and idempotently import the legacy ICM database into HZR memory")]
    Memory {
        /// Workspace root used when resolving the legacy ICM database
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
        /// Preview the memory import without writing HZR memory
        #[arg(long)]
        dry_run: bool,
        /// Perform the memory import
        #[arg(long, conflicts_with = "dry_run")]
        force: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum SearchModeArg {
    Exact,
    Semantic,
    Auto,
}

impl From<SearchModeArg> for SearchMode {
    fn from(value: SearchModeArg) -> Self {
        match value {
            SearchModeArg::Exact => Self::Exact,
            SearchModeArg::Semantic => Self::Semantic,
            SearchModeArg::Auto => Self::Auto,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ImportanceArg {
    Critical,
    High,
    Medium,
    Low,
}

impl From<ImportanceArg> for MemoryImportance {
    fn from(value: ImportanceArg) -> Self {
        match value {
            ImportanceArg::Critical => Self::Critical,
            ImportanceArg::High => Self::High,
            ImportanceArg::Medium => Self::Medium,
            ImportanceArg::Low => Self::Low,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum FidelityArg {
    Exact,
    Structural,
    Semantic,
    Summary,
}

impl From<FidelityArg> for FidelityClass {
    fn from(value: FidelityArg) -> Self {
        match value {
            FidelityArg::Exact => Self::Exact,
            FidelityArg::Structural => Self::LosslessStructural,
            FidelityArg::Semantic => Self::Semantic,
            FidelityArg::Summary => Self::Summary,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ProfileArg {
    Off,
    Safe,
    Adaptive,
    Compact,
    Shadow,
}

impl From<ProfileArg> for CodecProfile {
    fn from(value: ProfileArg) -> Self {
        match value {
            ProfileArg::Off => Self::Off,
            ProfileArg::Safe => Self::Safe,
            ProfileArg::Adaptive => Self::Adaptive,
            ProfileArg::Compact => Self::Compact,
            ProfileArg::Shadow => Self::Shadow,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum RiskArg {
    Low,
    Medium,
    High,
    Irreversible,
}

impl From<RiskArg> for RiskClass {
    fn from(value: RiskArg) -> Self {
        match value {
            RiskArg::Low => Self::Low,
            RiskArg::Medium => Self::Medium,
            RiskArg::High => Self::High,
            RiskArg::Irreversible => Self::Irreversible,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ResponseFormatArg {
    Text,
    Json,
}

impl From<ResponseFormatArg> for ResponseFormat {
    fn from(value: ResponseFormatArg) -> Self {
        match value {
            ResponseFormatArg::Text => Self::Text,
            ResponseFormatArg::Json => Self::Json,
        }
    }
}

fn parse_limit(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| "limit must be an integer between 1 and 100".to_owned())?;
    if (1..=100).contains(&parsed) {
        Ok(parsed)
    } else {
        Err("limit must be between 1 and 100".into())
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{
        ActivationCommand, Cli, Command, ContextCommand, DaemonCommand, ExecCommand, HooksCommand,
        IndexCommand, McpClientArg, McpCommand, MigrateCommand, ServiceCommand,
    };

    fn root_help() -> String {
        Cli::command().render_long_help().to_string()
    }

    fn strip_ansi(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\u{1b}' {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for next in chars.by_ref() {
                        if next.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                continue;
            }
            out.push(ch);
        }
        out
    }

    /// Fleet reconciliation has to see which workspaces would be written before writing to
    /// any of them, so the plan and the "only where needed" guard must combine.
    #[test]
    fn init_plans_without_writing_where_reconciliation_is_needed() {
        let cli = Cli::try_parse_from(["hzr", "init", "--dry-run", "--if-needed"])
            .expect("--dry-run must combine with --if-needed");
        assert!(matches!(
            cli.command,
            Command::Init {
                dry_run: true,
                if_needed: true,
                ..
            }
        ));
        assert!(
            Cli::try_parse_from(["hzr", "init", "--dry-run", "--if-enabled"]).is_err(),
            "--if-enabled decides activation, which a plan cannot answer"
        );
    }

    #[test]
    fn test_cli_parses_doctor_fix() {
        let cli = Cli::parse_from(["hzr", "doctor", "--fix", "--workspace", "/tmp/project"]);
        assert!(matches!(
            cli.command,
            Command::Doctor {
                workspace: Some(_),
                fix: true,
                ..
            }
        ));
        let fleet = Cli::try_parse_from([
            "hzr",
            "doctor",
            "--reconcile-fleet",
            "--migrate-legacy-indexes",
            "--dry-run",
        ])
        .expect("explicit fleet migration preview");
        assert!(matches!(
            fleet.command,
            Command::Doctor {
                reconcile_fleet: true,
                migrate_legacy_indexes: true,
                dry_run: true,
                ..
            }
        ));
        assert!(
            Cli::try_parse_from(["hzr", "doctor", "--migrate-legacy-indexes"]).is_err(),
            "fleet migration must require the registered-workspace reconciliation boundary"
        );
    }

    #[test]
    fn test_help_ux_root_groups_top_level_commands() {
        let help = strip_ansi(&root_help());
        for heading in [
            "Setup:",
            "Runtime:",
            "Search & Memory:",
            "Agent tools:",
            "Distribution:",
            "Legacy:",
        ] {
            assert!(
                help.contains(heading),
                "root help missing command group {heading}\n{help}"
            );
        }
        assert!(
            help.find("Setup:").expect("Setup") < help.find("Runtime:").expect("Runtime"),
            "Setup should precede Runtime"
        );
        assert!(
            help.contains("init") && help.contains("doctor"),
            "Setup group should list setup commands"
        );
        assert!(
            help.contains("search") && help.contains("memory"),
            "Search & Memory group should list search/memory"
        );
        assert!(
            help.contains("migrate") && help.contains("rtk"),
            "Legacy group should list migrate/rtk"
        );
    }

    #[test]
    fn test_help_ux_root_includes_examples_footer() {
        let help = strip_ansi(&root_help());
        assert!(
            help.contains("Examples:"),
            "root help missing Examples footer\n{help}"
        );
        for fragment in [
            "hzr init",
            "hzr install",
            "hzr doctor",
            "hzr search",
            "hzr stats",
        ] {
            assert!(
                help.contains(fragment),
                "Examples footer missing `{fragment}`\n{help}"
            );
        }
        let examples_at = help.find("Examples:").expect("Examples");
        let options_at = help.find("Options:").expect("Options");
        assert!(
            options_at < examples_at,
            "Examples should be a footer after Options"
        );
    }

    #[test]
    fn test_help_ux_global_flags_have_help_text() {
        let cmd = Cli::command();
        for name in ["config", "json"] {
            let arg = cmd
                .get_arguments()
                .find(|arg| arg.get_long() == Some(name))
                .expect("global flag must exist on Cli");
            let help = arg
                .get_help()
                .map(|text| text.to_string())
                .unwrap_or_default();
            assert!(
                !help.trim().is_empty(),
                "global --{name} must have help text"
            );
        }
    }

    #[test]
    fn test_help_ux_frequent_flags_expose_non_empty_help() {
        let cmd = Cli::command();
        let mut commands = vec![&cmd];
        collect_subcommands(&cmd, &mut commands);

        let mut missing = Vec::new();
        for name in [
            "force",
            "dry-run",
            "workspace",
            "cwd",
            "limit",
            "mode",
            "include-content",
            "keep-data",
            "file",
            "keyword",
            "raw",
            "timeout-ms",
            "install-root",
            "data-dir",
            "if-needed",
            "quiet",
            "config",
            "json",
        ] {
            let mut seen = 0usize;
            let mut empty = 0usize;
            for command in &commands {
                for arg in command.get_arguments() {
                    if arg.get_long() == Some(name) || arg.get_id().as_str() == name {
                        seen += 1;
                        let help = arg
                            .get_help()
                            .map(|text| text.to_string())
                            .unwrap_or_default();
                        if help.trim().is_empty() {
                            empty += 1;
                        }
                    }
                }
            }
            if seen == 0 || empty > 0 {
                missing.push(format!("{name} (seen={seen}, empty={empty})"));
            }
        }
        assert!(
            missing.is_empty(),
            "flags missing non-empty help: {missing:?}"
        );
    }

    #[test]
    fn test_help_ux_groups_list_every_visible_command() {
        let help = strip_ansi(&root_help());
        let cmd = Cli::command();
        let mut missing = Vec::new();
        for sub in cmd.get_subcommands().filter(|sub| !sub.is_hide_set()) {
            let name = sub.get_name();
            if name == "help" {
                continue;
            }
            if !help.contains(name) {
                missing.push(name.to_owned());
            }
        }
        assert!(
            missing.is_empty(),
            "grouped root help missing visible commands: {missing:?}"
        );
    }

    #[test]
    fn test_help_ux_root_command_about_strings_stay_compact() {
        let cmd = Cli::command();
        let mut too_long = Vec::new();
        for sub in cmd.get_subcommands().filter(|sub| !sub.is_hide_set()) {
            let about = sub
                .get_about()
                .map(|text| text.to_string())
                .unwrap_or_default();
            if about.chars().count() > 60 {
                too_long.push(format!("{} ({})", sub.get_name(), about.chars().count()));
            }
        }
        assert!(
            too_long.is_empty(),
            "root command about strings should be <= 60 chars: {too_long:?}"
        );
    }

    fn subcommand_long_help(path: &[&str]) -> String {
        let mut command = Cli::command();
        let mut current = &mut command;
        for name in path {
            current = current
                .find_subcommand_mut(name)
                .expect("missing subcommand");
        }
        strip_ansi(&current.render_long_help().to_string())
    }

    fn assert_help_has_examples(help: &str, command: &str, fragments: &[&str]) {
        assert!(
            help.contains("Examples:"),
            "{command} help missing Examples footer\n{help}"
        );
        for fragment in fragments {
            assert!(
                help.contains(fragment),
                "{command} Examples missing `{fragment}`\n{help}"
            );
        }
    }

    #[test]
    fn test_subcommand_help_update_includes_long_about_and_examples() {
        let help = subcommand_long_help(&["update"]);
        assert!(
            help.contains("SHA-256") || help.contains("checksum"),
            "update help should explain verification\n{help}"
        );
        assert_help_has_examples(&help, "update", &["hzr update"]);
    }

    #[test]
    fn test_subcommand_help_stats_includes_long_about_and_examples() {
        let help = subcommand_long_help(&["stats"]);
        assert!(
            help.contains("ledger") || help.contains("zero-redundancy"),
            "stats help should explain the ledger view\n{help}"
        );
        assert_help_has_examples(&help, "stats", &["hzr stats", "--workspace"]);
    }

    #[test]
    fn test_subcommand_help_enable_disable_doctor_include_examples() {
        for (path, fragment) in [
            (&["enable"][..], "hzr enable"),
            (&["disable"][..], "hzr disable"),
            (&["doctor"][..], "hzr doctor"),
        ] {
            let help = subcommand_long_help(path);
            assert_help_has_examples(&help, path[0], &[fragment]);
            assert!(
                help.contains("project-only")
                    || help.contains("index")
                    || help.contains("ownership")
                    || help.contains("pins"),
                "{} help should expand beyond a one-liner\n{help}",
                path[0]
            );
        }
    }

    #[test]
    fn test_subcommand_help_agent_and_mcp_leaves_include_examples() {
        let agent = subcommand_long_help(&["agent"]);
        assert_help_has_examples(&agent, "agent", &["hzr agent run"]);

        let agent_run = subcommand_long_help(&["agent", "run"]);
        assert_help_has_examples(&agent_run, "agent run", &["hzr agent run"]);

        let mcp_serve = subcommand_long_help(&["mcp", "serve"]);
        assert_help_has_examples(&mcp_serve, "mcp serve", &["hzr mcp serve"]);

        let mcp_config = subcommand_long_help(&["mcp", "config"]);
        assert_help_has_examples(
            &mcp_config,
            "mcp config",
            &["hzr mcp config", "--workspace"],
        );

        let mcp_status = subcommand_long_help(&["mcp", "status"]);
        assert_help_has_examples(&mcp_status, "mcp status", &["hzr mcp status"]);
    }

    #[test]
    fn test_subcommand_help_memory_leaves_include_examples() {
        for (path, fragment) in [
            (&["memory", "recall"][..], "hzr memory recall"),
            (&["memory", "store"][..], "hzr memory store"),
            (&["memory", "forget"][..], "hzr memory forget"),
            (&["memory", "update"][..], "hzr memory update"),
            (&["memory", "prune"][..], "hzr memory prune"),
            (&["memory", "status"][..], "hzr memory status"),
        ] {
            let help = subcommand_long_help(path);
            assert_help_has_examples(&help, &path.join(" "), &[fragment]);
        }
    }

    #[test]
    fn test_subcommand_help_high_traffic_flags_have_non_empty_help() {
        let mut missing = Vec::new();
        let cases: &[(&[&str], &[&str])] = &[
            (&[], &["config", "json"]),
            (&["enable"], &["workspace"]),
            (&["disable"], &["workspace"]),
            (&["doctor"], &["workspace", "fix"]),
            (&["stats"], &["workspace"]),
            (&["mcp", "serve"], &["workspace"]),
            (&["mcp", "config"], &["client", "workspace"]),
            (
                &["agent", "run"],
                &[
                    "file",
                    "workspace",
                    "max-turns",
                    "response-format",
                    "timeout-ms",
                ],
            ),
            (
                &["memory", "recall"],
                &["workspace", "topic", "keyword", "limit", "scope"],
            ),
            (
                &["memory", "store"],
                &["workspace", "file", "importance", "keyword", "raw", "scope"],
            ),
        ];

        for (path, flags) in cases {
            let mut command = Cli::command();
            let mut current = &mut command;
            for name in *path {
                current = current
                    .find_subcommand_mut(name)
                    .expect("missing subcommand");
            }
            for flag in *flags {
                let Some(arg) = current
                    .get_arguments()
                    .find(|argument| argument.get_long() == Some(*flag))
                else {
                    missing.push(format!("{} --{flag} (absent)", path.join(" ")));
                    continue;
                };
                let help = arg
                    .get_help()
                    .map(|text| text.to_string())
                    .unwrap_or_default();
                if help.trim().is_empty() {
                    missing.push(format!("{} --{flag}", path.join(" ")));
                }
            }
        }

        assert!(
            missing.is_empty(),
            "flags missing non-empty help: {missing:?}"
        );
    }

    fn collect_subcommands<'a>(cmd: &'a clap::Command, out: &mut Vec<&'a clap::Command>) {
        for sub in cmd.get_subcommands() {
            out.push(sub);
            collect_subcommands(sub, out);
        }
    }

    /// `hzr search q --mode exact --path crates fork-core/src` used to fail with clap's
    /// "unexpected argument", which is the exact moment an agent gives up on `hzr search`
    /// and switches to `raw rg` for the rest of the session.
    #[test]
    fn test_search_accepts_several_paths_instead_of_rejecting_the_second() {
        let cli = Cli::try_parse_from([
            "hzr",
            "search",
            "RewriteDecision",
            "--mode",
            "exact",
            "--path",
            "crates",
            "fork-core/src",
        ])
        .expect("multiple search paths are valid");

        assert!(matches!(
            cli.command,
            Command::Search(ref arguments)
                if arguments.path
                    == [
                        std::path::PathBuf::from("crates"),
                        std::path::PathBuf::from("fork-core/src"),
                    ]
        ));
    }

    #[test]
    fn test_search_without_a_path_still_covers_the_whole_workspace() {
        let cli = Cli::try_parse_from(["hzr", "search", "needle"]).expect("bare search");

        assert!(matches!(
            cli.command,
            Command::Search(ref arguments) if arguments.path.is_empty()
        ));
    }

    #[test]
    fn acceptance_gate_read_write_have_first_class_hzr_aliases() {
        let read = Cli::try_parse_from(["hzr", "read", "README.md", "--outline"])
            .expect("first-class read alias");
        let write = Cli::try_parse_from([
            "hzr",
            "write",
            "patch",
            "README.md",
            "--patch",
            "change.diff",
        ])
        .expect("first-class write alias");

        assert!(matches!(
            read.command,
            Command::Read(ref arguments)
                if arguments.args
                    == [
                        std::ffi::OsString::from("README.md"),
                        std::ffi::OsString::from("--outline"),
                    ]
        ));
        assert!(matches!(
            write.command,
            Command::Write(ref arguments)
                if arguments.args.first().and_then(|value| value.to_str()) == Some("patch")
                    && arguments.args.get(1).and_then(|value| value.to_str())
                        == Some("README.md")
        ));
    }

    #[test]
    fn acceptance_gate_test_and_native_policy_have_first_class_cli_surfaces() {
        let test = Cli::try_parse_from(["hzr", "test", "bun", "test", "--watch"])
            .expect("first-class test alias");
        assert!(matches!(
            test.command,
            Command::Test(ref arguments)
                if arguments.args
                    == [
                        std::ffi::OsString::from("bun"),
                        std::ffi::OsString::from("test"),
                        std::ffi::OsString::from("--watch"),
                    ]
        ));

        let install = Cli::try_parse_from([
            "hzr",
            "install",
            "--dry-run",
            "--native-tool-mode",
            "strict",
        ])
        .expect("strict native mode");
        assert!(matches!(
            install.command,
            Command::Install {
                native_tool_mode: Some(crate::adoption::NativeToolMode::Strict),
                ..
            }
        ));
    }

    #[test]
    fn test_cli_parses_adoption_and_idempotent_init_surface() {
        let install =
            Cli::try_parse_from(["hzr", "install", "--dry-run"]).expect("valid install preview");
        let hooks = Cli::try_parse_from(["hzr", "hooks", "status", "--json"]).expect("hook status");
        let init = Cli::try_parse_from(["hzr", "init", "--if-needed", "--quiet"])
            .expect("idempotent init");

        assert!(matches!(
            install.command,
            Command::Install {
                dry_run: true,
                force: false,
                ..
            }
        ));
        assert!(hooks.json);
        assert!(matches!(
            hooks.command,
            Command::Hooks {
                command: HooksCommand::Status
            }
        ));
        assert!(matches!(
            init.command,
            Command::Init {
                if_needed: true,
                quiet: true,
                ..
            }
        ));
    }

    #[test]
    fn test_init_reset_is_explicit_and_dry_run_is_first_class() {
        let init = Cli::try_parse_from(["hzr", "init", "--force", "--reset", "--dry-run"])
            .expect("explicit reset preview");
        assert!(matches!(
            init.command,
            Command::Init {
                force: true,
                reset: true,
                dry_run: true,
                ..
            }
        ));
        assert!(Cli::try_parse_from(["hzr", "init", "--reset"]).is_err());
    }

    #[test]
    fn test_cli_exposes_first_class_project_only_activation_and_scoped_stats() {
        let install = Cli::try_parse_from(["hzr", "install", "--project-only", "--dry-run"])
            .expect("project-only install preview");
        let enable = Cli::try_parse_from(["hzr", "enable", "--workspace", "/work/app"])
            .expect("enable one workspace");
        let disable = Cli::try_parse_from(["hzr", "disable", "--workspace", "/work/app"])
            .expect("disable one workspace");
        let stats = Cli::try_parse_from(["hzr", "stats", "--workspace", "/work/app"])
            .expect("project-scoped stats");

        assert!(matches!(
            install.command,
            Command::Install {
                project_only: true,
                ..
            }
        ));
        assert!(matches!(
            enable.command,
            Command::Enable { workspace: Some(ref path) }
                if path == std::path::Path::new("/work/app")
        ));
        assert!(matches!(
            disable.command,
            Command::Disable { workspace: Some(ref path) }
                if path == std::path::Path::new("/work/app")
        ));
        assert!(matches!(
            stats.command,
            Command::Stats { workspace: Some(ref path), .. }
                if path == std::path::Path::new("/work/app")
        ));
    }

    #[test]
    fn acceptance_gate_stats_since_accepts_h_d_w_and_rejects_ambiguous_values() {
        for (value, expected_seconds) in [
            ("6h", 6 * 60 * 60),
            ("7d", 7 * 24 * 60 * 60),
            ("4w", 4 * 7 * 24 * 60 * 60),
        ] {
            let cli = Cli::try_parse_from(["hzr", "stats", "--since", value])
                .expect("valid stats duration");
            assert!(matches!(
                cli.command,
                Command::Stats { since: Some(ref duration), .. }
                    if duration.seconds() == expected_seconds && duration.label() == value
            ));
        }

        for value in [
            "",
            "0h",
            "1",
            "1H",
            "1.5d",
            "-1d",
            "1 d",
            "999999999999999999999w",
        ] {
            assert!(
                Cli::try_parse_from(["hzr", "stats", "--since", value]).is_err(),
                "duration {value:?} must be rejected deterministically"
            );
        }
    }

    #[test]
    fn acceptance_gate_stats_exposes_bounded_evasion_view() {
        let cli = Cli::try_parse_from(["hzr", "stats", "--evasion", "--since", "7d"])
            .expect("evasion stats");
        assert!(matches!(
            cli.command,
            Command::Stats {
                evasion: true,
                since: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn test_cli_parses_activation_status() {
        let cli = Cli::try_parse_from(["hzr", "activation", "status", "--json"])
            .expect("activation status");

        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Command::Activation {
                command: ActivationCommand::Status
            }
        ));
    }

    #[test]
    fn test_cli_accepts_global_json_after_subcommand() {
        let cli = Cli::try_parse_from(["hzr", "daemon", "status", "--json"])
            .expect("valid daemon command");

        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Command::Daemon {
                command: DaemonCommand::Status
            }
        ));
    }

    #[test]
    fn test_cli_parses_native_mcp_status() {
        let cli =
            Cli::try_parse_from(["hzr", "mcp", "status", "--json"]).expect("native MCP status");

        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Command::Mcp {
                command: McpCommand::Status
            }
        ));
    }

    /// `--apply` is the write path for a pinned registration; without it `mcp config` stays
    /// print-only so pasting a snippet remains available.
    #[test]
    fn test_cli_parses_mcp_config_apply_with_workspace() {
        let cli = Cli::try_parse_from([
            "hzr",
            "mcp",
            "config",
            "--client",
            "claude-desktop",
            "--workspace",
            "/Users/andrew/code/app",
            "--apply",
        ])
        .expect("mcp config apply");

        assert!(matches!(
            cli.command,
            Command::Mcp {
                command: McpCommand::Config {
                    client: McpClientArg::ClaudeDesktop,
                    apply: true,
                    workspace: Some(ref path),
                }
            } if path == std::path::Path::new("/Users/andrew/code/app")
        ));
    }

    #[test]
    fn test_cli_parses_native_tdd_contract() {
        let cli = Cli::try_parse_from(["hzr", "tdd", "--json"]).expect("native TDD contract");

        assert!(cli.json);
        assert!(matches!(cli.command, Command::Tdd));
    }

    #[test]
    fn test_cli_parses_update_command() {
        let cli = Cli::try_parse_from(["hzr", "update"]).expect("native update command");

        assert!(matches!(cli.command, Command::Update { check: false }));
    }

    #[test]
    fn test_cli_parses_update_check_without_installing() {
        let cli = Cli::try_parse_from(["hzr", "update", "--check"]).expect("update --check");

        assert!(matches!(cli.command, Command::Update { check: true }));
    }

    #[test]
    fn test_update_check_help_documents_agent_friendly_exit_codes() {
        let help = Cli::command()
            .find_subcommand_mut("update")
            .expect("update subcommand")
            .render_long_help()
            .to_string();

        assert!(help.contains("--check"));
        assert!(help.contains("without downloading or installing"));
        assert!(help.contains("Exit status"));
        assert!(help.contains("Non-zero only when the check fails"));
    }

    #[test]
    fn test_cli_parses_daemon_service_lifecycle() {
        let cli = Cli::try_parse_from(["hzr", "daemon", "service", "restart", "--json"])
            .expect("service restart");
        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Command::Daemon {
                command: DaemonCommand::Service {
                    command: ServiceCommand::Restart
                }
            }
        ));
    }

    #[test]
    fn test_cli_preserves_quoted_shell_command_as_one_value() {
        let cli = Cli::try_parse_from(["hzr", "exec", "run", "git status --short"])
            .expect("valid exec command");

        assert!(matches!(
            &cli.command,
            Command::Exec {
                command: ExecCommand::Run(_)
            }
        ));
        if let Command::Exec {
            command: ExecCommand::Run(arguments),
        } = cli.command
        {
            assert_eq!(arguments.command, "git status --short");
        }
    }

    #[test]
    fn test_cli_accepts_explicit_fork_approval_decisions() {
        let approve = Cli::try_parse_from(["hzr", "exec", "approve", "decision-1"])
            .expect("valid approval command");
        let deny = Cli::try_parse_from(["hzr", "exec", "deny", "decision-2"])
            .expect("valid denial command");

        assert!(matches!(
            approve.command,
            Command::Exec {
                command: ExecCommand::Approve { ref decision_id }
            } if decision_id == "decision-1"
        ));
        assert!(matches!(
            deny.command,
            Command::Exec {
                command: ExecCommand::Deny { ref decision_id }
            } if decision_id == "decision-2"
        ));
    }

    #[test]
    fn test_cli_parses_bounded_context_plan_inputs() {
        let cli = Cli::try_parse_from([
            "hzr",
            "context",
            "plan",
            "authentication flow",
            "--workspace",
            "/workspace",
            "--path",
            "src/auth",
            "--topic",
            "architecture",
            "--search-limit",
            "7",
            "--memory-limit",
            "3",
        ])
        .expect("valid context plan command");

        assert!(matches!(
            cli.command,
            Command::Context {
                command: ContextCommand::Plan(ref arguments)
            } if arguments.intent == "authentication flow"
                && arguments.workspace.as_deref() == Some(std::path::Path::new("/workspace"))
                && arguments.path.as_deref() == Some(std::path::Path::new("src/auth"))
                && arguments.topic.as_deref() == Some("architecture")
                && arguments.search_limit == 7
                && arguments.memory_limit == 3
        ));
    }

    #[test]
    fn test_cli_parses_index_lifecycle_and_explicit_migration_apply() {
        let index = Cli::try_parse_from(["hzr", "index", "init", "--workspace", "/workspace"])
            .expect("valid index init command");
        let migration =
            Cli::try_parse_from(["hzr", "migrate", "apply", "--workspace", "/workspace"])
                .expect("valid migration apply command");
        let archive = Cli::try_parse_from([
            "hzr",
            "migrate",
            "archive-index",
            "--workspace",
            "/workspace",
            "--source",
            "/workspace/vendor/.grepai",
            "--dry-run",
        ])
        .expect("valid explicit duplicate archive preview");

        assert!(matches!(
            index.command,
            Command::Index {
                command: IndexCommand::Init { ref workspace }
            } if workspace.as_deref() == Some(std::path::Path::new("/workspace"))
        ));
        assert!(matches!(
            archive.command,
            Command::Migrate {
                command: MigrateCommand::ArchiveIndex {
                    dry_run: true,
                    force: false,
                    ..
                }
            }
        ));
        assert!(matches!(
            migration.command,
            Command::Migrate {
                command: MigrateCommand::Apply { ref workspace }
            } if workspace.as_deref() == Some(std::path::Path::new("/workspace"))
        ));
    }

    #[test]
    fn test_cli_requires_explicit_history_migration_mode() {
        let preview = Cli::try_parse_from(["hzr", "migrate", "history", "--dry-run"])
            .expect("history preview");
        let apply =
            Cli::try_parse_from(["hzr", "migrate", "history", "--force"]).expect("history apply");

        assert!(matches!(
            preview.command,
            Command::Migrate {
                command: MigrateCommand::History {
                    dry_run: true,
                    force: false
                }
            }
        ));
        assert!(matches!(
            apply.command,
            Command::Migrate {
                command: MigrateCommand::History {
                    dry_run: false,
                    force: true
                }
            }
        ));
    }

    #[test]
    fn test_cli_memory_prune_requires_apply_for_deletion() {
        let preview =
            Cli::try_parse_from(["hzr", "memory", "prune"]).expect("memory prune preview");
        let apply =
            Cli::try_parse_from(["hzr", "memory", "prune", "--apply"]).expect("memory prune apply");

        assert!(matches!(
            preview.command,
            Command::Memory {
                command: super::MemoryCommand::Prune { apply: false, .. }
            }
        ));
        assert!(matches!(
            apply.command,
            Command::Memory {
                command: super::MemoryCommand::Prune { apply: true, .. }
            }
        ));
    }

    #[test]
    fn test_cli_memory_scope_comes_from_workspace_not_project_override() {
        let recall = Cli::try_parse_from([
            "hzr",
            "memory",
            "recall",
            "decision",
            "--workspace",
            "/workspace",
        ])
        .expect("valid scoped memory recall");

        assert!(matches!(
            recall.command,
            Command::Memory {
                command: super::MemoryCommand::Recall { ref workspace, .. }
            } if workspace.as_deref() == Some(std::path::Path::new("/workspace"))
        ));
        assert!(
            Cli::try_parse_from([
                "hzr",
                "memory",
                "recall",
                "decision",
                "--project",
                "foreign",
            ])
            .is_err()
        );
    }
}
