mod activation;
mod adoption;
mod build;
mod cli;
mod cli_help;
mod cli_subcommand_help;
mod client;
mod client_config;
mod diagnostics;
mod fleet_exemption;
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

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::Parser;
use fs2::FileExt;
use hzr_agent::{BearerToken, HzrApi, ManagedAgent, ManagedAgentConfig};
use hzr_core::{
    Config, ConfigPaths, Ledger, PolicyEvent, ProviderEconomicReceipt, discover_legacy_rtk_history,
    inspect_legacy_efficiency, load_pricing_catalog,
};
use hzr_index::{
    Deadlines, GrepAi, IndexPlacement, InitOptions, InitOutcome, Workspace, WorkspaceRegistration,
    archive_duplicate_index, migrate_legacy_index,
};
use hzr_protocol::{
    AccountingAttribution, AccountingChannel, AccountingMeasurement, AccountingOperationKind,
    AccountingOperationMode, AccountingRoute, AccountingSearchStrategy, AccountingStage,
    CodecApiRequest, ContextPlanApiRequest, EnforcementTier, EvasionAttribution, EvasionClass,
    EvasionPathForm, ExecApiRequest, ExecApprovalApiRequest, FidelityReason,
    FidelityReconcileApiRequest, FidelityUnknownResolution, FidelityValidation,
    MemoryForgetApiRequest, MemoryPruneApiRequest, MemoryRecallApiRequest, MemoryStoreApiRequest,
    MemoryUpdateApiRequest, OperationApiRequest, PROTOCOL_VERSION, PolicyDecision,
    SearchApiRequest, SearchApiResponse, SearchMode, SearchStrategy, SessionId,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use toml_edit::{DocumentMut, value};

use crate::cli::{
    ActivationCommand, AgentCommand, BillingCommand, Cli, CodecCommand, Command, ContextCommand,
    ContextPlanArgs, DaemonCommand, EnginesCommand, ExecArgs, ExecCommand, HooksCommand,
    IndexCommand, McpCommand, MemoryCommand, MigrateCommand, SearchArgs, ServiceCommand,
};
use crate::client::DaemonClient;
use crate::diagnostics::{doctor, integration_layout, repair_legacy_index};
use crate::input::read_text;
use crate::invocation::normalize;
use crate::migration::scan;
use crate::output::{
    print_agent, print_context, print_doctor, print_engines, print_execution,
    print_fleet_reconcile, print_health, print_index_archive, print_index_init, print_index_status,
    print_json, print_memories, print_memory_health, print_migration, print_migration_apply,
    print_rewrite, print_stats, print_transform, render_search,
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
            native_tool_mode,
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
                    native_tool_mode: *native_tool_mode,
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
        reset,
        dry_run,
        data_dir,
        if_needed,
        if_enabled,
        quiet,
        session_start_hook,
        skip_service,
        ..
    } = &cli.command
    {
        // `--if-needed --dry-run` asks the same question the writing path asks, without
        // writing: the plan's `changes_required` is the answer.
        if *if_needed && *dry_run {
            return initialize(
                &config_path,
                *force,
                *reset,
                true,
                data_dir.as_deref(),
                *skip_service,
                cli.json,
            )
            .await;
        }
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
            *reset,
            *dry_run,
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
            command: HooksCommand::Dispatch { native_mode },
        } => {
            hook_runner::dispatch(&config, native_mode).await;
            Ok(ExitCode::SUCCESS)
        }
        Command::Hooks {
            command: HooksCommand::Observe { native_mode },
        } => {
            hook_runner::observe(&config, native_mode).await;
            Ok(ExitCode::SUCCESS)
        }
        Command::Hooks {
            command: HooksCommand::Feedback,
        } => {
            hook_runner::feedback(&config).await;
            Ok(ExitCode::SUCCESS)
        }
        Command::Doctor {
            workspace,
            fix,
            reconcile_fleet,
            dry_run,
            migrate_legacy_indexes,
            resolve_fidelity,
            acknowledge_executed,
            prove_not_executed,
        } => {
            let workspace = canonical_directory(workspace.as_deref())?;
            // Reconcile before diagnosing, so the report that follows is the state the
            // operator is left in rather than the one they arrived with.
            let fleet_reconcile = if reconcile_fleet {
                let executable =
                    std::env::current_exe().context("cannot resolve the HZR executable")?;
                let contract = contract_asset_path(&executable_source_directory(&executable)?);
                let binary = project_mcp_binary()?;
                Some(
                    diagnostics::reconcile_fleet_contracts(
                        &config,
                        &contract,
                        &binary,
                        dry_run,
                        migrate_legacy_indexes,
                    )
                    .await,
                )
            } else {
                None
            };
            let repair = if fix {
                repair_legacy_index(&config, &workspace).await?
            } else {
                None
            };
            let fidelity_reconcile = match resolve_fidelity {
                Some(reservation_id) => {
                    let resolution = match (acknowledge_executed, prove_not_executed) {
                        (true, false) => FidelityUnknownResolution::AcknowledgeExecuted,
                        (false, true) => FidelityUnknownResolution::ProveNotExecuted,
                        _ => anyhow::bail!(
                            "--resolve-fidelity requires exactly one of --acknowledge-executed or --prove-not-executed"
                        ),
                    };
                    Some(
                        DaemonClient::from_config(&config)?
                            .fidelity_reconcile(&FidelityReconcileApiRequest {
                                reservation_id: reservation_id.clone(),
                                resolution,
                            })
                            .await?,
                    )
                }
                None => None,
            };
            let mut report = doctor(&config_path, &config, &workspace).await;
            report.repair = repair;
            report.fidelity_reconcile = fidelity_reconcile;
            report.fleet_reconcile = fleet_reconcile;
            if let Some(fleet) = &report.fleet_reconcile {
                let completion = fleet.completion_check();
                if completion.status == diagnostics::CheckStatus::Error {
                    report.healthy = false;
                }
                report.checks.push(completion);
            }
            if cli.json {
                print_json(&report)?;
            } else {
                if let Some(outcome) = &report.repair {
                    print_migration_apply(outcome)?;
                }
                if let Some(fleet) = &report.fleet_reconcile {
                    print_fleet_reconcile(fleet)?;
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
        Command::Billing { command } => execute_billing(&config, command, cli.json).await,
        Command::Agent { command } => execute_agent(&config, command, cli.json).await,
        Command::Mcp { command } => match command {
            McpCommand::Serve { workspace } => {
                let workspace = canonical_directory(workspace.as_deref())?;
                mcp::serve(&config, &config_path, &workspace).await?;
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
                let workspace = canonical_directory(None)?;
                let clients = client_config::status_all_for_workspace(&workspace)?;
                let bindings = clients
                    .iter()
                    .map(|status| client_config::evaluate_workspace_binding(status, &workspace))
                    .collect::<Vec<_>>();
                if cli.json {
                    print_json(&serde_json::json!({
                        "lifecycle": mcp::lifecycle_metadata(),
                        "workspace": workspace,
                        "clients": clients,
                        "workspace_bindings": bindings,
                    }))?;
                } else {
                    println!(
                        "lifecycle={} workspace={} started-by-init=false launch='MCP client connection'",
                        client_config::MCP_LIFECYCLE,
                        workspace.display()
                    );
                    for (client, binding) in clients.into_iter().zip(bindings) {
                        println!(
                            "{} {} exists={} registered={} binding={} scope={} availability={} pinned={} direct-icm={} command={} action={}",
                            client.client.as_str(),
                            client.path.display(),
                            client.config_exists,
                            client.registered,
                            client.workspace_binding_capability.as_str(),
                            client.registration_scope.as_str(),
                            binding.availability.as_str(),
                            client.pinned_workspace.as_deref().unwrap_or("-"),
                            client.direct_icm_registrations,
                            client.command.as_deref().unwrap_or("-"),
                            binding.action
                        );
                    }
                }
                Ok(ExitCode::SUCCESS)
            }
        },
        Command::Build(arguments) => {
            // Compatibility surface only: this is the inherited fork-core self-build
            // pipeline, not a generic project build wrapper. Project builds use exec policy.
            let args = forwarded_fork_args("build", &arguments.args);
            fork::passthrough(&config, &args).await
        }
        Command::Test(arguments) => {
            let args = forwarded_fork_args("test", &arguments.args);
            fork::passthrough(&config, &args).await
        }
        Command::Read(arguments) => {
            let args = forwarded_fork_args("read", &arguments.args);
            let args = bounded_read_arguments(&args, exact_read_fidelity_requested());
            fork::passthrough(&config, &args).await
        }
        Command::Write(arguments) => {
            let args = forwarded_fork_args("write", &arguments.args);
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
            evasion,
            since,
            accounting_version,
        } => {
            show_stats(
                &config,
                workspace.as_deref(),
                cli.json,
                all,
                evasion,
                since.as_ref(),
                accounting_version,
            )
            .await
        }
        Command::Savings => {
            show_stats(
                &config,
                None,
                cli.json,
                false,
                false,
                None,
                crate::cli::AccountingVersion::Current,
            )
            .await
        }
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
            MigrateCommand::ArchiveIndex {
                workspace,
                source,
                dry_run,
                force,
            } => {
                if !dry_run && !force {
                    bail!(
                        "index archive requires `--dry-run` for inspection or `--force` to apply"
                    );
                }
                let workspace = canonical_directory(workspace.as_deref())?;
                let outcome = archive_duplicate_index(
                    &workspace,
                    &source,
                    Path::new("git"),
                    &config.data_dir,
                    Deadlines::default().version,
                    force,
                )
                .await?;
                if cli.json {
                    print_json(&outcome)?;
                } else {
                    print_index_archive(&outcome)?;
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
            reject_direct_fork_bypass(&config, &arguments.args)?;
            let args = bounded_read_arguments(&arguments.args, exact_read_fidelity_requested());
            fork::passthrough(&config, &args).await
        }
    }
}

fn reject_direct_fork_bypass(config: &Config, args: &[std::ffi::OsString]) -> Result<()> {
    let Some(bypass) = args.first().and_then(|arg| arg.to_str()) else {
        return Ok(());
    };
    if !matches!(bypass, "raw" | "proxy") {
        return Ok(());
    }

    let project_path = std::env::current_dir()
        .context("resolve direct bypass working directory")?
        .to_string_lossy()
        .into_owned();
    let agent = std::env::var("HZR_CLIENT").ok();
    let session_id = std::env::var("HZR_SESSION_ID")
        .ok()
        .or_else(|| std::env::var("CODEX_THREAD_ID").ok())
        .or_else(|| std::env::var("CLAUDE_SESSION_ID").ok());
    let hatch_marker =
        std::env::var_os("HZR_RAW_FIDELITY").as_deref() == Some(std::ffi::OsStr::new("1"));
    let (fidelity_reason, fidelity_validation) = if bypass != "raw" || !hatch_marker {
        (None, FidelityValidation::NotRequested)
    } else {
        match std::env::var("HZR_RAW_FIDELITY_REASON").ok().as_deref() {
            Some("binary") => (Some(FidelityReason::Binary), FidelityValidation::Valid),
            Some("checksum") => (Some(FidelityReason::Checksum), FidelityValidation::Valid),
            Some("machine_protocol") => (
                Some(FidelityReason::MachineProtocol),
                FidelityValidation::Valid,
            ),
            Some("complete_log") => (Some(FidelityReason::CompleteLog), FidelityValidation::Valid),
            Some("full_patch") => (Some(FidelityReason::FullPatch), FidelityValidation::Valid),
            Some("verbatim_source") => (
                Some(FidelityReason::VerbatimSource),
                FidelityValidation::Valid,
            ),
            Some(_) => (None, FidelityValidation::InvalidReason),
            None => (None, FidelityValidation::MissingReason),
        }
    };
    let accounting = Ledger::open(&config.data_dir.join("ledger/hzr.sqlite")).and_then(|ledger| {
        ledger.record_policy_event(PolicyEvent {
            project_path: &project_path,
            agent: agent.as_deref(),
            session_id: session_id.as_deref(),
            evasion: EvasionAttribution {
                class: if bypass == "raw" {
                    EvasionClass::E7FidelityHatch
                } else {
                    EvasionClass::E9DiagnosticBypass
                },
                wrapper_depth: 0,
                interpreter: None,
                path_form: EvasionPathForm::Bare,
                stage_count: 1,
                hatch_marker,
                avoidable: true,
                tier: EnforcementTier::T2DenyWithPrescription,
                fidelity_reason,
                fidelity_validation,
            },
            decision: PolicyDecision::Deny,
            replacement_family: Some("hzr-exec"),
        })
    });
    if let Err(error) = accounting {
        bail!(
            "direct managed raw execution is disabled; use `hzr exec run <command>` so fidelity policy, session budget, and E7 accounting are enforced; denial accounting failed: {error}"
        );
    }
    bail!(
        "direct managed raw execution is disabled; use `hzr exec run <command>` so fidelity policy, session budget, and E7 accounting are enforced"
    )
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
    native_tool_mode: Option<adoption::NativeToolMode>,
    workspace: Option<PathBuf>,
}

pub(crate) const INSTALL_JOURNAL_SCHEMA_VERSION: u16 = 2;
pub(crate) const INSTALL_STAGES: [&str; 8] = [
    "config",
    "workspace",
    "prefix",
    "hooks",
    "instructions",
    "client_configs",
    "project_mcp",
    "service",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum InstallJournalState {
    Applying,
    Recovering,
    Complete,
}

#[derive(Clone, Debug, Serialize, serde::Deserialize)]
struct InstallJournal {
    schema_version: u16,
    state: InstallJournalState,
    config_path: PathBuf,
    workspace: PathBuf,
    plan_sha256: String,
    planned_stages: Vec<String>,
    completed_stages: Vec<String>,
    attempt: u32,
}

impl InstallJournal {
    fn begin(path: &Path, config_path: &Path, workspace: &Path, plan_sha256: &str) -> Result<Self> {
        let journal = if path.is_file() {
            let existing: Self = serde_json::from_slice(&std::fs::read(path)?)
                .with_context(|| format!("parse install recovery journal {}", path.display()))?;
            existing.validate(path)?;
            if existing.state != InstallJournalState::Complete
                && (existing.config_path != config_path || existing.workspace != workspace)
            {
                bail!(
                    "incomplete install journal {} belongs to config {} workspace {}; recover that install before starting another",
                    path.display(),
                    existing.config_path.display(),
                    existing.workspace.display()
                );
            }
            if existing.state != InstallJournalState::Complete
                && existing.plan_sha256 != plan_sha256
            {
                bail!(
                    "incomplete install journal {} has plan {}; rerun the same install options before changing desired state",
                    path.display(),
                    existing.plan_sha256
                );
            }
            let completed = existing.state == InstallJournalState::Complete;
            Self {
                state: InstallJournalState::Recovering,
                config_path: if completed {
                    config_path.to_path_buf()
                } else {
                    existing.config_path.clone()
                },
                workspace: if completed {
                    workspace.to_path_buf()
                } else {
                    existing.workspace.clone()
                },
                completed_stages: if completed {
                    Vec::new()
                } else {
                    existing.completed_stages
                },
                plan_sha256: plan_sha256.to_owned(),
                attempt: existing.attempt.saturating_add(1),
                ..existing
            }
        } else {
            Self {
                schema_version: INSTALL_JOURNAL_SCHEMA_VERSION,
                state: InstallJournalState::Applying,
                config_path: config_path.to_path_buf(),
                workspace: workspace.to_path_buf(),
                plan_sha256: plan_sha256.to_owned(),
                planned_stages: INSTALL_STAGES.iter().map(ToString::to_string).collect(),
                completed_stages: Vec::new(),
                attempt: 1,
            }
        };
        journal.persist(path)?;
        Ok(journal)
    }

    fn stage(&mut self, path: &Path, stage: &str) -> Result<()> {
        if !self
            .completed_stages
            .iter()
            .any(|completed| completed == stage)
        {
            self.completed_stages.push(stage.to_owned());
        }
        self.persist(path)
    }

    fn complete(&mut self, path: &Path) -> Result<()> {
        self.state = InstallJournalState::Complete;
        self.persist(path)
    }

    fn validate(&self, path: &Path) -> Result<()> {
        if self.schema_version != INSTALL_JOURNAL_SCHEMA_VERSION {
            bail!(
                "unsupported install journal schema {} in {}",
                self.schema_version,
                path.display()
            );
        }
        let expected = INSTALL_STAGES
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        let planned = self
            .planned_stages
            .iter()
            .map(String::as_str)
            .collect::<std::collections::HashSet<_>>();
        if planned != expected || self.planned_stages.len() != INSTALL_STAGES.len() {
            bail!(
                "install journal {} has an invalid stage plan",
                path.display()
            );
        }
        if self
            .completed_stages
            .iter()
            .any(|stage| !expected.contains(stage.as_str()))
        {
            bail!(
                "install journal {} has an unknown completed stage",
                path.display()
            );
        }
        if self
            .completed_stages
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != self.completed_stages.len()
            || self.plan_sha256.len() != 64
            || !self
                .plan_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.attempt == 0
            || !self.config_path.is_absolute()
            || !self.workspace.is_absolute()
        {
            bail!(
                "install journal {} failed integrity validation",
                path.display()
            );
        }
        Ok(())
    }

    fn persist(&self, path: &Path) -> Result<()> {
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        adoption::atomic_write(path, &bytes)
    }
}

fn install_plan_sha256(
    options: &InstallOptions,
    config_path: &Path,
    workspace: &Path,
) -> Result<String> {
    let value = serde_json::json!({
        "schema_version": INSTALL_JOURNAL_SCHEMA_VERSION,
        "config_path": config_path,
        "workspace": workspace,
        "stages": INSTALL_STAGES,
        "prefix": options.prefix,
        "binary": options.binary,
        "allow_dev_path": options.allow_dev_path,
        "adopt_icm": options.adopt_icm,
        "wire_instructions": options.wire_instructions,
        "start_service": options.start_service,
        "project_only": options.project_only,
        "native_tool_mode": options.native_tool_mode,
    });
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&value)?)))
}

