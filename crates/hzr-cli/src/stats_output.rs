//! Terminal rendering for `hzr stats`.
//!
//! The layout enforces one rule the previous dashboard broke: **no panel mixes estimated
//! with actual.** A local byte-reduction figure and a provider bill are different kinds of
//! number, and showing "89.1% eliminated" beside "$0.000000" made an estimate read as
//! proven savings. PRD §3.1 is explicit that the inherited `bytes / 4` heuristic is not a
//! provider bill, and §4.2 forbids mixing the two.
//!
//! Three further rules follow from that:
//!
//! * every percentage appears next to the absolute numbers it came from, so a ratio can
//!   never stand alone as a claim;
//! * commands rank by absolute tokens avoided, never by percentage — ranking by percentage
//!   just promotes whichever command happens to have the most verbose output;
//! * an empty provider ledger is stated as "not measured", never as `$0.000000`, which
//!   reads as "free".

use std::io::{self, IsTerminal, Write};

use crate::stats::StatsReport;

/// Inner width of every framed panel. Kept in one place so the alignment assertion in the
/// tests stays meaningful.
const WIDTH: usize = 72;

pub fn print(report: &StatsReport) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    write_stats(&mut output, report, stdout.is_terminal())
}

fn write_stats(output: &mut impl Write, report: &StatsReport, color: bool) -> io::Result<()> {
    writeln!(
        output,
        "{} {}",
        style(&format!("HZR {}", report.hzr_version), "1;38;5;208", color),
        style("// ZERO-REDUNDANCY LEDGER", "1;37", color)
    )?;

    write_local_reduction(output, report, color)?;
    write_subsystems(output, report, color)?;
    write_hot_paths(output, report, color)?;
    write_provider_usage(output, report, color)?;
    write_integrity(output, report, color)
}

/// Section 1 — locally estimated output reduction. Every figure here comes from the fork
/// heuristic, so the provenance is stated in the header rather than a footnote.
fn write_local_reduction(
    output: &mut impl Write,
    report: &StatsReport,
    color: bool,
) -> io::Result<()> {
    let savings = &report.direct_savings;
    let avoided = savings.net_avoided_tokens_estimated.max(0) as u64;

    writeln!(output)?;
    writeln!(
        output,
        "{}  {}",
        style("LOCAL OUTPUT REDUCTION", "1;38;5;208", color),
        style("estimated · not a provider bill", "2;37", color)
    )?;
    writeln!(output, "╭{}╮", "─".repeat(WIDTH))?;
    writeln!(
        output,
        "│  {:<68}  │",
        truncate(&report.scope.replace('_', " "), 68)
    )?;
    writeln!(output, "│{}│", " ".repeat(WIDTH))?;

    // Absolutes first, ratio second: the ratio is derived from the two numbers above it.
    writeln!(
        output,
        "│  {:<22}{:<22}{:<24}  │",
        "TOOL OUTPUT BEFORE", "DELIVERED TO MODEL", "OPERATIONS"
    )?;
    writeln!(
        output,
        "│  {:<22}{:<22}{:<24}  │",
        format_count(savings.input_tokens_estimated),
        format_count(savings.delivered_tokens_estimated),
        format_count(savings.operations)
    )?;
    writeln!(output, "│{}│", " ".repeat(WIDTH))?;
    let avoided_line = format!("{} TOKENS AVOIDED", format_count(avoided));
    let ratio = format!("{:.1}% of tool output", savings.reduction_pct);
    writeln!(output, "│  {avoided_line:<40}{ratio:>28}  │")?;
    writeln!(
        output,
        "│  {}  │",
        style(&progress_bar(savings.reduction_pct, 68), "38;5;208", color)
    )?;
    if savings.regression_tokens_estimated > 0 {
        // Regressions are part of the net figure; hiding them would overstate the gain.
        writeln!(output, "│{}│", " ".repeat(WIDTH))?;
        writeln!(
            output,
            "│  {:<68}  │",
            truncate(
                &format!(
                    "including {} tokens of regression (output grew)",
                    format_count(savings.regression_tokens_estimated)
                ),
                68
            )
        )?;
    }
    writeln!(output, "╰{}╯", "─".repeat(WIDTH))?;
    writeln!(
        output,
        "   {}",
        style(
            &format!("measurement: {}", savings.measurement),
            "2;37",
            color
        )
    )
}

