use std::collections::BTreeMap;

use anyhow::Result;
use hzr_core::{Config, EfficiencySummary, Ledger, LedgerSummary};
use serde::Serialize;

use crate::hook_runner;

#[derive(Clone, Debug, Serialize)]
pub struct StatsReport {
    pub hzr_version: &'static str,
    pub scope: &'static str,
    pub direct_savings: DirectSavings,
    pub by_subsystem: Vec<SubsystemSavings>,
    pub by_command: Vec<CommandSavings>,
    pub observed_model_usage: LedgerSummary,
    pub degraded_rewrites: usize,
    pub runtime_accounting_complete: bool,
    pub economic_claim_ready: bool,
    pub notes: Vec<&'static str>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct DirectSavings {
    pub operations: u64,
    pub input_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub gross_avoided_tokens_estimated: u64,
    pub regression_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub reduction_pct: f64,
    pub total_execution_ms: u64,
    pub measurement: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct SubsystemSavings {
    pub subsystem: &'static str,
    pub operations: u64,
    pub gross_avoided_tokens_estimated: u64,
    pub regression_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub share_pct: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct CommandSavings {
    pub command: String,
    pub subsystem: &'static str,
    pub executions: u64,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub gross_avoided_tokens_estimated: u64,
    pub regression_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub avg_savings_pct: f64,
    pub avg_time_ms: u64,
}

pub fn collect(config: &Config) -> Result<StatsReport> {
    let ledger = Ledger::open(&config.data_dir.join("ledger/hzr.sqlite"))?;
    let gain = ledger.efficiency_summary()?;
    let observed_model_usage = ledger.summary()?;
    let degraded_rewrites = hook_runner::degraded_rewrite_count(config)?;
    Ok(build_report(gain, observed_model_usage, degraded_rewrites))
}

fn build_report(
    gain: EfficiencySummary,
    observed_model_usage: LedgerSummary,
    degraded_rewrites: usize,
) -> StatsReport {
    let commands = gain
        .by_command
        .into_iter()
        .map(|stats| CommandSavings {
            subsystem: classify_command(&stats.command),
            command: normalize_command(&stats.command),
            executions: stats.executions,
            baseline_tokens_estimated: stats.baseline_tokens_estimated,
            delivered_tokens_estimated: stats.delivered_tokens_estimated,
            gross_avoided_tokens_estimated: stats.gross_avoided_tokens_estimated,
            regression_tokens_estimated: stats.regression_tokens_estimated,
            net_avoided_tokens_estimated: stats.net_avoided_tokens_estimated,
            avg_savings_pct: signed_percentage(
                stats.net_avoided_tokens_estimated,
                stats.baseline_tokens_estimated,
            ),
            avg_time_ms: stats.avg_time_ms,
        })
        .collect::<Vec<_>>();
    let mut subsystems = BTreeMap::<&'static str, (u64, u64, u64, i64)>::new();
    for command in &commands {
        let totals = subsystems.entry(command.subsystem).or_default();
        totals.0 = totals.0.saturating_add(command.executions);
        totals.1 = totals
            .1
            .saturating_add(command.gross_avoided_tokens_estimated);
        totals.2 = totals.2.saturating_add(command.regression_tokens_estimated);
        totals.3 = totals
            .3
            .saturating_add(command.net_avoided_tokens_estimated);
    }
    let mut by_subsystem = subsystems
        .into_iter()
        .map(
            |(
                subsystem,
                (
                    operations,
                    gross_avoided_tokens_estimated,
                    regression_tokens_estimated,
                    net_avoided_tokens_estimated,
                ),
            )| SubsystemSavings {
                subsystem,
                operations,
                gross_avoided_tokens_estimated,
                regression_tokens_estimated,
                net_avoided_tokens_estimated,
                share_pct: signed_percentage(
                    net_avoided_tokens_estimated,
                    gain.net_avoided_tokens_estimated.max(0) as u64,
                ),
            },
        )
        .collect::<Vec<_>>();
    by_subsystem.sort_by(|left, right| {
        right
            .net_avoided_tokens_estimated
            .cmp(&left.net_avoided_tokens_estimated)
    });

    StatsReport {
        hzr_version: env!("CARGO_PKG_VERSION"),
        scope: "global_lifetime",
        direct_savings: DirectSavings {
            operations: gain.operations,
            input_tokens_estimated: gain.baseline_tokens_estimated,
            delivered_tokens_estimated: gain.delivered_tokens_estimated,
            gross_avoided_tokens_estimated: gain.gross_avoided_tokens_estimated,
            regression_tokens_estimated: gain.regression_tokens_estimated,
            net_avoided_tokens_estimated: gain.net_avoided_tokens_estimated,
            reduction_pct: signed_percentage(
                gain.net_avoided_tokens_estimated,
                gain.baseline_tokens_estimated,
            ),
            total_execution_ms: gain.total_execution_ms,
            measurement: "estimated_utf8_bytes_div_4_v1",
        },
        by_subsystem,
        by_command: commands,
        observed_model_usage,
        degraded_rewrites,
        runtime_accounting_complete: degraded_rewrites == 0,
        economic_claim_ready: false,
        notes: vec![
            "direct savings are estimated from before/after output size and never mixed with provider usage",
            "read, write, rgai/search, and command filters share the same cumulative HZR-owned history",
            "context selection, memory recall, and response contracts receive no savings credit without a measured counterfactual",
        ],
    }
}

fn classify_command(command: &str) -> &'static str {
    let command = command.to_ascii_lowercase();
    if command.contains("rtk write") {
        "write"
    } else if command.contains("rtk rgai")
        || command.contains("rtk grep")
        || command.contains("rtk rg")
    {
        "search"
    } else if command.contains("rtk read") || command.starts_with("cat ") {
        "read"
    } else if command.contains("rtk memory") {
        "memory"
    } else {
        "execution"
    }
}

fn normalize_command(command: &str) -> String {
    command
        .strip_prefix("rtk ")
        .map_or_else(|| command.to_owned(), |rest| format!("hzr {rest}"))
}

fn signed_percentage(part: i64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        part as f64 * 100.0 / total as f64
    }
}