fn acquire_install_lock() -> Result<File> {
    let settings = adoption::default_settings_path()?;
    let identity = hex::encode(Sha256::digest(settings.as_os_str().as_encoded_bytes()));
    let path = std::env::temp_dir().join(format!("hzr-install-{}.lock", &identity[..24]));
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open install transaction lock {}", path.display()))?;
    lock.lock_exclusive()
        .with_context(|| format!("lock install transaction {}", path.display()))?;
    Ok(lock)
}

fn inject_install_failure(_point: &str) -> Result<()> {
    #[cfg(debug_assertions)]
    if std::env::var("HZR_TEST_INSTALL_FAIL_AFTER").as_deref() == Ok(_point) {
        bail!("injected install failure after {_point}; rerun `hzr install --force` to recover");
    }
    Ok(())
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
    let _install_lock = acquire_install_lock()?;
    let workspace_root = canonical_directory(options.workspace.as_deref())?;
    let config_path = if config_path.is_absolute() {
        config_path.to_path_buf()
    } else {
        std::env::current_dir()?.join(config_path)
    };
    let config_before = std::fs::read(&config_path).ok();
    let executable = std::env::current_exe().context("cannot resolve the HZR executable")?;
    let source_dir = executable_source_directory(&executable)?;
    let mut config = Config::load_or_default(&config_path)?;
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
    let workspace = Some(activation::discover(&config, &workspace_root).await?);
    if options.project_only {
        config.activation.mode = hzr_core::ActivationMode::Selected;
        config
            .activation
            .enable(activation::record(workspace.as_ref().context("workspace")?));
    } else {
        config.activation.mode = hzr_core::ActivationMode::All;
        config.activation.enabled_workspaces.clear();
    }
    let journal_path = config.data_dir.join("runtime/install-transaction.json");
    let plan_sha256 = install_plan_sha256(&options, &config_path, &workspace_root)?;
    let mut journal = if options.dry_run {
        None
    } else {
        Some(InstallJournal::begin(
            &journal_path,
            &config_path,
            &workspace_root,
            &plan_sha256,
        )?)
    };
    if !options.dry_run {
        if std::fs::read(&config_path).ok() != config_before {
            bail!(
                "configuration {} changed after install planning; no config write was attempted",
                config_path.display()
            );
        }
        config.write(&config_path)?;
        journal
            .as_mut()
            .context("install journal")?
            .stage(&journal_path, "config")?;
        inject_install_failure("config")?;
        if options.project_only {
            initialize_workspace_at(&config, &workspace_root).await?;
        }
        journal
            .as_mut()
            .context("install journal")?
            .stage(&journal_path, "workspace")?;
        inject_install_failure("workspace")?;
    }

    let prefix_dir = match options.prefix.clone() {
        Some(prefix) => prefix,
        None => prefix::default_prefix()?,
    };
    let prefix_report = prefix::install(&prefix_dir, &source_dir, options.dry_run, options.force)?;
    if let Some(journal) = journal.as_mut() {
        journal.stage(&journal_path, "prefix")?;
        inject_install_failure("prefix")?;
    }

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
        adoption::HookInstallPolicy {
            native_tool_mode: options.native_tool_mode,
            dry_run: options.dry_run,
            confirmed: options.force,
        },
    )?;
    if let Some(journal) = journal.as_mut() {
        journal.stage(&journal_path, "hooks")?;
        inject_install_failure("hooks")?;
    }

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
                    instruction_reports.push(instructions::install(
                        surface,
                        &target,
                        &contract,
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
    if let Some(journal) = journal.as_mut() {
        journal.stage(&journal_path, "instructions")?;
        inject_install_failure("instructions")?;
    }

    let mut client_reports = if options.project_only {
        client_config::uninstall_all(options.dry_run, options.force)?
    } else {
        client_config::install_all(
            &hook_binary,
            &workspace_root,
            options.dry_run,
            options.force,
        )?
    };
    if let Some(journal) = journal.as_mut() {
        journal.stage(&journal_path, "client_configs")?;
        inject_install_failure("client_configs")?;
    }

    let project_mcp = client_config::install_project_codex(
        &hook_binary,
        &workspace_root,
        options.dry_run,
        options.force,
    )?;
    client_reports.push(project_mcp);
    if let Some(journal) = journal.as_mut() {
        journal.stage(&journal_path, "project_mcp")?;
        inject_install_failure("project_mcp")?;
    }

    let foreign_report = foreign::scan(&ConfigPaths::discover().data_dir).ok();
    let service_report = if options.start_service && !options.dry_run {
        service::ensure_running_if_installed()?
    } else {
        None
    };
    if let Some(journal) = journal.as_mut() {
        journal.stage(&journal_path, "service")?;
        inject_install_failure("service")?;
        journal.complete(&journal_path)?;
    }

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
    let (contract, targets) = agent_instruction_targets(config, workspace_root)?;
    targets
        .into_iter()
        .map(|(surface, path)| instructions::install(surface, &path, &contract, false, true))
        .collect()
}

fn plan_agent_instructions(
    config: &Config,
    workspace_root: &Path,
) -> Result<Vec<instructions::InstructionReport>> {
    let (contract, targets) = agent_instruction_targets(config, workspace_root)?;
    targets
        .into_iter()
        .map(|(surface, path)| instructions::install(surface, &path, &contract, true, true))
        .collect()
}

fn agent_instruction_targets(
    config: &Config,
    workspace_root: &Path,
) -> Result<(PathBuf, Vec<(instructions::Surface, PathBuf)>)> {
    let executable = std::env::current_exe().context("cannot resolve the HZR executable")?;
    let contract = contract_asset_path(&executable_source_directory(&executable)?);
    let targets = scoped_instruction_targets(config.activation.mode, workspace_root)?;
    Ok((contract, targets))
}

fn scoped_instruction_targets(
    activation_mode: hzr_core::ActivationMode,
    workspace_root: &Path,
) -> Result<Vec<(instructions::Surface, PathBuf)>> {
    let targets = match activation_mode {
        hzr_core::ActivationMode::All => {
            let mut targets = [instructions::Surface::Claude, instructions::Surface::Codex]
                .into_iter()
                .map(|surface| surface.default_path().map(|path| (surface, path)))
                .collect::<Result<Vec<_>>>()?;
            // The local routing pointer keeps policy visible at the point where an agent
            // discovers repository instructions. The managed region preserves all user text.
            targets.extend(activation::local_instruction_paths(workspace_root));
            targets
        }
        hzr_core::ActivationMode::Selected => {
            activation::local_instruction_paths(workspace_root).to_vec()
        }
    };
    Ok(targets)
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
    reset: bool,
    dry_run: bool,
    data_dir: Option<&Path>,
    skip_service: bool,
    json: bool,
) -> Result<ExitCode> {
    let existed = path.exists();
    if existed && !force && !dry_run {
        bail!(
            "configuration {} already exists; pass --force to reconcile it without replacing user settings, or --force --reset for an explicit reset",
            path.display()
        );
    }
    let mut config = if existed && !reset {
        Config::load(path).with_context(|| format!("failed to preserve {}", path.display()))?
    } else {
        Config::default()
    };
    let original_data_dir = config.data_dir.clone();
    if let Some(data_dir) = data_dir {
        config.data_dir = data_dir.to_path_buf();
    }
    let data_dir_changed = existed && !reset && config.data_dir != original_data_dir;
    let workspace_root = canonical_directory(None)?;
    let instruction_plan = plan_agent_instructions(&config, &workspace_root)?;
    let planned_workspace = Workspace::discover_managed(
        &workspace_root,
        Path::new("git"),
        &config.data_dir,
        Deadlines::default().version,
    )
    .await?;
    let registration_path = config
        .data_dir
        .join("workspaces")
        .join(&planned_workspace.identity.repository_id)
        .join(&planned_workspace.identity.worktree_id)
        .join("workspace.json");
    let project_mcp_binary = project_mcp_binary()?;
    let project_mcp_plan =
        client_config::install_project_codex(&project_mcp_binary, &workspace_root, true, true)?;
    if dry_run {
        let mut mutations = Vec::new();
        if !existed {
            mutations.push(init_mutation("create_config", path, None));
        } else if reset {
            mutations.push(init_mutation("backup_config", path, Some("before reset")));
            mutations.push(init_mutation("reset_config", path, None));
        } else if data_dir_changed {
            mutations.push(init_mutation(
                "backup_config",
                path,
                Some("before data_dir update"),
            ));
            mutations.push(init_mutation("update_data_dir_preserving_toml", path, None));
        }
        for directory in init_layout_directories(&config) {
            if !directory.exists() {
                mutations.push(init_mutation("create_directory", &directory, None));
            }
        }
        for report in instruction_plan.iter().filter(|report| report.changed) {
            mutations.push(serde_json::json!({
                "action": "reconcile_instruction",
                "path": report.path,
                "surface": report.surface,
                "before_sha256": report.before_sha256,
                "after_sha256": report.after_sha256,
                "backup": report.backup_path,
            }));
        }
        if project_mcp_plan.changed {
            mutations.push(serde_json::json!({
                "action": "reconcile_project_codex_mcp",
                "path": project_mcp_plan.path,
                "before_sha256": project_mcp_plan.before_sha256,
                "after_sha256": project_mcp_plan.after_sha256,
                "backup": project_mcp_plan.backup_path,
            }));
        }
        if matches!(
            planned_workspace.placement()?,
            IndexPlacement::Missing { .. }
        ) {
            mutations.push(init_mutation(
                "create_managed_index_placement",
                &planned_workspace.index.project_entry,
                Some("canonical workspace index owner"),
            ));
        }
        mutations.push(init_mutation(
            if registration_path.exists() {
                "update_workspace_registration"
            } else {
                "create_workspace_registration"
            },
            &registration_path,
            None,
        ));
        if !skip_service {
            mutations.push(init_mutation(
                "ensure_daemon_service",
                &config.data_dir.join("runtime"),
                Some("only when an installed service definition exists"),
            ));
        }
        // Whether anything on disk would actually change. Registry and service mutations are
        // always listed, so their presence alone cannot answer "is this workspace stale?".
        let changes_required = !existed
            || reset
            || data_dir_changed
            || instruction_plan.iter().any(|report| report.changed)
            || project_mcp_plan.changed
            || init_layout_directories(&config)
                .iter()
                .any(|directory| !directory.exists());
        let payload = serde_json::json!({
            "dry_run": true,
            "changes_required": changes_required,
            "config": path,
            "config_exists": existed,
            "config_action": if reset { "reset_with_backup" } else if existed { "preserve" } else { "create" },
            "data_dir": config.data_dir,
            "workspace": workspace_root,
            "skip_service": skip_service,
            "mutations": mutations,
            "instructions": instruction_plan,
            "project_codex_mcp": project_mcp_plan,
            "index_placement": planned_workspace.placement()?,
        });
        if json {
            print_json(&payload)?;
        } else {
            println!("dry-run: {}", serde_json::to_string(&payload)?);
        }
        return Ok(ExitCode::SUCCESS);
    }

    let mut transaction = InitTransaction::acquire(path, &workspace_root, &config.data_dir)?;
    transaction.capture(path)?;
    for directory in init_transaction_directories(&config, &planned_workspace) {
        transaction.capture(&directory)?;
    }
    for report in &instruction_plan {
        if let Some(parent) = report.path.parent() {
            transaction.capture(parent)?;
        }
        transaction.capture(&report.path)?;
        if let Some(backup) = &report.backup_path {
            transaction.capture(backup)?;
        }
    }
    if let Some(parent) = project_mcp_plan.path.parent() {
        transaction.capture(parent)?;
    }
    transaction.capture(&project_mcp_plan.path)?;
    if let Some(backup) = &project_mcp_plan.backup_path {
        transaction.capture(backup)?;
    }
    transaction.capture(&planned_workspace.index.project_entry)?;
    transaction.capture(&planned_workspace.index.directory)?;
    transaction.capture(&registration_path)?;

    let applied = async {
        let backup = if existed && (reset || data_dir_changed) {
            let backup = backup_config(path)?;
            transaction.remove_on_rollback(&backup.path);
            Some(backup)
        } else {
            None
        };
        config.ensure_layout()?;
        for directory in init_layout_directories(&config) {
            transaction.mark_written(&directory)?;
        }
        transaction.mark_written(&config.data_dir.join("memory"))?;
        if !existed || reset {
            config.write(path)?;
            transaction.mark_written(path)?;
        } else if data_dir_changed {
            write_data_dir_preserving_toml(path, &config.data_dir)?;
            transaction.mark_written(path)?;
        }
        let instruction_reports = reconcile_agent_instructions(&config, &workspace_root)?;
        for report in instruction_reports.iter().filter(|report| report.changed) {
            transaction.mark_written(&report.path)?;
            if let Some(backup) = &report.backup_path {
                transaction.mark_written(backup)?;
            }
        }
        let project_mcp = client_config::install_project_codex(
            &project_mcp_binary,
            &workspace_root,
            false,
            true,
        )?;
        if project_mcp.changed {
            transaction.mark_written(&project_mcp.path)?;
            if let Some(backup) = &project_mcp.backup_path {
                transaction.mark_written(backup)?;
            }
        }
        inject_init_failure("after_instructions")?;
        let initialized = initialize_workspace_at_inner(&config, &workspace_root, false).await?;
        for directory in init_transaction_directories(&config, &planned_workspace) {
            transaction.mark_written(&directory)?;
        }
        transaction.mark_written(&planned_workspace.index.project_entry)?;
        transaction.mark_written(&registration_path)?;
        inject_init_failure("after_workspace")?;
        Ok::<_, anyhow::Error>((backup, instruction_reports, project_mcp, initialized))
    }
    .await;
    let (backup, instruction_reports, project_mcp, initialized) = match applied {
        Ok(applied) => {
            transaction.commit();
            applied
        }
        Err(error) => {
            transaction
                .rollback()
                .context("init failed and rollback did not fully restore filesystem state")?;
            return Err(error);
        }
    };
    let (workspace, mut outcome, mut changed, git_backed, registration) = initialized;
    if let Some(warm_outcome) = warm_workspace_index(&config, &workspace)
        .await
        .context("core init committed, but grepai index warm-up failed")?
    {
        outcome = warm_outcome;
        changed = true;
    }
    let service_report = if skip_service {
        None
    } else {
        service::ensure_running_if_installed()
            .context("core init committed, but daemon service reconciliation failed")?
    };
    let dashboard = format!("http://{}", config.daemon.bind);
    if json {
        print_json(&serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "config": path,
            "config_backup": backup,
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
            "project_codex_mcp": project_mcp,
            "dashboard": dashboard,
            "daemon_service": service_report,
            "mcp": mcp::lifecycle_metadata(),
        }))?;
    } else {
        let stdout = io::stdout();
        let mut output = stdout.lock();
        writeln!(output, "initialized {}", path.display())?;
        if let Some(backup) = &backup {
            writeln!(output, "config backup {}", backup.path.display())?;
        }
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
        if project_mcp.changed {
            writeln!(
                output,
                "updated project Codex MCP {}",
                project_mcp.path.display()
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

#[derive(Clone, Debug, Serialize)]
struct ConfigBackup {
    path: PathBuf,
    created_at_ms: u128,
    sha256: String,
}

fn backup_config(path: &Path) -> Result<ConfigBackup> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("configuration path has no UTF-8 file name")?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let content = std::fs::read(path)
        .with_context(|| format!("read config backup source {}", path.display()))?;
    let created_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_millis();
    let sha256 = hex::encode(Sha256::digest(&content));
    for suffix in 0_u32..10_000 {
        let candidate = parent.join(if suffix == 0 {
            format!("{file_name}.hzr-backup-{created_at_ms}-{}", &sha256[..12])
        } else {
            format!(
                "{file_name}.hzr-backup-{created_at_ms}-{}-{suffix}",
                &sha256[..12]
            )
        });
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(mut output) => {
                let result = output
                    .write_all(&content)
                    .and_then(|()| output.sync_all())
                    .with_context(|| format!("write config backup {}", candidate.display()));
                if let Err(error) = result {
                    let _ = std::fs::remove_file(&candidate);
                    return Err(error);
                }
                return Ok(ConfigBackup {
                    path: candidate,
                    created_at_ms,
                    sha256,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("create config backup {}", candidate.display()));
            }
        }
    }
    bail!(
        "could not allocate a unique backup path for {}",
        path.display()
    )
}

fn init_layout_directories(config: &Config) -> [PathBuf; 6] {
    [
        config.data_dir.clone(),
        config.data_dir.join("runtime"),
        config.data_dir.join("workspaces"),
        config.data_dir.join("memory/icm"),
        config.data_dir.join("ledger"),
        config.data_dir.join("engines"),
    ]
}

fn init_transaction_directories(config: &Config, workspace: &Workspace) -> Vec<PathBuf> {
    let repository = config
        .data_dir
        .join("workspaces")
        .join(&workspace.identity.repository_id);
    let worktree = repository.join(&workspace.identity.worktree_id);
    let mut directories = vec![
        config.data_dir.clone(),
        config.data_dir.join("runtime"),
        config.data_dir.join("workspaces"),
        repository,
        worktree.clone(),
        worktree.join("index"),
        workspace.index.directory.clone(),
        config.data_dir.join("memory"),
        config.data_dir.join("memory/icm"),
        config.data_dir.join("ledger"),
        config.data_dir.join("engines"),
    ];
    directories.sort_by_key(|path| path.components().count());
    directories.dedup();
    directories
}

fn init_mutation(action: &str, path: &Path, detail: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "action": action,
        "path": path,
        "detail": detail,
    })
}

