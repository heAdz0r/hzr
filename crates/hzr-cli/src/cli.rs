use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use hzr_agent::ResponseFormat;
use hzr_protocol::{
    CodecProfile, FidelityClass, MemoryImportance, MemoryScopeSelector, MemoryWriteScope,
    RiskClass, SearchMode,
};

#[derive(Debug, Parser)]
#[command(name = "hzr", version, about = "Unified agent efficiency platform")]
pub struct Cli {
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(
        about = "Initialize the workspace registry, private data layout, and visualizer service"
    )]
    Init {
        #[arg(long)]
        force: bool,
        #[arg(long, value_name = "DIR")]
        data_dir: Option<PathBuf>,
        #[arg(long, conflicts_with = "force")]
        if_needed: bool,
        #[arg(long, requires = "if_needed")]
        quiet: bool,
        /// Register the workspace without installing or starting the production daemon.
        #[arg(long)]
        skip_service: bool,
    },
    #[command(
        about = "Adopt HZR: PATH binaries, one hook dispatcher, agent instructions, and visualizer service"
    )]
    Install {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        force: bool,
        /// Directory that receives durable `hzr`/`hzrd` binaries and must be on PATH.
        #[arg(long, value_name = "DIR")]
        prefix: Option<PathBuf>,
        /// Explicit binary the hooks should invoke instead of the resolved current executable.
        #[arg(long, value_name = "PATH")]
        binary: Option<PathBuf>,
        /// Allow hooks to point at a `target/debug` or `target/release` build (development only).
        #[arg(long)]
        allow_dev_path: bool,
        /// Keep external `icm hook` entries instead of centralizing memory ownership in HZR.
        #[arg(long)]
        keep_external_icm: bool,
        /// Skip `CLAUDE.md`/`AGENTS.md` instruction wiring and install hooks only.
        #[arg(long)]
        skip_instructions: bool,
        /// Skip service startup and keep the installed SessionStart hook from starting it.
        #[arg(long)]
        skip_service: bool,
    },
    #[command(about = "Remove HZR adoption hooks without restoring RTK implicitly")]
    Uninstall {
        #[arg(long)]
        keep_data: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        force: bool,
    },
    #[command(about = "Inspect or execute the single HZR hook dispatcher")]
    Hooks {
        #[command(subcommand)]
        command: HooksCommand,
    },
    #[command(about = "Verify pins, ownership, daemon health, and duplicate indexes")]
    Doctor {
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
    },
    #[command(about = "Operate the authenticated loopback daemon")]
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    #[command(about = "Inspect the engine manifest served by hzrd")]
    Engines {
        #[command(subcommand)]
        command: EnginesCommand,
    },
    #[command(about = "Inspect or initialize the one canonical grepai index")]
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },
    #[command(about = "Search through the canonical exact/semantic router")]
    Search(SearchArgs),
    #[command(about = "Use the rgai-compatible facade over the same canonical index")]
    Rgai(SearchArgs),
    #[command(about = "Plan bounded code and ICM context through the unified control plane")]
    Context {
        #[command(subcommand)]
        command: ContextCommand,
    },
    #[command(about = "Recall, store, or inspect centralized ICM memory")]
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    #[command(about = "Rewrite or execute a shell command through HZR policy")]
    Exec {
        #[command(subcommand)]
        command: ExecCommand,
    },
    #[command(about = "Compile protected response-density transforms")]
    Codec {
        #[command(subcommand)]
        command: CodecCommand,
    },
    #[command(about = "Run the managed caveman-code agent with HZR-owned tools")]
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    #[command(
        about = "Serve HZR-owned tools to external agents over stdio MCP (one store, no orphans)"
    )]
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    #[command(
        about = "Build this source tree into a bundle, install it globally, switch `current`, and verify every engine"
    )]
    Release {
        /// Synchronize every current version surface before building the release.
        #[arg(value_name = "VERSION")]
        version: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        force: bool,
        /// Keep the running daemon on the previous bundle instead of restarting it.
        #[arg(long)]
        skip_service: bool,
        #[arg(long, value_name = "DIR")]
        install_root: Option<PathBuf>,
    },
    #[command(about = "Download, verify, and install a newer GitHub release when available")]
    Update,
    #[command(about = "Print the strict HZR Red-Green-Refactor contract before implementation")]
    Tdd,
    /// Build *your* project through the inherited token-optimized wrapper.
    ///
    /// Deliberately kept as `build` rather than folded into `hzr rtk -- build`: the
    /// inherited fork already used this verb for project builds, so muscle memory from
    /// RTK keeps working. Building the HZR distribution itself is `hzr release`.
    #[command(
        about = "Build your project through the inherited fork-core build wrapper (token-optimized output)"
    )]
    Build(ForkForwardArgs),
    #[command(about = "Show global cumulative zero-redundancy gains and observed model usage")]
    Stats,
    #[command(hide = true)]
    Savings,
    #[command(about = "Inspect or safely centralize legacy state")]
    Migrate {
        #[command(subcommand)]
        command: MigrateCommand,
    },
    #[command(about = "Run the complete managed fork-core CLI without changing its arguments")]
    Rtk(RtkArgs),
}

#[derive(Debug, Subcommand)]
pub enum McpCommand {
    #[command(
        about = "Serve stdio MCP until the parent agent closes stdin; routes to the one HZR store"
    )]
    Serve {
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
    },
    #[command(about = "Print the MCP server registration snippet for an agent configuration")]
    Config {
        #[arg(long, value_enum, default_value_t = McpClientArg::Codex)]
        client: McpClientArg,
        /// Pin the project the server's memory is scoped to. Without it the namespace comes
        /// from whatever directory the client launched from, which is `/` for the Claude
        /// desktop app and a per-session directory for Codex.
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
    },
    #[command(about = "Report native client registrations and the client-managed stdio lifecycle")]
    Status,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum McpClientArg {
    Codex,
    ClaudeDesktop,
}