fn write_subsystems(output: &mut impl Write, report: &StatsReport, color: bool) -> io::Result<()> {
    if report.by_subsystem.is_empty() {
        return Ok(());
    }
    writeln!(output)?;
    writeln!(
        output,
        "{}  {}",
        style("WHERE IT WAS AVOIDED", "1;38;5;208", color),
        style("estimated", "2;37", color)
    )?;
    let columns = [14, 10, 13, 9, 22];
    write_rule(output, '╭', '┬', '╮', &columns)?;
    writeln!(
        output,
        "│ {:<12} │ {:>8} │ {:>11} │ {:>7} │ {:<20} │",
        "SUBSYSTEM", "CALLS", "AVOIDED", "SHARE", "DISTRIBUTION"
    )?;
    write_rule(output, '├', '┼', '┤', &columns)?;
    for subsystem in &report.by_subsystem {
        writeln!(
            output,
            "│ {:<12} │ {:>8} │ {:>11} │ {:>7} │ {} │",
            subsystem.subsystem,
            format_count(subsystem.operations),
            format_count(subsystem.net_avoided_tokens_estimated.max(0) as u64),
            format!("{:.1}%", subsystem.share_pct),
            style(&progress_bar(subsystem.share_pct, 20), "38;5;208", color)
        )?;
    }
    write_rule(output, '╰', '┴', '╯', &columns)
}

/// Section 2 — the commands that avoided the most tokens in absolute terms.
///
/// The per-command percentage is deliberately the *last* column and labelled as a ratio,
/// not a headline: a command can cut 99% of a tiny output and matter far less than one
/// that cut 40% of a huge one.
fn write_hot_paths(output: &mut impl Write, report: &StatsReport, color: bool) -> io::Result<()> {
    if report.by_command.is_empty() {
        return Ok(());
    }
    // Rank by absolute tokens avoided regardless of how the query ordered them, so the
    // ranking claim in the header is always true of what is displayed.
    let mut ranked: Vec<_> = report.by_command.iter().collect();
    ranked.sort_by_key(|command| std::cmp::Reverse(command.net_avoided_tokens_estimated));

    writeln!(output)?;
    writeln!(
        output,
        "{}  {}",
        style("TOP COMMANDS BY TOKENS AVOIDED", "1;38;5;208", color),
        style("estimated · ranked by absolute, not percent", "2;37", color)
    )?;
    let columns = [37, 10, 13, 9];
    write_rule(output, '╭', '┬', '╮', &columns)?;
    writeln!(
        output,
        "│ {:<35} │ {:>8} │ {:>11} │ {:>7} │",
        "COMMAND", "CALLS", "AVOIDED", "RATIO"
    )?;
    write_rule(output, '├', '┼', '┤', &columns)?;
    for command in ranked.iter().take(12) {
        writeln!(
            output,
            "│ {:<35} │ {:>8} │ {:>11} │ {:>7} │",
            truncate(&command.command, 35),
            format_count(command.executions),
            format_count(command.net_avoided_tokens_estimated.max(0) as u64),
            format!("{:.0}%", command.avg_savings_pct)
        )?;
    }
    write_rule(output, '╰', '┴', '╯', &columns)?;
    if ranked.len() > 12 {
        // Never let a truncated table read as the whole picture.
        writeln!(
            output,
            "   {}",
            style(
                &format!("{} more commands not shown", ranked.len() - 12),
                "2;37",
                color
            )
        )?;
    }
    Ok(())
}