fn project_mcp_binary() -> Result<PathBuf> {
    let durable = prefix::default_prefix()?.join("hzr");
    if durable.is_file() {
        Ok(durable)
    } else {
        std::env::current_exe().context("cannot resolve HZR binary for project Codex MCP")
    }
}

fn write_data_dir_preserving_toml(path: &Path, data_dir: &Path) -> Result<()> {
    let existing = std::fs::read_to_string(path)
        .with_context(|| format!("read configuration {}", path.display()))?;
    let mut document = existing
        .parse::<DocumentMut>()
        .with_context(|| format!("parse configuration {}", path.display()))?;
    document["data_dir"] = value(data_dir.to_string_lossy().into_owned());
    let rendered = document.to_string();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("stage configuration in {}", parent.display()))?;
    staged
        .write_all(rendered.as_bytes())
        .with_context(|| format!("stage configuration {}", path.display()))?;
    staged.as_file().sync_all()?;
    Config::load(staged.path()).context("validate staged configuration")?;
    staged
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("replace configuration {}", path.display()))?;
    Ok(())
}

#[derive(Debug)]
enum InitPathState {
    Missing,
    File(Vec<u8>),
    Directory,
    Symlink(PathBuf),
}

#[derive(Debug)]
struct InitPathSnapshot {
    path: PathBuf,
    state: InitPathState,
    written_fingerprint: Option<InitPathFingerprint>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum InitPathFingerprint {
    Missing,
    File(String),
    Directory,
    Symlink(PathBuf),
}

#[derive(Debug)]
struct InitTransaction {
    snapshots: Vec<InitPathSnapshot>,
    finished: bool,
    _locks: Vec<File>,
}

impl InitTransaction {
    fn acquire(config_path: &Path, workspace_root: &Path, data_dir: &Path) -> Result<Self> {
        let absolute_config = if config_path.is_absolute() {
            config_path.to_path_buf()
        } else {
            std::env::current_dir()?.join(config_path)
        };
        let identities = [
            absolute_config.as_os_str().as_encoded_bytes().to_vec(),
            [
                workspace_root.as_os_str().as_encoded_bytes(),
                b"\0",
                data_dir.as_os_str().as_encoded_bytes(),
            ]
            .concat(),
        ];
        let mut lock_paths = identities
            .into_iter()
            .map(|identity| {
                let key = hex::encode(Sha256::digest(identity));
                std::env::temp_dir().join(format!("hzr-init-{}.lock", &key[..24]))
            })
            .collect::<Vec<_>>();
        lock_paths.sort();
        lock_paths.dedup();
        let mut locks = Vec::with_capacity(lock_paths.len());
        for lock_path in lock_paths {
            let lock = OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(false)
                .open(&lock_path)
                .with_context(|| format!("open init transaction lock {}", lock_path.display()))?;
            lock.lock_exclusive()
                .with_context(|| format!("lock init transaction {}", lock_path.display()))?;
            locks.push(lock);
        }
        Ok(Self {
            snapshots: Vec::new(),
            finished: false,
            _locks: locks,
        })
    }

