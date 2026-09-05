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
//! * privacy-safe routes rank by net tokens avoided, never by percentage — ranking by percentage
//!   just promotes whichever route happens to have the most verbose output;
//! * an empty provider ledger is stated as "not measured", never as `$0.000000`, which
//!   reads as "free".

use std::io::{self, IsTerminal, Write};

use hzr_protocol::AccountingStage;

use crate::stats::{
    EconomicScopeRow, EconomicsReport, StatsReport, ZeroReductionCause, with_explicit_mcp_channel,
};

/// Inner width of every framed panel. Kept in one place so the alignment assertion in the
/// tests stays meaningful.
const WIDTH: usize = 72;

pub fn print_fleet(report: &hzr_core::FleetStatsSnapshot) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    writeln!(
        output,
        "Fleet snapshot [{}..{}) Unix seconds; {}",
        report.since_unix_seconds, report.until_unix_seconds, report.consistency
    )?;
    writeln!(
        output,
        "{} projects; {} recorded operations; {} measured operations; {} net tokens avoided (estimate)",
        report.by_project.len(),
        report.totals.recorded_operations,
        report.totals.measured_operations,
        report.totals.net_avoided_tokens_estimated
    )?;
    writeln!(
        output,
        "Host coverage: {}. Economic claim ready: {}.",
        report.host_coverage, report.economic_claim_ready
    )?;
    writeln!(
        output,
        "Repeated after filtering: {} operations, {} delivered tokens (association, not causation).",
        report.totals.repeated_after_filter_operations,
        report.totals.repeated_after_filter_tokens_estimated
    )?;
    for project in report
        .by_project
        .iter()
        .filter(|project| project.metrics.recorded_operations > 0)
        .take(20)
    {
        writeln!(
            output,
            "{}  operations={}  measured={}  net_estimated={}  workspace_exists={:?}",
            project.project_id,
            project.metrics.recorded_operations,
            project.metrics.measured_operations,
            project.metrics.net_avoided_tokens_estimated,
            project.workspace_exists
        )?;
    }
    writeln!(
        output,
        "Use --json or --export <file> for all projects, hosts and command families."
    )
}

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

    write_economics(output, &report.economics, color)?;
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

/// Section 0 — what the reduction is worth, for the two scopes an operator compares.
///
/// This sits above the token headline deliberately. 0.6.3 priced exactly one scope, at the very
/// bottom of the output, and only when an opt-in flag was already set — so the release that
/// introduced money shipped a surface on which money was, in practice, never visible.
///
/// Potential and billed are adjacent columns and never a sum. One is public-list arithmetic on
/// an estimate; the other is an imported receipt. Adding them would manufacture a number with no
/// referent, which is exactly the mistake the estimated/actual split exists to prevent.
fn write_economics(
    output: &mut impl Write,
    economics: &EconomicsReport,
    color: bool,
) -> io::Result<()> {
    writeln!(output)?;
    writeln!(
        output,
        "{}  {}",
        style("ECONOMICS", "1;38;5;208", color),
        style(
            "estimated potential · public list price · never an invoice",
            "2;37",
            color
        )
    )?;
    let columns = [
        Column::left(17),
        Column::right(14),
        Column::right(15),
        Column::right(15),
    ];
    write_rule(output, '╭', '┬', '╮', &columns)?;
    write_row(
        output,
        &columns,
        &[
            "SCOPE".into(),
            "AVOIDED TOKENS".into(),
            "POTENTIAL SAVED".into(),
            "BILLED (ACTUAL)".into(),
        ],
    )?;
    write_rule(output, '├', '┼', '┤', &columns)?;
    for row in &economics.rows {
        write_economic_row(output, &columns, row)?;
    }
    write_rule(output, '╰', '┴', '╯', &columns)?;

    if let Some(pricing) = &economics.pricing {
        writeln!(
            output,
            "   {} / {} / {} / {} · basis={}",
            pricing.harness, pricing.provider, pricing.model, pricing.method, pricing.pricing_basis
        )?;
        writeln!(
            output,
            "   {}",
            style(
                &format!(
                    "catalog={} retrieved={}",
                    pricing.price_table_identity, pricing.retrieved_at
                ),
                "2;37",
                color
            )
        )?;
    }
    if let Some(reason) = &economics.unavailable_reason {
        writeln!(output, "   potential value unavailable: {reason}")?;
    }
    for step in &economics.enable_steps {
        writeln!(output, "   {}", style(step, "2;37", color))?;
    }
    writeln!(
        output,
        "   {}",
        style(
            "potential and billed are different evidence and are never summed",
            "2;37",
            color
        )
    )
}

