use std::io::{self, IsTerminal, Write};

use crate::stats::StatsReport;

pub fn print(report: &StatsReport) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    write_stats(&mut output, report, stdout.is_terminal())
}

fn write_stats(output: &mut impl Write, report: &StatsReport, color: bool) -> io::Result<()> {
    const WIDTH: usize = 72;

    writeln!(
        output,
        "{} {}",
        style(&format!("HZR {}", report.hzr_version), "1;38;5;208", color),
        style("// ZERO-REDUNDANCY LEDGER", "1;37", color)
    )?;
    writeln!(output, "╭{}╮", "─".repeat(WIDTH))?;
    let scope = report.scope.replace('_', " ").to_ascii_uppercase();
    writeln!(output, "│  {:<68}  │", truncate(&scope, 68))?;
    writeln!(output, "│{}│", " ".repeat(WIDTH))?;

    let eliminated = format!(
        "{} TOKENS ELIMINATED",
        format_count(report.direct_savings.net_avoided_tokens_estimated.max(0) as u64)
    );
    let efficiency = format!("{:.1}% EFFICIENCY", report.direct_savings.reduction_pct);
    writeln!(output, "│  {eliminated:<43}{efficiency:>25}  │")?;
    let efficiency_bar = progress_bar(report.direct_savings.reduction_pct, 68);
    writeln!(
        output,
        "│  {}  │",
        style(&efficiency_bar, "38;5;208", color)
    )?;
    writeln!(output, "│{}│", " ".repeat(WIDTH))?;
    writeln!(
        output,
        "│  {:<22}{:<22}{:<24}  │",
        "BEFORE HZR", "DELIVERED", "OPERATIONS"
    )?;
    writeln!(
        output,
        "│  {:<22}{:<22}{:<24}  │",
        format_count(report.direct_savings.input_tokens_estimated),
        format_count(report.direct_savings.delivered_tokens_estimated),
        format_count(report.direct_savings.operations)
    )?;
    writeln!(output, "╰{}╯", "─".repeat(WIDTH))?;

    if !report.by_subsystem.is_empty() {
        writeln!(output)?;
        writeln!(output, "{}", style("EFFICIENCY VEINS", "1;38;5;208", color))?;
        writeln!(
            output,
            "╭{}┬{}┬{}┬{}┬{}╮",
            "─".repeat(14),
            "─".repeat(10),
            "─".repeat(13),
            "─".repeat(9),
            "─".repeat(22)
        )?;
        writeln!(
            output,
            "│ {:<12} │ {:>8} │ {:>11} │ {:>7} │ {:<20} │",
            "SUBSYSTEM", "CALLS", "SAVED", "SHARE", "DISTRIBUTION"
        )?;
        writeln!(
            output,
            "├{}┼{}┼{}┼{}┼{}┤",
            "─".repeat(14),
            "─".repeat(10),
            "─".repeat(13),
            "─".repeat(9),
            "─".repeat(22)
        )?;
        for subsystem in &report.by_subsystem {
            let share = format!("{:.1}%", subsystem.share_pct);
            let distribution = progress_bar(subsystem.share_pct, 20);
            writeln!(
                output,
                "│ {:<12} │ {:>8} │ {:>11} │ {:>7} │ {} │",
                subsystem.subsystem,
                format_count(subsystem.operations),
                format_count(subsystem.net_avoided_tokens_estimated.max(0) as u64),
                share,
                style(&distribution, "38;5;208", color)
            )?;
        }
        writeln!(
            output,
            "╰{}┴{}┴{}┴{}┴{}╯",
            "─".repeat(14),
            "─".repeat(10),
            "─".repeat(13),
            "─".repeat(9),
            "─".repeat(22)
        )?;
    }

    if !report.by_command.is_empty() {
        writeln!(output)?;
        writeln!(output, "{}", style("HOT PATHS", "1;38;5;208", color))?;
        writeln!(
            output,
            "╭{}┬{}┬{}┬{}╮",
            "─".repeat(37),
            "─".repeat(10),
            "─".repeat(13),
            "─".repeat(9)
        )?;
        writeln!(
            output,
            "│ {:<35} │ {:>8} │ {:>11} │ {:>7} │",
            "COMMAND", "CALLS", "SAVED", "AVG"
        )?;
        writeln!(
            output,
            "├{}┼{}┼{}┼{}┤",
            "─".repeat(37),
            "─".repeat(10),
            "─".repeat(13),
            "─".repeat(9)
        )?;
        for command in report.by_command.iter().take(12) {
            writeln!(
                output,
                "│ {:<35} │ {:>8} │ {:>11} │ {:>7} │",
                truncate(&command.command, 35),
                format_count(command.executions),
                format_count(command.net_avoided_tokens_estimated.max(0) as u64),
                format!("{:.1}%", command.avg_savings_pct)
            )?;
        }
        writeln!(
            output,
            "╰{}┴{}┴{}┴{}╯",
            "─".repeat(37),
            "─".repeat(10),
            "─".repeat(13),
            "─".repeat(9)
        )?;
    }

    let usage = report.observed_model_usage;
    writeln!(output)?;
    writeln!(
        output,
        "{}",
        style("OBSERVED MODEL USAGE", "1;38;5;208", color)
    )?;
    writeln!(
        output,
        "╭{}┬{}┬{}┬{}┬{}╮",
        "─".repeat(10),
        "─".repeat(12),
        "─".repeat(12),
        "─".repeat(14),
        "─".repeat(20)
    )?;
    writeln!(
        output,
        "│ {:>8} │ {:>10} │ {:>10} │ {:>12} │ {:>18} │",
        "TASKS", "ACTUAL IN", "ACTUAL OUT", "EST. INPUT", "OBSERVED COST"
    )?;
    writeln!(
        output,
        "├{}┼{}┼{}┼{}┼{}┤",
        "─".repeat(10),
        "─".repeat(12),
        "─".repeat(12),
        "─".repeat(14),
        "─".repeat(20)
    )?;
    writeln!(
        output,
        "│ {:>8} │ {:>10} │ {:>10} │ {:>12} │ {:>18} │",
        format_count(usage.tasks),
        format_count(usage.actual_input_tokens),
        format_count(usage.actual_output_tokens),
        format_count(usage.estimated_input_tokens),
        format!("${:.6}", usage.cost_microusd as f64 / 1_000_000.0)
    )?;
    writeln!(
        output,
        "╰{}┴{}┴{}┴{}┴{}╯",
        "─".repeat(10),
        "─".repeat(12),
        "─".repeat(12),
        "─".repeat(14),
        "─".repeat(20)
    )?;

    let (accounting, status_color) = if report.runtime_accounting_complete {
        ("● COMPLETE", "1;32")
    } else {
        ("▲ INCOMPLETE", "1;33")
    };
    writeln!(output)?;
    writeln!(
        output,
        "{}  {}",
        style("ACCOUNTING", "1;38;5;208", color),
        style(accounting, status_color, color)
    )?;
    writeln!(output, "├─ degraded rewrites  {}", report.degraded_rewrites)?;
    writeln!(
        output,
        "╰─ estimated before/after · provider usage remains actual"
    )
}