#[derive(Debug, Subcommand)]
pub enum HooksCommand {
    #[command(about = "Report HZR, legacy RTK, and external ICM hook ownership")]
    Status,
    #[command(hide = true)]
    Dispatch,
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
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
    },
    #[command(about = "Initialize the canonical index; hzrd remains the watcher owner")]
    Init {
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
    #[arg(long, value_name = "DIR")]
    pub workspace: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub path: Option<PathBuf>,
    #[arg(long)]
    pub topic: Option<String>,
    #[arg(long, default_value_t = 10, value_parser = parse_limit)]
    pub search_limit: usize,
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
    #[arg(long, value_name = "DIR")]
    pub workspace: Option<PathBuf>,
    /// Restrict the search to one or more subtrees.
    ///
    /// Accepts several values because `--path crates fork-core/src` is the natural way to
    /// write it, and rejecting that with clap's "unexpected argument" sent agents straight
    /// to `raw rg` and kept them there.
    #[arg(long, value_name = "PATH", num_args = 1..)]
    pub path: Vec<PathBuf>,
    #[arg(long, default_value_t = 10, value_parser = parse_limit)]
    pub limit: usize,
    #[arg(long, value_enum)]
    pub mode: Option<SearchModeArg>,
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
    #[command(about = "Recall relevant memories with full ICM semantics")]
    Recall {
        query: String,
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
        #[arg(long)]
        topic: Option<String>,
        #[arg(long)]
        keyword: Option<String>,
        #[arg(long, default_value_t = 10, value_parser = parse_limit)]
        limit: usize,
        /// Which namespaces to reach. Defaults to this project plus your global memory.
        #[arg(long, value_enum, default_value_t = RecallScopeArg::ProjectAndGlobal)]
        scope: RecallScopeArg,
    },
    #[command(about = "Store or update a centralized ICM memory")]
    Store {
        topic: String,
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
        #[arg(value_name = "CONTENT", conflicts_with = "file")]
        content: Option<String>,
        #[arg(long, value_name = "PATH", conflicts_with = "content")]
        file: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = ImportanceArg::Medium)]
        importance: ImportanceArg,
        #[arg(long = "keyword")]
        keywords: Vec<String>,
        #[arg(long)]
        raw: Option<String>,
        /// `global` for a user-wide preference or rule; `project` (default) for a fact
        /// about this repository only.
        #[arg(long, value_enum, default_value_t = StoreScopeArg::Project)]
        scope: StoreScopeArg,
    },
    #[command(about = "Show ICM state reported by hzrd")]
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
    #[arg(long, value_name = "DIR")]
    pub cwd: Option<PathBuf>,
    #[arg(long)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Subcommand)]
pub enum CodecCommand {
    #[command(about = "Apply a safe transform; reads stdin when TEXT and --file are absent")]
    Compile {
        #[arg(value_name = "TEXT", conflicts_with = "file")]
        text: Option<String>,
        #[arg(long, value_name = "PATH", conflicts_with = "text")]
        file: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = FidelityArg::Semantic)]
        fidelity: FidelityArg,
        #[arg(long, value_enum)]
        profile: Option<ProfileArg>,
        #[arg(long, value_enum, default_value_t = RiskArg::Low)]
        risk: RiskArg,
    },
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    #[command(about = "Run one managed prompt; reads stdin when PROMPT and --file are absent")]
    Run {
        #[arg(value_name = "PROMPT", conflicts_with = "file")]
        prompt: Option<String>,
        #[arg(long, value_name = "PATH", conflicts_with = "prompt")]
        file: Option<PathBuf>,
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=100))]
        max_turns: u32,
        #[arg(long, value_enum, default_value_t = ResponseFormatArg::Text)]
        response_format: ResponseFormatArg,
        #[arg(long, default_value_t = 1_800_000)]
        timeout_ms: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum MigrateCommand {
    #[command(about = "Report legacy indexes, memory, settings, wrappers, and process markers")]
    Scan {
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
    },
    #[command(about = "Move one legacy .grepai into HZR with a retained backup and manifest")]
    Apply {
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
    },
    #[command(
        about = "Snapshot and idempotently import platform RTK history into the canonical ledger"
    )]
    History {
        #[arg(long)]
        dry_run: bool,
        #[arg(long, conflicts_with = "dry_run")]
        force: bool,
    },
    #[command(about = "Snapshot and idempotently import the legacy ICM database into HZR memory")]
    Memory {
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
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
    use clap::Parser;

    use super::{
        Cli, Command, ContextCommand, DaemonCommand, ExecCommand, HooksCommand, IndexCommand,
        McpCommand, MigrateCommand, ServiceCommand,
    };

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

    #[test]
    fn test_cli_parses_native_tdd_contract() {
        let cli = Cli::try_parse_from(["hzr", "tdd", "--json"]).expect("native TDD contract");

        assert!(cli.json);
        assert!(matches!(cli.command, Command::Tdd));
    }

    #[test]
    fn test_cli_parses_update_command() {
        let cli = Cli::try_parse_from(["hzr", "update"]).expect("native update command");

        assert!(matches!(cli.command, Command::Update));
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

        assert!(matches!(
            index.command,
            Command::Index {
                command: IndexCommand::Init { ref workspace }
            } if workspace.as_deref() == Some(std::path::Path::new("/workspace"))
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
