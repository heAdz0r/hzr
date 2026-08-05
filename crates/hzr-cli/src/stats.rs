use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use hzr_core::{
    BypassSummary, Config, EfficiencySummary, Ledger, LedgerSummary, OperationChannel,
    classify_operation,
};
use serde::Serialize;

use crate::hook_runner::{self, AccountingCoverage};

#[derive(Clone, Debug, Serialize)]
pub struct StatsReport {
    pub hzr_version: &'static str,
    pub scope: String,
    pub direct_savings: DirectSavings,
    pub by_subsystem: Vec<SubsystemSavings>,
    pub by_command: Vec<CommandSavings>,
    pub observed_model_usage: LedgerSummary,
    pub observed_model_usage_scope: &'static str,
    /// Operations that skipped the optimizer. Reported next to the headline ratio because
    /// a bypassed row cancels out of that ratio instead of lowering it.
    pub bypass: BypassReport,
    pub traffic_coverage: TrafficCoverage,
    pub degraded_rewrites: usize,
    /// Full accounting-coverage state: the open gap, the historical total, and when the
    /// last gap occurred. `degraded_rewrites` above is the open gap alone, retained for
    /// callers that already read it.
    pub coverage: AccountingCoverage,
    pub runtime_accounting_complete: bool,
    pub economic_claim_ready: bool,
    pub notes: Vec<&'static str>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct TrafficCoverage {
    pub accounted_operations: u64,
    pub total_observed_operations: u64,
    pub native_unaccounted_operations: u64,
    pub unmeasured_bypass_operations: u64,
    pub accounted_share_pct: f64,
    pub by_channel: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct BypassReport {
    pub operations: u64,
    pub total_operations: u64,
    pub operation_share_pct: f64,
    pub delivered_tokens_estimated: u64,
    pub total_delivered_tokens_estimated: u64,
    pub token_share_pct: f64,
    pub by_tool: Vec<BypassToolReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BypassToolReport {
    pub tool: String,
    pub executions: u64,
    pub delivered_tokens_estimated: u64,
    pub example_command: String,
    /// The first-class HZR command that would have replaced the example, when one exists.
    pub replacement: Option<String>,
    pub rationale: Option<String>,
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

pub fn collect(config: &Config, workspace: Option<&Path>) -> Result<StatsReport> {
    let ledger = Ledger::open(&config.data_dir.join("ledger/hzr.sqlite"))?;
    let (gain, bypass, scope, observed_model_usage, observed_model_usage_scope) = match workspace {
        Some(workspace) => {
            let workspace = workspace.to_string_lossy();
            (
                ledger.efficiency_summary_for_project(&workspace)?,
                ledger.bypass_summary_for_project(&workspace)?,
                format!("project {}", workspace),
                ledger.summary_for_project(&workspace)?,
                "project_matched",
            )
        }
        None => (
            ledger.efficiency_summary()?,
            ledger.bypass_summary()?,
            "global lifetime".to_owned(),
            ledger.summary()?,
            "global_lifetime",
        ),
    };
    let coverage = hook_runner::degraded_rewrite_coverage(config)?;
    Ok(build_report(
        gain,
        observed_model_usage,
        observed_model_usage_scope,
        coverage,
        bypass,
        scope,
    ))
}

fn build_report(
    gain: EfficiencySummary,
    observed_model_usage: LedgerSummary,
    observed_model_usage_scope: &'static str,
    coverage: AccountingCoverage,
    bypass: BypassSummary,
    scope: String,
) -> StatsReport {
    let traffic_coverage = TrafficCoverage {
        // The reduction ratio is computed only from measured, non-native rows. An
        // explicitly unmeasured bypass is known to the control plane, but it is not
        // evidence that the ratio covered that operation.
        accounted_operations: gain.operations,
        total_observed_operations: gain.total_observed_operations,
        native_unaccounted_operations: gain.native_unaccounted_operations,
        unmeasured_bypass_operations: gain.unmeasured_bypass_operations,
        accounted_share_pct: if gain.total_observed_operations == 0 {
            100.0
        } else {
            gain.operations as f64 * 100.0 / gain.total_observed_operations as f64
        },
        by_channel: with_explicit_mcp_channel(gain.by_channel.clone()),
    };
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
        scope,
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
        observed_model_usage_scope,
        bypass: bypass_report(bypass),
        traffic_coverage,
        degraded_rewrites: coverage.unreconciled_rewrites,
        coverage,
        runtime_accounting_complete: coverage.complete,
        economic_claim_ready: false,
        notes: provider_usage_notes(observed_model_usage_scope),
    }
}

fn provider_usage_notes(observed_model_usage_scope: &str) -> Vec<&'static str> {
    let mut notes = vec![
        "direct savings are estimated from before/after output size and never mixed with provider usage",
        "read, write, rgai/search, and command filters share the same cumulative HZR-owned history",
        "a bypassed operation delivers as many tokens as it consumed, so it cancels out of the reduction ratio instead of lowering it",
        "context selection, memory recall, and response contracts receive no savings credit without a measured counterfactual",
    ];
    notes.push(match observed_model_usage_scope {
        "project_matched" => {
            "provider usage is scoped to receipts that carry a matching workspace identity; older unscoped receipts stay in the global lifetime view only"
        }
        _ => {
            "provider usage is the global lifetime total across scoped and legacy unscoped receipts"
        }
    });
    notes.push(
        "degraded-hook accounting coverage remains process-local and is not project-filtered",
    );
    notes
}

fn bypass_report(bypass: BypassSummary) -> BypassReport {
    BypassReport {
        operations: bypass.lifetime.operations,
        total_operations: bypass.lifetime.total_operations,
        operation_share_pct: bypass.lifetime.operation_share_pct(),
        delivered_tokens_estimated: bypass.lifetime.delivered_tokens_estimated,
        total_delivered_tokens_estimated: bypass.lifetime.total_delivered_tokens_estimated,
        token_share_pct: bypass.lifetime.token_share_pct(),
        by_tool: bypass
            .by_tool
            .into_iter()
            .map(|tool| BypassToolReport {
                tool: tool.tool,
                executions: tool.executions,
                delivered_tokens_estimated: tool.delivered_tokens_estimated,
                example_command: tool.example_command,
                replacement: tool.replacement,
                rationale: tool.rationale,
            })
            .collect(),
    }
}

/// Route one recorded command to its subsystem through the shared classifier.
fn classify_command(command: &str) -> &'static str {
    classify_operation(command).subsystem.as_str()
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

/// Гарантирует ключ `mcp` в channel split: отсутствие трафика — явный 0, а не «канал не учтён».
pub(crate) fn with_explicit_mcp_channel(
    mut by_channel: BTreeMap<String, u64>,
) -> BTreeMap<String, u64> {
    by_channel
        .entry(OperationChannel::Mcp.as_str().to_owned())
        .or_insert(0);
    by_channel
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use hzr_core::LedgerSummary;

    use super::{build_report, classify_command, normalize_command};
    use crate::hook_runner::AccountingCoverage;
    use hzr_core::{
        BypassSummary, BypassTool, BypassWindow, EfficiencyCommandSummary, EfficiencySummary,
    };

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
            ..EfficiencySummary::default()
        };
        let usage = LedgerSummary {
            tasks: 2,
            actual_input_tokens: 900,
            actual_output_tokens: 100,
            ..LedgerSummary::default()
        };