/// Section 3 — actual provider usage. Physically separate from section 1 because this is
/// the only number that is a real bill.
fn write_provider_usage(
    output: &mut impl Write,
    report: &StatsReport,
    color: bool,
) -> io::Result<()> {
    let usage = report.observed_model_usage;
    writeln!(output)?;
    writeln!(
        output,
        "{}  {}",
        style("PROVIDER USAGE", "1;38;5;208", color),
        style("actual · billed by the provider", "2;37", color)
    )?;

    if usage.tasks == 0 {
        // A zero here means "never measured", which is the opposite of "cost nothing".
        // Printing $0.000000 was actively misleading.
        writeln!(output, "╭{}╮", "─".repeat(WIDTH))?;
        writeln!(
            output,
            "│  {:<68}  │",
            "no provider-billed task recorded yet"
        )?;
        writeln!(
            output,
            "│  {:<68}  │",
            "run `hzr agent run …`, or a paired benchmark, to populate this"
        )?;
        writeln!(output, "│{}│", " ".repeat(WIDTH))?;
        writeln!(
            output,
            "│  {:<68}  │",
            "the reduction above is a local estimate and is NOT a measured saving"
        )?;
        return writeln!(output, "╰{}╯", "─".repeat(WIDTH));
    }

    let columns = [10, 12, 12, 14, 20];
    write_rule(output, '╭', '┬', '╮', &columns)?;
    writeln!(
        output,
        "│ {:>8} │ {:>10} │ {:>10} │ {:>12} │ {:>18} │",
        "TASKS", "ACTUAL IN", "ACTUAL OUT", "EST. INPUT", "BILLED COST"
    )?;
    write_rule(output, '├', '┼', '┤', &columns)?;
    writeln!(
        output,
        "│ {:>8} │ {:>10} │ {:>10} │ {:>12} │ {:>18} │",
        format_count(usage.tasks),
        format_count(usage.actual_input_tokens),
        format_count(usage.actual_output_tokens),
        format_count(usage.estimated_input_tokens),
        format!("${:.4}", usage.cost_microusd as f64 / 1_000_000.0)
    )?;
    write_rule(output, '╰', '┴', '╯', &columns)?;
    if !report.economic_claim_ready {
        writeln!(
            output,
            "   {}",
            style(
                "not enough paired data for a cost-per-accepted-task claim",
                "2;37",
                color
            )
        )?;
    }
    Ok(())
}

/// Section 4 — how much of the above HZR actually saw. Degraded rewrites bypass the
/// ledger, so a reader must be able to tell partial coverage from complete coverage.
fn write_integrity(output: &mut impl Write, report: &StatsReport, color: bool) -> io::Result<()> {
    let (label, status_color) = if report.runtime_accounting_complete {
        ("● COMPLETE", "1;32")
    } else {
        ("▲ INCOMPLETE", "1;33")
    };
    writeln!(output)?;
    writeln!(
        output,
        "{}  {}",
        style("ACCOUNTING COVERAGE", "1;38;5;208", color),
        style(label, status_color, color)
    )?;
    if report.degraded_rewrites > 0 {
        writeln!(
            output,
            "├─ {} daemon-free rewrite(s) are absent from the ledger",
            report.degraded_rewrites
        )?;
        writeln!(
            output,
            "├─ start the daemon (`hzr daemon service status`) to account for them"
        )?;
    }
    for note in &report.notes {
        writeln!(output, "├─ {note}")?;
    }
    writeln!(
        output,
        "╰─ estimated and actual are reported separately and never summed"
    )
}