#[cfg(test)]
mod tests {
    use hzr_core::LedgerSummary;

    use super::{build_report, classify_command, normalize_command};
    use hzr_core::{EfficiencyCommandSummary, EfficiencySummary};

    #[test]
    fn test_build_report_keeps_estimated_savings_separate_from_actual_usage() {
        let gain = EfficiencySummary {
            operations: 3,
            baseline_tokens_estimated: 1_000,
            delivered_tokens_estimated: 270,
            gross_avoided_tokens_estimated: 750,
            regression_tokens_estimated: 20,
            net_avoided_tokens_estimated: 730,
            total_execution_ms: 42,
            by_command: vec![
                EfficiencyCommandSummary {
                    command: "rtk write".into(),
                    executions: 1,
                    baseline_tokens_estimated: 400,
                    delivered_tokens_estimated: 100,
                    gross_avoided_tokens_estimated: 300,
                    regression_tokens_estimated: 0,
                    net_avoided_tokens_estimated: 300,
                    avg_time_ms: 4,
                },
                EfficiencyCommandSummary {
                    command: "rtk rgai".into(),
                    executions: 2,
                    baseline_tokens_estimated: 600,
                    delivered_tokens_estimated: 170,
                    gross_avoided_tokens_estimated: 450,
                    regression_tokens_estimated: 20,
                    net_avoided_tokens_estimated: 430,
                    avg_time_ms: 8,
                },
            ],
        };
        let usage = LedgerSummary {
            tasks: 2,
            actual_input_tokens: 900,
            actual_output_tokens: 100,
            ..LedgerSummary::default()
        };

        let report = build_report(gain, usage, 0);

        assert_eq!(report.direct_savings.net_avoided_tokens_estimated, 730);
        assert_eq!(report.direct_savings.regression_tokens_estimated, 20);
        assert_eq!(report.observed_model_usage.actual_input_tokens, 900);
        assert_eq!(report.by_subsystem.len(), 2);
        assert!(report.runtime_accounting_complete);
        assert!(!report.economic_claim_ready);
    }

    #[test]
    fn test_command_classification_covers_new_hzr_surfaces() {
        assert_eq!(classify_command("rtk read"), "read");
        assert_eq!(classify_command("rtk write"), "write");
        assert_eq!(classify_command("rtk rgai"), "search");
        assert_eq!(classify_command("rtk memory (hook)"), "memory");
        assert_eq!(classify_command("rtk cargo test"), "execution");
        assert_eq!(normalize_command("rtk rgai"), "hzr rgai");
    }
}
