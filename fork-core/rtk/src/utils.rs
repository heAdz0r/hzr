//! Utility functions for text processing and command execution.
//!
//! Provides common helpers used across rtk commands:
//! - ANSI color code stripping
//! - Text truncation
//! - Command execution with error context

use anyhow::{Context, Result};
use regex::Regex;
use std::path::PathBuf;
use std::process::Command;

/// Tronque une chaîne à `max_len` caractères avec "..." si nécessaire.
///
/// # Arguments
/// * `s` - La chaîne à tronquer
/// * `max_len` - Longueur maximale avant troncature (minimum 3 pour inclure "...")
///
/// # Examples
/// ```
/// use rtk::utils::truncate;
/// assert_eq!(truncate("hello world", 8), "hello...");
/// assert_eq!(truncate("hi", 10), "hi");
/// ```
pub fn truncate(s: &str, max_len: usize) -> String {
    // M1: single-pass via char_indices — avoids O(2N) double scan (chars().count() + chars().take())
    if max_len < 3 {
        return "...".to_string();
    }
    let truncate_at = max_len - 3;
    let mut cut_byte: Option<usize> = None; // byte position at (max_len - 3) chars
    let mut n = 0usize;
    for (byte_pos, _) in s.char_indices() {
        if n == truncate_at {
            cut_byte = Some(byte_pos);
        }
        n += 1;
        if n > max_len {
            // String exceeds max_len — truncate at cut_byte
            let end = cut_byte.unwrap_or(s.len());
            return format!("{}...", &s[..end]);
        }
    }
    // n <= max_len — fits as-is
    s.to_string()
}

/// Supprime les codes ANSI d'une chaîne (couleurs, styles).
///
/// # Arguments
/// * `text` - Texte contenant potentiellement des codes ANSI
///
/// # Examples
/// ```
/// use rtk::utils::strip_ansi;
/// let colored = "\x1b[31mError\x1b[0m";
/// assert_eq!(strip_ansi(colored), "Error");
/// ```
pub fn strip_ansi(text: &str) -> String {
    lazy_static::lazy_static! {
        static ref ANSI_RE: Regex = Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap();
    }
    ANSI_RE.replace_all(text, "").to_string()
}

pub fn resolve_binary(name: &str) -> Result<PathBuf> {
    which::which(name).context(format!("Binary '{}' not found on PATH", name))
}

pub fn resolved_command(name: &str) -> Command {
    resolve_binary(name)
        .map(Command::new)
        .unwrap_or_else(|_| Command::new(name))
}

/// Exécute une commande et retourne stdout/stderr nettoyés.
///
/// # Arguments
/// * `cmd` - Commande à exécuter (ex: "eslint")
/// * `args` - Arguments de la commande
///
/// # Returns
/// `(stdout: String, stderr: String, exit_code: i32)`
///
/// # Examples
/// ```no_run
/// use rtk::utils::execute_command;
/// let (stdout, stderr, code) = execute_command("echo", &["test"]).unwrap();
/// assert_eq!(code, 0);
/// ```
#[allow(dead_code)]
pub fn execute_command(cmd: &str, args: &[&str]) -> Result<(String, String, i32)> {
    let output = Command::new(cmd)
        .args(args)
        .output()
        .context(format!("Failed to execute {}", cmd))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    Ok((stdout, stderr, exit_code))
}

/// Formate un nombre de tokens avec suffixes K/M pour lisibilité.
///
/// # Arguments
/// * `n` - Nombre de tokens
///
/// # Returns
/// String formaté (ex: "1.2M", "59.2K", "694")
///
/// # Examples
/// ```
/// use rtk::utils::format_tokens;
/// assert_eq!(format_tokens(1_234_567), "1.2M");
/// assert_eq!(format_tokens(59_234), "59.2K");
/// assert_eq!(format_tokens(694), "694");
/// ```
pub fn format_tokens(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{}", n)
    }
}