fn style(text: &str, code: &str, enabled: bool) -> String {
    if enabled {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

fn progress_bar(percentage: f64, width: usize) -> String {
    let filled = ((percentage.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn format_count(value: u64) -> String {
    if value >= 1_000_000_000 {
        format!("{:.2}B", value as f64 / 1_000_000_000.0)
    } else if value >= 1_000_000 {
        format!("{:.2}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }
    value
        .chars()
        .take(width.saturating_sub(1))
        .collect::<String>()
        + "…"
}

#[cfg(test)]
mod tests {
    use hzr_core::LedgerSummary;

    use crate::stats::{CommandSavings, DirectSavings, StatsReport, SubsystemSavings};

    use super::write_stats;

    #[test]
    fn test_write_stats_renders_aligned_plain_text_dashboard() {
        let report = StatsReport {
            hzr_version: "0.2.0",
            scope: "global_lifetime",
            direct_savings: DirectSavings {
                operations: 42,
                input_tokens_estimated: 10_000,
                delivered_tokens_estimated: 2_000,
                gross_avoided_tokens_estimated: 8_000,
                regression_tokens_estimated: 0,
                net_avoided_tokens_estimated: 8_000,
                reduction_pct: 80.0,
                total_execution_ms: 100,
                measurement: "estimated_utf8_bytes_div_4_v1",
            },
            by_subsystem: vec![SubsystemSavings {
                subsystem: "search",
                operations: 12,
                gross_avoided_tokens_estimated: 8_000,
                regression_tokens_estimated: 0,
                net_avoided_tokens_estimated: 8_000,
                share_pct: 80.0,
            }],
            by_command: vec![CommandSavings {
                command: "hzr command with a deliberately excessive argument list".into(),
                subsystem: "execution",
                executions: 12,
                baseline_tokens_estimated: 10_000,
                delivered_tokens_estimated: 2_000,
                gross_avoided_tokens_estimated: 8_000,
                regression_tokens_estimated: 0,
                net_avoided_tokens_estimated: 8_000,
                avg_savings_pct: 80.0,
                avg_time_ms: 3,
            }],
            observed_model_usage: LedgerSummary {
                tasks: 2,
                actual_input_tokens: 900,
                actual_output_tokens: 100,
                estimated_input_tokens: 1_000,
                cost_microusd: 125_000,
                ..LedgerSummary::default()
            },
            degraded_rewrites: 3,
            runtime_accounting_complete: false,
            economic_claim_ready: false,
            notes: Vec::new(),
        };
        let mut output = Vec::new();

        write_stats(&mut output, &report, false).expect("render stats");

        let rendered = String::from_utf8(output).expect("UTF-8 output");
        assert!(rendered.contains("HZR 0.2.0 // ZERO-REDUNDANCY LEDGER"));
        assert!(rendered.contains("8.0K TOKENS ELIMINATED"));
        assert!(rendered.contains("████████████████░░░░"));
        assert!(rendered.contains("▲ INCOMPLETE"));
        assert!(rendered.contains('…'));
        assert!(!rendered.contains("\x1b["));
        for line in rendered.lines().filter(|line| {
            matches!(
                (line.chars().next(), line.chars().last()),
                (Some('╭'), Some('╮'))
                    | (Some('├'), Some('┤'))
                    | (Some('│'), Some('│'))
                    | (Some('╰'), Some('╯'))
            )
        }) {
            assert_eq!(line.chars().count(), 74, "misaligned line: {line}");
        }
    }
}
