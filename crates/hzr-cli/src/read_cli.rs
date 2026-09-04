use crate::client::DaemonClient;
use anyhow::Result;
use clap::Parser;
use hzr_core::Config;
use hzr_protocol::{AccountingOperationKind, AccountingOperationMode, ReadApiRequest};
use std::{ffi::OsString, process::ExitCode};

#[derive(Parser)]
#[command(
    name = "hzr --json read",
    about = "Read exact sources with shared budget and source-hash expansion"
)]
struct ReadArgs {
    #[arg(required = true, num_args = 1..)]
    paths: Vec<String>,
    /// Accept several files under one response budget
    #[arg(long)]
    batch: bool,
    /// First source line (1 based)
    #[arg(long)]
    from: Option<u64>,
    /// Last source line, inclusive
    #[arg(long)]
    to: Option<u64>,
    /// Maximum lines per file
    #[arg(long)]
    max_lines: Option<u64>,
    /// Shared estimated UTF-8/4 response budget (1024..48000)
    #[arg(long)]
    max_tokens: Option<u64>,
    /// Reject expansion if the source changed
    #[arg(long)]
    expected_sha256: Option<String>,
    /// Advisory read episode; change after compaction, fork or resume
    #[arg(long)]
    context_epoch: Option<String>,
    /// Session identity required with --context-epoch (defaults to ambient session)
    #[arg(long)]
    session_id: Option<String>,
}

pub async fn run(config: &Config, args: &[OsString]) -> Result<ExitCode> {
    let args = ReadArgs::try_parse_from(
        std::iter::once(OsString::from("read")).chain(args.iter().cloned()),
    )?;
    if args.paths.len() > 1 && !args.batch {
        anyhow::bail!("multiple paths require --batch");
    }
    let workspace = super::canonical_directory(None)?;
    let cwd = super::path_text(&workspace, "read workspace")?;
    let client = DaemonClient::from_config(config)?;
    let response = client
        .read_files(&ReadApiRequest {
            context_epoch: args.context_epoch,
            cwd: cwd.clone(),
            paths: args.paths,
            from: args.from,
            to: args.to,
            max_lines: args.max_lines,
            max_tokens: args.max_tokens,
            expected_sha256: args.expected_sha256,
            agent: None,
            session_id: args.session_id.or_else(hzr_core::ambient_session_id),
        })
        .await?;
    super::print_json(&response)?;
    let mode = if !response.files.is_empty() && response.files.iter().all(|file| file.complete) {
        AccountingOperationMode::ReadFull
    } else {
        AccountingOperationMode::ReadRange
    };
    super::record_cli_standalone_delivery(
        config,
        &client,
        &cwd,
        "hzr read",
        AccountingOperationKind::Read,
        mode,
        &response,
    )
    .await;
    Ok(ExitCode::SUCCESS)
}