fn write_rule(
    output: &mut impl Write,
    left: char,
    joint: char,
    right: char,
    columns: &[usize],
) -> io::Result<()> {
    let mut line = String::from(left);
    for (index, width) in columns.iter().enumerate() {
        if index > 0 {
            line.push(joint);
        }
        line.push_str(&"─".repeat(*width));
    }
    line.push(right);
    writeln!(output, "{line}")
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

    fn command(name: &str, avoided: i64, pct: f64) -> CommandSavings {
        CommandSavings {
            command: name.into(),
            subsystem: "execution",
            executions: 12,
            baseline_tokens_estimated: 10_000,
            delivered_tokens_estimated: 2_000,
            gross_avoided_tokens_estimated: avoided.max(0) as u64,
            regression_tokens_estimated: 0,
            net_avoided_tokens_estimated: avoided,
            avg_savings_pct: pct,
            avg_time_ms: 3,
        }
    }

    fn report(usage: LedgerSummary, commands: Vec<CommandSavings>) -> StatsReport {
        StatsReport {
            hzr_version: "0.3.0",
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
            by_command: commands,
            observed_model_usage: usage,
            degraded_rewrites: 3,
            runtime_accounting_complete: false,
            economic_claim_ready: false,
            notes: Vec::new(),
        }
    }

    fn render(report: &StatsReport) -> String {
        let mut output = Vec::new();
        write_stats(&mut output, report, false).expect("render stats");
        String::from_utf8(output).expect("UTF-8 output")
    }

    fn assert_aligned(rendered: &str) {
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

    #[test]
    fn test_estimated_and_actual_are_labelled_and_never_share_a_panel() {
        let usage = LedgerSummary {
            tasks: 2,
            actual_input_tokens: 900,
            actual_output_tokens: 100,
            estimated_input_tokens: 1_000,
            cost_microusd: 125_000,
            ..LedgerSummary::default()
        };
        let rendered = render(&report(usage, vec![command("hzr read", 8_000, 80.0)]));

        assert!(rendered.contains("LOCAL OUTPUT REDUCTION"));
        assert!(
            rendered.contains("estimated · not a provider bill"),
            "the local figure must state that it is not a bill"
        );
        assert!(rendered.contains("PROVIDER USAGE"));
        assert!(rendered.contains("actual · billed by the provider"));

        // The two sections must be separate blocks, so the local panel has to close
        // before the provider header appears.
        let local = rendered
            .find("LOCAL OUTPUT REDUCTION")
            .expect("local section");
        let provider = rendered.find("PROVIDER USAGE").expect("provider section");
        assert!(local < provider);
        assert!(
            rendered[local..provider].contains('╯'),
            "the estimated panel must close before actual usage begins"
        );
        assert_aligned(&rendered);
    }

    #[test]
    fn test_empty_ledger_says_not_measured_instead_of_zero_cost() {
        let rendered = render(&report(LedgerSummary::default(), Vec::new()));

        assert!(
            rendered.contains("no provider-billed task recorded yet"),
            "an empty ledger must be stated, not rendered as a price"
        );
        assert!(
            !rendered.contains("$0.00"),
            "a zero price reads as free; it must not appear"
        );
        assert!(
            rendered.contains("NOT a measured saving"),
            "the reader must be told the reduction is unproven"
        );
        assert_aligned(&rendered);
    }

    #[test]
    fn test_commands_rank_by_absolute_tokens_not_percentage() {
        // A tiny command with a huge ratio must not outrank a large absolute saver.
        let rendered = render(&report(
            LedgerSummary::default(),
            vec![
                command("hzr tiny-but-99-percent", 500, 99.0),
                command("hzr large-but-40-percent", 900_000, 40.0),
            ],
        ));
        let large = rendered
            .find("hzr large-but-40-percent")
            .expect("large command");
        let tiny = rendered
            .find("hzr tiny-but-99-percent")
            .expect("tiny command");
        assert!(
            large < tiny,
            "ranking by percentage promotes whichever command is merely verbose"
        );
    }

    #[test]
    fn test_percentage_is_always_accompanied_by_its_absolutes() {
        let rendered = render(&report(LedgerSummary::default(), Vec::new()));
        let ratio_line = rendered
            .lines()
            .find(|line| line.contains("% of tool output"))
            .expect("ratio line");
        assert!(ratio_line.contains("TOKENS AVOIDED"));
        // And the inputs it derives from appear above it.
        assert!(rendered.contains("TOOL OUTPUT BEFORE"));
        assert!(rendered.contains("DELIVERED TO MODEL"));
    }

    #[test]
    fn test_incomplete_coverage_is_reported_with_remediation() {
        let rendered = render(&report(LedgerSummary::default(), Vec::new()));
        assert!(rendered.contains("▲ INCOMPLETE"));
        assert!(rendered.contains("absent from the ledger"));
        assert!(rendered.contains("never summed"));
    }

    #[test]
    fn test_truncated_command_table_states_what_was_hidden() {
        let many: Vec<_> = (0..15)
            .map(|index| command(&format!("hzr command-{index}"), 1_000 - index, 50.0))
            .collect();
        let rendered = render(&report(LedgerSummary::default(), many));
        assert!(
            rendered.contains("3 more commands not shown"),
            "a truncated table must not read as the whole picture"
        );
    }

    #[test]
    fn test_plain_output_contains_no_ansi_escapes() {
        let rendered = render(&report(LedgerSummary::default(), Vec::new()));
        assert!(!rendered.contains("\x1b["));
    }
}