    fn capture(&mut self, path: &Path) -> Result<()> {
        if self.snapshots.iter().any(|snapshot| snapshot.path == path) {
            return Ok(());
        }
        let state = match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => InitPathState::Symlink(
                std::fs::read_link(path)
                    .with_context(|| format!("read init symlink {}", path.display()))?,
            ),
            Ok(metadata) if metadata.is_file() => InitPathState::File(
                std::fs::read(path)
                    .with_context(|| format!("snapshot init file {}", path.display()))?,
            ),
            Ok(metadata) if metadata.is_dir() => InitPathState::Directory,
            Ok(_) => bail!(
                "init target is not a regular file, directory, or symlink: {}",
                path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => InitPathState::Missing,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect init target {}", path.display()));
            }
        };
        self.snapshots.push(InitPathSnapshot {
            path: path.to_path_buf(),
            state,
            written_fingerprint: None,
        });
        Ok(())
    }

    fn remove_on_rollback(&mut self, path: &Path) {
        self.snapshots.push(InitPathSnapshot {
            path: path.to_path_buf(),
            state: InitPathState::Missing,
            written_fingerprint: fingerprint_init_path(path).ok(),
        });
    }

    fn mark_written(&mut self, path: &Path) -> Result<()> {
        let fingerprint = fingerprint_init_path(path)?;
        let snapshot = self
            .snapshots
            .iter_mut()
            .rev()
            .find(|snapshot| snapshot.path == path)
            .with_context(|| format!("init transaction did not snapshot {}", path.display()))?;
        snapshot.written_fingerprint = Some(fingerprint);
        Ok(())
    }