fn write_economic_row(
    output: &mut impl Write,
    columns: &[Column],
    row: &EconomicScopeRow,
) -> io::Result<()> {
    // "not measured" is not a cosmetic choice. A scope with no receipt has produced no evidence
    // about money at all, and rendering that as `USD 0.00` states it cost nothing.
    let avoided = if row.scope_resolved {
        format_count(row.avoided_input_tokens_estimated)
    } else {
        "—".to_owned()
    };
    let potential = match (&row.potential_saved, row.scope_resolved) {
        (Some(amount), _) => format_money(&amount.currency, amount.microunits),
        (None, true) => "unavailable".to_owned(),
        (None, false) => "—".to_owned(),
    };
    let billed = match &row.billed_actual {
        Some(amount) => format_money(&amount.currency, amount.microunits),
        None => "not measured".to_owned(),
    };
    write_row(
        output,
        columns,
        &[
            row.scope.into(),
            avoided.as_str().into(),
            potential.as_str().into(),
            billed.as_str().into(),
        ],
    )
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
    if let (Some(started), Some(seconds)) = (
        report.coverage.gap_started_at_unix,
        report.coverage.open_gap_seconds,
    ) {
        writeln!(
            output,
            "   ACCOUNTING GAP OPEN · started unix {} · missing last {}s · {} operation(s) absent from ledger",
            started,
            seconds,
            format_count(report.coverage.unreconciled_rewrites as u64),
        )?;
    } else if report.coverage.lifetime_rewrites > 0 {
        writeln!(
            output,
            "   accounting gaps closed · {} lifetime degraded rewrite(s)",
            format_count(report.coverage.lifetime_rewrites as u64),
        )?;
    }
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
    let columns = [
        Column::left(13),
        Column::left(17),
        Column::left(10),
        Column::left(3),
        Column::right(5),
        Column::right(7),
    ];
    write_rule(output, '╭', '┬', '╮', &columns)?;
    write_row(
        output,
        &columns,
        &[
            "FAMILY".into(),
            "MODE".into(),
            "STAGE".into(),
            "RAT".into(),
            "CALLS".into(),
            "DELIV".into(),
        ],
    )?;
    write_rule(output, '├', '┼', '┤', &columns)?;
    for mode in report.by_mode.iter().take(12) {
        let calls = format_count(mode.operations);
        let delivered = format_count(mode.delivered_tokens_estimated);
        write_row(
            output,
            &columns,
            &[
                mode.operation.as_str().into(),
                mode.mode.as_str().into(),
                short_stage(mode.stage).into(),
                if stage_in_ratio(mode.stage) {
                    "yes"
                } else {
                    "no"
                }
                .into(),
                calls.as_str().into(),
                delivered.as_str().into(),
            ],
        )?;
    }
    write_rule(output, '╰', '┴', '╯', &columns)?;
    // Without this the panel and the headline disagree by hundreds of operations and the output
    // offers no way to tell that they are answering two different questions.
    writeln!(
        output,
        "   {}",
        style(
            "RAT = counted in the reduction ratio. Delivery and control-plane stages are shown \
             here but excluded there, so a delivery cannot double-count the row that measured it.",
            "2;37",
            color
        )
    )?;
    if report.by_mode.len() > 12 {
        writeln!(
            output,
            "   {} more mode/stage groups available in `hzr stats --json`",
            report.by_mode.len() - 12
        )?;
    }
    Ok(())
}

/// Whether a stage is inside the reduction ratio's denominator.
///
/// This is the renderer's copy of the SQL predicate every efficiency query applies. It is a
/// closed enum, so a new stage forces a decision here instead of silently defaulting.
const fn stage_in_ratio(stage: AccountingStage) -> bool {
    match stage {
        AccountingStage::InternalTransport | AccountingStage::StandaloneDelivery => true,
        AccountingStage::FinalDelivery | AccountingStage::ControlPlane => false,
    }
}

