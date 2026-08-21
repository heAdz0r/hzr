mod activation;
mod adoption;
mod build;
mod cli;
mod cli_help;
mod cli_subcommand_help;
mod client;
mod client_config;
mod diagnostics;
mod foreign;
mod fork;
mod hook_runner;
mod input;
mod instructions;
mod invocation;
mod mcp;
mod memory_migration;
mod migration;
mod output;
mod prefix;
mod release_version;
mod service;
mod stats;
mod stats_output;
mod tdd;
mod update;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use hzr_agent::{BearerToken, HzrApi, ManagedAgent, ManagedAgentConfig};
use hzr_core::{
    Config, ConfigPaths, Ledger, discover_legacy_rtk_history, inspect_legacy_efficiency,
};
use hzr_index::{
    Deadlines, GrepAi, IndexPlacement, InitOptions, Workspace, WorkspaceRegistration,
    migrate_legacy_index,
};
use hzr_protocol::{
    AccountingAttribution, AccountingChannel, AccountingMeasurement, AccountingOperationKind,
    AccountingOperationMode, AccountingRoute, AccountingSearchStrategy, AccountingStage,
    CodecApiRequest, ContextPlanApiRequest, ExecApiRequest, ExecApprovalApiRequest,
    MemoryForgetApiRequest, MemoryPruneApiRequest, MemoryRecallApiRequest, MemoryStoreApiRequest,
    MemoryUpdateApiRequest, OperationApiRequest, PROTOCOL_VERSION, SearchApiRequest,
    SearchApiResponse, SearchMode, SearchStrategy, SessionId,
};

