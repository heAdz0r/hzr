use crate::tracking;
use anyhow::{Context, Result};
use std::process::Command;

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("ps");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: ps {}", args.join(" "));
    }

    let output = cmd.output().context("Failed to run ps. Is ps available?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = crate::utils::make_raw(&stdout, &stderr); // fix #18: no double \n

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    let filtered = filter_ps(&stdout, args, verbose);

    if let Some(hint) = crate::tee::tee_and_hint(&raw, "ps", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    if !stderr.trim().is_empty() && verbose > 0 {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        &format!("ps {}", args.join(" ")),
        &format!("rtk ps {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }
    Ok(())
}

// System process prefixes to skip in compact mode
const SYSTEM_CMDS: &[&str] = &[
    "launchd",
    "kernel_task",
    "syslogd",
    "kextd",
    "UserEventAgen",
    "loginwindow",
    "WindowServer",
    "hidd",
    "bluetoothd",
    "locationd",
    "configd",
    "mDNSResponder",
    "diskarbitrationd",
    "coreaudiod",
    "powerd",
    "notifyd",
    "sysmond",
    "logd",
    "opendirectoryd",
    "(kernel)",
    "[kworker",
    "[kthreadd",
    "[ksoftirqd",
    "[rcu",
    "systemd",
    "dbus-daemon",
    "networkd",
    "resolved",
];

pub fn filter_ps(output: &str, args: &[String], verbose: u8) -> String {
    let mut lines = output.lines();

    // Pass through single-value queries (e.g. ps -o lstart= -p PID)
    let is_specific = args.iter().any(|a| a == "-p" || a.starts_with("-p"));
    let header = match lines.next() {
        Some(h) => h.to_string(),
        None => return "(no output)".to_string(),
    };

    // For specific PID queries pass through unchanged (already compact output) // fix #9
    if is_specific {
        let rest: Vec<&str> = output.lines().collect();
        return rest.join("\n");
    }

    // Parse logical columns, not byte offsets. `ps` right-aligns values beneath
    // short headers on macOS, so the byte position of `PID` can point inside the
    // value (`1234` -> `234`). The whitespace-delimited field order is stable.
    let header_fields: Vec<&str> = header.split_whitespace().collect();
    let col_pid = find_col_index(&header_fields, "PID");
    let col_cpu = find_col_index(&header_fields, "%CPU");
    let col_mem = find_col_index(&header_fields, "%MEM");
    let col_cmd =
        find_col_index(&header_fields, "COMMAND").or_else(|| find_col_index(&header_fields, "CMD"));

    let (col_pid, col_cmd) = match (col_pid, col_cmd) {
        (Some(p), Some(c)) => (p, c),
        // Can't parse header — pass through trimmed
        _ => {
            let all: Vec<&str> = output.lines().take(30).collect();
            return all.join("\n");
        }
    };

    let mut rows: Vec<PsRow> = Vec::new();

    for line in lines {
        if line.trim().is_empty() {
            continue;
        }

        let fields: Vec<&str> = line.split_whitespace().collect();
        let command = fields
            .get(col_cmd..)
            .map(|parts| parts.join(" "))
            .unwrap_or_default();

        // Skip obvious system processes in compact mode
        if verbose == 0 && is_system_process(&command) {
            continue;
        }

        let pid = fields.get(col_pid).copied().unwrap_or_default().to_string();
        let cpu = col_cpu
            .and_then(|column| fields.get(column).copied())
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);
        let mem = col_mem
            .and_then(|column| fields.get(column).copied())
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);

        // Truncate long commands
        let cmd_display = truncate_command(&command, 60);

        rows.push(PsRow {
            pid,
            cpu,
            mem,
            cmd: cmd_display,
        });
    }

    if rows.is_empty() {
        return format!("{}\n(no user processes)", header.trim());
    }

    // Sort by CPU desc, then MEM desc
    rows.sort_by(|a, b| {
        b.cpu
            .partial_cmp(&a.cpu)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.mem
                    .partial_cmp(&a.mem)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    // Cap at 25 rows in compact mode
    let limit = if verbose > 0 { rows.len() } else { 25 };

    let mut result = String::new();
    result.push_str(&format!(
        "{:<8} {:>5} {:>5}  {}\n",
        "PID", "%CPU", "%MEM", "COMMAND"
    ));
    result.push_str(&"─".repeat(72));
    result.push('\n');

    let total = rows.len();
    for row in rows.iter().take(limit) {
        result.push_str(&format!(
            "{:<8} {:>5.1} {:>5.1}  {}\n",
            row.pid, row.cpu, row.mem, row.cmd
        ));
    }

    if total > limit {
        result.push_str(&format!("\n… +{} more (use -v to show all)", total - limit));
    } else {
        result.push_str(&format!("\n{} processes", total));
    }

    result.trim().to_string()
}

struct PsRow {
    pid: String,
    cpu: f32,
    mem: f32,
    cmd: String,
}

fn find_col_index(header_fields: &[&str], name: &str) -> Option<usize> {
    header_fields.iter().position(|field| *field == name)
}

fn is_system_process(command: &str) -> bool {
    let cmd_lower = command.to_lowercase();
    SYSTEM_CMDS
        .iter()
        .any(|s| cmd_lower.starts_with(&s.to_lowercase()))
        || command.starts_with('[') // kernel threads
}

fn truncate_command(cmd: &str, max: usize) -> String {
    // Strip full path prefix for common tools
    let display = if cmd.starts_with('/') {
        cmd.rsplit('/').next().unwrap_or(cmd)
    } else {
        cmd
    };

    let char_count = display.chars().count(); // fix #2: char-aware, prevents multibyte panic
    if char_count <= max {
        display.to_string()
    } else {
        format!("{}…", display.chars().take(max - 1).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_ps_output() -> &'static str {
        "USER      PID  %CPU %MEM      VSZ    RSS   TT  STAT STARTED      TIME COMMAND\n\
         andrew   1234  52.3  4.2  1234567  89012   ??  S    11:00AM   0:01.23 go run ./cmd/serve\n\
         andrew   5678   3.1  1.0   456789  12345   s1  S     9:00AM   0:00.42 node server.js\n\
         andrew   9012   0.0  0.1    12345   6789   ??  S     8:00AM   0:00.01 bash\n\
         root        1   0.0  0.0    12345    123   ??  Ss   Jan01   0:00.50 launchd\n\
         root        2   0.0  0.0        0      0   ??  S    Jan01   0:00.00 kernel_task\n"
    }

    #[test]
    fn test_filter_ps_sorts_by_cpu() {
        let args = vec!["aux".to_string()];
        let result = filter_ps(sample_ps_output(), &args, 0);
        let lines: Vec<&str> = result.lines().collect();
        // First data row (after header+divider) should be highest CPU
        let first_data = lines
            .iter()
            .skip(2)
            .find(|l| l.contains("go") || l.contains("node"));
        assert!(
            first_data.map(|l| l.contains("52.3")).unwrap_or(false),
            "highest CPU process should be first, got: {:?}",
            first_data
        );
    }

    #[test]
    fn test_filter_ps_excludes_system() {
        let args = vec!["aux".to_string()];
        let result = filter_ps(sample_ps_output(), &args, 0);
        assert!(!result.contains("launchd"), "should filter system launchd");
        assert!(!result.contains("kernel_task"), "should filter kernel_task");
    }

    #[test]
    fn test_filter_ps_shows_user_processes() {
        let args = vec!["aux".to_string()];
        let result = filter_ps(sample_ps_output(), &args, 0);
        assert!(result.contains("go run"), "should show go server");
        assert!(result.contains("node"), "should show node process");
    }

    #[test]
    fn test_filter_ps_preserves_right_aligned_macos_pid_and_command() {
        let output = "USER               PID  %CPU %MEM      VSZ    RSS   TT  STAT STARTED      TIME COMMAND\n\
                      andrew            1086   9.8  3.6 1949531792 912112   ??  S    Mon12AM 124:32.86 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT --flag value\n\
                      _driverkit         578   1.7  0.2 435309488  44736   ??  Ss   Mon12AM  45:24.77 /System/Library/DriverExtensions/example\n";
        let result = filter_ps(output, &["aux".to_string()], 1);

        assert!(result.contains("1086"), "full four-digit PID must survive");
        assert!(result.contains("578"), "full three-digit PID must survive");
        assert!(result.contains("ChatGPT --flag value"));
        assert!(!result.contains("nd       "));
    }

    #[test]
    fn test_filter_ps_specific_pid_passthrough() {
        let output = "  STARTED\nMon Feb 24 11:00:00 2026\n";
        let args = vec![
            "-o".to_string(),
            "lstart=".to_string(),
            "-p".to_string(),
            "1234".to_string(),
        ];
        let result = filter_ps(output, &args, 0);
        // Should pass through unchanged for specific PID queries
        assert!(result.contains("STARTED") || result.contains("2026"));
    }

    #[test]
    fn test_filter_ps_empty() {
        let args = vec!["aux".to_string()];
        let result = filter_ps("", &args, 0);
        assert!(result.contains("no output"), "empty should say no output");
    }
}
