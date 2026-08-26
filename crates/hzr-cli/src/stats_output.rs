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

use crate::stats::{StatsReport, with_explicit_mcp_channel};

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
    write_optimizer_bypass(output, report, color)?;
    write_evasion(output, report, color)?;
    write_operation_families(output, report, color)?;
    write_operation_modes(output, report, color)?;
    write_subsystems(output, report, color)?;
    write_hot_paths(output, report, color)?;
    write_provider_usage(output, report, color)?;
    write_integrity(output, report, color)
}

fn write_evasion(output: &mut impl Write, report: &StatsReport, color: bool) -> io::Result<()> {
    let Some(evasion) = report.evasion.as_ref() else {
        return Ok(());
    };
    writeln!(output)?;
    writeln!(
        output,
        "{}  {}",
        style("EVASION", "1;38;5;208", color),
        style("aggregate-only · payloads omitted", "2;37", color)
    )?;
    // The taxonomy is closed and small, so every class that occurred is listed.
    // A `take(n)` here would silently drop the lowest-volume class as the
    // taxonomy grows, and a truncated list reads as "these are all of them".
    for class in &evasion.by_class {
        writeln!(
            output,
            "   {:<4} calls {:>8} · delivered {:>10} · avoidable {:>10}",
            class.class.as_str(),
            format_count(class.operations),
            format_count(class.delivered_tokens),
            format_count(class.avoidable_tokens)
        )?;
    }
    writeln!(
        output,
        "   fidelity {} ops / {} tokens · invalid {} · allowance {}/{}",
        format_count(evasion.fidelity_operations),
        format_count(evasion.fidelity_delivered_tokens),
        format_count(evasion.fidelity_invalid_operations),
        format_count(evasion.default_allowance.max_operations),
        format_count(evasion.default_allowance.max_delivered_tokens)
    )?;
    writeln!(
        output,
        "   policy attempts {} (separate from executed operations)",
        format_count(evasion.policy_attempts)
    )?;
    for event in &evasion.policy_by_class {
        writeln!(
            output,
            "   {:<4} {:<10} attempts {:>8} · avoidable {:>8}",
            event.class.as_str(),
            event.decision.as_str(),
            format_count(event.attempts),
            format_count(event.avoidable_attempts)
        )?;
    }
    Ok(())
}

fn write_operation_modes(
    output: &mut impl Write,
    report: &StatsReport,
    color: bool,
) -> io::Result<()> {
    if report.by_mode.is_empty() {
        return Ok(());
    }
    writeln!(output)?;
    writeln!(
        output,
        "{}  {}",
        style("OPERATION MODES", "1;38;5;208", color),
        style("estimated · stage-aware · top 12", "2;37", color)
    )?;
    let columns = [10, 19, 20, 8, 11];
    write_rule(output, '╭', '┬', '╮', &columns)?;
    writeln!(
        output,
        "│ {:<8} │ {:<17} │ {:<18} │ {:>6} │ {:>9} │",
        "FAMILY", "MODE", "STAGE", "CALLS", "DELIVERED"
    )?;
    write_rule(output, '├', '┼', '┤', &columns)?;
    for mode in report.by_mode.iter().take(12) {
        writeln!(
            output,
            "│ {:<8} │ {:<17} │ {:<18} │ {:>6} │ {:>9} │",
            mode.operation.as_str(),
            truncate(mode.mode.as_str(), 17),
            truncate(mode.stage.as_str(), 18),
            format_count(mode.operations),
            format_count(mode.delivered_tokens_estimated)
        )?;
    }
    write_rule(output, '╰', '┴', '╯', &columns)?;
    if report.by_mode.len() > 12 {
        writeln!(
            output,
            "   {} more mode/stage groups available in `hzr stats --json`",
            report.by_mode.len() - 12
        )?;
    }
    Ok(())
}

