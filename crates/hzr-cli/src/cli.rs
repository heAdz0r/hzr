use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use hzr_agent::ResponseFormat;
use hzr_protocol::{CodecProfile, FidelityClass, MemoryImportance, RiskClass, SearchMode};

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
    #[command(about = "Create private configuration and canonical data layout")]
    Init {
        #[arg(long)]
        force: bool,
        #[arg(long, value_name = "DIR")]
        data_dir: Option<PathBuf>,
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
    #[command(about = "Report observed usage and cost without mixing estimates into actuals")]
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
pub enum DaemonCommand {
    #[command(about = "Serve hzrd in the foreground until interrupted")]
    Serve,
    #[command(about = "Read typed health from the authenticated daemon")]
    Status,
    #[command(about = "Read the pinned engine manifest from the daemon")]
    Engines,
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
    #[arg(long, value_name = "PATH")]
    pub path: Option<PathBuf>,
    #[arg(long, default_value_t = 10, value_parser = parse_limit)]
    pub limit: usize,
    #[arg(long, value_enum)]
    pub mode: Option<SearchModeArg>,
    #[arg(long)]
    pub include_content: bool,
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
        Cli, Command, ContextCommand, DaemonCommand, ExecCommand, IndexCommand, MigrateCommand,
    };

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