        let report = build_report(
            gain,
            usage,
            "global_lifetime",
            AccountingCoverage::default_complete(),
            BypassSummary::default(),
            "global lifetime".into(),
        );

        assert_eq!(report.direct_savings.net_avoided_tokens_estimated, 730);
        assert_eq!(report.direct_savings.regression_tokens_estimated, 20);
        assert_eq!(report.observed_model_usage.actual_input_tokens, 900);
        assert_eq!(report.observed_model_usage_scope, "global_lifetime");
        assert_eq!(report.by_subsystem.len(), 2);
        assert!(report.runtime_accounting_complete);
        assert!(!report.economic_claim_ready);
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("never mixed with provider usage"))
        );
    }

    #[test]
    fn test_project_scoped_report_labels_matched_provider_usage() {
        let report = build_report(
            EfficiencySummary::default(),
            LedgerSummary {
                tasks: 1,
                actual_input_tokens: 40,
                ..LedgerSummary::default()
            },
            "project_matched",
            AccountingCoverage::default_complete(),
            BypassSummary::default(),
            "project /work/a".into(),
        );

        assert_eq!(report.observed_model_usage_scope, "project_matched");
        assert_eq!(report.observed_model_usage.actual_input_tokens, 40);
        assert!(report.notes.iter().any(|note| {
            note.contains("matching workspace identity") && note.contains("unscoped")
        }));
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

    /// A bypassed command must never be counted as an optimized execution: that is exactly
    /// how thousands of `sed`/`rg` invocations hid inside the `execution` subsystem while
    /// contributing zero savings.
    #[test]
    fn test_bypassed_commands_leave_the_execution_bucket() {
        assert_eq!(
            classify_command("rtk proxy sed -n 1,80p src/lib.rs"),
            "bypass"
        );
        assert_eq!(classify_command("rtk proxy cargo test"), "bypass");
        assert_eq!(classify_command("rtk fallback: grep -rn needle"), "bypass");
    }

    #[test]
    fn test_report_states_the_bypass_share_and_its_replacements() {
        let gain = EfficiencySummary {
            operations: 2,
            baseline_tokens_estimated: 1_000,
            delivered_tokens_estimated: 900,
            gross_avoided_tokens_estimated: 100,
            regression_tokens_estimated: 0,
            net_avoided_tokens_estimated: 100,
            total_execution_ms: 10,
            by_command: Vec::new(),
            ..EfficiencySummary::default()
        };
        let bypass = BypassSummary {
            lifetime: BypassWindow {
                operations: 3,
                total_operations: 8,
                delivered_tokens_estimated: 600,
                total_delivered_tokens_estimated: 1_000,
            },
            by_tool: vec![BypassTool {
                tool: "sed".into(),
                executions: 3,
                delivered_tokens_estimated: 600,
                example_command: "rtk proxy sed -n 1,80p src/lib.rs".into(),
                replacement: Some("hzr rtk -- read src/lib.rs --from 1 --to 80".into()),
                rationale: Some("hzr read streams the requested span".into()),
            }],
        };

        let report = build_report(
            gain,
            LedgerSummary::default(),
            "global_lifetime",
            AccountingCoverage::default_complete(),
            bypass,
            "global lifetime".into(),
        );

        assert_eq!(report.bypass.operations, 3);
        assert_eq!(report.bypass.operation_share_pct.round(), 38.0);
        assert_eq!(report.bypass.token_share_pct.round(), 60.0);
        assert_eq!(report.bypass.by_tool.len(), 1);
        assert_eq!(
            report.bypass.by_tool[0].replacement.as_deref(),
            Some("hzr rtk -- read src/lib.rs --from 1 --to 80")
        );
    }

    /// The headline ratio is honest only when it is read next to the bypass share, so the
    /// report must never omit the second number.
    #[test]
    fn test_a_clean_ledger_reports_a_zero_bypass_share_rather_than_nothing() {
        let report = build_report(
            EfficiencySummary::default(),
            LedgerSummary::default(),
            "global_lifetime",
            AccountingCoverage::default_complete(),
            BypassSummary::default(),
            "global lifetime".into(),
        );

        assert_eq!(report.bypass.operations, 0);
        assert_eq!(report.bypass.operation_share_pct, 0.0);
        assert!(report.bypass.by_tool.is_empty());
    }

    /// Absent MCP traffic must still appear as an explicit zero so JSON consumers never
    /// confuse a missing key with "MCP is outside the channel split."
    #[test]
    fn test_channel_split_always_includes_explicit_mcp_zero() {
        let mut by_channel = BTreeMap::new();
        by_channel.insert("hook_cli".into(), 4);
        by_channel.insert("native_host".into(), 1);
        let gain = EfficiencySummary {
            operations: 5,
            total_observed_operations: 5,
            by_channel,
            ..EfficiencySummary::default()
        };

        let report = build_report(
            gain,
            LedgerSummary::default(),
            "global_lifetime",
            AccountingCoverage::default_complete(),
            BypassSummary::default(),
            "global lifetime".into(),
        );

        assert_eq!(
            report.traffic_coverage.by_channel.get("mcp"),
            Some(&0),
            "mcp must be present as 0 when the ledger recorded no MCP rows"
        );
        assert_eq!(report.traffic_coverage.by_channel.get("hook_cli"), Some(&4));
        assert_eq!(
            report.traffic_coverage.by_channel.get("native_host"),
            Some(&1)
        );
    }

    #[test]
    fn test_empty_ledger_still_exposes_mcp_zero_in_channel_split() {
        let report = build_report(
            EfficiencySummary::default(),
            LedgerSummary::default(),
            "global_lifetime",
            AccountingCoverage::default_complete(),
            BypassSummary::default(),
            "global lifetime".into(),
        );

        assert_eq!(report.traffic_coverage.by_channel.get("mcp"), Some(&0));
    }
}
