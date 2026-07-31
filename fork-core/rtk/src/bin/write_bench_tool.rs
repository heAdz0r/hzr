use clap::Parser;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;
use tempfile::NamedTempFile;

#[path = "../write_core.rs"]
mod write_core;

use write_core::{AtomicWriter, DurabilityMode, WriteOptions};

#[derive(Parser, Debug)]
#[command(name = "write_bench_tool")]
#[command(about = "Low-level write benchmark driver for write_core")]
struct Cli {
    /// Target file path to write
    #[arg(long)]
    path: PathBuf,

    /// File whose bytes will be written to target
    #[arg(long = "content-file")]
    content_file: PathBuf,

    /// Durability mode: durable | fast
    #[arg(long, default_value = "durable")]
    mode: String,

    /// Implementation: write_core | native_safe
    #[arg(long = "implementation", default_value = "write_core")]
    implementation: String,

    /// Skip idempotent precheck (for known-changed paths)
    #[arg(long)]
    assume_changed: bool,
}

fn parse_mode(mode: &str) -> anyhow::Result<DurabilityMode> {
    match mode {
        "durable" => Ok(DurabilityMode::Durable),
        "fast" => Ok(DurabilityMode::Fast),
        _ => anyhow::bail!("Unsupported mode: {} (expected durable|fast)", mode),
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let durability = parse_mode(&cli.mode)?;

    let content = fs::read(&cli.content_file)
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", cli.content_file.display(), e))?;

    if cli.implementation == "write_core" {
        // changed: struct update syntax instead of field assignment after Default
        let opts = WriteOptions {
            durability,
            idempotent_skip: !cli.assume_changed,
            ..WriteOptions::default()
        };
        let writer = AtomicWriter::new(opts);
        let stats = writer.write_bytes(&cli.path, &content)?;

        println!(
            "{},{},{},{},{}",
            stats.elapsed.as_micros(),
            stats.bytes_written,
            stats.fsync_count,
            stats.rename_count,
            if stats.skipped_unchanged { 1 } else { 0 }
        );
        return Ok(());
    }

    if cli.implementation == "native_safe" {
        let parent = cli.path.parent().ok_or_else(|| {
            anyhow::anyhow!("Cannot write to {}: path has no parent", cli.path.display())
        })?;
        let start = Instant::now();

        let mut temp_file = NamedTempFile::new_in(parent)?;
        {
            let mut writer = BufWriter::with_capacity(64 * 1024, temp_file.as_file_mut());
            writer.write_all(&content)?;
            writer.flush()?;
        }

        let mut fsync_count = 0u32;
        let mut rename_count = 0u32;
        if durability == DurabilityMode::Durable {
            temp_file.as_file().sync_data()?;
            fsync_count += 1;
        }
        temp_file.persist(&cli.path).map_err(|e| {
            anyhow::anyhow!(
                "Failed to persist temp file to {}: {}",
                cli.path.display(),
                e.error
            )
        })?;
        rename_count += 1;

        if durability == DurabilityMode::Durable {
            let dir = std::fs::File::open(parent)?;
            dir.sync_all()?;
            fsync_count += 1;
        }

        println!(
            "{},{},{},{},{}",
            start.elapsed().as_micros(),
            content.len(),
            fsync_count,
            rename_count,
            0
        );
        return Ok(());
    }

    anyhow::bail!(
        "Unsupported implementation: {} (expected write_core|native_safe)",
        cli.implementation
    );
}
