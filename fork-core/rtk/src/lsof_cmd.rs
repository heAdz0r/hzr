use crate::tracking;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::process::Command;

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("lsof");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: lsof {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run lsof. Is lsof installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = crate::utils::make_raw(&stdout, &stderr); // fix #18: no double \n

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    let filtered = filter_lsof(&stdout, verbose);

    if let Some(hint) = crate::tee::tee_and_hint(&raw, "lsof", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    if !stderr.trim().is_empty() && verbose > 0 {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        &format!("lsof {}", args.join(" ")),
        &format!("rtk lsof {}", args.join(" ")),
        &raw,
        &filtered,
    );

    // lsof exits 1 when no matches — don't propagate as error
    if !output.status.success() {
        // fix #3: propagate lsof exit code
        std::process::exit(exit_code);
    }
    Ok(())
}

#[derive(Debug, Default)]
struct PortEntry {
    port: String,
    proto: String,
    pid: String,
    command: String,
    listen: bool,
    established: usize,
    other: usize,
}

pub fn filter_lsof(output: &str, verbose: u8) -> String {
    let mut lines = output.lines();

    // Skip header line
    let header = lines.next().unwrap_or("");
    if header.is_empty() {
        return "(no output)".to_string();
    }

    // Detect column positions from header
    let name_col = header.find("NAME").unwrap_or(0);

    // port_key → PortEntry
    let mut ports: HashMap<String, PortEntry> = HashMap::new();
    // Non-network entries (files, pipes, etc.)
    let mut non_inet: Vec<String> = Vec::new();
    // Track unique PIDs per port for multi-process listeners

    for line in lines {
        if line.trim().is_empty() {
            continue;
        }

        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 9 {
            continue;
        }

        let command = cols[0].to_string();
        let pid = cols[1].to_string();
        let fd_type = cols[4]; // TYPE column

        // Only handle IPv4/IPv6 internet sockets
        if fd_type != "IPv4" && fd_type != "IPv6" {
            if verbose > 0 {
                non_inet.push(format!("  {} ({})", command, fd_type));
            }
            continue;
        }

        // NAME column: e.g. "*:8080 (LISTEN)" or "host:port->host:port (ESTABLISHED)"
        let name = if name_col > 0 && name_col < line.len() {
            &line[name_col..]
        } else {
            cols.last().map(|s| *s).unwrap_or("")
        };

        let state = extract_state(name);
        let (local_port, proto) = extract_port_proto(name, fd_type);

        if local_port.is_empty() {
            continue;
        }

        let entry = ports
            .entry(local_port.clone())
            .or_insert_with(|| PortEntry {
                port: local_port.clone(),
                proto: proto.clone(),
                ..Default::default()
            });

        match state {
            "LISTEN" => {
                entry.listen = true;
                entry.pid = pid.clone();
                entry.command = command.clone();
            }
            "ESTABLISHED" | "CLOSE_WAIT" | "TIME_WAIT" => {
                entry.established += 1;
            }
            _ => {
                entry.other += 1;
                if entry.command.is_empty() {
                    entry.pid = pid.clone();
                    entry.command = command.clone();
                }
            }
        }
    }

    if ports.is_empty() {
        return "lsof: no matching sockets found".to_string();
    }

    // Sort: listening first, then by port number
    let mut entries: Vec<&PortEntry> = ports.values().collect();
    entries.sort_by(|a, b| {
        b.listen
            .cmp(&a.listen)
            .then_with(|| port_num(&a.port).cmp(&port_num(&b.port)))
    });

    let mut result = String::new();
    result.push_str(&format!(
        "{:<8} {:<6} {:<8} {:<12} {}\n",
        "PORT", "PROTO", "PID", "COMMAND", "STATE"
    ));
    result.push_str(&"─".repeat(52));
    result.push('\n');

    for e in &entries {
        let state_str = if e.listen && e.established > 0 {
            format!("LISTEN ({} conn)", e.established)
        } else if e.listen {
            "LISTEN".to_string()
        } else if e.established > 0 {
            format!("{} ESTABLISHED", e.established)
        } else {
            "other".to_string()
        };

        let cmd_short = truncate_cmd(&e.command, 12);
        result.push_str(&format!(
            "{:<8} {:<6} {:<8} {:<12} {}\n",
            e.port, e.proto, e.pid, cmd_short, state_str
        ));
    }

    result.push_str(&format!("\n{} sockets", entries.len()));
    if verbose > 0 && !non_inet.is_empty() {
        // fix #5: emit non_inet entries
        result.push_str("\n\nNon-inet:\n");
        for entry in &non_inet {
            result.push_str(&format!("  {}\n", entry));
        }
    }
    result.trim().to_string()
}

fn extract_state(name: &str) -> &str {
    if name.contains("(LISTEN)") {
        "LISTEN"
    } else if name.contains("(ESTABLISHED)") {
        "ESTABLISHED"
    } else if name.contains("(CLOSE_WAIT)") {
        "CLOSE_WAIT"
    } else if name.contains("(TIME_WAIT)") {
        "TIME_WAIT"
    } else if name.contains("(SYN_SENT)") {
        "SYN_SENT"
    } else {
        "OTHER"
    }
}

fn extract_port_proto(name: &str, fd_type: &str) -> (String, String) {
    let proto = if fd_type == "IPv6" { "tcp6" } else { "tcp" };

    // Format: "*:8080 (LISTEN)" or "host:port->... (ESTABLISHED)"
    // Extract local part (before " " or "->")
    let local = name
        .split("->")
        .next()
        .unwrap_or(name)
        .split_whitespace()
        .next()
        .unwrap_or("");

    // Extract port from "host:port" or "*:port"
    let port = local.rsplit(':').next().unwrap_or("").trim_matches(')');

    if port.is_empty() || port.chars().all(|c| c.is_alphabetic()) {
        // Named port (http, https, etc.) — keep as-is
        (port.to_string(), proto.to_string())
    } else {
        (port.to_string(), proto.to_string())
    }
}

fn port_num(port: &str) -> u32 {
    port.parse().unwrap_or(u32::MAX)
}

fn truncate_cmd(cmd: &str, max: usize) -> String {
    let char_count = cmd.chars().count(); // fix #1: char-aware, prevents multibyte panic
    if char_count <= max {
        cmd.to_string()
    } else {
        format!("{}…", cmd.chars().take(max - 1).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_lsof_output() -> &'static str {
        "COMMAND   PID   USER   FD   TYPE  DEVICE SIZE/OFF NODE NAME\n\
         go       1234  user   8u   IPv6  0x1    0t0      TCP  *:8080 (LISTEN)\n\
         go       1234  user   9u   IPv6  0x2    0t0      TCP  localhost:8080->localhost:54321 (ESTABLISHED)\n\
         go       1234  user   10u  IPv6  0x3    0t0      TCP  localhost:8080->localhost:54322 (ESTABLISHED)\n\
         node     5678  user   25u  IPv4  0x4    0t0      TCP  *:3000 (LISTEN)\n\
         python3  9012  user   18u  IPv4  0x5    0t0      TCP  *:5173 (LISTEN)\n"
    }

    #[test]
    fn test_filter_lsof_listen_ports() {
        let result = filter_lsof(sample_lsof_output(), 0);
        assert!(result.contains("8080"), "should show port 8080");
        assert!(result.contains("3000"), "should show port 3000");
        assert!(result.contains("5173"), "should show port 5173");
        assert!(result.contains("LISTEN"), "should show LISTEN state");
    }

    #[test]
    fn test_filter_lsof_connection_count() {
        let result = filter_lsof(sample_lsof_output(), 0);
        assert!(
            result.contains("2 conn"),
            "port 8080 should show 2 connections"
        );
    }

    #[test]
    fn test_filter_lsof_shows_pid_and_command() {
        let result = filter_lsof(sample_lsof_output(), 0);
        assert!(result.contains("1234"), "should show PID 1234 for go");
        assert!(result.contains("5678"), "should show PID 5678 for node");
    }

    #[test]
    fn test_filter_lsof_empty_output() {
        let result = filter_lsof("", 0);
        assert!(result.contains("no output"), "empty should say no output");
    }

    #[test]
    fn test_filter_lsof_no_sockets() {
        let result = filter_lsof("COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\n", 0);
        assert!(result.contains("no matching"), "should indicate no sockets");
    }
}
