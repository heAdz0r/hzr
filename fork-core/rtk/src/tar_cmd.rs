use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::fidelity::{self, FidelityReason};
use crate::tracking;

const DEFAULT_MAX_ENTRIES: usize = 100;
const MAX_ENTRIES: usize = 1_000;
const EXACT_REASONS: &[FidelityReason] = &[
    FidelityReason::MachineProtocol,
    FidelityReason::VerbatimSource,
];

pub fn run(args: &[String], max_entries: usize, verbose: u8) -> Result<()> {
    validate_listing(args)?;
    if !(1..=MAX_ENTRIES).contains(&max_entries) {
        bail!("--max-entries must be between 1 and {MAX_ENTRIES}");
    }
    let exact = fidelity::exact_requested(EXACT_REASONS)?;
    if verbose > 0 {
        eprintln!("tar bounded listing");
    }

    let timer = tracking::TimedExecution::start();
    let output = Command::new("tar")
        .args(args)
        .output()
        .context("Failed to run tar. Is tar available?")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = crate::utils::make_raw(&stdout, &stderr);
    if !output.status.success() {
        timer.track("tar <args omitted>", "rtk tar", &raw, "FAILED");
        if !stderr.trim().is_empty() {
            eprintln!("{}", stderr.trim());
        }
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let shown = if exact {
        stdout.into_owned()
    } else {
        bounded_listing(&stdout, max_entries)
    };
    print!("{shown}");
    if !shown.ends_with('\n') {
        println!();
    }
    timer.track("tar <args omitted>", "rtk tar", &raw, &shown);
    Ok(())
}

pub(crate) fn validate_listing(args: &[String]) -> Result<()> {
    if args.is_empty() {
        bail!("tar route requires a bounded -t/--list invocation");
    }
    let mut lists = false;
    let mut archive = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if index == 0 && !arg.starts_with('-') {
            if !valid_short_cluster(arg) {
                return listing_only_error();
            }
            lists |= arg.contains('t');
            if arg.contains('f') {
                index += 1;
                archive = args.get(index).is_some_and(|value| !value.starts_with('-'));
                if !archive {
                    return listing_only_error();
                }
            }
        } else if arg == "--list" {
            lists = true;
        } else if arg == "--gzip" || arg == "--verbose" {
        } else if let Some(path) = arg.strip_prefix("--file=") {
            archive = !path.is_empty();
        } else if arg == "--file" {
            index += 1;
            archive = args.get(index).is_some_and(|value| !value.starts_with('-'));
            if !archive {
                return listing_only_error();
            }
        } else if let Some(flags) = arg.strip_prefix('-') {
            if flags.is_empty() || !valid_short_cluster(flags) {
                return listing_only_error();
            }
            lists |= flags.contains('t');
            if flags.contains('f') {
                index += 1;
                archive = args.get(index).is_some_and(|value| !value.starts_with('-'));
                if !archive {
                    return listing_only_error();
                }
            }
        } else if !archive {
            return listing_only_error();
        }
        index += 1;
    }
    if !lists || !archive {
        return listing_only_error();
    }
    Ok(())
}

fn valid_short_cluster(flags: &str) -> bool {
    flags
        .chars()
        .all(|flag| matches!(flag, 't' | 'z' | 'v' | 'f'))
}

fn listing_only_error() -> Result<()> {
    Err(anyhow::anyhow!(
        "tar mutation, extraction, or non-list options remain Ask/E10; use only -t/-tzf/--list with an archive"
    ))
}

fn bounded_listing(output: &str, max_entries: usize) -> String {
    let lines = output.lines().collect::<Vec<_>>();
    let shown = lines.len().min(max_entries);
    let mut result = lines[..shown].join("\n");
    if !result.is_empty() {
        result.push('\n');
    }
    if lines.len() > shown {
        result.push_str(&format!(
            "... +{} entries; recovery: rerun with --max-entries N (max {MAX_ENTRIES}) or HZR_RAW_FIDELITY=1\n",
            lines.len() - shown
        ));
    }
    result
}

pub const fn default_max_entries() -> usize {
    DEFAULT_MAX_ENTRIES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_list_modes_are_accepted() {
        assert!(validate_listing(&["-tf".into(), "archive.tar".into()]).is_ok());
        assert!(validate_listing(&["-tzf".into(), "archive.tgz".into()]).is_ok());
        assert!(validate_listing(&["tf".into(), "archive.tar".into()]).is_ok());
        assert!(validate_listing(&["--list".into(), "--file=archive.tar".into()]).is_ok());
        assert!(validate_listing(&["-cf".into(), "archive.tar".into(), "src".into()]).is_err());
        assert!(validate_listing(&["-xf".into(), "archive.tar".into()]).is_err());
        assert!(
            validate_listing(&["-tf".into(), "archive.tar".into(), "--to-command=sh".into()])
                .is_err()
        );
    }

    #[test]
    fn listing_is_capped_with_recovery() {
        let output = (1..=150)
            .map(|entry| format!("dir/file-{entry}.txt"))
            .collect::<Vec<_>>()
            .join("\n");
        let shown = bounded_listing(&output, default_max_entries());

        assert_eq!(shown.lines().count(), 101);
        assert!(shown.contains("dir/file-100.txt"));
        assert!(!shown.contains("dir/file-101.txt"));
        assert!(shown.contains("... +50 entries; recovery:"));
    }

    #[test]
    fn exact_listing_accepts_only_semantic_listing_reasons() {
        for reason in ["machine_protocol", "verbatim_source"] {
            assert!(fidelity::validate_request(
                Some(std::ffi::OsStr::new("1")),
                Some(std::ffi::OsStr::new(reason)),
                EXACT_REASONS,
            )
            .unwrap());
        }
    }
}
