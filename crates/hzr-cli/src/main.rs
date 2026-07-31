mod cli;
mod client;
mod diagnostics;
mod fork;
mod input;
mod invocation;
mod migration;
mod output;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use hzr_agent::{BearerToken, HzrApi, ManagedAgent, ManagedAgentConfig};
use hzr_core::{Config, ConfigPaths, Ledger, LedgerSummary};
use hzr_index::{Deadlines, GrepAi, InitOptions, Workspace, migrate_legacy_index};
use hzr_protocol::{
    CodecApiRequest, ContextPlanApiRequest, ExecApiRequest, ExecApprovalApiRequest,
    MemoryRecallApiRequest, MemoryStoreApiRequest, PROTOCOL_VERSION, SearchApiRequest, SearchMode,
    SessionId,
};

use crate::cli::{
    AgentCommand, Cli, CodecCommand, Command, ContextCommand, ContextPlanArgs, DaemonCommand,
    EnginesCommand, ExecArgs, ExecCommand, IndexCommand, MemoryCommand, MigrateCommand, SearchArgs,
};
use crate::client::DaemonClient;
use crate::diagnostics::{doctor, integration_layout};
use crate::input::read_text;
use crate::invocation::normalize;
use crate::migration::scan;
use crate::output::{
    print_agent, print_context, print_doctor, print_engines, print_execution, print_health,
    print_index_init, print_index_status, print_json, print_memories, print_memory_health,
    print_migration, print_migration_apply, print_rewrite, print_savings, print_search,
    print_transform,
};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse_from(normalize(std::env::args_os().collect()));
    let json = cli.json;
    match run(cli).await {
        Ok(code) => code,
        Err(error) => {
            let stderr = io::stderr();
            let mut output = stderr.lock();
            if json {
                let payload = serde_json::json!({
                    "error": {
                        "message": format!("{error:#}"),
                    }
                });
                let _ = serde_json::to_writer(&mut output, &payload);
                let _ = output.write_all(b"\n");
            } else {
                let _ = writeln!(output, "hzr: {error:#}");
            }
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode> {
    let paths = ConfigPaths::discover();
    let config_path = cli.config.unwrap_or(paths.config_file);
    if let Command::Init { force, data_dir } = &cli.command {
        return initialize(&config_path, *force, data_dir.as_deref(), cli.json);
    }

    let config = Config::load_or_default(&config_path)
        .with_context(|| format!("failed to load {}", config_path.display()))?;
    match cli.command {
        Command::Init { .. } => bail!("init command entered configured execution path"),
        Command::Doctor { workspace } => {
            let workspace = canonical_directory(workspace.as_deref())?;
            let report = doctor(&config_path, &config, &workspace).await;
            if cli.json {
                print_json(&report)?;
            } else {
                print_doctor(&report)?;
            }
            Ok(if report.healthy {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Command::Daemon { command } => match command {
            DaemonCommand::Serve => {
                hzr_daemon::serve(config, async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await?;
                Ok(ExitCode::SUCCESS)
            }
            DaemonCommand::Status => {
                let health = DaemonClient::from_config(&config)?.health().await?;
                if cli.json {
                    print_json(&health)?;
                } else {
                    print_health(&health)?;
                }
                Ok(ExitCode::SUCCESS)
            }
            DaemonCommand::Engines => show_engines(&config, cli.json).await,
        },
        Command::Engines {
            command: EnginesCommand::Status,
        } => show_engines(&config, cli.json).await,
        Command::Index { command } => execute_index(&config, command, cli.json).await,
        Command::Search(arguments) => {
            execute_search(&config, arguments, SearchMode::Auto, cli.json).await
        }
        Command::Rgai(arguments) => {
            execute_search(&config, arguments, SearchMode::Auto, cli.json).await
        }
        Command::Context {
            command: ContextCommand::Plan(arguments),
        } => execute_context_plan(&config, arguments, cli.json).await,
        Command::Memory { command } => execute_memory(&config, command, cli.json).await,
        Command::Exec { command } => execute_command(&config, command, cli.json).await,
        Command::Codec { command } => execute_codec(&config, command, cli.json).await,
        Command::Agent { command } => execute_agent(&config, command, cli.json).await,
        Command::Savings => show_savings(&config, cli.json),
        Command::Migrate { command } => match command {
            MigrateCommand::Scan { workspace } => {
                let workspace = canonical_directory(workspace.as_deref())?;
                let report = scan(&config, &workspace).await;
                if cli.json {
                    print_json(&report)?;
                } else {
                    print_migration(&report)?;
                }
                Ok(ExitCode::SUCCESS)
            }
            MigrateCommand::Apply { workspace } => {
                let workspace = canonical_directory(workspace.as_deref())?;
                let outcome = migrate_legacy_index(
                    &workspace,
                    Path::new("git"),
                    &config.data_dir,
                    Deadlines::default().version,
                )
                .await?;
                if cli.json {
                    print_json(&outcome)?;
                } else {
                    print_migration_apply(&outcome)?;
                }
                Ok(ExitCode::SUCCESS)
            }
        },
        Command::Rtk(arguments) => fork::passthrough(&config, &arguments.args).await,
    }
}

async fn execute_index(config: &Config, command: IndexCommand, json: bool) -> Result<ExitCode> {
    let workspace_path = match &command {
        IndexCommand::Status { workspace } | IndexCommand::Init { workspace } => {
            canonical_directory(workspace.as_deref())?
        }
    };
    let deadlines = Deadlines::default();
    let workspace = Workspace::discover_managed(
        &workspace_path,
        Path::new("git"),
        &config.data_dir,
        deadlines.version,
    )
    .await?;
    match &command {
        IndexCommand::Status { .. } => workspace.require_single_index()?,
        IndexCommand::Init { .. } => workspace.require_managed_index()?,
    }
    let grepai = GrepAi::connect(config.engines.binary("grepai"), workspace, deadlines).await?;

    match command {
        IndexCommand::Status { .. } => {
            let status = grepai.status()?;
            if json {
                print_json(&status)?;
            } else {
                print_index_status(&status)?;
            }
        }
        IndexCommand::Init { .. } => {
            let outcome = grepai.initialize(&InitOptions::default()).await?;
            let status = grepai.status()?;
            if json {
                print_json(&serde_json::json!({
                    "outcome": outcome,
                    "status": status,
                }))?;
            } else {
                print_index_init(outcome, &status)?;
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn initialize(path: &Path, force: bool, data_dir: Option<&Path>, json: bool) -> Result<ExitCode> {
    if path.exists() && !force {
        bail!(
            "configuration {} already exists; pass --force to replace it",
            path.display()
        );
    }
    let mut config = Config::default();
    if let Some(data_dir) = data_dir {
        config.data_dir = data_dir.to_path_buf();
    }
    config.ensure_layout()?;
    config.write(path)?;
    if json {
        print_json(&serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "config": path,
            "data_dir": config.data_dir,
        }))?;
    } else {
        let stdout = io::stdout();
        let mut output = stdout.lock();
        writeln!(output, "initialized {}", path.display())?;
        writeln!(output, "data root {}", config.data_dir.display())?;
    }
    Ok(ExitCode::SUCCESS)
}

async fn show_engines(config: &Config, json: bool) -> Result<ExitCode> {
    let manifest = DaemonClient::from_config(config)?.engines().await?;
    if json {
        print_json(&manifest)?;
    } else {
        print_engines(&manifest)?;
    }
    Ok(ExitCode::SUCCESS)
}

async fn execute_search(
    config: &Config,
    arguments: SearchArgs,
    default_mode: SearchMode,
    json: bool,
) -> Result<ExitCode> {
    let workspace = canonical_directory(arguments.workspace.as_deref())?;
    let request = SearchApiRequest {
        workspace: workspace.to_string_lossy().into_owned(),
        query: arguments.query,
        path: arguments
            .path
            .map(|path| path.to_string_lossy().into_owned()),
        limit: arguments.limit,
        mode: arguments.mode.map_or(default_mode, Into::into),
        include_content: arguments.include_content,
    };
    let response = DaemonClient::from_config(config)?.search(&request).await?;
    if json {
        print_json(&response)?;
    } else {
        print_search(&response)?;
    }
    Ok(ExitCode::SUCCESS)
}

async fn execute_context_plan(
    config: &Config,
    arguments: ContextPlanArgs,
    json: bool,
) -> Result<ExitCode> {
    let workspace = canonical_directory(arguments.workspace.as_deref())?;
    let request = ContextPlanApiRequest {
        workspace: path_text(&workspace, "workspace")?,
        intent: arguments.intent,
        path: arguments
            .path
            .as_deref()
            .map(|path| path_text(path, "context path"))
            .transpose()?,
        topic: arguments.topic,
        search_limit: arguments.search_limit,
        memory_limit: arguments.memory_limit,
    };
    let response = DaemonClient::from_config(config)?
        .context_plan(&request)
        .await?;
    if json {
        print_json(&response)?;
    } else {
        print_context(&response)?;
    }
    Ok(ExitCode::SUCCESS)
}

async fn execute_memory(config: &Config, command: MemoryCommand, json: bool) -> Result<ExitCode> {
    let client = DaemonClient::from_config(config)?;
    match command {
        MemoryCommand::Recall {
            query,
            workspace,
            topic,
            keyword,
            limit,
        } => {
            let workspace = canonical_directory(workspace.as_deref())?;
            let response = client
                .memory_recall(&MemoryRecallApiRequest {
                    workspace: path_text(&workspace, "memory workspace")?,
                    query,
                    topic,
                    limit,
                    keyword,
                })
                .await?;
            if json {
                print_json(&response)?;
            } else {
                print_memories(&response)?;
            }
        }
        MemoryCommand::Store {
            topic,
            workspace,
            content,
            file,
            importance,
            keywords,
            raw,
        } => {
            let workspace = canonical_directory(workspace.as_deref())?;
            let content = read_text(
                content,
                file.as_deref(),
                payload_limit(config.daemon.request_limit_bytes),
            )?;
            let response = client
                .memory_store(&MemoryStoreApiRequest {
                    workspace: path_text(&workspace, "memory workspace")?,
                    topic,
                    content,
                    importance: importance.into(),
                    keywords,
                    raw,
                })
                .await?;
            print_json(&response)?;
        }
        MemoryCommand::Status => {
            let health = client.health().await?;
            let engine = health
                .engines
                .iter()
                .find(|engine| engine.name == "icm")
                .context("daemon health response does not include ICM")?;
            if json {
                print_json(engine)?;
            } else {
                print_memory_health(engine)?;
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn execute_command(config: &Config, command: ExecCommand, json: bool) -> Result<ExitCode> {
    let client = DaemonClient::from_config(config)?;
    match command {
        ExecCommand::Rewrite(arguments) => {
            let request = exec_request(arguments)?;
            let decision = client.exec_rewrite(&request).await?;
            if json {
                print_json(&decision)?;
            } else {
                print_rewrite(&decision)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        ExecCommand::Run(arguments) => {
            let request = exec_request(arguments)?;
            let outcome = client.exec_run(&request).await?;
            Ok(print_execution(&outcome, json)?)
        }
        ExecCommand::Approve { decision_id } => {
            let outcome = client
                .exec_approval(&ExecApprovalApiRequest {
                    decision_id,
                    approved: true,
                })
                .await?;
            Ok(print_execution(&outcome, json)?)
        }
        ExecCommand::Deny { decision_id } => {
            let outcome = client
                .exec_approval(&ExecApprovalApiRequest {
                    decision_id,
                    approved: false,
                })
                .await?;
            Ok(print_execution(&outcome, json)?)
        }
    }
}

fn exec_request(arguments: ExecArgs) -> Result<ExecApiRequest> {
    let cwd = canonical_directory(arguments.cwd.as_deref())?;
    Ok(ExecApiRequest {
        cwd: cwd.to_string_lossy().into_owned(),
        command: arguments.command,
        timeout_ms: arguments.timeout_ms,
    })
}

async fn execute_codec(config: &Config, command: CodecCommand, json: bool) -> Result<ExitCode> {
    let CodecCommand::Compile {
        text,
        file,
        fidelity,
        profile,
        risk,
    } = command;
    let content = read_text(
        text,
        file.as_deref(),
        payload_limit(config.daemon.request_limit_bytes),
    )?;
    let transform = DaemonClient::from_config(config)?
        .codec_compile(&CodecApiRequest {
            content,
            fidelity: fidelity.into(),
            risk: risk.into(),
            profile: profile.map_or(config.policy.codec_profile, Into::into),
        })
        .await?;
    if json {
        print_json(&transform)?;
    } else {
        print_transform(&transform)?;
    }
    Ok(ExitCode::SUCCESS)
}

async fn execute_agent(config: &Config, command: AgentCommand, json: bool) -> Result<ExitCode> {
    let AgentCommand::Run {
        prompt,
        file,
        workspace,
        max_turns,
        response_format,
        timeout_ms,
    } = command;
    if timeout_ms == 0 {
        bail!("agent timeout must be positive");
    }
    let prompt = read_text(prompt, file.as_deref(), 4 * 1024 * 1024)?;
    let workspace = canonical_directory(workspace.as_deref())?;
    let client = DaemonClient::from_config(config)?;
    let health = client.health().await?;
    if health.protocol_version != PROTOCOL_VERSION {
        bail!(
            "daemon protocol {} is incompatible with CLI protocol {}",
            health.protocol_version,
            PROTOCOL_VERSION
        );
    }
    let token = BearerToken::new(client.token().to_owned())?;
    let api = HzrApi::new(client.endpoint().into(), token)?;
    let node = std::env::var_os("HZR_NODE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("node"));
    let agent_data = config
        .data_dir
        .join("sessions")
        .join(SessionId::new().to_string());
    let mut agent_config =
        ManagedAgentConfig::new(node, integration_layout(config), workspace, agent_data, api);
    agent_config.timeout = Duration::from_millis(timeout_ms);
    let run = ManagedAgent::new(agent_config)
        .run(&prompt, response_format.into(), max_turns)
        .await?;
    print_agent(&run, json)?;
    Ok(ExitCode::SUCCESS)
}

fn show_savings(config: &Config, json: bool) -> Result<ExitCode> {
    let path = config.data_dir.join("ledger/usage.sqlite");
    let summary = if path.is_file() {
        Ledger::open(&path)?.summary()?
    } else {
        LedgerSummary::default()
    };
    if json {
        print_json(&summary)?;
    } else {
        print_savings(&summary)?;
    }
    Ok(ExitCode::SUCCESS)
}

fn canonical_directory(path: Option<&Path>) -> Result<PathBuf> {
    let path = match path {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir().context("failed to read current directory")?,
    };
    let canonical = std::fs::canonicalize(&path)
        .with_context(|| format!("invalid directory {}", path.display()))?;
    if !canonical.is_dir() {
        bail!("{} is not a directory", canonical.display());
    }
    Ok(canonical)
}

fn path_text(path: &Path, label: &str) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .with_context(|| format!("{label} must be valid UTF-8: {}", path.display()))
}

fn payload_limit(request_limit: usize) -> usize {
    request_limit.saturating_sub(4_096)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{canonical_directory, payload_limit};

    #[test]
    fn test_payload_limit_reserves_json_envelope_space() {
        assert_eq!(payload_limit(10_000), 5_904);
        assert_eq!(payload_limit(100), 0);
    }

    #[test]
    fn test_canonical_directory_rejects_regular_file() {
        let directory = tempdir().expect("temporary directory");
        let file = directory.path().join("file");
        std::fs::write(&file, []).expect("write fixture");

        assert!(canonical_directory(Some(&file)).is_err());
    }
}