use crate::cli::{
    ActivationCommand, AgentCommand, Cli, CodecCommand, Command, ContextCommand, ContextPlanArgs,
    DaemonCommand, EnginesCommand, ExecArgs, ExecCommand, HooksCommand, IndexCommand, McpCommand,
    MemoryCommand, MigrateCommand, SearchArgs, ServiceCommand,
};
use crate::client::DaemonClient;
use crate::diagnostics::{doctor, integration_layout, repair_legacy_index};
use crate::input::read_text;
use crate::invocation::normalize;
use crate::migration::scan;
use crate::output::{
    print_agent, print_context, print_doctor, print_engines, print_execution, print_health,
    print_index_init, print_index_status, print_json, print_memories, print_memory_health,
    print_migration, print_migration_apply, print_rewrite, print_stats, print_transform,
    render_search,
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
    match &cli.command {
        Command::Install {
            dry_run,
            force,
            prefix: prefix_arg,
            binary,
            allow_dev_path,
            keep_external_icm,
            skip_instructions,
            skip_service,
            project_only,
            workspace,
        } => {
            return run_install(
                InstallOptions {
                    dry_run: *dry_run,
                    force: *force,
                    prefix: prefix_arg.clone(),
                    binary: binary.clone(),
                    allow_dev_path: *allow_dev_path,
                    adopt_icm: !*keep_external_icm,
                    wire_instructions: !*skip_instructions,
                    start_service: !*skip_service,
                    project_only: *project_only,
                    workspace: workspace.clone(),
                },
                &config_path,
                cli.json,
            )
            .await;
        }
        Command::Uninstall {
            keep_data,
            dry_run,
            force,
        } => {
            let _ = keep_data;
            let path = adoption::default_settings_path()?;
            let report = adoption::uninstall(&path, *dry_run, *force)?;
            // Instruction blocks are removed too: leaving them would keep telling the
            // agent to call `hzr` after its hooks are gone. Binaries on PATH stay —
            // deleting a binary the user may invoke directly is not ours to decide.
            let mut instruction_reports = Vec::new();
            for surface in [instructions::Surface::Claude, instructions::Surface::Codex] {
                let target = surface.default_path()?;
                instruction_reports
                    .push(instructions::uninstall(surface, &target, *dry_run, *force)?);
            }
            let workspace = canonical_directory(None)?;
            for (surface, target) in activation::local_instruction_paths(&workspace) {
                instruction_reports
                    .push(instructions::uninstall(surface, &target, *dry_run, *force)?);
            }
            let client_reports = client_config::uninstall_all(*dry_run, *force)?;
            return print_adoption_bundle(
                &report,
                None,
                &instruction_reports,
                &client_reports,
                None,
                None,
                cli.json,
            );
        }
        Command::Hooks {
            command: HooksCommand::Status,
        } => {
            let status = adoption::status(&adoption::default_settings_path()?)?;
            // Adoption is only real when hooks, instructions and PATH all agree, so
            // report all three rather than letting hooks alone imply success.
            let config = Config::load_or_default(&config_path)?;
            let project_only = config.activation.mode == hzr_core::ActivationMode::Selected;
            let workspace = canonical_directory(None)?;
            let claude_md = if project_only {
                workspace.join("CLAUDE.md")
            } else {
                instructions::Surface::Claude.default_path()?
            };
            let codex_md = if project_only {
                workspace.join("AGENTS.md")
            } else {
                instructions::Surface::Codex.default_path()?
            };
            let claude_wired = instructions::is_installed(&claude_md)?;
            let codex_wired = instructions::is_installed(&codex_md)?;
            let prefix_dir = prefix::default_prefix()?;
            let hzr_on_path = prefix::is_on_path(&prefix_dir) && prefix_dir.join("hzr").exists();
            let foreign_report = foreign::scan(&ConfigPaths::discover().data_dir).ok();

            if cli.json {
                print_json(&serde_json::json!({
                    "hooks": status,
                    "instructions": {
                        "claude": {"path": claude_md, "installed": claude_wired},
                        "codex": {"path": codex_md, "installed": codex_wired},
                    },
                    "path": {"prefix": prefix_dir, "hzr_reachable": hzr_on_path},
                    "activation": config.activation,
                    "foreign": foreign_report,
                }))?;
            } else {
                println!(
                    "HZR={} RTK={} external-ICM={} installed={} conflict={}",
                    status.hzr_entries,
                    status.rtk_entries,
                    status.external_icm_entries,
                    status.installed,
                    status.conflict
                );
                println!(
                    "instructions claude={claude_wired} codex={codex_wired}; \
                     hzr-on-path={hzr_on_path} ({})",
                    prefix_dir.display()
                );
                println!(
                    "activation={} workspace-enabled={}",
                    if project_only { "selected" } else { "all" },
                    activation::is_enabled(&config, &workspace)
                        .await
                        .unwrap_or(false)
                );
                if let Some(report) = &foreign_report {
                    for (engine, count) in &report.unmanaged_by_engine {
                        println!("unmanaged {engine} process(es): {count}");
                    }
                    for (engine, count) in &report.unmanaged_wrappers_by_engine {
                        println!("unmanaged {engine} wrapper(s): {count}");
                    }
                }
            }
            return Ok(if status.conflict {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            });
        }
        _ => {}
    }
    if let Command::Init {
        force,
        data_dir,
        if_needed,
        if_enabled,
        quiet,
        session_start_hook,
        skip_service,
        ..
    } = &cli.command
    {
        if *if_needed {
            return initialize_if_needed(
                &config_path,
                data_dir.as_deref(),
                *quiet,
                *session_start_hook,
                *skip_service,
                cli.json,
            )
            .await;
        }
        if *if_enabled {
            return initialize_if_enabled(
                &config_path,
                data_dir.as_deref(),
                *quiet,
                *session_start_hook,
                *skip_service,
                cli.json,
            )
            .await;
        }
        return initialize(
            &config_path,
            *force,
            data_dir.as_deref(),
            *skip_service,
            cli.json,
        )
        .await;
    }

    match &cli.command {
        Command::Enable { workspace } => {
            return set_workspace_activation(&config_path, workspace.as_deref(), true, cli.json)
                .await;
        }
        Command::Disable { workspace } => {
            return set_workspace_activation(&config_path, workspace.as_deref(), false, cli.json)
                .await;
        }
        Command::Activation {
            command: ActivationCommand::Status,
        } => {
            let config = Config::load_or_default(&config_path)
                .with_context(|| format!("failed to load {}", config_path.display()))?;
            return show_activation_status(&config, cli.json);
        }
        _ => {}
    }

    if let Command::Update { check } = &cli.command {
        return update::execute(cli.json, *check).await;
    }

    let config = Config::load_or_default(&config_path)
        .with_context(|| format!("failed to load {}", config_path.display()))?;
    match cli.command {
        Command::Init { .. } => bail!("init command entered configured execution path"),
        Command::Install { .. }
        | Command::Uninstall { .. }
        | Command::Enable { .. }
        | Command::Disable { .. }
        | Command::Activation { .. } => {
            bail!("adoption command entered configured execution path")
        }
        Command::Hooks {
            command: HooksCommand::Status,
        } => bail!("hook status entered configured execution path"),
        Command::Update { .. } => bail!("update command entered configured execution path"),
        Command::Hooks {
            command: HooksCommand::Dispatch,
        } => {
            hook_runner::dispatch(&config).await?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Hooks {
            command: HooksCommand::Observe,
        } => {
            hook_runner::observe(&config).await;
            Ok(ExitCode::SUCCESS)
        }
        Command::Doctor { workspace, fix } => {
            let workspace = canonical_directory(workspace.as_deref())?;
            let repair = if fix {
                repair_legacy_index(&config, &workspace).await?
            } else {
                None
            };
            let mut report = doctor(&config_path, &config, &workspace).await;
            report.repair = repair;
            if cli.json {
                print_json(&report)?;
            } else {
                if let Some(outcome) = &report.repair {
                    print_migration_apply(outcome)?;
                }
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
            DaemonCommand::Service { command } => {
                let report = service::execute(command)?;
                if cli.json {
                    print_json(&report)?;
                } else {
                    println!(
                        "service {} manager={:?} active={} changed={} definition={} binary={}",
                        report.action,
                        report.manager,
                        report.active,
                        report.changed,
                        report.definition.display(),
                        report.binary.display()
                    );
                }
                Ok(if command == ServiceCommand::Status && !report.active {
                    ExitCode::FAILURE
                } else {
                    ExitCode::SUCCESS
                })
            }
        },
        Command::Engines {
            command: EnginesCommand::Status,
        } => show_engines(&config, cli.json).await,
        Command::Index { command } => execute_index(&config, command, cli.json).await,
        Command::Search(arguments) | Command::Rgai(arguments) => {
            execute_search(&config, arguments, SearchMode::Auto, cli.json).await
        }
        Command::Context {
            command: ContextCommand::Plan(arguments),
        } => execute_context_plan(&config, arguments, cli.json).await,
        Command::Memory { command } => execute_memory(&config, command, cli.json).await,
        Command::Exec { command } => execute_command(&config, command, cli.json).await,
        Command::Codec { command } => execute_codec(&config, command, cli.json).await,
        Command::Agent { command } => execute_agent(&config, command, cli.json).await,
        Command::Mcp { command } => match command {
            McpCommand::Serve { workspace } => {
                let workspace = canonical_directory(workspace.as_deref())?;
                mcp::serve(&config, &workspace).await?;
                Ok(ExitCode::SUCCESS)
            }
            McpCommand::Config {
                client,
                workspace,
                apply,
            } => {
                let binary = prefix::default_prefix()?.join("hzr");
                if apply {
                    // `--apply` uses the same write path as `hzr install`: Codex/Desktop are
                    // HZR-owned registrations. Default pin is cwd when `--workspace` is omitted.
                    let workspace = canonical_directory(workspace.as_deref())?;
                    let report =
                        client_config::apply(client.into(), &binary, &workspace, false, true)?;
                    if cli.json {
                        print_json(&report)?;
                    } else {
                        println!(
                            "client-mcp {} {} changed={} direct-icm-removed={} hzr-registered={} pinned={}",
                            report.client.as_str(),
                            report.path.display(),
                            report.changed,
                            report.direct_icm_removed,
                            report.hzr_registered,
                            workspace.display()
                        );
                    }
                } else {
                    let workspace = match workspace {
                        Some(path) => Some(canonical_directory(Some(&path))?),
                        None => None,
                    };
                    print!(
                        "{}",
                        mcp::registration_snippet(client, &binary, workspace.as_deref())
                    );
                }
                Ok(ExitCode::SUCCESS)
            }
            McpCommand::Status => {
                let clients = client_config::status_all()?;
                if cli.json {
                    print_json(&serde_json::json!({
                        "lifecycle": mcp::lifecycle_metadata(),
                        "clients": clients,
                    }))?;
                } else {
                    println!(
                        "lifecycle={} started-by-init=false launch='MCP client connection'",
                        client_config::MCP_LIFECYCLE
                    );
                    for client in clients {
                        println!(
                            "{} {} exists={} registered={} direct-icm={} command={}",
                            client.client.as_str(),
                            client.path.display(),
                            client.config_exists,
                            client.registered,
                            client.direct_icm_registrations,
                            client.command.as_deref().unwrap_or("-")
                        );
                    }
                }
                Ok(ExitCode::SUCCESS)
            }
        },
        Command::Build(arguments) => {
            // One inherited subcommand, forwarded verbatim: `hzr build --release` must
            // reach the fork exactly as `rtk build --release` did.
            let mut args = vec![std::ffi::OsString::from("build")];
            args.extend(arguments.args.iter().cloned());
            fork::passthrough(&config, &args).await
        }
        Command::Read(arguments) => {
            let mut args = vec![std::ffi::OsString::from("read")];
            args.extend(arguments.args.iter().cloned());
            let args = bounded_read_arguments(&args, exact_read_fidelity_requested());
            fork::passthrough(&config, &args).await
        }
        Command::Write(arguments) => {
            let mut args = vec![std::ffi::OsString::from("write")];
            args.extend(arguments.args.iter().cloned());
            fork::passthrough(&config, &args).await
        }
        Command::Release {
            version,
            dry_run,
            force,
            skip_service,
            install_root,
        } => {
            let report = build::run(build::BuildOptions {
                target_version: version,
                dry_run,
                force,
                skip_service,
                install_root,
            })?;
            if cli.json {
                print_json(&report)?;
            } else {
                println!(
                    "build v{}-{} current={} switched={} dry_run={}",
                    report.version,
                    report.platform,
                    report.current.display(),
                    report.switched,
                    report.dry_run
                );
                for engine in &report.engines {
                    println!(
                        "  {} {} expected={} {}",
                        if engine.ok { "ok  " } else { "FAIL" },
                        engine.name,
                        engine.expected,
                        engine.reported
                    );
                }
                if !report.dry_run {
                    println!("service restarted: {}", report.service_restarted);
                }
            }
            // A stale engine must fail the command, not just print a line.
            Ok(if report.dry_run || report.healthy() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Command::Tdd => {
            let contract = tdd::contract();
            if cli.json {
                print_json(&contract)?;
            } else {
                print!("{}", tdd::render_text(&contract));
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Stats {
            workspace,
            all,
            since,
        } => show_stats(&config, workspace.as_deref(), cli.json, all, since.as_ref()).await,
        Command::Savings => show_stats(&config, None, cli.json, false, None).await,
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
            MigrateCommand::History { dry_run, force } => {
                if !dry_run && !force {
                    bail!(
                        "history migration requires `--dry-run` for inspection or `--force` to apply"
                    );
                }
                let sources = discover_legacy_rtk_history();
                if dry_run {
                    let reports = sources
                        .iter()
                        .map(|source| inspect_legacy_efficiency(source))
                        .collect::<Result<Vec<_>, _>>()?;
                    if cli.json {
                        print_json(&reports)?;
                    } else if reports.is_empty() {
                        println!("no platform RTK history database found");
                    } else {
                        for report in reports {
                            println!(
                                "history {} sha256={} operations={} gross={} regressions={} net={}",
                                report.path.display(),
                                report.sha256,
                                report.operations,
                                report.gross_avoided_tokens_estimated,
                                report.regression_tokens_estimated,
                                report.net_avoided_tokens_estimated
                            );
                        }
                    }
                } else {
                    let ledger = Ledger::open(&config.data_dir.join("ledger/hzr.sqlite"))?;
                    let migration_root = config.data_dir.join("migrations");
                    let reports = sources
                        .iter()
                        .map(|source| ledger.migrate_legacy_efficiency(source, &migration_root))
                        .collect::<Result<Vec<_>, _>>()?;
                    if cli.json {
                        print_json(&reports)?;
                    } else if reports.is_empty() {
                        println!("no platform RTK history database found");
                    } else {
                        for report in reports {
                            println!(
                                "history {} imported={} failures={} changed={} backup={} manifest={}",
                                report.source.path.display(),
                                report.imported_commands,
                                report.imported_parse_failures,
                                report.changed,
                                report.backup_path.display(),
                                report.manifest_path.display()
                            );
                        }
                    }
                }
                Ok(ExitCode::SUCCESS)
            }
            MigrateCommand::Memory {
                workspace,
                dry_run,
                force,
            } => {
                if !dry_run && !force {
                    bail!(
                        "memory migration requires `--dry-run` for inspection or `--force` to apply"
                    );
                }
                let workspace = canonical_directory(workspace.as_deref())?;
                let source = memory_migration::discover_legacy_icm_database()
                    .ok_or_else(|| anyhow::anyhow!("no platform legacy ICM database found"))?;
                if dry_run {
                    let report = memory_migration::inspect(&source)?;
                    if cli.json {
                        print_json(&report)?;
                    } else {
                        println!(
                            "memory {} sha256={} rows={}",
                            report.path.display(),
                            report.sha256,
                            report.rows_by_table.values().sum::<u64>()
                        );
                    }
                } else {
                    let managed = Workspace::discover_managed(
                        &workspace,
                        Path::new("git"),
                        &config.data_dir,
                        Deadlines::default().version,
                    )
                    .await?;
                    let report = memory_migration::migrate(
                        &source,
                        &config.data_dir.join("memory/icm/memories.db"),
                        &config.data_dir.join("migrations"),
                        &managed.identity.repository_id,
                    )?;
                    if cli.json {
                        print_json(&report)?;
                    } else {
                        println!(
                            "memory {} imported={} changed={} backup={} canonical-backup={} manifest={}",
                            report.source.path.display(),
                            report.imported_rows,
                            report.changed,
                            report.source_backup.display(),
                            report.canonical_backup.display(),
                            report.manifest_path.display()
                        );
                    }
                }
                Ok(ExitCode::SUCCESS)
            }
        },
        Command::Rtk(arguments) => {
            let args = bounded_read_arguments(&arguments.args, exact_read_fidelity_requested());
            fork::passthrough(&config, &args).await
        }
    }
}

fn exact_read_fidelity_requested() -> bool {
    std::env::var_os("HZR_EXACT_FIDELITY").as_deref() == Some(std::ffi::OsStr::new("1"))
}

/// Remove an unbounded exact-text request before entering fork-core.
///
/// This is an argv transformation, not a shell reconstruction: non-UTF-8 file names and every
/// untouched argument retain their original bytes. Bounded/structural reads and the explicit
/// fidelity marker remain unchanged.
fn bounded_read_arguments(
    arguments: &[std::ffi::OsString],
    exact_fidelity: bool,
) -> Vec<std::ffi::OsString> {
    if exact_fidelity || arguments.first().and_then(|value| value.to_str()) != Some("read") {
        return arguments.to_vec();
    }

    let mut level_none = None;
    let mut index = 1;
    let mut bounded = false;
    while index < arguments.len() {
        let Some(argument) = arguments[index].to_str() else {
            index += 1;
            continue;
        };
        bounded |= matches!(
            argument,
            "--from"
                | "--to"
                | "-n"
                | "--line-numbers"
                | "--max-lines"
                | "--tail-lines"
                | "--outline"
                | "--symbols"
                | "--changed"
                | "--since"
        ) || [
            "--from=",
            "--to=",
            "--max-lines=",
            "--tail-lines=",
            "--since=",
        ]
        .iter()
        .any(|prefix| argument.starts_with(prefix));

        let candidate = if matches!(argument, "--level" | "-l") {
            let Some(value) = arguments.get(index + 1).and_then(|value| value.to_str()) else {
                return arguments.to_vec();
            };
            if value != "none" || level_none.is_some() {
                return arguments.to_vec();
            }
            Some((index, index + 1))
        } else if argument == "--level=none" {
            if level_none.is_some() {
                return arguments.to_vec();
            }
            Some((index, index))
        } else if argument.starts_with("--level=") {
            return arguments.to_vec();
        } else {
            None
        };
        if let Some(candidate) = candidate {
            level_none = Some(candidate);
            index = candidate.1 + 1;
        } else {
            index += 1;
        }
    }

    let Some((start, end)) = level_none else {
        return arguments.to_vec();
    };
    if bounded {
        return arguments.to_vec();
    }
    arguments
        .iter()
        .enumerate()
        .filter(|(index, _)| *index < start || *index > end)
        .map(|(_, argument)| argument.clone())
        .collect()
}

struct InstallOptions {
    dry_run: bool,
    force: bool,
    prefix: Option<PathBuf>,
    binary: Option<PathBuf>,
    allow_dev_path: bool,
    adopt_icm: bool,
    wire_instructions: bool,
    start_service: bool,
    project_only: bool,
    workspace: Option<PathBuf>,
}

/// Full adoption in one confirmed operation: durable binaries on PATH, one hook
/// dispatcher, and agent instructions. Ordering matters — binaries are placed first
/// so the hook command and the `CLAUDE.md` contract both name a path that already
/// exists, and so a `--dry-run` preview shows the same target the real run will use.
async fn run_install(options: InstallOptions, config_path: &Path, json: bool) -> Result<ExitCode> {
    if !options.dry_run && !options.force {
        bail!(
            "installation changes user configuration; inspect `hzr install --dry-run`, then rerun with `--force` to confirm"
        );
    }
    let executable = std::env::current_exe().context("cannot resolve the HZR executable")?;
    let source_dir = executable_source_directory(&executable)?;
    let mut config = Config::load_or_default(config_path)?;
    let previously_enabled_roots = config
        .activation
        .enabled_workspaces
        .iter()
        .map(|workspace| workspace.root.clone())
        .collect::<Vec<_>>();
    if !options.dry_run {
        config.ensure_layout()?;
    }
    // Пин MCP и корень активации: явный `--workspace`, иначе cwd, откуда запущен install.
    let workspace_root = canonical_directory(options.workspace.as_deref())?;
    let workspace = if options.project_only && !options.dry_run {
        let (workspace, _, _, _, _) = initialize_workspace_at(&config, &workspace_root).await?;
        Some(workspace)
    } else {
        Some(activation::discover(&config, &workspace_root).await?)
    };
    if options.project_only {
        config.activation.mode = hzr_core::ActivationMode::Selected;
        config
            .activation
            .enable(activation::record(workspace.as_ref().context("workspace")?));
    } else {
        config.activation.mode = hzr_core::ActivationMode::All;
        config.activation.enabled_workspaces.clear();
    }
    if !options.dry_run {
        config.write(config_path)?;
    }

    let prefix_dir = match options.prefix.clone() {
        Some(prefix) => prefix,
        None => prefix::default_prefix()?,
    };
    let prefix_report = prefix::install(&prefix_dir, &source_dir, options.dry_run, options.force)?;

    // Hooks always name the durable copy in the prefix — never the binary that happens
    // to be running, which may live in `target/debug` or a temporary bundle. During
    // `--dry-run` that copy does not exist yet, so the preview is allowed to name the
    // path the confirmed run will create. An explicit --binary still wins.
    let hook_binary = match options.binary.clone() {
        Some(binary) => {
            adoption::resolve_hook_binary(Some(&binary), options.allow_dev_path, false)?
        }
        None => adoption::resolve_hook_binary(
            Some(&prefix_dir.join("hzr")),
            options.allow_dev_path,
            options.dry_run,
        )?,
    };

    let settings_path = adoption::default_settings_path()?;
    let hooks = adoption::install(
        &settings_path,
        &hook_binary,
        options.adopt_icm,
        options.start_service,
        options.project_only,
        options.dry_run,
        options.force,
    )?;

    let mut instruction_reports = Vec::new();
    if options.project_only {
        for surface in [instructions::Surface::Claude, instructions::Surface::Codex] {
            let target = surface.default_path()?;
            instruction_reports.push(instructions::uninstall(
                surface,
                &target,
                options.dry_run,
                options.force,
            )?);
        }
    }
    if options.wire_instructions {
        let contract = contract_asset_path(&source_dir);
        if options.project_only {
            for (surface, target) in activation::local_instruction_paths(&workspace_root) {
                instruction_reports.push(instructions::install(
                    surface,
                    &target,
                    &contract,
                    options.dry_run,
                    options.force,
                )?);
            }
        } else {
            let mut roots = previously_enabled_roots;
            roots.push(workspace_root.clone());
            roots.sort();
            roots.dedup();
            for root in roots {
                for (surface, target) in activation::local_instruction_paths(&root) {
                    instruction_reports.push(instructions::uninstall(
                        surface,
                        &target,
                        options.dry_run,
                        options.force,
                    )?);
                }
            }
            for surface in [instructions::Surface::Claude, instructions::Surface::Codex] {
                let target = surface.default_path()?;
                instruction_reports.push(instructions::install(
                    surface,
                    &target,
                    &contract,
                    options.dry_run,
                    options.force,
                )?);
            }
        }
    }

    let client_reports = if options.project_only {
        client_config::uninstall_all(options.dry_run, options.force)?
    } else {
        client_config::install_all(
            &hook_binary,
            &workspace_root,
            options.dry_run,
            options.force,
        )?
    };

    let foreign_report = foreign::scan(&ConfigPaths::discover().data_dir).ok();
    let service_report = if options.start_service && !options.dry_run {
        service::ensure_running_if_installed()?
    } else {
        None
    };

    print_adoption_bundle(
        &hooks,
        Some(&prefix_report),
        &instruction_reports,
        &client_reports,
        foreign_report.as_ref(),
        service_report.as_ref(),
        json,
    )
}

/// Reconcile the instruction scope selected by activation policy. SessionStart calls
/// `init --if-needed`, so upgrades repair stale managed blocks without duplicating them.
/// Only HZR's delimited region is changed; repository and user-authored rules remain intact.
fn reconcile_agent_instructions(
    config: &Config,
    workspace_root: &Path,
) -> Result<Vec<instructions::InstructionReport>> {
    let executable = std::env::current_exe().context("cannot resolve the HZR executable")?;
    let contract = contract_asset_path(&executable_source_directory(&executable)?);
    let targets = match config.activation.mode {
        hzr_core::ActivationMode::All => {
            let mut targets = [instructions::Surface::Claude, instructions::Surface::Codex]
                .into_iter()
                .map(|surface| surface.default_path().map(|path| (surface, path)))
                .collect::<Result<Vec<_>>>()?;
            // Global instructions cover ordinary repositories. A pre-HZR local contract can
            // override them, so migrate only local files that already contain an HZR block or
            // a known conflicting RTK/ICM mandate. Clean projects remain byte-for-byte intact.
            for (surface, path) in activation::local_instruction_paths(workspace_root) {
                let audit = instructions::audit(surface, &path)?;
                if audit.installed || !audit.conflicting_mandates.is_empty() {
                    targets.push((surface, path));
                }
            }
            targets
        }
        hzr_core::ActivationMode::Selected => {
            activation::local_instruction_paths(workspace_root).to_vec()
        }
    };

    targets
        .into_iter()
        .map(|(surface, path)| instructions::install(surface, &path, &contract, false, true))
        .collect()
}

/// Locate `HZR.md`. An assembled bundle ships it under `share/hzr/`; a development
/// tree has it at the repository root. Both are resolved relative to the binary so
/// the reference written into `CLAUDE.md` stays valid after relocation.
fn contract_asset_path(source_dir: &Path) -> PathBuf {
    // Installed releases live at <root>/versions/<release>/bin while <root>/current
    // is the upgradeable public pointer. Keep instructions on that pointer instead
    // of canonicalizing them onto the release that ran `hzr install`.
    if let Some(current) = source_dir.parent() {
        if current.file_name().is_some_and(|name| name == "current") {
            let stable_contract = current.join("share/hzr/HZR.md");
            if stable_contract.is_file() {
                return stable_contract;
            }
        }
    }
    if let Some(release_root) = source_dir.parent() {
        let is_versioned_release = release_root
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "versions");
        if is_versioned_release {
            if let Some(install_root) = release_root.parent().and_then(Path::parent) {
                let current = install_root.join("current");
                let stable_contract = current.join("share/hzr/HZR.md");
                if stable_contract.is_file()
                    && current.canonicalize().ok().as_deref()
                        == release_root.canonicalize().ok().as_deref()
                {
                    return stable_contract;
                }
            }
        }
    }

    let candidates = [
        source_dir.join("../share/hzr/HZR.md"),
        source_dir.join("../../HZR.md"),
        source_dir.join("../../../HZR.md"),
    ];
    for candidate in candidates {
        if let Ok(resolved) = candidate.canonicalize() {
            return resolved;
        }
    }
    source_dir.join("../share/hzr/HZR.md")
}

fn executable_source_directory(executable: &Path) -> Result<PathBuf> {
    executable
        .canonicalize()
        .with_context(|| format!("cannot resolve HZR executable {}", executable.display()))?
        .parent()
        .map(Path::to_path_buf)
        .context("HZR executable has no parent directory")
}

fn print_adoption_bundle(
    hooks: &adoption::AdoptionReport,
    prefix_report: Option<&prefix::PrefixReport>,
    instruction_reports: &[instructions::InstructionReport],
    client_reports: &[client_config::ClientConfigReport],
    foreign_report: Option<&foreign::ForeignReport>,
    service_report: Option<&service::ServiceReport>,
    json: bool,
) -> Result<ExitCode> {
    if json {
        print_json(&serde_json::json!({
            "hooks": hooks,
            "prefix": prefix_report,
            "instructions": instruction_reports,
            "client_mcp": client_reports,
            "foreign": foreign_report,
            "daemon_service": service_report,
        }))?;
        return Ok(ExitCode::SUCCESS);
    }

    print_adoption(hooks, false)?;
    if let Some(report) = prefix_report {
        println!(
            "prefix {} changed={} on_path={}",
            report.prefix.display(),
            report.changed,
            report.on_path
        );
        if !report.on_path {
            println!(
                "warning: {} is not on PATH; add it so agents can run `hzr` by name",
                report.prefix.display()
            );
        }
    }
    for report in instruction_reports {
        println!(
            "instructions {} {} changed={} installed={} legacy-rtk-imports-removed={} legacy-directives-migrated={}",
            report.surface.as_str(),
            report.path.display(),
            report.changed,
            report.installed,
            report.legacy_rtk_imports_removed,
            report.legacy_directives_migrated
        );
    }
    for report in client_reports {
        println!(
            "client-mcp {} {} changed={} direct-icm-removed={} hzr-registered={}",
            report.client.as_str(),
            report.path.display(),
            report.changed,
            report.direct_icm_removed,
            report.hzr_registered
        );
    }
    if let Some(report) = service_report {
        println!(
            "visualizer {} active={} definition={}",
            report.action,
            report.active,
            report.definition.display()
        );
    }
    if let Some(report) = foreign_report {
        let total = report.unmanaged_active_total();
        if total > 0 {
            let detail = report
                .unmanaged_by_engine
                .iter()
                .map(|(engine, count)| format!("{engine}={count}"))
                .collect::<Vec<_>>()
                .join(" ");
            println!(
                "warning: {total} unmanaged engine process(es) detected ({detail}); \
                 HZR never stops them automatically — review with `hzr doctor`"
            );
        }
        let wrappers = report.unmanaged_wrapper_total();
        if wrappers > 0 {
            let detail = report
                .unmanaged_wrappers_by_engine
                .iter()
                .map(|(engine, count)| format!("{engine}={count}"))
                .collect::<Vec<_>>()
                .join(" ");
            println!(
                "warning: {wrappers} client wrapper(s) still reference direct engines ({detail}); restart those clients after migration"
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print_adoption(report: &adoption::AdoptionReport, json: bool) -> Result<ExitCode> {
    if json {
        print_json(report)?;
    } else {
        println!(
            "{}: changed={} dry_run={} HZR={} RTK={} external-ICM={}",
            report.action,
            report.changed,
            report.dry_run,
            report.status.hzr_entries,
            report.status.rtk_entries,
            report.status.external_icm_entries
        );
        println!(
            "settings {} sha256 {} -> {}",
            report.settings_path.display(),
            report.before_sha256,
            report.after_sha256
        );
        if let Some(backup) = &report.backup_path {
            println!("backup {}", backup.display());
        }
    }
    Ok(ExitCode::SUCCESS)
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

async fn initialize(
    path: &Path,
    force: bool,
    data_dir: Option<&Path>,
    skip_service: bool,
    json: bool,
) -> Result<ExitCode> {
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
    let workspace_root = canonical_directory(None)?;
    let instruction_reports = reconcile_agent_instructions(&config, &workspace_root)?;
    let (workspace, outcome, changed, git_backed, registration) =
        initialize_workspace_at(&config, &workspace_root).await?;
    let dashboard = format!("http://{}", config.daemon.bind);
    let service_report = if skip_service {
        None
    } else {
        service::ensure_running_if_installed()?
    };
    if json {
        print_json(&serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "config": path,
            "data_dir": config.data_dir,
            "outcome": outcome,
            "changed": changed,
            "workspace": workspace.identity.root,
            "git_backed": git_backed,
            "repository_id": workspace.identity.repository_id,
            "worktree_id": workspace.identity.worktree_id,
            "index": workspace.index.directory,
            "registration": registration,
            "instructions": instruction_reports,
            "dashboard": dashboard,
            "daemon_service": service_report,
            "mcp": mcp::lifecycle_metadata(),
        }))?;
    } else {
        let stdout = io::stdout();
        let mut output = stdout.lock();
        writeln!(output, "initialized {}", path.display())?;
        writeln!(output, "data root {}", config.data_dir.display())?;
        writeln!(output, "{outcome} {}", workspace.identity.root.display())?;
        for report in instruction_reports.iter().filter(|report| report.changed) {
            writeln!(
                output,
                "updated {} instructions {}",
                report.surface.as_str(),
                report.path.display()
            )?;
        }
        writeln!(output, "visualizer {dashboard} (served by hzrd)")?;
        if let Some(report) = &service_report {
            writeln!(
                output,
                "visualizer service {} active={}",
                report.action, report.active
            )?;
        } else if !skip_service {
            writeln!(
                output,
                "visualizer service source-only; run `hzr daemon serve`"
            )?;
        }
        writeln!(
            output,
            "mcp client-managed stdio; register once with `hzr install --force`"
        )?;
    }
    Ok(ExitCode::SUCCESS)
}

async fn initialize_if_needed(
    config_path: &Path,
    data_dir: Option<&Path>,
    quiet: bool,
    session_start_hook: bool,
    skip_service: bool,
    json: bool,
) -> Result<ExitCode> {
    let (config, config_created) = if config_path.exists() {
        let config = Config::load_or_default(config_path)
            .with_context(|| format!("failed to load {}", config_path.display()))?;
        if let Some(requested) = data_dir {
            if requested != config.data_dir {
                bail!(
                    "configured data root is {}; --data-dir requested {}",
                    config.data_dir.display(),
                    requested.display()
                );
            }
        }
        (config, false)
    } else {
        let mut config = Config::default();
        if let Some(data_dir) = data_dir {
            config.data_dir = data_dir.to_path_buf();
        }
        config.ensure_layout()?;
        config.write(config_path)?;
        (config, true)
    };

    let workspace_root = canonical_directory(None)?;
    if config.activation.mode == hzr_core::ActivationMode::Selected
        && !activation::is_enabled(&config, &workspace_root)
            .await
            .unwrap_or(false)
    {
        if json {
            print_json(&serde_json::json!({
                "outcome": "disabled",
                "workspace": workspace_root,
                "changed": false,
                "config_created": config_created,
            }))?;
        }
        return Ok(ExitCode::SUCCESS);
    }

    // Instruction repair is independent from index health. A conflicting local RTK block must
    // not survive merely because duplicate or legacy index state correctly blocks registration.
    let instruction_reports = reconcile_agent_instructions(&config, &workspace_root)?;
    let (workspace, outcome, changed, git_backed, registration) =
        initialize_workspace_at(&config, &workspace_root).await?;
    let dashboard = format!("http://{}", config.daemon.bind);
    let service_report = if skip_service {
        None
    } else {
        service::ensure_running_if_installed()?
    };

    if json {
        print_json(&serde_json::json!({
            "outcome": outcome,
            "changed": changed,
            "config_created": config_created,
            "workspace": workspace.identity.root,
            "git_backed": git_backed,
            "repository_id": workspace.identity.repository_id,
            "worktree_id": workspace.identity.worktree_id,
            "index": workspace.index.directory,
            "registration": registration,
            "instructions": instruction_reports,
            "dashboard": dashboard,
            "daemon_service": service_report,
            "mcp": mcp::lifecycle_metadata(),
        }))?;
    } else if !quiet {
        let stdout = io::stdout();
        let mut output = stdout.lock();
        writeln!(output, "{outcome} {}", workspace.identity.root.display())?;
        for report in instruction_reports.iter().filter(|report| report.changed) {
            writeln!(
                output,
                "updated {} instructions {}",
                report.surface.as_str(),
                report.path.display()
            )?;
        }
        writeln!(output, "visualizer {dashboard} (served by hzrd)")?;
        if let Some(report) = &service_report {
            writeln!(
                output,
                "visualizer service {} active={}",
                report.action, report.active
            )?;
        } else if !skip_service {
            writeln!(
                output,
                "visualizer service source-only; run `hzr daemon serve`"
            )?;
        }
        writeln!(
            output,
            "mcp client-managed stdio; register once with `hzr install --force`"
        )?;
        if outcome == "migration_required" {
            writeln!(
                output,
                "run `hzr migrate apply --workspace {}`",
                workspace.identity.root.display()
            )?;
        }
    }
    if !json {
        if let Some(notice) = update::startup_notice(&config.data_dir).await {
            if session_start_hook {
                print_json(&update::session_start_payload(&notice))?;
            } else {
                println!("{notice}");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn initialize_if_enabled(
    config_path: &Path,
    data_dir: Option<&Path>,
    quiet: bool,
    session_start_hook: bool,
    skip_service: bool,
    json: bool,
) -> Result<ExitCode> {
    if !config_path.is_file() {
        return Ok(ExitCode::SUCCESS);
    }
    let config = Config::load(config_path)?;
    if let Some(requested) = data_dir {
        if requested != config.data_dir {
            bail!(
                "configured data root is {}; --data-dir requested {}",
                config.data_dir.display(),
                requested.display()
            );
        }
    }
    let workspace = canonical_directory(None)?;
    if !activation::is_enabled(&config, &workspace)
        .await
        .unwrap_or(false)
    {
        if json {
            print_json(&serde_json::json!({
                "outcome": "disabled",
                "workspace": workspace,
                "changed": false,
            }))?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    initialize_if_needed(
        config_path,
        data_dir,
        quiet,
        session_start_hook,
        skip_service,
        json,
    )
    .await
}

fn show_activation_status(config: &Config, json: bool) -> Result<ExitCode> {
    let report = activation::ActivationStatusReport::from_config(&config.activation);
    if json {
        print_json(&report)?;
    } else {
        print!("{}", activation::render_status_text(&report));
    }
    Ok(ExitCode::SUCCESS)
}

async fn set_workspace_activation(
    config_path: &Path,
    workspace_path: Option<&Path>,
    enabled: bool,
    json: bool,
) -> Result<ExitCode> {
    let root = canonical_directory(workspace_path)?;
    let mut config = Config::load_or_default(config_path)?;
    config.ensure_layout()?;
    config.activation.mode = hzr_core::ActivationMode::Selected;
    let workspace = if enabled {
        initialize_workspace_at(&config, &root).await?.0
    } else {
        activation::discover(&config, &root).await?
    };
    let changed = if enabled {
        let already_enabled = config.activation.allows(
            &workspace.identity.repository_id,
            &workspace.identity.worktree_id,
        );
        config.activation.enable(activation::record(&workspace));
        !already_enabled
    } else {
        config.activation.disable(
            &workspace.identity.repository_id,
            &workspace.identity.worktree_id,
        )
    };
    config.write(config_path)?;

    let executable = std::env::current_exe().context("cannot resolve the HZR executable")?;
    let contract = contract_asset_path(&executable_source_directory(&executable)?);
    for surface in [instructions::Surface::Claude, instructions::Surface::Codex] {
        instructions::uninstall(surface, &surface.default_path()?, false, true)?;
    }
    client_config::uninstall_all(false, true)?;
    let hook_status = adoption::status(&adoption::default_settings_path()?)?;
    if hook_status.installed {
        let prefix_binary = prefix::default_prefix()?.join("hzr");
        let hook_binary = if prefix_binary.is_file() {
            prefix_binary
        } else {
            adoption::resolve_hook_binary(Some(&executable), false, false)?
        };
        adoption::install(
            &adoption::default_settings_path()?,
            &hook_binary,
            true,
            true,
            true,
            false,
            true,
        )?;
    }
    for (surface, target) in activation::local_instruction_paths(&workspace.identity.root) {
        if enabled {
            instructions::install(surface, &target, &contract, false, true)?;
        } else {
            instructions::uninstall(surface, &target, false, true)?;
        }
    }

    if json {
        print_json(&serde_json::json!({
            "enabled": enabled,
            "changed": changed,
            "activation_mode": "selected",
            "workspace": workspace.identity.root,
            "repository_id": workspace.identity.repository_id,
            "worktree_id": workspace.identity.worktree_id,
        }))?;
    } else {
        println!(
            "{} {} (project-only activation)",
            if enabled { "enabled" } else { "disabled" },
            workspace.identity.root.display()
        );
    }
    Ok(ExitCode::SUCCESS)
}

async fn initialize_workspace_at(
    config: &Config,
    workspace_path: &Path,
) -> Result<(Workspace, &'static str, bool, bool, WorkspaceRegistration)> {
    let workspace = Workspace::discover_managed(
        workspace_path,
        Path::new("git"),
        &config.data_dir,
        Deadlines::default().version,
    )
    .await?;
    // A non-Git directory has a path-derived identity. If `git init` happens later,
    // `adopt_relocated_index` moves only HZR-owned index state to the repository-derived
    // identity before the fresh registration is written.
    let git_backed = workspace.identity.git_common_dir.is_some();
    let relocated = workspace.adopt_relocated_index()?;
    workspace.require_single_index()?;
    let (outcome, changed) = match workspace.placement()? {
        IndexPlacement::ManagedSymlink { .. } if relocated => ("relocated_to_git_identity", true),
        IndexPlacement::ManagedSymlink { .. } => ("already_initialized", false),
        IndexPlacement::Missing { .. } => {
            workspace.ensure_managed_location()?;
            if git_backed {
                ("initialized", true)
            } else {
                ("initialized_without_git", true)
            }
        }
        IndexPlacement::LegacyProject { .. } => ("migration_required", false),
        placement => bail!("unsupported grepai placement: {placement:?}"),
    };
    let registration = workspace.register()?;
    Ok((workspace, outcome, changed, git_backed, registration))
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
    let mode = arguments.mode.map_or(default_mode, Into::into);
    let path_scope_count = arguments.path.len().max(1);
    // One request per subtree, merged. The daemon filter takes a single path, and running
    // the searches here keeps that contract while letting an agent write the multi-path
    // invocation it would reach for anyway.
    let paths = if arguments.path.is_empty() {
        vec![None]
    } else {
        arguments
            .path
            .iter()
            .map(|path| Some(path.to_string_lossy().into_owned()))
            .collect()
    };
    let client = DaemonClient::from_config(config)?;
    let mut merged: Option<SearchApiResponse> = None;
    for path in paths {
        let response = client
            .search(&SearchApiRequest {
                workspace: workspace.to_string_lossy().into_owned(),
                query: arguments.query.clone(),
                path,
                limit: arguments.limit,
                mode,
                include_content: arguments.include_content,
            })
            .await?;
        merged = Some(match merged {
            None => response,
            Some(accumulated) => merge_search_responses(accumulated, response),
        });
    }
    let mut response = merged.unwrap_or_else(|| unreachable!("at least one search is issued"));
    response.hits.truncate(arguments.limit);
    response.shown_hits = response.hits.len();
    let output = if json {
        let mut output = serde_json::to_vec_pretty(&response)?;
        output.push(b'\n');
        output
    } else {
        render_search(&response)?
    };
    io::stdout().lock().write_all(&output)?;
    record_search_delivery(
        config,
        &client,
        &workspace,
        mode,
        arguments.include_content,
        arguments.limit,
        path_scope_count,
        &response,
        output.len(),
    )
    .await;
    Ok(ExitCode::SUCCESS)
}

#[allow(clippy::too_many_arguments)]
async fn record_search_delivery(
    config: &Config,
    client: &DaemonClient,
    workspace: &Path,
    requested_mode: SearchMode,
    include_content: bool,
    limit: usize,
    path_scope_count: usize,
    response: &SearchApiResponse,
    output_bytes: usize,
) {
    let delivered = u64::try_from(output_bytes / 4).unwrap_or(u64::MAX).max(1);
    let effective_mode = accounting_effective_search_mode(response);
    let request = OperationApiRequest {
        original_command: "hzr search".to_owned(),
        recorded_command: "hzr search".to_owned(),
        baseline_tokens_estimated: delivered,
        delivered_tokens_estimated: delivered,
        execution_ms: 0,
        project_path: workspace.to_string_lossy().into_owned(),
        channel: AccountingChannel::HookCli,
        measurement: AccountingMeasurement::Estimated,
        route: AccountingRoute::Optimized,
        agent: Some("cli".to_owned()),
        session_id: ["CODEX_THREAD_ID", "CLAUDE_SESSION_ID"]
            .into_iter()
            .find_map(|name| {
                std::env::var(name)
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            }),
        attribution: Some(AccountingAttribution {
            operation: AccountingOperationKind::Search,
            mode: effective_mode,
            stage: AccountingStage::FinalDelivery,
            requested_mode: Some(accounting_search_mode(requested_mode)),
            effective_mode: Some(effective_mode),
            search_strategy: Some(match response.strategy {
                SearchStrategy::ForkRgaiAdaptive => AccountingSearchStrategy::ForkRgaiAdaptive,
                SearchStrategy::ForkRgaiBuiltin => AccountingSearchStrategy::ForkRgaiBuiltin,
                SearchStrategy::ForkRgaiGrepai => AccountingSearchStrategy::ForkRgaiGrepai,
                SearchStrategy::ForkRgaiRipgrep => AccountingSearchStrategy::ForkRgaiRipgrep,
                SearchStrategy::ForkRgaiFiles => AccountingSearchStrategy::ForkRgaiFiles,
            }),
            search_fallback_code: response.fallback_code,
            include_content: Some(include_content),
            limit: Some(u64::try_from(limit).unwrap_or(u64::MAX)),
            path_scope_count: Some(u64::try_from(path_scope_count).unwrap_or(u64::MAX)),
            filter_level: None,
            from_line: None,
            to_line: None,
            source_bytes: None,
        }),
    };
    if client.record_operation(&request).await.is_err() {
        let _ = hook_runner::record_daemon_unavailable_operation(config);
    }
}

const fn accounting_effective_search_mode(response: &SearchApiResponse) -> AccountingOperationMode {
    match response.strategy {
        SearchStrategy::ForkRgaiBuiltin if matches!(response.effective_mode, SearchMode::Exact) => {
            AccountingOperationMode::SearchExact
        }
        SearchStrategy::ForkRgaiBuiltin => AccountingOperationMode::SearchBuiltin,
        SearchStrategy::ForkRgaiAdaptive => accounting_search_mode(response.effective_mode),
        SearchStrategy::ForkRgaiGrepai
        | SearchStrategy::ForkRgaiRipgrep
        | SearchStrategy::ForkRgaiFiles => AccountingOperationMode::SearchSemantic,
    }
}

const fn accounting_search_mode(mode: SearchMode) -> AccountingOperationMode {
    match mode {
        SearchMode::Auto => AccountingOperationMode::SearchAuto,
        SearchMode::Semantic => AccountingOperationMode::SearchSemantic,
        SearchMode::Exact => AccountingOperationMode::SearchExact,
    }
}

/// Combine two subtree searches into one response, keeping the strongest hit per file.
fn merge_search_responses(
    mut accumulated: SearchApiResponse,
    other: SearchApiResponse,
) -> SearchApiResponse {
    accumulated.path = format!("{}, {}", accumulated.path, other.path);
    accumulated.total_hits += other.total_hits;
    accumulated.scanned_files += other.scanned_files;
    accumulated.skipped_large += other.skipped_large;
    accumulated.skipped_binary += other.skipped_binary;
    for hit in other.hits {
        if !accumulated
            .hits
            .iter()
            .any(|existing| existing.path == hit.path)
        {
            accumulated.hits.push(hit);
        }
    }
    accumulated
        .hits
        .sort_by(|left, right| right.score.total_cmp(&left.score));
    accumulated.shown_hits = accumulated.hits.len();
    accumulated
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
            scope,
        } => {
            let workspace = canonical_directory(workspace.as_deref())?;
            let response = client
                .memory_recall(&MemoryRecallApiRequest {
                    workspace: path_text(&workspace, "memory workspace")?,
                    query,
                    topic,
                    limit,
                    keyword,
                    scope: scope.into(),
                })
                .await?;
            if json {
                print_json(&response)?;
            } else {
                print_memories(&response.memories)?;
                if response.count < response.total_matches {
                    println!(
                        "showing {} of {} matches; rerun with --limit {}",
                        response.count,
                        response.total_matches,
                        response.total_matches.min(50)
                    );
                }
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
            scope,
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
                    scope: scope.into(),
                })
                .await?;
            print_json(&response)?;
        }
        MemoryCommand::Forget {
            id,
            workspace,
            scope,
        } => {
            let workspace = canonical_directory(workspace.as_deref())?;
            let response = client
                .memory_forget(&MemoryForgetApiRequest {
                    workspace: path_text(&workspace, "memory workspace")?,
                    id,
                    scope: scope.into(),
                })
                .await?;
            print_json(&response)?;
        }
        MemoryCommand::Update {
            id,
            workspace,
            content,
            file,
            importance,
            keywords,
            scope,
        } => {
            let workspace = canonical_directory(workspace.as_deref())?;
            let content = read_text(
                content,
                file.as_deref(),
                payload_limit(config.daemon.request_limit_bytes),
            )?;
            let response = client
                .memory_update(&MemoryUpdateApiRequest {
                    workspace: path_text(&workspace, "memory workspace")?,
                    id,
                    content,
                    scope: scope.into(),
                    importance: importance.map(Into::into),
                    keywords,
                })
                .await?;
            print_json(&response)?;
        }
        MemoryCommand::Prune {
            workspace,
            threshold,
            apply,
            scope,
        } => {
            let workspace = canonical_directory(workspace.as_deref())?;
            let response = client
                .memory_prune(&MemoryPruneApiRequest {
                    workspace: path_text(&workspace, "memory workspace")?,
                    threshold,
                    dry_run: !apply,
                    scope: scope.into(),
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
        caller_path: std::env::var("PATH").ok(),
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
            channel: None,
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
        .unwrap_or_else(|| config.engines.binary("node"));
    let agent_data = config
        .data_dir
        .join("sessions")
        .join(SessionId::new().to_string());
    let mut agent_config =
        ManagedAgentConfig::new(node, integration_layout(config), workspace, agent_data, api);
    agent_config.timeout = Duration::from_millis(timeout_ms);
    let agent = ManagedAgent::new(agent_config);
    let running = agent.run(&prompt, response_format.into(), max_turns);
    tokio::pin!(running);
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(15),
        Duration::from_secs(15),
    );
    let run = loop {
        tokio::select! {
            result = &mut running => break result?,
            _ = heartbeat.tick() => eprintln!("hzr agent is still working"),
        }
    };
    print_agent(&run, json)?;
    Ok(ExitCode::SUCCESS)
}

async fn show_stats(
    config: &Config,
    workspace: Option<&Path>,
    json: bool,
    include_all_commands: bool,
    since: Option<&crate::cli::StatsDuration>,
) -> Result<ExitCode> {
    let workspace = workspace
        .map(|path| canonical_directory(Some(path)))
        .transpose()?;
    let report = stats::collect(config, workspace.as_deref(), include_all_commands, since).await?;
    if json {
        print_json(&report)?;
    } else {
        print_stats(&report)?;
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
    use std::path::Path;

    use tempfile::tempdir;

    use super::{
        bounded_read_arguments, canonical_directory, contract_asset_path,
        executable_source_directory, payload_limit,
    };

    #[test]
    fn acceptance_gate_direct_cli_reduces_only_unbounded_exact_reads() {
        let arguments = |values: &[&str]| {
            values
                .iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            bounded_read_arguments(&arguments(&["read", "README.md", "--level", "none"]), false),
            arguments(&["read", "README.md"])
        );
        assert_eq!(
            bounded_read_arguments(&arguments(&["read", "README.md", "--level=none"]), false),
            arguments(&["read", "README.md"])
        );
        for preserved in [
            arguments(&[
                "read",
                "README.md",
                "--from",
                "1",
                "--to",
                "20",
                "--level",
                "none",
            ]),
            arguments(&["read", "README.md", "--outline", "--level", "none"]),
            arguments(&["read", "README.md", "--level", "none"]),
            arguments(&["write", "replace", "README.md", "old", "new"]),
        ] {
            let exact_fidelity = preserved == arguments(&["read", "README.md", "--level", "none"]);
            assert_eq!(
                bounded_read_arguments(&preserved, exact_fidelity),
                preserved
            );
        }
    }

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

    #[cfg(unix)]
    #[test]
    fn contract_uses_current_pointer_for_an_installed_release() {
        let directory = tempdir().expect("temporary directory");
        let release = directory.path().join("versions/v0.4.3-test");
        let source = release.join("bin");
        let contract = release.join("share/hzr/HZR.md");
        std::fs::create_dir_all(&source).expect("release bin");
        std::fs::create_dir_all(contract.parent().expect("contract parent"))
            .expect("release share");
        std::fs::write(&contract, "contract").expect("contract fixture");
        std::os::unix::fs::symlink(&release, directory.path().join("current"))
            .expect("current symlink");

        let stable = contract_asset_path(&source);
        assert_eq!(stable, directory.path().join("current/share/hzr/HZR.md"));
        assert!(!stable.to_string_lossy().contains("/versions/"));
        assert!(Path::new(&stable).is_file());
    }

    #[cfg(unix)]
    #[test]
    fn contract_keeps_a_logical_current_source_upgradeable() {
        let directory = tempdir().expect("temporary directory");
        let release = directory.path().join("versions/v0.4.3-test");
        let contract = release.join("share/hzr/HZR.md");
        std::fs::create_dir_all(release.join("bin")).expect("release bin");
        std::fs::create_dir_all(contract.parent().expect("contract parent"))
            .expect("release share");
        std::fs::write(&contract, "contract").expect("contract fixture");
        let current = directory.path().join("current");
        std::os::unix::fs::symlink(&release, &current).expect("current symlink");

        let stable = contract_asset_path(&current.join("bin"));
        assert_eq!(stable, current.join("share/hzr/HZR.md"));
        assert!(!stable.to_string_lossy().contains("/versions/"));
    }

    #[cfg(unix)]
    #[test]
    fn public_binary_symlink_resolves_to_the_versioned_source_directory() {
        let directory = tempdir().expect("temporary directory");
        let release_bin = directory.path().join("versions/v0.4.3-test/bin");
        let release_binary = release_bin.join("hzr");
        let public_bin = directory.path().join("bin");
        std::fs::create_dir_all(&release_bin).expect("release bin");
        std::fs::create_dir_all(&public_bin).expect("public bin");
        std::fs::write(&release_binary, "binary").expect("binary fixture");
        std::os::unix::fs::symlink(&release_binary, public_bin.join("hzr"))
            .expect("public binary symlink");

        assert_eq!(
            executable_source_directory(&public_bin.join("hzr")).expect("source directory"),
            release_bin.canonicalize().expect("canonical release bin")
        );
    }
}