    fn commit(&mut self) {
        self.finished = true;
    }

    fn rollback(&mut self) -> Result<()> {
        let result = self.rollback_inner();
        self.finished = true;
        result
    }

    fn rollback_inner(&mut self) -> Result<()> {
        let mut first_error = None;
        for snapshot in self.snapshots.iter().rev() {
            if let Err(error) = restore_init_path(snapshot) {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }
}

impl Drop for InitTransaction {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.rollback_inner();
        }
    }
}

fn restore_init_path(snapshot: &InitPathSnapshot) -> Result<()> {
    if let Some(expected) = &snapshot.written_fingerprint {
        let current = fingerprint_init_path(&snapshot.path)?;
        if &current != expected {
            bail!(
                "refusing to roll back concurrently modified init target {}",
                snapshot.path.display()
            );
        }
    }
    match &snapshot.state {
        InitPathState::Missing => remove_init_path(&snapshot.path),
        InitPathState::Directory => {
            if !snapshot.path.is_dir() {
                bail!(
                    "refusing to recreate concurrently replaced init directory {}",
                    snapshot.path.display()
                );
            }
            Ok(())
        }
        InitPathState::File(content) => {
            remove_init_path(&snapshot.path)?;
            if let Some(parent) = snapshot.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&snapshot.path, content)
                .with_context(|| format!("restore init file {}", snapshot.path.display()))
        }
        InitPathState::Symlink(target) => {
            remove_init_path(&snapshot.path)?;
            if let Some(parent) = snapshot.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            create_init_symlink(target, &snapshot.path)
        }
    }
}