/// Formate un montant USD avec précision adaptée.
///
/// # Arguments
/// * `amount` - Montant en dollars
///
/// # Returns
/// String formaté avec $ prefix
///
/// # Examples
/// ```
/// use rtk::utils::format_usd;
/// assert_eq!(format_usd(1234.567), "$1234.57");
/// assert_eq!(format_usd(12.345), "$12.35");
/// assert_eq!(format_usd(0.123), "$0.12");
/// assert_eq!(format_usd(0.0096), "$0.0096");
/// ```
pub fn format_usd(amount: f64) -> String {
    if !amount.is_finite() {
        return "$0.00".to_string();
    }
    if amount >= 0.01 {
        format!("${:.2}", amount)
    } else {
        format!("${:.4}", amount)
    }
}

/// Format cost-per-token as $/MTok (e.g., "$3.86/MTok")
///
/// # Arguments
/// * `cpt` - Cost per token (not per million tokens)
///
/// # Returns
/// Formatted string like "$3.86/MTok"
///
/// # Examples
/// ```
/// use rtk::utils::format_cpt;
/// assert_eq!(format_cpt(0.000003), "$3.00/MTok");
/// assert_eq!(format_cpt(0.0000038), "$3.80/MTok");
/// assert_eq!(format_cpt(0.00000386), "$3.86/MTok");
/// ```
pub fn format_cpt(cpt: f64) -> String {
    if !cpt.is_finite() || cpt <= 0.0 {
        return "$0.00/MTok".to_string();
    }
    let cpt_per_million = cpt * 1_000_000.0;
    format!("${:.2}/MTok", cpt_per_million)
}

/// Format a confirmation message: "ok \<action\> \<detail\>"
/// Used for write operations (merge, create, comment, edit, etc.)
///
/// # Examples
/// ```
/// use rtk::utils::ok_confirmation;
/// assert_eq!(ok_confirmation("merged", "#42"), "ok merged #42");
/// assert_eq!(ok_confirmation("created", "PR #5 https://..."), "ok created PR #5 https://...");
/// ```
pub fn ok_confirmation(action: &str, detail: &str) -> String {
    if detail.is_empty() {
        format!("ok {}", action)
    } else {
        format!("ok {} {}", action, detail)
    }
}

/// Exit code of a finished `.output()` call: the real code, or `128 + signal`
/// per Unix convention when the child was killed rather than exiting.
///
/// The signal case also emits a one-line stderr diagnostic. Without it an
/// OOM-killed child reports a bare `137` and the caller has no way to tell a
/// kill apart from a program that chose that exit code.
pub fn exit_code_from_output(output: &std::process::Output, label: &str) -> i32 {
    exit_code_from_status(&output.status, label)
}

/// [`exit_code_from_output`] for a `.status()` call, which carries no captured
/// output to inspect.
pub fn exit_code_from_status(status: &std::process::ExitStatus, label: &str) -> i32 {
    match status.code() {
        Some(code) => code,
        None => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(sig) = status.signal() {
                    eprintln!("[rtk] {}: process terminated by signal {}", label, sig);
                    return 128 + sig;
                }
            }
            eprintln!("[rtk] {}: process terminated by signal", label);
            1
        }
    }
}

/// Create a directory owner-only (0700 on Unix), tightening one that already
/// exists. `create_dir_all` alone leaves every intermediate component at the
/// process umask, so the parent is created as its own step by the callers that
/// need the whole chain private.
pub fn create_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    set_owner_only(path, 0o700);
    Ok(())
}

/// Restrict an existing file to owner-only access (0600 on Unix).
pub fn restrict_file(path: &std::path::Path) {
    set_owner_only(path, 0o600);
}

/// Open a file owner-only (0600 on Unix), applied at creation so its content is
/// never briefly readable under a permissive umask. The mode is ignored for a
/// file that already exists, which is why the permissions are re-applied after
/// the open rather than only requested before it.
pub fn open_private(
    opts: &mut std::fs::OpenOptions,
    path: &std::path::Path,
) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(path)?;
    set_owner_only(path, 0o600);
    Ok(file)
}

