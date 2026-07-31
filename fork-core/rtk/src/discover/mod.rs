pub mod lexer; // fork: ported from upstream v0.42.4 (rewrite engine tokenizer)
pub mod provider;
pub mod registry;
mod report;
pub mod rules; // fork: ported from upstream v0.42.4 (rule data for classify+rewrite)

use anyhow::Result;
use std::collections::HashMap;

use provider::{ClaudeProvider, SessionProvider, TaskEvent};
use registry::{category_avg_tokens, classify_command, split_command_chain, Classification};
use report::{DiscoverReport, SupportedEntry, UnsupportedEntry};

/// Aggregation bucket for supported commands.
struct SupportedBucket {
    rtk_equivalent: &'static str,
    category: &'static str,
    count: usize,
    total_output_tokens: usize,
    savings_pct: f64,
    // For display: the most common raw command
    command_counts: HashMap<String, usize>,
}

/// Aggregation bucket for unsupported commands.
struct UnsupportedBucket {
    count: usize,
    example: String,
}

pub fn run(
    project: Option<&str>,
    all: bool,
    since_days: u64,
    limit: usize,
    format: &str,
    verbose: u8,
) -> Result<()> {
    let provider = ClaudeProvider;

    // Determine project filter
    let project_filter = if all {
        None
    } else if let Some(p) = project {
        Some(p.to_string())
    } else {
        // Default: current working directory
        let cwd = std::env::current_dir()?;
        let cwd_str = cwd.to_string_lossy().to_string();
        let encoded = ClaudeProvider::encode_project_path(&cwd_str);
        Some(encoded)
    };

    let sessions = provider.discover_sessions(project_filter.as_deref(), Some(since_days))?;

    if verbose > 0 {
        eprintln!("Scanning {} session files...", sessions.len());
        for s in &sessions {
            eprintln!("  {}", s.display());
        }
    }

    let mut total_commands: usize = 0;
    let mut already_rtk: usize = 0;
    let mut parse_errors: usize = 0;
    let mut supported_map: HashMap<&'static str, SupportedBucket> = HashMap::new();
    let mut unsupported_map: HashMap<String, UnsupportedBucket> = HashMap::new();

    let mut task_events: Vec<TaskEvent> = Vec::new(); // T4: memory miss tracking

    for session_path in &sessions {
        // T4: collect Task events for memory miss detection
        if let Ok(events) = provider.extract_task_events(session_path) {
            task_events.extend(events);
        }

        let extracted = match provider.extract_commands(session_path) {
            Ok(cmds) => cmds,
            Err(e) => {
                if verbose > 0 {
                    eprintln!("Warning: skipping {}: {}", session_path.display(), e);
                }
                parse_errors += 1;
                continue;
            }
        };

        for ext_cmd in &extracted {
            let parts = split_command_chain(&ext_cmd.command);
            for part in parts {
                total_commands += 1;

                match classify_command(part) {
                    Classification::Supported {
                        rtk_equivalent,
                        category,
                        estimated_savings_pct,
                        status,
                    } => {
                        let bucket = supported_map.entry(rtk_equivalent).or_insert_with(|| {
                            SupportedBucket {
                                rtk_equivalent,
                                category,
                                count: 0,
                                total_output_tokens: 0,
                                savings_pct: estimated_savings_pct,
                                command_counts: HashMap::new(),
                            }
                        });

                        bucket.count += 1;

                        // Estimate tokens for this command
                        let output_tokens = if let Some(len) = ext_cmd.output_len {
                            // Real: from tool_result content length
                            len / 4
                        } else {
                            // Fallback: category average
                            let subcmd = extract_subcmd(part);
                            category_avg_tokens(category, subcmd)
                        };

                        let savings =
                            (output_tokens as f64 * estimated_savings_pct / 100.0) as usize;
                        bucket.total_output_tokens += savings;

                        // Track the display name with status
                        let display_name = truncate_command(part);
                        let entry = bucket
                            .command_counts
                            .entry(format!("{}:{:?}", display_name, status))
                            .or_insert(0);
                        *entry += 1;
                    }
                    Classification::Unsupported { base_command } => {
                        let bucket = unsupported_map.entry(base_command).or_insert_with(|| {
                            UnsupportedBucket {
                                count: 0,
                                example: part.to_string(),
                            }
                        });
                        bucket.count += 1;
                    }
                    Classification::Ignored => {
                        // Check if it starts with "rtk "
                        if part.trim().starts_with("rtk ") {
                            already_rtk += 1;
                        }
                        // Otherwise just skip
                    }
                }
            }
        }
    }

    // Build report
    let mut supported: Vec<SupportedEntry> = supported_map
        .into_values()
        .map(|bucket| {
            // Pick the most common command as the display name
            let (command_with_status, status) = bucket
                .command_counts
                .into_iter()
                .max_by_key(|(_, c)| *c)
                .map(|(name, _)| {
                    // Extract status from "command:Status" format
                    if let Some(colon_pos) = name.rfind(':') {
                        let cmd = name[..colon_pos].to_string();
                        let status_str = &name[colon_pos + 1..];
                        let status = match status_str {
                            "Passthrough" => report::RtkStatus::Passthrough,
                            "NotSupported" => report::RtkStatus::NotSupported,
                            _ => report::RtkStatus::Existing,
                        };
                        (cmd, status)
                    } else {
                        (name, report::RtkStatus::Existing)
                    }
                })
                .unwrap_or_else(|| (String::new(), report::RtkStatus::Existing));

            SupportedEntry {
                command: command_with_status,
                count: bucket.count,
                rtk_equivalent: bucket.rtk_equivalent,
                category: bucket.category,
                estimated_savings_tokens: bucket.total_output_tokens,
                estimated_savings_pct: bucket.savings_pct,
                rtk_status: status,
            }
        })
        .collect();

    // Sort by estimated savings descending
    supported.sort_by(|a, b| b.estimated_savings_tokens.cmp(&a.estimated_savings_tokens));

    let mut unsupported: Vec<UnsupportedEntry> = unsupported_map
        .into_iter()
        .map(|(base, bucket)| UnsupportedEntry {
            base_command: base,
            count: bucket.count,
            example: bucket.example,
        })
        .collect();

    // Sort by count descending
    unsupported.sort_by(|a, b| b.count.cmp(&a.count));

    let report = DiscoverReport {
        sessions_scanned: sessions.len(),
        total_commands,
        already_rtk,
        since_days,
        supported,
        unsupported,
        parse_errors,
        memory_total_tasks: 0, // filled below after task_events scan // [P1] fix
        memory_miss_count: 0,  // filled below after task_events scan // [P1] fix
    };

    // T4: compute memory miss stats and embed in report before serialisation // [P1] fix
    let mem_total_task = task_events.len();
    let mem_miss_count = task_events.iter().filter(|e| !e.has_memory_context).count();

    let report = report::DiscoverReport {
        memory_total_tasks: mem_total_task,
        memory_miss_count: mem_miss_count,
        ..report
    };

    match format {
        "json" => println!("{}", report::format_json(&report)),
        _ => {
            print!("{}", report::format_text(&report, limit, verbose > 0));

            // T4: Memory Context Misses section (text only) // [P1] fix: guard for JSON
            if mem_total_task > 0 {
                if mem_miss_count == 0 {
                    println!(
                        "[ok] Memory context: all Task calls had RTK memory injected ({}/{})",
                        mem_total_task, mem_total_task
                    );
                } else {
                    let misses: Vec<&TaskEvent> = task_events
                        .iter()
                        .filter(|e| !e.has_memory_context)
                        .collect();
                    println!();
                    println!(
                        "Memory Context Misses ({}/{})",
                        mem_miss_count, mem_total_task
                    );
                    println!("{}", "-".repeat(60));
                    for e in misses.iter().take(limit) {
                        let agent = e.subagent_type.as_deref().unwrap_or("unknown");
                        let prefix = if e.prompt_prefix.is_empty() {
                            "(no prompt)"
                        } else {
                            &e.prompt_prefix
                        };
                        println!(
                            "  [{}] {}: {}...",
                            &e.session_id[..8.min(e.session_id.len())],
                            agent,
                            prefix
                        );
                    }
                    println!();
                    println!("Fix: rtk memory doctor");
                }
            }
        }
    }

    Ok(())
}

/// Extract the subcommand from a command string (second word).
fn extract_subcmd(cmd: &str) -> &str {
    let parts: Vec<&str> = cmd.trim().splitn(3, char::is_whitespace).collect();
    if parts.len() >= 2 {
        parts[1]
    } else {
        ""
    }
}

/// Truncate a command for display (keep first meaningful portion).
fn truncate_command(cmd: &str) -> String {
    let trimmed = cmd.trim();
    // Keep first two words for display
    let parts: Vec<&str> = trimmed.splitn(3, char::is_whitespace).collect();
    match parts.len() {
        0 => String::new(),
        1 => parts[0].to_string(),
        _ => format!("{} {}", parts[0], parts[1]),
    }
}