fn write_operation_families(
    output: &mut impl Write,
    report: &StatsReport,
    color: bool,
) -> io::Result<()> {
    if report.by_family.is_empty() {
        return Ok(());
    }
    writeln!(output)?;
    writeln!(
        output,
        "{}  {}",
        style("OPERATION FAMILIES", "1;38;5;208", color),
        style("estimated · arguments and content omitted", "2;37", color)
    )?;
    let columns = [24, 20, 12, 13];
    write_rule(output, '╭', '┬', '╮', &columns)?;
    writeln!(
        output,
        "│ {:<22} │ {:<18} │ {:>10} │ {:>11} │",
        "FAMILY", "ROUTE / REPLACE?", "CALLS", "DELIVERED"
    )?;
    write_rule(output, '├', '┼', '┤', &columns)?;
    for family in report.by_family.iter().take(12) {
        let route = match family.route {
            hzr_core::OperationRoute::Optimized => "optimized",
            hzr_core::OperationRoute::Bypassed => "raw",
            hzr_core::OperationRoute::NativeUnaccounted => "native_unaccounted",
        };
        let capability = match family.replacement_capability {
            hzr_core::ReplacementCapability::Available => "available",
            hzr_core::ReplacementCapability::Unavailable => "no filter",
            hzr_core::ReplacementCapability::Unknown => "unknown",
        };
        let route = format!("{route} / {capability}");
        writeln!(
            output,
            "│ {:<22} │ {:<18} │ {:>10} │ {:>11} │",
            truncate(&family.family, 22),
            truncate(&route, 18),
            format_count(family.operations),
            format_count(family.delivered_tokens_estimated)
        )?;
    }
    write_rule(output, '╰', '┴', '╯', &columns)
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
    writeln!(output, "│  {:<68}  │", truncate(&report.scope, 68))?;
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

/// Section 2 — what the headline ratio does not say.
///
/// A bypassed operation delivers exactly as many tokens as it consumed, so it raises both
/// sides of the reduction ratio equally and leaves the percentage looking healthy. This
/// panel sits directly under the headline because the two numbers are only meaningful
/// together: "87% avoided" next to "49% of delivered tokens never reached the optimizer"
/// tells a very different story than the headline alone.
fn write_optimizer_bypass(
    output: &mut impl Write,
    report: &StatsReport,
    color: bool,
) -> io::Result<()> {
    let bypass = &report.bypass;
    if bypass.operations == 0 {
        return Ok(());
    }
    writeln!(output)?;
    writeln!(
        output,
        "{}  {}",
        style("OPTIMIZER BYPASS", "1;38;5;208", color),
        style("estimated · these operations skipped HZR", "2;37", color)
    )?;
    writeln!(output, "╭{}╮", "─".repeat(WIDTH))?;
    writeln!(
        output,
        "│  {:<68}  │",
        truncate(
            &format!(
                "{} of {} operations ({:.1}%) reached the shell unfiltered",
                format_count(bypass.operations),
                format_count(bypass.total_operations),
                bypass.operation_share_pct
            ),
            68
        )
    )?;
    writeln!(
        output,
        "│  {:<68}  │",
        truncate(
            &format!(
                "{} of {} delivered tokens ({:.1}%) received zero filtering",
                format_count(bypass.delivered_tokens_estimated),
                format_count(bypass.total_delivered_tokens_estimated),
                bypass.token_share_pct
            ),
            68
        )
    )?;
    writeln!(output, "│{}│", " ".repeat(WIDTH))?;
    writeln!(
        output,
        "│  {}  │",
        style(&progress_bar(bypass.token_share_pct, 68), "1;33", color)
    )?;
    writeln!(output, "╰{}╯", "─".repeat(WIDTH))?;

    if report.traffic_coverage.unmeasured_bypass_operations > 0 {
        writeln!(
            output,
            "   {} passthrough operation(s) were unmeasured; the ratio measures a shrinking fraction of traffic",
            report.traffic_coverage.unmeasured_bypass_operations
        )?;
    }

    if bypass.by_tool.is_empty() {
        return Ok(());
    }
    // A free-form list rather than a table: the replacement is a command an operator
    // should be able to copy, and truncating it into a fixed column would defeat that.
    writeln!(output)?;
    for tool in bypass.by_tool.iter().take(8) {
        writeln!(
            output,
            "   {:<10} {:>8} calls · {:>9} delivered",
            truncate(&tool.tool, 10),
            format_count(tool.executions),
            format_count(tool.delivered_tokens_estimated)
        )?;
        match tool.replacement_capability {
            hzr_core::ReplacementCapability::Available => writeln!(
                output,
                "     {}",
                style(
                    &format!(
                        "→ {}",
                        tool.replacement.as_deref().unwrap_or(
                            "first-class HZR route available (exact route not retained)"
                        )
                    ),
                    "32",
                    color
                )
            )?,
            hzr_core::ReplacementCapability::Unavailable => writeln!(
                output,
                "     {}",
                style(
                    "→ no HZR filter yet; use hzr exec run (tracked fallback, zero savings credit)",
                    "2;37",
                    color
                )
            )?,
            hzr_core::ReplacementCapability::Unknown => writeln!(
                output,
                "     {}",
                style(
                    "→ capability unknown; historical/redacted evidence was insufficient",
                    "2;37",
                    color
                )
            )?,
        }
    }
    if bypass.by_tool_total > 8 {
        writeln!(
            output,
            "   {} more bypass tools not shown; exact details: {}",
            bypass.by_tool_total - 8,
            bypass.by_tool_recovery
        )?;
    }
    writeln!(
        output,
        "   {}",
        style(
            "available/no-filter states come from execution-time registry evidence; older rows remain unknown",
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
    if report.by_command_total > 12 {
        // Never let a truncated table read as the whole picture.
        writeln!(
            output,
            "   {}",
            style(
                &format!(
                    "{} more commands not shown; exact details: {}",
                    report.by_command_total - 12,
                    report.by_command_recovery
                ),
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
        style(
            &format!(
                "actual · billed by the provider · {}",
                report.observed_model_usage_scope.replace('_', " ")
            ),
            "2;37",
            color
        )
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
        ("● COMPLETE FOR OBSERVED CHANNELS", "1;32")
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
    let coverage = &report.coverage;
    let traffic = &report.traffic_coverage;
    writeln!(
        output,
        "├─ reduction ratio covers {} of {} observed operations ({:.1}%)",
        traffic.accounted_operations,
        traffic.total_observed_operations,
        traffic.accounted_share_pct
    )?;
    if traffic.native_unaccounted_operations > 0 {
        writeln!(
            output,
            "├─ {} host-native operation(s) were observed outside the optimizer",
            traffic.native_unaccounted_operations
        )?;
    }
    // MCP всегда в строке split, иначе отсутствие ключа читается как «канал вне учёта».
    let channels = with_explicit_mcp_channel(traffic.by_channel.clone())
        .iter()
        .map(|(channel, count)| format!("{channel}={count}"))
        .collect::<Vec<_>>()
        .join(" ");
    writeln!(output, "├─ channels {channels}")?;
    if coverage.unreconciled_rewrites > 0 {
        writeln!(
            output,
            "├─ {} daemon-free rewrite(s) are absent from the ledger",
            coverage.unreconciled_rewrites
        )?;
        writeln!(
            output,
            "├─ start the daemon (`hzr daemon service status`); the next managed rewrite closes this gap"
        )?;
    } else if coverage.lifetime_rewrites > 0 {
        // Closing a gap must not read as if it never happened.
        writeln!(
            output,
            "├─ {} daemon-free rewrite(s) occurred historically and are reconciled",
            coverage.lifetime_rewrites
        )?;
    }
    if coverage.daemon_unavailable_operations > 0 {
        writeln!(
            output,
            "├─ {} successful MCP operation(s) could not write their accounting row",
            coverage.daemon_unavailable_operations
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
    use hzr_core::{
        EvasionClassSummary, EvasionSummary, FidelityAllowance, LedgerSummary,
        OperationFamilySummary, OperationModeSummary, OperationRoute, PolicyEventSummary,
        ReadPipelineSummary, ReplacementCapability,
    };
    use hzr_protocol::{
        AccountingOperationKind, AccountingOperationMode, AccountingStage, EvasionClass,
        PolicyDecision,
    };

    use crate::hook_runner::AccountingCoverage;
    use crate::stats::{
        BypassReport, BypassToolReport, CommandSavings, DirectSavings, StatsReport,
        SubsystemSavings, TrafficCoverage,
    };

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
        let by_command_total = commands.len();
        StatsReport {
            hzr_version: "0.4.6",
            scope: "global lifetime".into(),
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
            by_mode: Vec::new(),
            read_pipeline: ReadPipelineSummary::default(),
            accounting_version_scope: "current_privacy_typed_policy",
            accounting_policy_version: "privacy_typed_v1",
            excluded_legacy_operations: 0,
            by_family: Vec::new(),
            evasion: None,
            by_command: commands,
            by_command_total,
            by_command_omitted: 0,
            by_command_recovery: "hzr stats --json --all --since 7d".into(),
            observed_model_usage: usage,
            observed_model_usage_scope: "global_lifetime",
            bypass: BypassReport::default(),
            traffic_coverage: TrafficCoverage::default(),
            degraded_rewrites: 3,
            coverage: AccountingCoverage {
                unreconciled_rewrites: 3,
                lifetime_rewrites: 3,
                daemon_unavailable_operations: 0,
                complete: false,
                last_degraded_at_unix: Some(1_785_531_432),
            },
            runtime_accounting_complete: false,
            economic_claim_ready: false,
            notes: Vec::new(),
        }
    }

    fn report_with_bypass(bypass: BypassReport) -> StatsReport {
        StatsReport {
            bypass,
            ..report(LedgerSummary::default(), Vec::new())
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

    /// The headline ratio is only honest when the bypass share is next to it, so the
    /// bypass panel must render before the reader reaches the subsystem breakdown.
    #[test]
    fn test_bypass_share_is_reported_next_to_the_headline_ratio() {
        let rendered = render(&report_with_bypass(BypassReport {
            operations: 3_152,
            total_operations: 8_393,
            operation_share_pct: 37.6,
            delivered_tokens_estimated: 6_865_388,
            total_delivered_tokens_estimated: 13_927_266,
            token_share_pct: 49.3,
            by_tool: vec![BypassToolReport {
                tool: "sed".into(),
                executions: 719,
                delivered_tokens_estimated: 983_969,
                example_command: "rtk proxy sed -n 1,80p install.sh".into(),
                replacement: Some("hzr rtk -- read install.sh --from 1 --to 80".into()),
                replacement_capability: ReplacementCapability::Available,
                rationale: Some("hzr read streams the requested span".into()),
            }],
            by_tool_total: 1,
            by_tool_omitted: 0,
            by_tool_recovery: "hzr stats --json --all --since 7d".into(),
        }));

        assert!(rendered.contains("OPTIMIZER BYPASS"));
        assert!(
            rendered.contains("37.6%"),
            "the operation share must be stated"
        );
        assert!(
            rendered.contains("49.3%"),
            "the delivered-token share is the number that matters"
        );
        assert!(
            rendered.contains("hzr rtk -- read install.sh --from 1 --to 80"),
            "every bypassed tool must show the command that replaces it"
        );
        assert!(
            rendered.find("OPTIMIZER BYPASS") < rendered.find("WHERE IT WAS AVOIDED"),
            "the bypass panel belongs beside the headline, not after the breakdown"
        );
        assert_aligned(&rendered);
    }

    #[test]
    fn acceptance_gate_bypass_capability_states_are_truthful_and_actionable() {
        let tools = [
            ("git", ReplacementCapability::Available),
            ("terraform", ReplacementCapability::Unavailable),
            ("legacy", ReplacementCapability::Unknown),
        ]
        .into_iter()
        .map(|(tool, replacement_capability)| BypassToolReport {
            tool: tool.into(),
            executions: 1,
            delivered_tokens_estimated: 10,
            example_command: format!("bypassed {tool} <arguments omitted>"),
            replacement: (replacement_capability == ReplacementCapability::Available)
                .then(|| "hzr rtk -- git status".into()),
            replacement_capability,
            rationale: None,
        })
        .collect();
        let rendered = render(&report_with_bypass(BypassReport {
            operations: 3,
            total_operations: 3,
            operation_share_pct: 100.0,
            delivered_tokens_estimated: 30,
            total_delivered_tokens_estimated: 30,
            token_share_pct: 100.0,
            by_tool: tools,
            by_tool_total: 3,
            by_tool_omitted: 0,
            by_tool_recovery: "hzr stats --json --all --since 7d".into(),
        }));

        assert!(rendered.contains("hzr rtk -- git status"));
        assert!(rendered.contains("no HZR filter yet; use hzr exec run"));
        assert!(rendered.contains("capability unknown"));
        assert!(!rendered.contains("raw is correct"));
    }

    /// A workspace that never bypassed the optimizer should not be shown an empty table.
    #[test]
    fn test_a_clean_bypass_record_renders_no_panel() {
        let rendered = render(&report(LedgerSummary::default(), Vec::new()));

        assert!(!rendered.contains("OPTIMIZER BYPASS"));
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
    fn test_complete_coverage_is_limited_to_observed_channels() {
        let mut report = report(LedgerSummary::default(), Vec::new());
        report.coverage = AccountingCoverage::default_complete();
        report.runtime_accounting_complete = true;
        let rendered = render(&report);
        assert!(rendered.contains("COMPLETE FOR OBSERVED CHANNELS"));
    }

    /// A missing MCP key looks like the channel is unaccounted; the split must show mcp=0.
    #[test]
    fn test_channel_split_always_shows_mcp_even_when_zero() {
        let mut traffic = TrafficCoverage {
            accounted_operations: 5,
            total_observed_operations: 5,
            accounted_share_pct: 100.0,
            ..TrafficCoverage::default()
        };
        traffic.by_channel.insert("hook_cli".into(), 5);
        let rendered = render(&StatsReport {
            traffic_coverage: traffic,
            ..report(LedgerSummary::default(), Vec::new())
        });

        let channels = rendered
            .lines()
            .find(|line| line.contains("channels "))
            .expect("channel split line");
        assert!(
            channels.contains("mcp=0"),
            "expected mcp=0 in channel split, got: {channels}"
        );
        assert!(channels.contains("hook_cli=5"));
    }

    #[test]
    fn test_empty_traffic_still_renders_mcp_zero_in_channel_split() {
        let rendered = render(&report(LedgerSummary::default(), Vec::new()));
        assert!(
            rendered.contains("channels ") && rendered.contains("mcp=0"),
            "empty traffic must still render an explicit mcp=0 channel"
        );
    }

    #[test]
    fn acceptance_gate_evasion_panel_is_bounded_and_aggregate_only() {
        let rendered = render(&StatsReport {
            evasion: Some(EvasionSummary {
                by_class: vec![EvasionClassSummary {
                    class: EvasionClass::E2ShellWrapper,
                    operations: 7,
                    delivered_tokens: 80_000,
                    avoidable_operations: 7,
                    avoidable_tokens: 70_000,
                }],
                fidelity_operations: 2,
                fidelity_delivered_tokens: 30_000,
                fidelity_invalid_operations: 1,
                default_allowance: FidelityAllowance::default(),
                policy_attempts: 3,
                policy_by_class: vec![PolicyEventSummary {
                    class: EvasionClass::E7FidelityHatch,
                    decision: PolicyDecision::Ask,
                    attempts: 3,
                    avoidable_attempts: 2,
                }],
            }),
            ..report(LedgerSummary::default(), Vec::new())
        });
        assert!(rendered.contains("EVASION"));
        assert!(rendered.contains("e2"));
        assert!(rendered.contains("policy attempts 3"));
        assert!(rendered.contains("e7") && rendered.contains("ask"));
        for sentinel in ["/private/path", "SELECT *", "secret=value", "HEREDOC"] {
            assert!(!rendered.contains(sentinel));
        }
    }

    #[test]
    fn evasion_panel_lists_every_class_that_occurred() {
        // The panel used to `take(10)`; with an 11-class taxonomy that silently
        // dropped the lowest-volume class while still reading as a full list.
        let classes = [
            EvasionClass::E1QuotedCoveredCommand,
            EvasionClass::E2ShellWrapper,
            EvasionClass::E3InterpreterRead,
            EvasionClass::E4ExecutablePath,
            EvasionClass::E5PipelineOrRedirect,
            EvasionClass::E6NestedUnboundedReader,
            EvasionClass::E7FidelityHatch,
            EvasionClass::E8NativeTool,
            EvasionClass::E9DiagnosticBypass,
            EvasionClass::E10CapabilityGap,
            EvasionClass::E11PrivilegedPrefix,
        ];
        let rendered = render(&StatsReport {
            evasion: Some(EvasionSummary {
                by_class: classes
                    .iter()
                    .enumerate()
                    .map(|(index, class)| EvasionClassSummary {
                        class: *class,
                        operations: 1,
                        // Descending, so the last class is the one a cap would drop.
                        delivered_tokens: (classes.len() - index) as u64 * 100,
                        avoidable_operations: 0,
                        avoidable_tokens: 0,
                    })
                    .collect(),
                default_allowance: FidelityAllowance::default(),
                ..EvasionSummary::default()
            }),
            ..report(LedgerSummary::default(), Vec::new())
        });
        for class in classes {
            assert!(
                rendered.contains(class.as_str()),
                "{} is missing from the evasion panel",
                class.as_str()
            );
        }
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

    #[test]
    fn acceptance_gate_family_panel_renders_only_safe_aggregates() {
        let mut report = report(LedgerSummary::default(), Vec::new());
        report.by_family = vec![OperationFamilySummary {
            family: "search".into(),
            route: OperationRoute::Bypassed,
            operations: 3,
            delivered_tokens_estimated: 1_024,
            replacement_capability: ReplacementCapability::Available,
        }];

        let rendered = render(&report);

        assert!(rendered.contains("OPERATION FAMILIES"));
        assert!(rendered.contains("raw / available"));
        assert!(rendered.contains("arguments and content omitted"));
        assert_aligned(&rendered);
    }

    #[test]
    fn acceptance_gate_mode_panel_is_stage_aware_and_bounded() {
        let mut report = report(LedgerSummary::default(), Vec::new());
        report.by_mode = (0..13)
            .map(|_| OperationModeSummary {
                operation: AccountingOperationKind::Search,
                mode: AccountingOperationMode::SearchExact,
                stage: AccountingStage::FinalDelivery,
                operations: 2,
                delivered_tokens_estimated: 8,
            })
            .collect();

        let rendered = render(&report);

        assert!(rendered.contains("OPERATION MODES"));
        assert!(rendered.contains("search_exact"));
        assert!(rendered.contains("final_delivery"));
        assert!(rendered.contains("1 more mode/stage groups"));
        assert_aligned(&rendered);
    }
}
