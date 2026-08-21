use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::fidelity::{self, FidelityReason};
use crate::tracking;

const MAX_TAIL_LINES: usize = 1_000;
const MAX_LINE_CHARS: usize = 500;
const EXACT_REASONS: &[FidelityReason] = &[FidelityReason::CompleteLog];

pub fn run(
    host: &str,
    container: &str,
    tail: usize,
    since: Option<&str>,
    timestamps: bool,
    verbose: u8,
) -> Result<()> {
    validate_safe_value("host", host, "._@:-")?;
    validate_safe_value("container", container, "_.-")?;
    if let Some(since) = since {
        validate_safe_value("since", since, "_.:+-TZ")?;
    }
    if !(1..=MAX_TAIL_LINES).contains(&tail) {
        bail!("--tail must be between 1 and {MAX_TAIL_LINES}");
    }
    let exact = fidelity::exact_requested(EXACT_REASONS)?;

    let mut remote = vec![
        "docker".to_owned(),
        "logs".to_owned(),
        "--tail".to_owned(),
        tail.to_string(),
    ];
    if let Some(since) = since {
        remote.extend(["--since".to_owned(), since.to_owned()]);
    }
    if timestamps {
        remote.push("--timestamps".to_owned());
    }
    remote.push(container.to_owned());

    if verbose > 0 {
        eprintln!("bounded remote docker logs: host={host} tail={tail}");
    }
    let timer = tracking::TimedExecution::start();
    let output = Command::new("ssh")
        .arg(host)
        .args(&remote)
        .output()
        .context("Failed to execute ssh for remote logs")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = crate::utils::make_raw(&stdout, &stderr);
    if !output.status.success() {
        timer.track(
            "ssh <host omitted> docker logs <container omitted>",
            "rtk logs",
            &raw,
            "FAILED",
        );
        if !stderr.trim().is_empty() {
            eprintln!("{}", stderr.trim());
        }
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let shown = if exact {
        raw.clone()
    } else {
        bounded_logs(&raw, tail)
    };
    print!("{shown}");
    if !shown.ends_with('\n') {
        println!();
    }
    timer.track(
        "ssh <host omitted> docker logs <container omitted>",
        "rtk logs",
        &raw,
        &shown,
    );
    Ok(())
}

pub(crate) fn validate_safe_value(label: &str, value: &str, punctuation: &str) -> Result<()> {
    if value.is_empty()
        || value.starts_with('-')
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || punctuation.contains(ch))
    {
        bail!("invalid {label}: typed remote logs reject shell syntax");
    }
    Ok(())
}

fn bounded_logs(output: &str, tail: usize) -> String {
    let lines = output.lines().collect::<Vec<_>>();
    let start = lines.len().saturating_sub(tail);
    let mut shown = lines[start..]
        .iter()
        .map(|line| truncate_line(line))
        .collect::<Vec<_>>()
        .join("\n");
    if !shown.is_empty() {
        shown.push('\n');
    }
    shown
}

fn truncate_line(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_owned();
    }
    let mut shown = line.chars().take(MAX_LINE_CHARS).collect::<String>();
    shown.push('…');
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_values_reject_remote_shell_syntax() {
        assert!(validate_safe_value("host", "deploy@example.internal", "._@:-").is_ok());
        assert!(validate_safe_value("container", "story-r11.2", "_.-").is_ok());
        assert!(validate_safe_value("host", "host;curl attacker", "._@:-").is_err());
        assert!(validate_safe_value("host", "-oProxyCommand=id", "._@:-").is_err());
        assert!(validate_safe_value("container", "$(id)", "_.-").is_err());
    }

    #[test]
    fn logs_are_tail_bounded_and_long_lines_are_truncated() {
        let input = format!("one\ntwo\n{}\n", "x".repeat(900));
        let shown = bounded_logs(&input, 2);

        assert!(!shown.contains("one"));
        assert!(shown.contains("two"));
        assert!(shown.contains('…'));
        assert!(shown.lines().count() == 2);
    }

    #[test]
    fn exact_remote_logs_require_complete_log() {
        assert!(fidelity::validate_request(
            Some(std::ffi::OsStr::new("1")),
            Some(std::ffi::OsStr::new("complete_log")),
            EXACT_REASONS,
        )
        .unwrap());
    }
}