/// Stable short labels for a closed enum.
///
/// Mid-word ellipsis on a fixed vocabulary destroys information for no reason — `standalone_delive…`
/// tells a reader less than `standalone` and looks like a rendering fault.
const fn short_stage(stage: AccountingStage) -> &'static str {
    match stage {
        AccountingStage::InternalTransport => "internal",
        AccountingStage::FinalDelivery => "delivery",
        AccountingStage::StandaloneDelivery => "standalone",
        AccountingStage::ControlPlane => "control",
    }
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
    let columns = [
        Column::left(22),
        Column::left(18),
        Column::right(10),
        Column::right(11),
    ];
    write_rule(output, '╭', '┬', '╮', &columns)?;
    write_row(
        output,
        &columns,
        &[
            "FAMILY".into(),
            "ROUTE / REPLACE?".into(),
            "CALLS".into(),
            "DELIVERED".into(),
        ],
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
        let calls = format_count(family.operations);
        let delivered = format_count(family.delivered_tokens_estimated);
        write_row(
            output,
            &columns,
            &[
                family.family.as_str().into(),
                route.as_str().into(),
                calls.as_str().into(),
                delivered.as_str().into(),
            ],
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
    let host_visible = &report.host_visible_savings;
    let avoided = host_visible.net_avoided_tokens_estimated;

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
        "PRODUCER INPUT", "PRODUCER OUTPUT", "PRODUCER OPERATIONS"
    )?;
    writeln!(
        output,
        "│  {:<22}{:<22}{:<24}  │",
        format_count(savings.input_tokens_estimated),
        format_count(savings.delivered_tokens_estimated),
        format_count(savings.operations)
    )?;
    writeln!(
        output,
        "│  {:<22}{:<22}{:<24}  │",
        "MODELED CAPPED INPUT", "MODELED CAPPED OUTPUT", "UNCAPPED HOST OPS"
    )?;
    writeln!(
        output,
        "│  {:<22}{:<22}{:<24}  │",
        format_count(host_visible.baseline_tokens_estimated),
        format_count(host_visible.delivered_tokens_estimated),
        format_count(host_visible.uncapped_operations)
    )?;
    writeln!(output, "│{}│", " ".repeat(WIDTH))?;
    let (avoided_line, ratio) = if !report.coverage.complete && savings.operations == 0 {
        (
            "ACCOUNTING UNKNOWN".to_owned(),
            "unknown · incomplete ledger".to_owned(),
        )
    } else if !report.coverage.complete || !host_visible.complete {
        (
            format!("{} HOST-CAPPED NET (PARTIAL)", format_signed_count(avoided)),
            format!(
                "{} · {} uncapped op(s)",
                format_truthful_percentage(host_visible.reduction_pct),
                host_visible.uncapped_operations
            ),
        )
    } else {
        (
            format!("{} NET TOKEN CHANGE", format_signed_count(avoided)),
            format!(
                "{} of tool output",
                format_truthful_percentage(host_visible.reduction_pct)
            ),
        )
    };
    writeln!(output, "│  {avoided_line:<40}{ratio:>28}  │")?;
    writeln!(
        output,
        "│  {}  │",
        style(
            &progress_bar(host_visible.reduction_pct, 68),
            "38;5;208",
            color
        )
    )?;
    // A zero headline has three very different meanings and 0.6.3 rendered all of them
    // identically. The upgrade case is the dangerous one: a policy-version bump moved the entire
    // recorded history out of the default scope, and the panel went on printing `0.0%` as though
    // it had measured something. An unexplained zero is a claim HZR cannot support.
    let disclosure = zero_reduction_disclosure(report);
    if !disclosure.is_empty() {
        writeln!(output, "│{}│", " ".repeat(WIDTH))?;
        for line in disclosure {
            debug_assert!(
                line.chars().count() <= 68,
                "a disclosure truncated mid-sentence explains nothing: {line}"
            );
            writeln!(output, "│  {:<68}  │", truncate(&line, 68))?;
        }
    }
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

/// State why the headline reads zero, in the panel that reads zero.
///
/// The recovery command is spelled out because it is the whole remedy for the common case: the
/// history is intact and one flag brings it back. Leaving the operator to discover
/// `--accounting-version all` from `--help` is how a working install reads as a broken one.
fn zero_reduction_disclosure(report: &StatsReport) -> Vec<String> {
    match report.zero_reduction_cause {
        ZeroReductionCause::NotZero => Vec::new(),
        ZeroReductionCause::ExcludedHistory => vec![
            "0.0% is a scope artifact, not a measurement.".to_owned(),
            format!(
                "{} operation(s) were recorded under an earlier accounting",
                format_count(report.excluded_legacy_operations)
            ),
            "policy outside the typed v1/v2 aggregate-compatible view.".to_owned(),
            "recover them with: hzr stats --accounting-version all".to_owned(),
        ],
        ZeroReductionCause::OnlyZeroCreditOperations => vec![
            "every operation in scope earns no savings credit by policy:".to_owned(),
            "generative operations and bypasses deliver exactly what they".to_owned(),
            "consumed. A clean zero, not a lost measurement.".to_owned(),
        ],
        ZeroReductionCause::NoOperations => {
            vec!["no operation has been recorded for this scope yet.".to_owned()]
        }
    }
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
                    "→ no HZR filter exists; tracked fallback has zero savings credit",
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
    let columns = [
        Column::left(12),
        Column::right(8),
        Column::right(11),
        Column::right(7),
        Column::left(20),
    ];
    write_rule(output, '╭', '┬', '╮', &columns)?;
    write_row(
        output,
        &columns,
        &[
            "SUBSYSTEM".into(),
            "CALLS".into(),
            "NET".into(),
            "SHARE".into(),
            "DISTRIBUTION".into(),
        ],
    )?;
    write_rule(output, '├', '┼', '┤', &columns)?;
    for subsystem in &report.by_subsystem {
        let calls = format_count(subsystem.operations);
        let avoided = format_signed_count(subsystem.net_avoided_tokens_estimated);
        let share = format_truthful_percentage(subsystem.share_pct);
        let bar = style(&progress_bar(subsystem.share_pct, 20), "38;5;208", color);
        write_row(
            output,
            &columns,
            &[
                subsystem.subsystem.into(),
                calls.as_str().into(),
                avoided.as_str().into(),
                share.as_str().into(),
                Cell::Styled(&bar),
            ],
        )?;
    }
    write_rule(output, '╰', '┴', '╯', &columns)
}

/// Section 2 — the privacy-safe operation routes that avoided the most tokens.
///
/// The per-route percentage is deliberately the *last* column and labelled as a ratio,
/// not a headline: a route can cut 99% of a tiny output and matter far less than one
/// that cut 40% of a huge one.
fn write_hot_paths(output: &mut impl Write, report: &StatsReport, color: bool) -> io::Result<()> {
    if report.by_command.is_empty() {
        return Ok(());
    }
    // Rank by net tokens avoided regardless of how the query ordered them, so the
    // ranking claim in the header is always true of what is displayed.
    let mut ranked: Vec<_> = report.by_command.iter().collect();
    ranked.sort_by_key(|command| std::cmp::Reverse(command.net_avoided_tokens_estimated));

    writeln!(output)?;
    writeln!(
        output,
        "{}  {}",
        style("TOP OPERATION ROUTES BY NET TOKENS", "1;38;5;208", color),
        style("estimated · privacy-safe aggregates", "2;37", color)
    )?;
    let columns = [
        Column::left(35),
        Column::right(8),
        Column::right(11),
        Column::right(7),
    ];
    write_rule(output, '╭', '┬', '╮', &columns)?;
    write_row(
        output,
        &columns,
        &[
            "ROUTE".into(),
            "CALLS".into(),
            "AVOIDED".into(),
            "RATIO".into(),
        ],
    )?;
    write_rule(output, '├', '┼', '┤', &columns)?;
    for command in ranked.iter().take(12) {
        let calls = format_count(command.executions);
        let avoided = format_signed_count(command.net_avoided_tokens_estimated);
        let ratio = format_truthful_percentage(command.avg_savings_pct);
        write_row(
            output,
            &columns,
            &[
                command.command.as_str().into(),
                calls.as_str().into(),
                avoided.as_str().into(),
                ratio.as_str().into(),
            ],
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
                    "{} more privacy-safe route(s) not shown; full aggregates: {}",
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

    let columns = [
        Column::right(8),
        Column::right(10),
        Column::right(10),
        Column::right(12),
        Column::right(18),
    ];
    write_rule(output, '╭', '┬', '╮', &columns)?;
    write_row(
        output,
        &columns,
        &[
            "TASKS".into(),
            "ACTUAL IN".into(),
            "ACTUAL OUT".into(),
            "EST. INPUT".into(),
            "BILLED COST".into(),
        ],
    )?;
    write_rule(output, '├', '┼', '┤', &columns)?;
    let tasks = format_count(usage.tasks);
    let actual_in = format_count(usage.actual_input_tokens);
    let actual_out = format_count(usage.actual_output_tokens);
    let estimated_in = format_count(usage.estimated_input_tokens);
    let billed = format!("${:.4}", usage.cost_microusd as f64 / 1_000_000.0);
    write_row(
        output,
        &columns,
        &[
            tasks.as_str().into(),
            actual_in.as_str().into(),
            actual_out.as_str().into(),
            estimated_in.as_str().into(),
            billed.as_str().into(),
        ],
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
    let coverage = &report.coverage;
    let (label, status_color) = if coverage.live_complete && coverage.historical_complete {
        ("● LIVE AND HISTORICAL COMPLETE", "1;32")
    } else if coverage.live_complete {
        ("● LIVE RECOVERED · HISTORICAL PARTIAL", "1;33")
    } else if coverage.open_intervals > 0
        || coverage.unreconciled_rewrites > 0
        || coverage.daemon_unavailable_operations > 0
    {
        ("▲ LIVE DEGRADED", "1;33")
    } else {
        ("? ACCOUNTING UNKNOWN", "1;33")
    };
    writeln!(output)?;
    writeln!(
        output,
        "{}  {}",
        style("ACCOUNTING COVERAGE", "1;38;5;208", color),
        style(label, status_color, color)
    )?;
    let traffic = &report.traffic_coverage;
    writeln!(
        output,
        "├─ accounting policy {} · scope {}",
        report.accounting_policy_version, report.accounting_version_scope
    )?;
    // The single most important line in this section after an upgrade. Without it the operator
    // sees a healthy-looking `100.0%` computed over ten rows while tens of thousands sit outside
    // the scope, and has no way to learn that from the output.
    if report.excluded_legacy_operations > 0 {
        writeln!(
            output,
            "├─ {} operation(s) written by an earlier accounting policy are EXCLUDED from every",
            format_count(report.excluded_legacy_operations)
        )?;
        writeln!(
            output,
            "├─ figure above; see them with `hzr stats --accounting-version all`"
        )?;
    }
    writeln!(
        output,
        "├─ reduction ratio covers {} of {} observed operations ({:.1}%) in the measured stages",
        traffic.accounted_operations,
        traffic.total_observed_operations,
        traffic.accounted_share_pct
    )?;
    // The re-run tax used to be zero by omission. Stating it beside the coverage figures is what
    // makes it arguable: an operator can now see whether filtering is paying for itself or being
    // undone by repeats, instead of being told a number nothing measured.
    if report.rerun_tax.operations > 0 {
        writeln!(
            output,
            "├─ RERUN TAX: {} operation(s) / {} token(s) repeated a command already filtered in",
            format_count(report.rerun_tax.operations),
            format_count(report.rerun_tax.tokens_estimated)
        )?;
        writeln!(
            output,
            "├─ the same session (within {} operations). Net avoided reads {} once that cost is",
            report.rerun_tax.detection_window_operations,
            format_signed_count(report.rerun_tax.net_avoided_after_rerun_tax_estimated)
        )?;
        writeln!(
            output,
            "├─ subtracted; the headline does not subtract it, because a repeat has other causes"
        )?;
    } else {
        writeln!(
            output,
            "├─ RERUN TAX: 0 measured repeats of an already-filtered command (measured, not assumed)"
        )?;
    }
    writeln!(
        output,
        "├─ explicit adapter delivery: {} token(s), {} record(s); host receipt/linkage unproven",
        report
            .explicit_delivery
            .tokens_estimated
            .map_or_else(|| "unknown".into(), format_count),
        format_count(report.explicit_delivery.operations)
    )?;
    if report.stage_exclusion.operations > 0 {
        writeln!(
            output,
            "├─ {} further operation(s) / {} estimated token(s) have non-producer stages",
            format_count(report.stage_exclusion.operations),
            format_count(report.stage_exclusion.delivered_tokens_estimated)
        )?;
        writeln!(
            output,
            "├─ stages: visible in OPERATION MODES, outside the ratio so a delivery cannot"
        )?;
        writeln!(
            output,
            "├─ double-count the internal_transport row that measured it"
        )?;
    }
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
    if coverage.open_intervals > 0 {
        let age = coverage.open_gap_seconds.map_or_else(
            || "unknown duration".to_owned(),
            |seconds| format!("{seconds}s open"),
        );
        let started = coverage.gap_started_at_unix.map_or_else(
            || "unknown start".to_owned(),
            |unix| format!("started unix {unix}"),
        );
        writeln!(
            output,
            "├─ {} accounting gap interval(s) OPEN · {started} · {age}",
            coverage.open_intervals
        )?;
    }
    if coverage.closed_intervals > 0 {
        let recovered = coverage.last_recovered_at_unix.map_or_else(
            || "last recovery unknown".to_owned(),
            |unix| format!("last recovered unix {unix}"),
        );
        // 0.8.3: the seconds are the duration of the closed gaps, not a count of anything
        // missing; the missing operations are itemized on the lines below.
        writeln!(
            output,
            "├─ {} closed gap interval(s) retained · {}s of closed gap · {recovered}",
            coverage.closed_intervals, coverage.closed_gap_seconds
        )?;
        writeln!(
            output,
            "├─ recovery restored live writes; absent historical rows were not backfilled"
        )?;
    }
    if coverage.unreconciled_rewrites > 0 {
        writeln!(
            output,
            "├─ {} operation(s) are absent from the ledger",
            coverage.unreconciled_rewrites
        )?;
        writeln!(
            output,
            "├─ start the daemon (`hzr daemon service status`); its receipt sweeper closes live fork gaps"
        )?;
    }
    // 0.8.3: one total, itemized so it reconciles. `lifetime_rewrites` used to be printed as
    // "daemon-free rewrites" although it also counted producer gaps and the imported pre-typed
    // total, while the producer line omitted the rewrite surface: 67 "rewrites" stood next to
    // "fork=3" and six intervals with no way to add them up.
    let typed_missing = coverage
        .hook_missing_operations
        .saturating_add(coverage.cli_missing_operations)
        .saturating_add(coverage.mcp_missing_operations)
        .saturating_add(coverage.fork_producer_missing_operations);
    if coverage.lifetime_rewrites > 0 {
        let undrained = if coverage.undrained_receipts > 0 {
            format!(" · {} undrained receipt(s)", coverage.undrained_receipts)
        } else {
            String::new()
        };
        writeln!(
            output,
            "├─ {} operation(s) absent from the ledger historically: {} before producer classification · {} daemon-free rewrite(s) · {} producer gap(s){undrained}",
            coverage.lifetime_rewrites,
            coverage.legacy_missing_operations,
            coverage.rewrite_missing_operations,
            typed_missing
        )?;
    }
    if typed_missing > 0 {
        writeln!(
            output,
            "├─ producer gaps by surface: hook={} cli={} mcp={} fork={}",
            coverage.hook_missing_operations,
            coverage.cli_missing_operations,
            coverage.mcp_missing_operations,
            coverage.fork_producer_missing_operations
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

#[derive(Clone, Copy, Eq, PartialEq)]
enum Align {
    Left,
    Right,
}

/// One table column: a content width and an alignment, declared once.
///
/// 0.6.3 wrote every row as a hand-built `format!` whose padding specifiers had to be kept in
/// sync with a separate array of rule widths. They drifted — the mode table formatted its family
/// cell as `{:<8}` and never truncated it, so a 13-character `observability` pushed every column
/// after it out of the frame. Declaring the column once and truncating through it makes that
/// class of defect unrepresentable rather than merely tested for.
#[derive(Clone, Copy)]
struct Column {
    width: usize,
    align: Align,
}

impl Column {
    const fn left(width: usize) -> Self {
        Self {
            width,
            align: Align::Left,
        }
    }

    const fn right(width: usize) -> Self {
        Self {
            width,
            align: Align::Right,
        }
    }
}

/// A table cell.
///
/// `Styled` exists for the one case a width cannot be measured from the string: an ANSI-coloured
/// progress bar, whose escape bytes are not visible columns. Its visible width is the column
/// width by construction, so it is emitted verbatim instead of being padded or truncated.
enum Cell<'a> {
    Text(&'a str),
    Styled(&'a str),
}

impl<'a> From<&'a str> for Cell<'a> {
    fn from(value: &'a str) -> Self {
        Self::Text(value)
    }
}

fn write_row(output: &mut impl Write, columns: &[Column], cells: &[Cell<'_>]) -> io::Result<()> {
    debug_assert_eq!(
        columns.len(),
        cells.len(),
        "every cell must belong to a declared column"
    );
    let mut line = String::from("│");
    for (column, cell) in columns.iter().zip(cells) {
        line.push(' ');
        match cell {
            Cell::Text(text) => {
                let text = truncate(text, column.width);
                let padding = " ".repeat(column.width.saturating_sub(text.chars().count()));
                match column.align {
                    Align::Left => {
                        line.push_str(&text);
                        line.push_str(&padding);
                    }
                    Align::Right => {
                        line.push_str(&padding);
                        line.push_str(&text);
                    }
                }
            }
            Cell::Styled(text) => line.push_str(text),
        }
        line.push_str(" │");
    }
    writeln!(output, "{line}")
}

fn write_rule(
    output: &mut impl Write,
    left: char,
    joint: char,
    right: char,
    columns: &[Column],
) -> io::Result<()> {
    let mut line = String::from(left);
    for (index, column) in columns.iter().enumerate() {
        if index > 0 {
            line.push(joint);
        }
        // Two cells of padding surround every column's content.
        line.push_str(&"─".repeat(column.width + 2));
    }
    line.push(right);
    writeln!(output, "{line}")
}

/// Money an operator can read at a glance without a rounding lie.
///
/// Two decimals for anything at or above a cent; full micro-unit precision below it, because
/// rendering a real 0.000123 as `0.00` states that HZR saved nothing when it did not.
fn format_money(currency: &str, microunits: u64) -> String {
    let units = microunits as f64 / 1_000_000.0;
    if microunits > 0 && microunits < 10_000 {
        format!("{currency} {units:.6}")
    } else {
        format!("{currency} {units:.2}")
    }
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

fn format_signed_count(value: i64) -> String {
    if value < 0 {
        format!("-{}", format_count(value.unsigned_abs()))
    } else {
        format_count(value as u64)
    }
}

fn format_truthful_percentage(value: f64) -> String {
    if value == 100.0 {
        "100%".to_owned()
    } else {
        format!("{:.1}%", (value * 10.0).trunc() / 10.0)
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
        PrivacySafeOperationKey, ReadPipelineSummary, ReplacementCapability,
    };
    use hzr_protocol::{
        AccountingOperationKind, AccountingOperationMode, AccountingStage, EvasionClass,
        PolicyDecision,
    };

    use crate::hook_runner::AccountingCoverage;
    use crate::stats::{
        BypassReport, BypassToolReport, CommandSavings, DirectSavings, EconomicScopeRow,
        EconomicsReport, HostVisibleSavings, MoneyAmount, PricingIdentity, RerunTax,
        StageExclusion, StatsReport, SubsystemSavings, TrafficCoverage, ZeroReductionCause,
    };

    use super::write_stats;

    fn command(name: &str, avoided: i64, pct: f64) -> CommandSavings {
        CommandSavings {
            key: PrivacySafeOperationKey {
                family: name.into(),
                operation: None,
                mode: None,
                stage: AccountingStage::InternalTransport,
                route: OperationRoute::Optimized,
            },
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
            explicit_delivery: hzr_core::DeliverySummary::default(),
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
            host_visible_savings: HostVisibleSavings {
                operations: 42,
                baseline_tokens_estimated: 10_000,
                delivered_tokens_estimated: 2_000,
                net_avoided_tokens_estimated: 8_000,
                reduction_pct: 80.0,
                uncapped_operations: 0,
                complete: true,
                method: "fixture",
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
                gap_started_at_unix: Some(1_785_531_400),
                open_gap_seconds: Some(32),
                ..AccountingCoverage::default()
            },
            runtime_accounting_complete: false,
            economic_claim_ready: false,
            raw_public_estimate: None,
            raw_public_estimate_unavailable_reason: Some("opt-in disabled".into()),
            economics: EconomicsReport {
                rows: vec![
                    EconomicScopeRow {
                        scope: "this project",
                        scope_resolved: true,
                        avoided_input_tokens_estimated: 8_000,
                        potential_saved: None,
                        billed_actual: None,
                        billed_receipts: 0,
                        notes: Vec::new(),
                    },
                    EconomicScopeRow {
                        scope: "global lifetime",
                        scope_resolved: true,
                        avoided_input_tokens_estimated: 8_000,
                        potential_saved: None,
                        billed_actual: None,
                        billed_receipts: 0,
                        notes: Vec::new(),
                    },
                ],
                pricing: None,
                unavailable_reason: Some("opt-in disabled".into()),
                enable_steps: Vec::new(),
            },
            stage_exclusion: StageExclusion {
                operations: 0,
                delivered_tokens_estimated: 0,
            },
            rerun_tax: RerunTax {
                operations: 0,
                tokens_estimated: 0,
                net_avoided_after_rerun_tax_estimated: 8_000,
                detection_window_operations: 8,
            },
            zero_reduction_cause: ZeroReductionCause::NotZero,
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

    #[test]
    fn acceptance_gate_evasion_names_when_the_open_accounting_gap_started() {
        let mut report = report(LedgerSummary::default(), Vec::new());
        report.evasion = Some(EvasionSummary::default());
        let rendered = render(&report);
        assert!(rendered.contains("ACCOUNTING GAP OPEN"));
        assert!(rendered.contains("started unix 1785531400"));
        assert!(rendered.contains("missing last 32s"));
        assert!(rendered.contains("3 operation(s) absent from ledger"));
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
        assert!(rendered.contains("no HZR filter exists; tracked fallback"));
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
        let mut report = report(LedgerSummary::default(), Vec::new());
        report.coverage = AccountingCoverage::default_complete();
        report.runtime_accounting_complete = true;
        let rendered = render(&report);
        let ratio_line = rendered
            .lines()
            .find(|line| line.contains("% of tool output"))
            .expect("ratio line");
        assert!(ratio_line.contains("NET TOKEN CHANGE"));
        // And the inputs it derives from appear above it.
        assert!(rendered.contains("PRODUCER INPUT"));
        assert!(rendered.contains("PRODUCER OUTPUT"));
        assert!(rendered.contains("MODELED CAPPED INPUT"));
        assert!(rendered.contains("MODELED CAPPED OUTPUT"));
    }

    #[test]
    fn test_incomplete_coverage_is_reported_with_remediation() {
        let rendered = render(&report(LedgerSummary::default(), Vec::new()));
        assert!(rendered.contains("▲ LIVE DEGRADED"));
        assert!(rendered.contains("absent from the ledger"));
        assert!(rendered.contains("never summed"));
    }

    #[test]
    fn test_complete_coverage_is_limited_to_observed_channels() {
        let mut report = report(LedgerSummary::default(), Vec::new());
        report.coverage = AccountingCoverage::default_complete();
        report.runtime_accounting_complete = true;
        let rendered = render(&report);
        assert!(rendered.contains("LIVE AND HISTORICAL COMPLETE"));
    }

    #[test]
    fn recovered_live_coverage_keeps_history_and_producer_breakdown_visible() {
        let mut report = report(LedgerSummary::default(), Vec::new());
        report.coverage = AccountingCoverage {
            lifetime_rewrites: 10,
            daemon_unavailable_operations: 10,
            live_complete: true,
            historical_complete: false,
            closed_intervals: 1,
            last_recovered_at_unix: Some(1_785_531_500),
            closed_gap_seconds: 720,
            hook_missing_operations: 4,
            cli_missing_operations: 2,
            mcp_missing_operations: 3,
            fork_producer_missing_operations: 1,
            ..AccountingCoverage::default()
        };

        let rendered = render(&report);
        assert!(rendered.contains("LIVE RECOVERED · HISTORICAL PARTIAL"));
        assert!(rendered.contains("1 closed gap interval(s) retained · 720s of closed gap"));
        assert!(rendered.contains("absent historical rows were not backfilled"));
        assert!(rendered.contains(
            "10 operation(s) absent from the ledger historically: 0 before producer classification · 0 daemon-free rewrite(s) · 10 producer gap(s)"
        ));
        assert!(rendered.contains("producer gaps by surface: hook=4 cli=2 mcp=3 fork=1"));
        assert!(!rendered.contains("successful MCP operation"));
        assert!(!rendered.contains("reconciled"));
        let json = serde_json::to_value(&report).expect("stats JSON");
        assert_eq!(json["coverage"]["live_complete"], true);
        assert_eq!(json["coverage"]["historical_complete"], false);
        assert_eq!(json["coverage"]["closed_gap_seconds"], 720);
        assert_eq!(json["coverage"]["mcp_missing_operations"], 3);
        assert_eq!(json["coverage"]["rewrite_missing_operations"], 0);
        assert_eq!(json["coverage"]["legacy_missing_operations"], 0);
    }

    // 0.8.3: the historical total itemizes into parts that add up to it.
    #[test]
    fn absent_operations_itemize_into_legacy_rewrite_and_producer_gaps() {
        let mut report = report(LedgerSummary::default(), Vec::new());
        report.coverage = AccountingCoverage {
            lifetime_rewrites: 69,
            daemon_unavailable_operations: 3,
            live_complete: true,
            historical_complete: false,
            closed_intervals: 6,
            last_recovered_at_unix: Some(1_788_610_767),
            closed_gap_seconds: 3_674,
            fork_producer_missing_operations: 3,
            rewrite_missing_operations: 4,
            legacy_missing_operations: 60,
            undrained_receipts: 2,
            ..AccountingCoverage::default()
        };

        let rendered = render(&report);
        assert!(rendered.contains(
            "69 operation(s) absent from the ledger historically: 60 before producer classification · 4 daemon-free rewrite(s) · 3 producer gap(s) · 2 undrained receipt(s)"
        ));
        assert!(rendered.contains("producer gaps by surface: hook=0 cli=0 mcp=0 fork=3"));
        assert!(
            !rendered.contains("daemon-free rewrite(s) remain absent"),
            "the old unitemized line must not reappear"
        );
        let json = serde_json::to_value(&report).expect("stats JSON");
        assert_eq!(json["coverage"]["rewrite_missing_operations"], 4);
        assert_eq!(json["coverage"]["legacy_missing_operations"], 60);
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
            rendered.contains("3 more privacy-safe route(s) not shown"),
            "a truncated table must not read as the whole picture"
        );
    }

    #[test]
    fn acceptance_gate_stats_does_not_hide_regression_or_round_up_evidence() {
        let rendered = render(&report(
            LedgerSummary::default(),
            vec![
                command("opt search:auto/int", -673, -11.324),
                command("opt read:range/int", 9_999, 99.99),
                command("opt read:outline/int", 10_000, 100.0),
            ],
        ));

        assert!(rendered.contains("-673"));
        assert!(rendered.contains("99.9%"));
        assert!(rendered.contains("100%"));
        assert!(!rendered.contains("99.99"));
        assert!(rendered.contains("PARTIAL"));

        let mut unknown_report = report(LedgerSummary::default(), Vec::new());
        unknown_report.direct_savings.operations = 0;
        unknown_report.direct_savings.input_tokens_estimated = 0;
        unknown_report.direct_savings.delivered_tokens_estimated = 0;
        unknown_report.direct_savings.net_avoided_tokens_estimated = 0;
        unknown_report.direct_savings.reduction_pct = 0.0;
        let unknown = render(&unknown_report);
        assert!(unknown.contains("ACCOUNTING UNKNOWN"));
        assert!(!unknown.contains("0 NET TOKEN CHANGE"));
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
        // The stage vocabulary is closed, so it renders as a stable short label rather than a
        // mid-word ellipsis. What the reader needs from it is whether the row is counted.
        assert!(rendered.contains("delivery"));
        assert!(
            rendered.contains("RAT = counted in the reduction ratio"),
            "a stage column is useless unless the reader learns which stages count"
        );
        assert!(rendered.contains("1 more mode/stage groups"));
        assert_aligned(&rendered);
    }

    /// Every column must bound its own cell.
    ///
    /// The mode table formatted its family cell as `{:<8}` with no truncation, so the
    /// 13-character `observability` shifted every column after it out of the frame. The gate
    /// existed; no fixture ever fed it a name long enough to fail. This one feeds every table
    /// the longest value its column can receive.
    #[test]
    fn acceptance_gate_no_cell_can_exceed_its_column() {
        let mut report = report(
            LedgerSummary::default(),
            vec![command(
                "an extremely long recorded command line that must not escape its column",
                4_000,
                62.5,
            )],
        );
        report.by_mode = vec![
            OperationModeSummary {
                operation: AccountingOperationKind::Observability,
                mode: AccountingOperationMode::ObservabilitySnapshot,
                stage: AccountingStage::ControlPlane,
                operations: 1,
                delivered_tokens_estimated: 150,
            },
            OperationModeSummary {
                operation: AccountingOperationKind::Search,
                mode: AccountingOperationMode::SearchSemantic,
                stage: AccountingStage::StandaloneDelivery,
                operations: 987_654_321,
                delivered_tokens_estimated: 987_654_321_000,
            },
        ];
        report.by_family = vec![OperationFamilySummary {
            family: "a-family-name-far-wider-than-its-column".into(),
            route: OperationRoute::NativeUnaccounted,
            operations: 987_654_321,
            delivered_tokens_estimated: 987_654_321_000,
            replacement_capability: ReplacementCapability::Unknown,
        }];
        report.by_subsystem = vec![SubsystemSavings {
            subsystem: "an-oversized-subsystem-label",
            operations: 987_654_321,
            gross_avoided_tokens_estimated: 987_654_321_000,
            regression_tokens_estimated: 0,
            net_avoided_tokens_estimated: 987_654_321_000,
            share_pct: 100.0,
        }];
        report.observed_model_usage = LedgerSummary {
            tasks: 987_654_321,
            accepted: 1,
            actual_input_tokens: 987_654_321_000,
            actual_output_tokens: 987_654_321_000,
            estimated_input_tokens: 987_654_321_000,
            cost_microusd: 987_654_321_000_000,
        };
        report.economics.rows = vec![EconomicScopeRow {
            scope: "global lifetime",
            scope_resolved: true,
            avoided_input_tokens_estimated: 987_654_321_000,
            potential_saved: Some(MoneyAmount {
                currency: "USD".into(),
                microunits: 987_654_321_000_000,
            }),
            billed_actual: Some(MoneyAmount {
                currency: "USD".into(),
                microunits: 987_654_321_000_000,
            }),
            billed_receipts: 3,
            notes: Vec::new(),
        }];

        assert_aligned(&render(&report));
    }

    /// A default view that hides recorded history must say so, on the same screen.
    ///
    /// 0.6.3 bumped the accounting-policy version, which removed every previously recorded
    /// operation from the default scope. The renderer went on printing `0 TOKENS AVOIDED /
    /// 0.0%` and never mentioned the exclusion, so a working install read as a dead one.
    #[test]
    fn acceptance_gate_excluded_history_is_disclosed_with_its_recovery() {
        let mut report = report(LedgerSummary::default(), Vec::new());
        report.direct_savings.net_avoided_tokens_estimated = 0;
        report.direct_savings.gross_avoided_tokens_estimated = 0;
        report.direct_savings.reduction_pct = 0.0;
        report.excluded_legacy_operations = 76_682;
        report.zero_reduction_cause = ZeroReductionCause::ExcludedHistory;

        let rendered = render(&report);

        assert!(
            rendered.contains("scope artifact"),
            "a zero produced by a scope boundary must not read as a measurement"
        );
        assert!(
            rendered.contains("76.7K"),
            "the excluded count must be shown"
        );
        assert!(
            rendered.contains("hzr stats --accounting-version all"),
            "the recovery command is the whole remedy and must be spelled out"
        );
        assert!(rendered.contains("EXCLUDED from every"));
        assert_aligned(&rendered);
    }

    /// A clean zero and a scope-artifact zero are different claims.
    #[test]
    fn acceptance_gate_a_zero_credit_scope_is_distinguished_from_excluded_history() {
        let mut report = report(LedgerSummary::default(), Vec::new());
        report.direct_savings.net_avoided_tokens_estimated = 0;
        report.direct_savings.reduction_pct = 0.0;
        report.excluded_legacy_operations = 0;
        report.zero_reduction_cause = ZeroReductionCause::OnlyZeroCreditOperations;

        let rendered = render(&report);

        assert!(rendered.contains("no savings"));
        assert!(rendered.contains("clean zero, not a lost measurement"));
        assert!(!rendered.contains("scope artifact"));
        assert_aligned(&rendered);
    }

    /// A re-run tax of zero must be a measurement, not an omission.
    ///
    /// Before this existed, a filtered result that made the model re-issue the same command cost
    /// real tokens and appeared nowhere: the second run looked like ordinary traffic. Reporting
    /// the zero explicitly is what distinguishes "measured none" from "never looked".
    #[test]
    fn acceptance_gate_rerun_tax_is_measured_rather_than_assumed() {
        let clean = report(LedgerSummary::default(), Vec::new());
        let rendered = render(&clean);
        assert!(rendered.contains("RERUN TAX: 0 measured repeats"));
        assert!(rendered.contains("measured, not assumed"));

        let mut taxed = report(LedgerSummary::default(), Vec::new());
        taxed.rerun_tax = RerunTax {
            operations: 12,
            tokens_estimated: 3_400,
            net_avoided_after_rerun_tax_estimated: 4_600,
            detection_window_operations: 8,
        };
        let rendered = render(&taxed);
        assert!(rendered.contains("RERUN TAX: 12 operation(s) / 3.4K token(s)"));
        assert!(
            rendered.contains("4.6K"),
            "the pessimistic reading must be stated, not left to the reader"
        );
        assert!(
            rendered.contains("the headline does not subtract it"),
            "a metric that shipped must not be silently redefined"
        );
        assert_aligned(&rendered);
    }

    /// The stage-excluded rows must reconcile the two panels that disagree without them.
    #[test]
    fn acceptance_gate_stage_excluded_rows_are_reported() {
        let mut report = report(LedgerSummary::default(), Vec::new());
        report.stage_exclusion = StageExclusion {
            operations: 205,
            delivered_tokens_estimated: 38_707,
        };

        let rendered = render(&report);

        assert!(rendered.contains("205"));
        assert!(rendered.contains("non-producer stages"));
        assert!(rendered.contains("explicit adapter delivery: unknown"));
        assert!(rendered.contains("host receipt/linkage unproven"));
        assert!(rendered.contains("double-count"));
    }

    /// Money is rendered for both scopes, and the two kinds of money never merge.
    #[test]
    fn acceptance_gate_economics_renders_both_scopes_without_summing_evidence() {
        let mut report = report(LedgerSummary::default(), Vec::new());
        report.economics = EconomicsReport {
            rows: vec![
                EconomicScopeRow {
                    scope: "this project",
                    scope_resolved: true,
                    avoided_input_tokens_estimated: 26_691_000,
                    potential_saved: Some(MoneyAmount {
                        currency: "USD".into(),
                        microunits: 133_455_000,
                    }),
                    billed_actual: None,
                    billed_receipts: 0,
                    notes: Vec::new(),
                },
                EconomicScopeRow {
                    scope: "global lifetime",
                    scope_resolved: true,
                    avoided_input_tokens_estimated: 252_654_876,
                    potential_saved: Some(MoneyAmount {
                        currency: "USD".into(),
                        microunits: 1_263_274_380,
                    }),
                    billed_actual: Some(MoneyAmount {
                        currency: "USD".into(),
                        microunits: 412_000,
                    }),
                    billed_receipts: 2,
                    notes: Vec::new(),
                },
            ],
            pricing: Some(PricingIdentity {
                harness: "claude_code".into(),
                provider: "anthropic".into(),
                model: "claude-opus-5".into(),
                method: "standard".into(),
                pricing_basis: "input".into(),
                price_table_identity: "hzr-public-api-pricing-2026-09-05-v1".into(),
                retrieved_at: "2026-09-05".into(),
            }),
            unavailable_reason: None,
            enable_steps: Vec::new(),
        };

        let rendered = render(&report);

        assert!(rendered.contains("ECONOMICS"));
        assert!(rendered.contains("this project"));
        assert!(rendered.contains("global lifetime"));
        assert!(rendered.contains("USD 1263.27"));
        assert!(rendered.contains("USD 0.41"));
        assert!(
            rendered.contains("not measured"),
            "a scope with no receipt must not render a currency zero"
        );
        assert!(rendered.contains("never summed"));
        assert!(
            rendered.find("ECONOMICS") < rendered.find("LOCAL OUTPUT REDUCTION"),
            "money belongs above the token headline, not at the bottom of the output"
        );
        assert_aligned(&rendered);
    }

    /// A disabled money view must state how to enable it, not merely that it is off.
    #[test]
    fn acceptance_gate_unavailable_pricing_names_the_steps_that_enable_it() {
        let mut report = report(LedgerSummary::default(), Vec::new());
        report.economics.unavailable_reason = Some("public pricing estimate is opt-in".into());
        report.economics.enable_steps = vec![
            "1. `hzr billing catalog` — find the exact harness/provider/model/method row",
            "2. set [billing] public_estimate_enabled = true in the HZR config",
        ];

        let rendered = render(&report);

        assert!(rendered.contains("potential value unavailable"));
        assert!(rendered.contains("hzr billing catalog"));
        assert!(rendered.contains("public_estimate_enabled = true"));
        assert!(
            rendered.contains("unavailable"),
            "an unpriced scope states that it is unpriced rather than showing a zero"
        );
        assert_aligned(&rendered);
    }
}