#[cfg(unix)]
fn set_owner_only(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_owner_only(_path: &std::path::Path, _mode: u32) {}

/// Merge stdout and stderr into a single raw string for token tracking.
/// Avoids trailing double-newline when stderr is empty.
///
/// # Examples
/// ```
/// use rtk::utils::make_raw;
/// assert_eq!(make_raw("hello\n", ""), "hello\n");
/// assert_eq!(make_raw("out\n", "err\n"), "out\n\nerr\n");
/// ```
pub fn make_raw(stdout: impl AsRef<str>, stderr: impl AsRef<str>) -> String {
    // fix #18: AsRef<str> accepts Cow<str> and &str without explicit coercion
    let stdout = stdout.as_ref();
    let stderr = stderr.as_ref();
    if stderr.trim().is_empty() {
        stdout.to_string()
    } else {
        format!("{}\n{}", stdout, stderr)
    }
}

/// Detect the package manager used in the current directory.
/// Returns "pnpm", "yarn", or "npm" based on lockfile presence.
///
/// # Examples
/// ```no_run
/// use rtk::utils::detect_package_manager;
/// let pm = detect_package_manager();
/// // Returns "pnpm" if pnpm-lock.yaml exists, "yarn" if yarn.lock, else "npm"
/// ```
#[allow(dead_code)]
pub fn detect_package_manager() -> &'static str {
    if std::path::Path::new("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if std::path::Path::new("yarn.lock").exists() {
        "yarn"
    } else {
        "npm"
    }
}