fn remove_init_path(path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspect rollback target {}", path.display()));
        }
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir(path).with_context(|| {
            format!(
                "remove empty rollback directory {}; refusing recursive deletion",
                path.display()
            )
        })
    } else {
        std::fs::remove_file(path)
            .with_context(|| format!("remove rollback file {}", path.display()))
    }
}

fn fingerprint_init_path(path: &Path) -> Result<InitPathFingerprint> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InitPathFingerprint::Missing);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("fingerprint init target {}", path.display()));
        }
    };
    if metadata.file_type().is_symlink() {
        return Ok(InitPathFingerprint::Symlink(std::fs::read_link(path)?));
    }
    if metadata.is_file() {
        return Ok(InitPathFingerprint::File(hex::encode(Sha256::digest(
            std::fs::read(path)?,
        ))));
    }
    if metadata.is_dir() {
        return Ok(InitPathFingerprint::Directory);
    }
    bail!("unsupported init target type: {}", path.display())
}

#[cfg(unix)]
fn create_init_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("restore init symlink {}", link.display()))
}

#[cfg(windows)]
fn create_init_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
        .with_context(|| format!("restore init symlink {}", link.display()))
}

fn inject_init_failure(_point: &str) -> Result<()> {
    #[cfg(debug_assertions)]
    if std::env::var("HZR_TEST_INIT_FAIL_AFTER").as_deref() == Ok(_point) {
        if let Some(path) = std::env::var_os("HZR_TEST_INIT_CONCURRENT_FILE") {
            let path = PathBuf::from(path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, b"concurrent-user-file\n")?;
        }
        if let Some(path) = std::env::var_os("HZR_TEST_INIT_CONCURRENT_EDIT") {
            let path = PathBuf::from(path);
            let mut file = OpenOptions::new().append(true).open(&path)?;
            file.write_all(b"\n# concurrent-user-edit\n")?;
            file.sync_all()?;
        }
        bail!("injected init failure after {_point}");
    }
    Ok(())
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

    // SessionStart owns both repository instruction surfaces and the project-scoped Codex MCP
    // pin. Apply them as one exact-file transaction so a stale contract cannot be refreshed
    // while Codex remains globally or cross-workspace bound.
    let (instruction_reports, project_mcp) =
        reconcile_session_surfaces(config_path, &config, &workspace_root)?;
    let instruction_alert = session_instruction_drift_alert(
        config.activation.mode,
        &workspace_root,
        &instruction_reports,
    )?;
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
            "project_codex_mcp": project_mcp,
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
        if project_mcp.changed {
            writeln!(
                output,
                "updated project Codex MCP {}",
                project_mcp.path.display()
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
        let update_notice = update::startup_notice(&config.data_dir).await;
        if session_start_hook {
            if let Some(payload) = session_start_payload(
                instruction_alert.as_deref(),
                update_notice.as_deref(),
                Some(response_codec_session_notice()),
            ) {
                print_json(&payload)?;
            }
        } else {
            if let Some(alert) = instruction_alert {
                println!("{alert}");
            }
            if let Some(notice) = update_notice {
                println!("{notice}");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn session_instruction_drift_alert(
    activation_mode: hzr_core::ActivationMode,
    workspace_root: &Path,
    reports: &[instructions::InstructionReport],
) -> Result<Option<String>> {
    let targets = scoped_instruction_targets(activation_mode, workspace_root)?;
    Ok(instruction_drift_alert_for_targets(reports, targets))
}

fn instruction_drift_alert_for_targets(
    reports: &[instructions::InstructionReport],
    targets: Vec<(instructions::Surface, PathBuf)>,
) -> Option<String> {
    let reconciled = reports
        .iter()
        .filter(|report| report.changed)
        .map(|report| report.path.clone())
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut unhealthy = BTreeSet::new();
    for (surface, path) in targets {
        if !seen.insert(path.clone()) {
            continue;
        }
        match instructions::audit(surface, &path) {
            Ok(audit) if audit.healthy() => {}
            Ok(_) | Err(_) => {
                unhealthy.insert(path);
            }
        }
    }
    if reconciled.is_empty() && unhealthy.is_empty() {
        return None;
    }
    let affected = reconciled
        .union(&unhealthy)
        .take(4)
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "HZR ALERT: instruction drift detected ({} reconciled now, {} still unhealthy; affected: {}). Run `hzr doctor` before continuing and report every remaining instruction or fleet finding. Use `hzr doctor --reconcile-fleet --dry-run` before applying fleet repairs.",
        reconciled.len(),
        unhealthy.len(),
        if affected.is_empty() {
            "none"
        } else {
            &affected
        },
    ))
}

fn session_start_payload(
    instruction_alert: Option<&str>,
    update_notice: Option<&str>,
    codec_notice: Option<&str>,
) -> Option<serde_json::Value> {
    if instruction_alert.is_none() && update_notice.is_none() && codec_notice.is_none() {
        return None;
    }
    let system_message = [instruction_alert, update_notice, codec_notice]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
    let additional_context = [
        instruction_alert.map(str::to_owned),
        update_notice.map(update::agent_notice),
        codec_notice.map(str::to_owned),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n");
    Some(serde_json::json!({
        "systemMessage": system_message,
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": additional_context,
        },
    }))
}

fn response_codec_session_notice() -> &'static str {
    "HZR CODEC: Claude Code cannot expose a global final-response replacement hook. For long low- or medium-risk prose where compression is useful, call `hzr_codec` once and use its returned `content`. Otherwise coverage is instructed-only and receives zero economic credit; `shadow` is measurement only."
}

fn reconcile_session_surfaces(
    config_path: &Path,
    config: &Config,
    workspace_root: &Path,
) -> Result<(
    Vec<instructions::InstructionReport>,
    client_config::ClientConfigReport,
)> {
    let instruction_plan = plan_agent_instructions(config, workspace_root)?;
    let binary = project_mcp_binary()?;
    let mcp_plan = client_config::install_project_codex(&binary, workspace_root, true, true)?;
    let mut transaction = InitTransaction::acquire(config_path, workspace_root, &config.data_dir)?;
    for report in &instruction_plan {
        if let Some(parent) = report.path.parent() {
            transaction.capture(parent)?;
        }
        transaction.capture(&report.path)?;
        if let Some(backup) = &report.backup_path {
            transaction.capture(backup)?;
        }
    }
    if let Some(parent) = mcp_plan.path.parent() {
        transaction.capture(parent)?;
    }
    transaction.capture(&mcp_plan.path)?;
    if let Some(backup) = &mcp_plan.backup_path {
        transaction.capture(backup)?;
    }

    let applied = (|| {
        let instructions = reconcile_agent_instructions(config, workspace_root)?;
        for report in instructions.iter().filter(|report| report.changed) {
            transaction.mark_written(&report.path)?;
            if let Some(backup) = &report.backup_path {
                transaction.mark_written(backup)?;
            }
        }
        inject_init_failure("after_session_instructions")?;
        let mcp = client_config::install_project_codex(&binary, workspace_root, false, true)?;
        if mcp.changed {
            transaction.mark_written(&mcp.path)?;
            if let Some(backup) = &mcp.backup_path {
                transaction.mark_written(backup)?;
            }
        }
        inject_init_failure("after_session_mcp")?;
        Ok::<_, anyhow::Error>((instructions, mcp))
    })();
    match applied {
        Ok(applied) => {
            transaction.commit();
            Ok(applied)
        }
        Err(error) => {
            transaction.rollback().context(
                "SessionStart surface reconciliation failed and rollback was incomplete",
            )?;
            Err(error)
        }
    }
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
            adoption::HookInstallPolicy {
                native_tool_mode: hook_status.native_tool_mode,
                dry_run: false,
                confirmed: true,
            },
        )?;
    }
    for (surface, target) in activation::local_instruction_paths(&workspace.identity.root) {
        if enabled {
            instructions::install(surface, &target, &contract, false, true)?;
        } else {
            instructions::uninstall(surface, &target, false, true)?;
        }
    }
    let project_mcp = if enabled {
        client_config::install_project_codex(
            &project_mcp_binary()?,
            &workspace.identity.root,
            false,
            true,
        )?
    } else {
        client_config::uninstall_project_codex(&workspace.identity.root, false, true)?
    };

    if json {
        print_json(&serde_json::json!({
            "enabled": enabled,
            "changed": changed,
            "activation_mode": "selected",
            "workspace": workspace.identity.root,
            "repository_id": workspace.identity.repository_id,
            "worktree_id": workspace.identity.worktree_id,
            "project_codex_mcp": project_mcp,
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
) -> Result<(
    Workspace,
    &'static str,
    bool,
    bool,
    Option<WorkspaceRegistration>,
)> {
    initialize_workspace_at_inner(config, workspace_path, true).await
}

async fn initialize_workspace_at_inner(
    config: &Config,
    workspace_path: &Path,
    warm_index: bool,
) -> Result<(
    Workspace,
    &'static str,
    bool,
    bool,
    Option<WorkspaceRegistration>,
)> {
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
    let placement = workspace.placement()?;
    let legacy = matches!(&placement, IndexPlacement::LegacyProject { .. });
    let (mut outcome, mut changed) = match placement {
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
    if !legacy && warm_index {
        // The workspace registration above is the part `init` owns; warming the index needs the
        // pinned engine. A bundle always ships one, but a source checkout or a partially
        // installed host may not have it yet, and `init --if-needed` also runs from the
        // SessionStart hook — failing the whole command there would break the session over a
        // missing optional warm-up. A genuinely broken engine still propagates; only an absent
        // binary degrades, and `hzr doctor` is what reports it.
        match GrepAi::connect(
            config.engines.binary("grepai"),
            workspace.clone(),
            Deadlines::default(),
        )
        .await
        {
            Ok(grepai) => match grepai.initialize(&InitOptions::default()).await? {
                InitOutcome::Initialized => {
                    outcome = "index_initialized";
                    changed = true;
                }
                InitOutcome::RepositoryGraphEnabled => {
                    outcome = "repository_graph_enabled";
                    changed = true;
                }
                InitOutcome::AlreadyInitialized => {}
            },
            // Leave the placement outcome alone. It describes what `init` did own — registering
            // the workspace — and overwriting it would make "already initialized" indistinguish-
            // able from a first run purely because the engine is missing. A missing engine is a
            // host condition, and `hzr doctor` is the single place that reports it.
            Err(error) if index_engine_is_absent(&error) => {}
            Err(error) => return Err(error.into()),
        }
    }
    let registration = if legacy {
        None
    } else {
        Some(workspace.register()?)
    };
    Ok((workspace, outcome, changed, git_backed, registration))
}

async fn warm_workspace_index(
    config: &Config,
    workspace: &Workspace,
) -> Result<Option<&'static str>> {
    if matches!(workspace.placement()?, IndexPlacement::LegacyProject { .. }) {
        return Ok(None);
    }
    match GrepAi::connect(
        config.engines.binary("grepai"),
        workspace.clone(),
        Deadlines::default(),
    )
    .await
    {
        Ok(grepai) => match grepai.initialize(&InitOptions::default()).await? {
            InitOutcome::Initialized => Ok(Some("index_initialized")),
            InitOutcome::RepositoryGraphEnabled => Ok(Some("repository_graph_enabled")),
            InitOutcome::AlreadyInitialized => Ok(None),
        },
        Err(error) if index_engine_is_absent(&error) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Whether an index error means the pinned engine binary is simply not installed.
///
/// Distinguishing absence from failure matters: a missing binary is a host state `hzr doctor`
/// already reports, while a spawn error of any other kind is a real fault that must surface.
fn index_engine_is_absent(error: &hzr_index::IndexError) -> bool {
    matches!(
        error,
        hzr_index::IndexError::CommandUnavailable { source, .. }
            if source.kind() == std::io::ErrorKind::NotFound
    )
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
            evasion: None,
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
    let workspace_text = path_text(&workspace, "workspace")?;
    let request = ContextPlanApiRequest {
        workspace: workspace_text.clone(),
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
    let client = DaemonClient::from_config(config)?;
    let response = client.context_plan(&request).await?;
    if json {
        print_json(&response)?;
    } else {
        print_context(&response)?;
    }
    record_cli_standalone_delivery(
        config,
        &client,
        &workspace_text,
        "hzr context plan",
        AccountingOperationKind::Context,
        AccountingOperationMode::ContextPlan,
        &response,
    )
    .await;
    Ok(ExitCode::SUCCESS)
}

async fn record_cli_standalone_delivery(
    config: &Config,
    client: &DaemonClient,
    workspace: &str,
    command: &str,
    operation: AccountingOperationKind,
    mode: AccountingOperationMode,
    response: &impl Serialize,
) {
    let delivered = serde_json::to_vec(response)
        .ok()
        .and_then(|bytes| u64::try_from(bytes.len() / 4).ok())
        .unwrap_or(1)
        .max(1);
    let request = OperationApiRequest {
        original_command: command.to_owned(),
        recorded_command: command.to_owned(),
        baseline_tokens_estimated: delivered,
        delivered_tokens_estimated: delivered,
        execution_ms: 0,
        project_path: workspace.to_owned(),
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
            operation,
            mode,
            stage: AccountingStage::StandaloneDelivery,
            requested_mode: None,
            effective_mode: Some(mode),
            search_strategy: None,
            search_fallback_code: None,
            include_content: None,
            limit: None,
            path_scope_count: None,
            filter_level: None,
            from_line: None,
            to_line: None,
            source_bytes: None,
            evasion: None,
        }),
    };
    if client.record_operation(&request).await.is_err() {
        let _ = hook_runner::record_daemon_unavailable_operation(config);
    }
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
            let workspace_text = path_text(&workspace, "memory workspace")?;
            let response = client
                .memory_recall(&MemoryRecallApiRequest {
                    workspace: workspace_text.clone(),
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
            record_cli_standalone_delivery(
                config,
                &client,
                &workspace_text,
                "hzr memory recall",
                AccountingOperationKind::Memory,
                AccountingOperationMode::MemoryRecall,
                &response,
            )
            .await;
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
            let workspace_text = path_text(&workspace, "memory workspace")?;
            let content = read_text(
                content,
                file.as_deref(),
                payload_limit(config.daemon.request_limit_bytes),
            )?;
            let response = client
                .memory_store(&MemoryStoreApiRequest {
                    workspace: workspace_text.clone(),
                    topic,
                    content,
                    importance: importance.into(),
                    keywords,
                    raw,
                    scope: scope.into(),
                })
                .await?;
            print_json(&response)?;
            record_cli_standalone_delivery(
                config,
                &client,
                &workspace_text,
                "hzr memory store",
                AccountingOperationKind::Memory,
                AccountingOperationMode::MemoryStore,
                &response,
            )
            .await;
        }
        MemoryCommand::Forget {
            id,
            workspace,
            scope,
        } => {
            let workspace = canonical_directory(workspace.as_deref())?;
            let workspace_text = path_text(&workspace, "memory workspace")?;
            let response = client
                .memory_forget(&MemoryForgetApiRequest {
                    workspace: workspace_text.clone(),
                    id,
                    scope: scope.into(),
                })
                .await?;
            print_json(&response)?;
            record_cli_standalone_delivery(
                config,
                &client,
                &workspace_text,
                "hzr memory forget",
                AccountingOperationKind::Memory,
                AccountingOperationMode::MemoryForget,
                &response,
            )
            .await;
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
            let workspace_text = path_text(&workspace, "memory workspace")?;
            let content = read_text(
                content,
                file.as_deref(),
                payload_limit(config.daemon.request_limit_bytes),
            )?;
            let response = client
                .memory_update(&MemoryUpdateApiRequest {
                    workspace: workspace_text.clone(),
                    id,
                    content,
                    scope: scope.into(),
                    importance: importance.map(Into::into),
                    keywords,
                })
                .await?;
            print_json(&response)?;
            record_cli_standalone_delivery(
                config,
                &client,
                &workspace_text,
                "hzr memory update",
                AccountingOperationKind::Memory,
                AccountingOperationMode::MemoryUpdate,
                &response,
            )
            .await;
        }
        MemoryCommand::Prune {
            workspace,
            threshold,
            apply,
            scope,
        } => {
            let workspace = canonical_directory(workspace.as_deref())?;
            let workspace_text = path_text(&workspace, "memory workspace")?;
            let response = client
                .memory_prune(&MemoryPruneApiRequest {
                    workspace: workspace_text.clone(),
                    threshold,
                    dry_run: !apply,
                    scope: scope.into(),
                })
                .await?;
            print_json(&response)?;
            record_cli_standalone_delivery(
                config,
                &client,
                &workspace_text,
                "hzr memory prune",
                AccountingOperationKind::Memory,
                AccountingOperationMode::MemoryPrune,
                &response,
            )
            .await;
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
            let outcome = client.exec_rewrite(&request).await?;
            if json {
                // JSON keeps the attribution: it is what makes a decision auditable.
                print_json(&outcome)?;
            } else {
                print_rewrite(&outcome.decision)?;
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
    let fidelity_requested =
        std::env::var_os("HZR_RAW_FIDELITY").as_deref() == Some(std::ffi::OsStr::new("1"));
    Ok(ExecApiRequest {
        cwd: cwd.to_string_lossy().into_owned(),
        command: arguments.command,
        fidelity_requested,
        fidelity_reason: fidelity_requested
            .then(|| std::env::var("HZR_RAW_FIDELITY_REASON").ok())
            .flatten(),
        timeout_ms: arguments.timeout_ms,
        caller_path: std::env::var("PATH").ok(),
        agent: Some("cli".into()),
        session_id: ["CODEX_THREAD_ID", "CLAUDE_SESSION_ID"]
            .into_iter()
            .find_map(|name| {
                std::env::var(name)
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            }),
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
            project_path: std::env::current_dir()?.to_string_lossy().into_owned(),
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

async fn execute_billing(config: &Config, command: BillingCommand, json: bool) -> Result<ExitCode> {
    match command {
        BillingCommand::Catalog => {
            let catalog = load_pricing_catalog(config.billing.pricing_file.as_deref())?;
            if json {
                print_json(&catalog)?;
            } else {
                println!(
                    "pricing-catalog identity={} retrieved={} entries={} runtime-network=false override={}",
                    catalog.identity,
                    catalog.retrieved_at,
                    catalog.entries.len(),
                    config
                        .billing
                        .pricing_file
                        .as_ref()
                        .map_or_else(|| "none".into(), |path| path.display().to_string()),
                );
                for entry in catalog.entries {
                    println!(
                        "{} {} {} {} {} source={}",
                        entry.harness,
                        entry.provider,
                        entry.model,
                        entry.method,
                        entry.currency,
                        entry.source_url,
                    );
                }
            }
        }
        BillingCommand::Receipt { file } => {
            let metadata = std::fs::symlink_metadata(&file)
                .with_context(|| format!("failed to inspect {}", file.display()))?;
            if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 262_144
            {
                bail!(
                    "provider receipt must be a regular non-symlink file of at most 262144 bytes"
                );
            }
            let bytes = std::fs::read(&file)
                .with_context(|| format!("failed to read {}", file.display()))?;
            let receipt: ProviderEconomicReceipt = serde_json::from_slice(&bytes)
                .with_context(|| format!("invalid provider receipt JSON in {}", file.display()))?;
            let result = DaemonClient::from_config(config)?
                .record_provider_receipt(&receipt)
                .await?;
            if json {
                print_json(&result)?;
            } else {
                println!(
                    "provider-receipt recorded={} idempotent-replay={} receipt={} invoice-actual={} public-estimate={} reason={}",
                    result.recorded,
                    result.idempotent_replay,
                    result.receipt_hash,
                    result.invoice_actual.is_some(),
                    result.public_estimate.is_some(),
                    result.unavailable_reason.as_deref().unwrap_or("none"),
                );
            }
        }
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
    show_evasion: bool,
    since: Option<&crate::cli::StatsDuration>,
    accounting_version: crate::cli::AccountingVersion,
) -> Result<ExitCode> {
    stats::validate_request_bounds(
        json,
        include_all_commands,
        workspace.is_some(),
        since.is_some(),
    )?;
    let workspace = workspace
        .map(|path| canonical_directory(Some(path)))
        .transpose()?;
    let report = stats::collect(
        config,
        workspace.as_deref(),
        include_all_commands,
        show_evasion,
        since,
        accounting_version,
    )
    .await?;
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

fn forwarded_fork_args(
    subcommand: &str,
    arguments: &[std::ffi::OsString],
) -> Vec<std::ffi::OsString> {
    std::iter::once(std::ffi::OsString::from(subcommand))
        .chain(arguments.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use hzr_core::Config;
    use tempfile::tempdir;

    use super::{
        bounded_read_arguments, canonical_directory, contract_asset_path,
        executable_source_directory, forwarded_fork_args, instruction_drift_alert_for_targets,
        payload_limit, reject_direct_fork_bypass, response_codec_session_notice,
        scoped_instruction_targets, session_instruction_drift_alert, session_start_payload,
    };

    #[test]
    fn acceptance_gate_selected_activation_does_not_require_global_instructions() {
        let directory = tempdir().expect("temporary directory");
        let contract = directory.path().join("HZR.md");
        fs::write(&contract, "canonical contract\n").expect("contract");
        for (surface, path) in
            scoped_instruction_targets(hzr_core::ActivationMode::Selected, directory.path())
                .expect("selected targets")
        {
            crate::instructions::install(surface, &path, &contract, false, true)
                .expect("managed local instructions");
        }

        let alert = session_instruction_drift_alert(
            hzr_core::ActivationMode::Selected,
            directory.path(),
            &[],
        )
        .expect("drift audit");
        assert!(alert.is_none());
    }

    #[test]
    fn acceptance_gate_session_start_alerts_when_user_instructions_drift() {
        let directory = tempdir().expect("temporary directory");
        let contract = directory.path().join("HZR.md");
        let instructions_path = directory.path().join("AGENTS.md");
        fs::write(&contract, "canonical contract\n").expect("contract");
        crate::instructions::install(
            crate::instructions::Surface::Codex,
            &instructions_path,
            &contract,
            false,
            true,
        )
        .expect("managed instructions");
        let targets = vec![(
            crate::instructions::Surface::Codex,
            instructions_path.clone(),
        )];
        assert!(instruction_drift_alert_for_targets(&[], targets.clone()).is_none());

        let mut drifted = fs::read_to_string(&instructions_path).expect("instructions");
        drifted.push_str("\nAlways use rtk cargo test directly.\n");
        fs::write(&instructions_path, drifted).expect("drifted instructions");

        let alert = instruction_drift_alert_for_targets(&[], targets).expect("drift alert");
        assert!(alert.contains("instruction drift detected"));
        assert!(alert.contains("Run `hzr doctor` before continuing"));
        assert!(alert.contains("hzr doctor --reconcile-fleet --dry-run"));
    }

    #[test]
    fn acceptance_gate_session_start_combines_drift_and_update_without_losing_actions() {
        let payload = session_start_payload(
            Some("Run `hzr doctor` before continuing."),
            Some("HZR 0.6.2 is available."),
            Some(response_codec_session_notice()),
        )
        .expect("session payload");
        let rendered = payload.to_string();

        assert!(rendered.contains("Run `hzr doctor` before continuing."));
        assert!(rendered.contains("HZR 0.6.2 is available."));
        assert!(rendered.contains("Inform the user once"));
        assert!(rendered.contains("Do not install it without explicit approval."));
        assert!(rendered.contains("instructed-only"));
        assert!(rendered.contains("zero economic credit"));
    }

    #[test]
    fn acceptance_gate_test_alias_forwards_argv_without_reconstruction() {
        let arguments = [
            std::ffi::OsString::from("npm"),
            std::ffi::OsString::from("quoted argument stays one argv"),
            std::ffi::OsString::from("--watch"),
        ];
        assert_eq!(
            forwarded_fork_args("test", &arguments),
            [std::ffi::OsString::from("test")]
                .into_iter()
                .chain(arguments)
                .collect::<Vec<_>>()
        );
    }

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
    fn acceptance_gate_direct_managed_raw_and_proxy_are_refused() {
        let directory = tempdir().expect("temporary directory");
        let config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        for bypass in ["raw", "proxy"] {
            let args = [bypass, "git", "status"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>();
            let error = reject_direct_fork_bypass(&config, &args)
                .expect_err("direct managed bypass must be refused")
                .to_string();
            assert!(error.contains("hzr exec run"));
            assert!(error.contains("session budget"));
        }
        assert!(reject_direct_fork_bypass(&config, &[std::ffi::OsString::from("git")]).is_ok());
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
        let release = directory.path().join("versions/v0.4.6-test");
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
        let release = directory.path().join("versions/v0.4.6-test");
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
        let release_bin = directory.path().join("versions/v0.4.6-test/bin");
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