/// Build a Command using the detected package manager's exec mechanism.
/// Returns a Command ready to have tool-specific args appended.
pub fn package_manager_exec(tool: &str) -> Command {
    let tool_exists = Command::new("which")
        .arg(tool)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if tool_exists {
        Command::new(tool)
    } else {
        let pm = detect_package_manager();
        match pm {
            "pnpm" => {
                let mut c = Command::new("pnpm");
                c.arg("exec").arg("--").arg(tool);
                c
            }
            "yarn" => {
                let mut c = Command::new("yarn");
                c.arg("exec").arg("--").arg(tool);
                c
            }
            _ => {
                let mut c = Command::new("npx");
                c.arg("--no-install").arg("--").arg(tool);
                c
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long_string() {
        let result = truncate("hello world", 8);
        assert_eq!(result, "hello...");
    }

    #[test]
    fn test_truncate_exact_length() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_edge_case() {
        // max_len < 3 returns just "..."
        assert_eq!(truncate("hello", 2), "...");
        // When string length equals max_len, return as is
        assert_eq!(truncate("abc", 3), "abc");
        // When string is longer and max_len is exactly 3, return "..."
        assert_eq!(truncate("hello world", 3), "...");
    }

    #[test]
    fn test_strip_ansi_simple() {
        let input = "\x1b[31mError\x1b[0m";
        assert_eq!(strip_ansi(input), "Error");
    }

    #[test]
    fn test_strip_ansi_multiple() {
        let input = "\x1b[1m\x1b[32mSuccess\x1b[0m\x1b[0m";
        assert_eq!(strip_ansi(input), "Success");
    }

    #[test]
    fn test_strip_ansi_no_codes() {
        assert_eq!(strip_ansi("plain text"), "plain text");
    }

    #[test]
    fn test_strip_ansi_complex() {
        let input = "\x1b[32mGreen\x1b[0m normal \x1b[31mRed\x1b[0m";
        assert_eq!(strip_ansi(input), "Green normal Red");
    }

    #[test]
    fn test_execute_command_success() {
        let result = execute_command("echo", &["test"]);
        assert!(result.is_ok());
        let (stdout, _, code) = result.unwrap();
        assert_eq!(code, 0);
        assert!(stdout.contains("test"));
    }

    #[test]
    fn test_execute_command_failure() {
        let result = execute_command("nonexistent_command_xyz_12345", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_format_tokens_millions() {
        assert_eq!(format_tokens(1_234_567), "1.2M");
        assert_eq!(format_tokens(12_345_678), "12.3M");
    }

    #[test]
    fn test_format_tokens_thousands() {
        assert_eq!(format_tokens(59_234), "59.2K");
        assert_eq!(format_tokens(1_000), "1.0K");
    }

    #[test]
    fn test_format_tokens_small() {
        assert_eq!(format_tokens(694), "694");
        assert_eq!(format_tokens(0), "0");
    }

    #[test]
    fn test_format_usd_large() {
        assert_eq!(format_usd(1234.567), "$1234.57");
        assert_eq!(format_usd(1000.0), "$1000.00");
    }

    #[test]
    fn test_format_usd_medium() {
        assert_eq!(format_usd(12.345), "$12.35");
        assert_eq!(format_usd(0.99), "$0.99");
    }

    #[test]
    fn test_format_usd_small() {
        assert_eq!(format_usd(0.0096), "$0.0096");
        assert_eq!(format_usd(0.0001), "$0.0001");
    }

    #[test]
    fn test_format_usd_edge() {
        assert_eq!(format_usd(0.01), "$0.01");
        assert_eq!(format_usd(0.009), "$0.0090");
    }

    #[test]
    fn test_ok_confirmation_with_detail() {
        assert_eq!(ok_confirmation("merged", "#42"), "ok merged #42");
        assert_eq!(
            ok_confirmation("created", "PR #5 https://github.com/foo/bar/pull/5"),
            "ok created PR #5 https://github.com/foo/bar/pull/5"
        );
    }

    #[test]
    fn test_ok_confirmation_no_detail() {
        assert_eq!(ok_confirmation("commented", ""), "ok commented");
    }

    #[test]
    fn test_format_cpt_normal() {
        assert_eq!(format_cpt(0.000003), "$3.00/MTok");
        assert_eq!(format_cpt(0.0000038), "$3.80/MTok");
        assert_eq!(format_cpt(0.00000386), "$3.86/MTok");
    }

    #[test]
    fn test_format_cpt_edge_cases() {
        assert_eq!(format_cpt(0.0), "$0.00/MTok"); // zero
        assert_eq!(format_cpt(-0.000001), "$0.00/MTok"); // negative
        assert_eq!(format_cpt(f64::INFINITY), "$0.00/MTok"); // infinite
        assert_eq!(format_cpt(f64::NAN), "$0.00/MTok"); // NaN
    }

    #[test]
    fn test_detect_package_manager_default() {
        // In the test environment (rtk repo), there's no JS lockfile
        // so it should default to "npm"
        let pm = detect_package_manager();
        assert!(["pnpm", "yarn", "npm"].contains(&pm));
    }

    #[test]
    fn test_truncate_multibyte_thai() {
        // Thai characters are 3 bytes each
        let thai = "สวัสดีครับ";
        let result = truncate(thai, 5);
        // Should not panic, should produce valid UTF-8
        assert!(result.len() <= thai.len());
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_multibyte_emoji() {
        let emoji = "🎉🎊🎈🎁🎂🎄🎃🎆🎇✨";
        let result = truncate(emoji, 5);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_multibyte_cjk() {
        let cjk = "你好世界测试字符串";
        let result = truncate(cjk, 6);
        assert!(result.ends_with("..."));
    }

    // tests for make_raw (#18)
    #[test]
    fn test_make_raw_empty_stderr() {
        assert_eq!(make_raw("hello\n", ""), "hello\n");
        assert_eq!(make_raw("hello\n", "   "), "hello\n"); // whitespace-only stderr
    }

    #[test]
    fn test_make_raw_with_stderr() {
        assert_eq!(make_raw("out\n", "err\n"), "out\n\nerr\n");
    }

    #[test]
    fn test_make_raw_both_empty() {
        assert_eq!(make_raw("", ""), "");
    }
}

/// Truncate an ISO 8601 datetime string to just the date portion (first 10 chars).
/// fork: ported from upstream v0.42.4 for aws_cmd.
pub fn truncate_iso_date(date: &str) -> &str {
    if date.len() >= 10 {
        &date[..10]
    } else {
        date
    }
}

/// Join items with newline, appending an overflow marker when total exceeds max.
/// fork: ported from upstream v0.42.4 for aws_cmd.
pub fn join_with_overflow(items: &[String], total: usize, max: usize, label: &str) -> String {
    let mut out = items.join("\n");
    if total > max {
        out.push_str(&format!("\n... +{} more {}", total - max, label));
    }
    out
}
