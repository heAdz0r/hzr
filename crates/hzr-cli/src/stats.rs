use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use hzr_core::{
    BypassSummary, CURRENT_ACCOUNTING_POLICY_VERSION, Config, EfficiencySummary, EvasionSummary,
    Ledger, LedgerSummary, OperationChannel, OperationFamilySummary, OperationModeSummary,
    OperationRoute, ReadPipelineSummary, ReplacementCapability, StatsQuery, classify_operation,
    privacy_identity_hash,
};
use serde::Serialize;

use crate::cli::{AccountingVersion, StatsDuration};
use crate::hook_runner::{self, AccountingCoverage};

const DEFAULT_COMMAND_LIMIT: usize = 12;
const DEFAULT_BYPASS_TOOL_LIMIT: usize = 12;

pub fn validate_request_bounds(
    json: bool,
    include_all_commands: bool,
    has_workspace: bool,
    has_since: bool,
) -> Result<()> {
    if json && include_all_commands && !has_workspace && !has_since {
        anyhow::bail!(
            "unbounded `hzr stats --json --all` is refused; add `--since <duration>` or `--workspace <dir>`"
        );
    }
    Ok(())
}

struct ReportInputs {
    gain: EfficiencySummary,
    observed_model_usage: LedgerSummary,
    observed_model_usage_scope: &'static str,
    coverage: AccountingCoverage,
    bypass: BypassSummary,
    by_family: Vec<OperationFamilySummary>,
    evasion: Option<EvasionSummary>,
    scope: String,
    accounting_version: AccountingVersion,
}

struct ReportOptions {
    command_limit: Option<usize>,
    recovery: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct StatsReport {
    pub hzr_version: &'static str,
    pub scope: String,
    pub direct_savings: DirectSavings,
    pub by_subsystem: Vec<SubsystemSavings>,
    pub by_mode: Vec<OperationModeSummary>,
    pub read_pipeline: ReadPipelineSummary,
    pub accounting_version_scope: &'static str,
    pub accounting_policy_version: &'static str,
    pub excluded_legacy_operations: u64,
    /// Argument-free aggregation safe to retain and serialize even for sensitive commands.
    pub by_family: Vec<OperationFamilySummary>,
    /// Present only for the explicit `--evasion` view; always aggregate-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evasion: Option<EvasionSummary>,
    pub by_command: Vec<CommandSavings>,
    pub by_command_total: usize,
    pub by_command_omitted: usize,
    pub by_command_recovery: String,
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
    pub observability_scope: &'static str,
    pub completeness: &'static str,
    pub complete: bool,
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
    pub by_tool_total: usize,
    pub by_tool_omitted: usize,
    pub by_tool_recovery: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct BypassToolReport {
    pub tool: String,
    pub executions: u64,
    pub delivered_tokens_estimated: u64,
    pub example_command: String,
    /// The first-class HZR command that would have replaced the example, when one exists.
    pub replacement: Option<String>,
    pub replacement_capability: ReplacementCapability,
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

pub async fn collect(
    config: &Config,
    workspace: Option<&Path>,
    include_all_commands: bool,
    show_evasion: bool,
    since: Option<&StatsDuration>,
    accounting_version: AccountingVersion,
) -> Result<StatsReport> {
    let ledger_path = config.data_dir.join("ledger/hzr.sqlite");
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let cutoff = since
        .map(|duration| now.saturating_sub(duration.seconds()))
        .map(i64::try_from)
        .transpose()?;
    let workspace_text = workspace.map(|path| path.to_string_lossy());
    let workspace_identity = workspace_text
        .as_deref()
        .map(|value| privacy_identity_hash("project", value));
    let collection = Ledger::stats_collection_read_only(
        &ledger_path,
        StatsQuery {
            project_path: workspace_text.as_deref(),
            since_unix_seconds: cutoff,
            include_legacy_versions: accounting_version == AccountingVersion::All,
        },
    )?;
    let snapshot = collection.snapshot;
    let scope = match (workspace_identity.as_deref(), since) {
        (Some(workspace_hash), Some(duration)) => {
            format!("project {workspace_hash} since {}", duration.label())
        }
        (Some(workspace_hash), None) => format!("project {workspace_hash}"),
        (None, Some(duration)) => format!("global since {}", duration.label()),
        (None, None) => "global lifetime".to_owned(),
    };
    let observed_model_usage_scope = match (workspace.is_some(), since.is_some()) {
        (true, true) => "project_matched_window",
        (true, false) => "project_matched",
        (false, true) => "global_window",
        (false, false) => "global_lifetime",
    };
    let mut recovery = "hzr stats --json --all".to_owned();
    if workspace_text.is_some() {
        recovery.push_str(" --workspace <workspace>");
    }
    if let Some(duration) = since {
        recovery.push_str(&format!(" --since {}", duration.label()));
    } else if workspace_text.is_none() {
        recovery.push_str(" --since 7d");
    }
    let coverage = hook_runner::degraded_rewrite_coverage(config)?;
    Ok(build_report_with_command_limit(
        ReportInputs {
            gain: snapshot.efficiency,
            observed_model_usage: snapshot.provider_usage,
            observed_model_usage_scope,
            coverage,
            bypass: snapshot.bypass,
            by_family: snapshot.by_family,
            evasion: show_evasion.then_some(snapshot.evasion),
            scope,
            accounting_version,
        },
        ReportOptions {
            command_limit: (!include_all_commands).then_some(DEFAULT_COMMAND_LIMIT),
            recovery: Some(recovery),
        },
    ))
}

#[cfg(test)]
fn build_report(
    gain: EfficiencySummary,
    observed_model_usage: LedgerSummary,
    observed_model_usage_scope: &'static str,
    coverage: AccountingCoverage,
    bypass: BypassSummary,
    scope: String,
) -> StatsReport {
    build_report_with_command_limit(
        ReportInputs {
            gain,
            observed_model_usage,
            observed_model_usage_scope,
            coverage,
            bypass,
            by_family: Vec::new(),
            evasion: None,
            scope,
            accounting_version: AccountingVersion::Current,
        },
        ReportOptions {
            command_limit: Some(DEFAULT_COMMAND_LIMIT),
            recovery: None,
        },
    )
}

fn build_report_with_command_limit(inputs: ReportInputs, options: ReportOptions) -> StatsReport {
    let ReportInputs {
        gain,
        observed_model_usage,
        observed_model_usage_scope,
        coverage,
        bypass,
        by_family,
        evasion,
        scope,
        accounting_version,
    } = inputs;
    let ReportOptions {
        command_limit,
        recovery,
    } = options;
    let by_mode = gain.by_mode.clone();
    let reveal_command_details = false;
    let traffic_complete = coverage.complete
        && gain.total_observed_operations > 0
        && gain.native_unaccounted_operations == 0
        && gain.unmeasured_bypass_operations == 0;
    let traffic_completeness = if gain.total_observed_operations == 0 {
        "no_observed_operations"
    } else if !coverage.complete {
        "degraded_rewrite_gap"
    } else if gain.native_unaccounted_operations > 0 || gain.unmeasured_bypass_operations > 0 {
        "known_unmeasured_operations"
    } else {
        "observed_scope_complete"
    };
    let traffic_coverage = TrafficCoverage {
        observability_scope: "observed_channels_only",
        completeness: traffic_completeness,
        complete: traffic_complete,
        // The reduction ratio is computed only from measured, non-native rows. An
        // explicitly unmeasured bypass is known to the control plane, but it is not
        // evidence that the ratio covered that operation.
        accounted_operations: gain.operations,
        total_observed_operations: gain.total_observed_operations,
        native_unaccounted_operations: gain.native_unaccounted_operations,
        unmeasured_bypass_operations: gain.unmeasured_bypass_operations,
        accounted_share_pct: if gain.total_observed_operations == 0 {
            0.0
        } else {
            gain.operations as f64 * 100.0 / gain.total_observed_operations as f64
        },
        by_channel: with_explicit_mcp_channel(gain.by_channel.clone()),
    };
    let mut commands = gain
        .by_command
        .into_iter()
        .map(|stats| CommandSavings {
            subsystem: classify_command(&stats.command),
            command: command_label(&stats.command, reveal_command_details),
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

    let by_command_total = commands.len();
    if let Some(limit) = command_limit {
        commands.truncate(limit);
    }
    let by_command_omitted = by_command_total.saturating_sub(commands.len());
    let by_command_recovery = recovery.unwrap_or_else(|| {
        if scope == "global lifetime" {
            "hzr stats --json --all --since 7d".to_owned()
        } else {
            format!(
                "hzr stats --json --all --workspace {}",
                scope.trim_start_matches("project ")
            )
        }
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
        by_mode,
        read_pipeline: gain.read_pipeline,
        accounting_version_scope: match accounting_version {
            AccountingVersion::Current => "current_privacy_typed_policy",
            AccountingVersion::All => "all_versions_compatibility_only",
        },
        accounting_policy_version: CURRENT_ACCOUNTING_POLICY_VERSION,
        excluded_legacy_operations: gain.excluded_legacy_operations,
        by_family,
        evasion,
        by_command: commands,
        by_command_total,
        by_command_omitted,
        by_command_recovery: by_command_recovery.clone(),
        observed_model_usage,
        observed_model_usage_scope,
        bypass: bypass_report(bypass, reveal_command_details, by_command_recovery.clone()),
        traffic_coverage,
        degraded_rewrites: coverage.unreconciled_rewrites,
        coverage,
        runtime_accounting_complete: traffic_complete,
        economic_claim_ready: false,
        notes: provider_usage_notes(observed_model_usage_scope),
    }
}

fn provider_usage_notes(observed_model_usage_scope: &str) -> Vec<&'static str> {
    let mut notes = vec![
        "direct savings are estimated from before/after output size and never mixed with provider usage",
        "read, write, rgai/search, and command filters share the same HZR-owned ledger scope",
        "a bypassed operation delivers as many tokens as it consumed, so it cancels out of the reduction ratio instead of lowering it",
        "context selection, memory recall, and response contracts receive no savings credit without a measured counterfactual",
        "accounting completeness applies only to observed channels; a host-native tool without an installed observer is outside the denominator",
    ];
    notes.push(match observed_model_usage_scope {
        "project_matched" | "project_matched_window" => {
            "provider usage is scoped to receipts that carry a matching workspace identity; older unscoped receipts stay in the global lifetime view only"
        }
        "global_window" => {
            "provider usage is limited to receipts in the same requested time window"
        }
        _ => {
            "provider usage is the global lifetime total across scoped and legacy unscoped receipts"
        }
    });
    notes.push(
        "degraded-hook accounting coverage remains process-local and is not project- or time-window-filtered",
    );
    notes
}

fn bypass_report(
    bypass: BypassSummary,
    reveal_command_details: bool,
    recovery: String,
) -> BypassReport {
    let mut merged: BTreeMap<(String, ReplacementCapability), BypassToolReport> = BTreeMap::new();
    for tool in bypass.by_tool {
        let label = privacy_safe_family_label(&tool.tool);
        let replacement = tool.replacement.as_deref().and_then(safe_replacement_route);
        let key = (label.clone(), tool.replacement_capability);
        let entry = merged.entry(key).or_insert_with(|| BypassToolReport {
            tool: label.clone(),
            executions: 0,
            delivered_tokens_estimated: 0,
            example_command: format!("bypassed {label} <arguments omitted>"),
            replacement: replacement.clone(),
            replacement_capability: tool.replacement_capability,
            rationale: match tool.replacement_capability {
                ReplacementCapability::Available => {
                    Some("execution-time registry route available".to_owned())
                }
                ReplacementCapability::Unavailable => {
                    Some("execution-time registry found no HZR filter".to_owned())
                }
                ReplacementCapability::Unknown => None,
            },
        });
        entry.executions = entry.executions.saturating_add(tool.executions);
        entry.delivered_tokens_estimated = entry
            .delivered_tokens_estimated
            .saturating_add(tool.delivered_tokens_estimated);
        if entry.replacement.is_none() {
            entry.replacement = replacement;
        }
    }
    let mut by_tool = merged.into_values().collect::<Vec<_>>();
    // Merging reorders, and the report's contract is costliest leak first.
    by_tool.sort_by(|left, right| {
        right
            .delivered_tokens_estimated
            .cmp(&left.delivered_tokens_estimated)
            .then_with(|| left.tool.cmp(&right.tool))
    });
    let by_tool_total = by_tool.len();
    if !reveal_command_details {
        by_tool.truncate(DEFAULT_BYPASS_TOOL_LIMIT);
    }
    let by_tool_omitted = by_tool_total.saturating_sub(by_tool.len());

    BypassReport {
        operations: bypass.lifetime.operations,
        total_operations: bypass.lifetime.total_operations,
        operation_share_pct: bypass.lifetime.operation_share_pct(),
        delivered_tokens_estimated: bypass.lifetime.delivered_tokens_estimated,
        total_delivered_tokens_estimated: bypass.lifetime.total_delivered_tokens_estimated,
        token_share_pct: bypass.lifetime.token_share_pct(),
        by_tool,
        by_tool_total,
        by_tool_omitted,
        by_tool_recovery: recovery,
    }
}

fn privacy_safe_family_label(tool: &str) -> String {
    let valid = !tool.is_empty()
        && tool.len() <= 48
        && tool
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
    if valid {
        tool.to_owned()
    } else {
        "other".to_owned()
    }
}

fn safe_replacement_route(route: &str) -> Option<String> {
    let generic_exec = route
        .strip_prefix("hzr exec run '<")
        .and_then(|route| route.strip_suffix(">'"))
        .is_some_and(|route| {
            !route.is_empty()
                && route.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'_' | b'-' | b'.')
                })
        });
    let static_route = route.starts_with("hzr ")
        && !route.bytes().any(|byte| {
            byte.is_ascii_control()
                || matches!(byte, b'=' | b'/' | b'\\' | b'\'' | b'"' | b';' | b'$')
        });
    let valid = route.len() <= 160 && (generic_exec || static_route);
    valid.then(|| route.to_owned())
}

/// Route one recorded command to its subsystem through the shared classifier.
fn classify_command(command: &str) -> &'static str {
    classify_operation(command).subsystem.as_str()
}

fn command_label(command: &str, reveal_command_details: bool) -> String {
    let classification = classify_operation(command);
    if classification.route == OperationRoute::Bypassed {
        return format!(
            "hzr raw {} <arguments omitted>",
            privacy_safe_tool(&classification.operation)
        );
    }
    let _ = reveal_command_details;
    format!(
        "hzr {} <arguments omitted>",
        classification.subsystem.as_str()
    )
}

fn privacy_safe_tool(tool: &str) -> &'static str {
    match tool {
        "read" => "read",
        "search" | "rgai" | "rg" | "grep" => "search",
        "write" => "write",
        "memory" => "memory",
        "codec" => "codec",
        "git" => "git",
        "cargo" | "rustc" | "rustup" => "rust",
        "sed" | "cat" | "find" | "fd" | "awk" => "file",
        "python" | "python3" => "python",
        "sh" | "bash" | "zsh" => "shell",
        "ssh" => "ssh",
        "gh" => "gh",
        "bun" | "npm" | "pnpm" | "yarn" | "node" | "deno" => "javascript",
        "docker" | "podman" => "container",
        "curl" | "wget" => "http",
        _ => "other",
    }
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

    use super::{
        AccountingVersion, DEFAULT_BYPASS_TOOL_LIMIT, DEFAULT_COMMAND_LIMIT, ReportInputs,
        ReportOptions, build_report, build_report_with_command_limit, classify_command,
    };
    use crate::hook_runner::AccountingCoverage;
    use hzr_core::{
        BypassSummary, BypassTool, BypassWindow, EfficiencyCommandSummary, EfficiencySummary,
        OperationModeSummary, ReplacementCapability,
    };
    use hzr_protocol::{AccountingOperationKind, AccountingOperationMode, AccountingStage};

    #[test]
    fn test_build_report_keeps_estimated_savings_separate_from_actual_usage() {
        let gain = EfficiencySummary {
            operations: 3,
            total_observed_operations: 3,
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
    fn test_report_exposes_typed_internal_and_final_mode_attribution() {
        let report = build_report(
            EfficiencySummary {
                by_mode: vec![OperationModeSummary {
                    operation: AccountingOperationKind::Search,
                    mode: AccountingOperationMode::SearchExact,
                    stage: AccountingStage::FinalDelivery,
                    operations: 2,
                    delivered_tokens_estimated: 8,
                }],
                ..EfficiencySummary::default()
            },
            LedgerSummary::default(),
            "global_lifetime",
            AccountingCoverage::default_complete(),
            BypassSummary::default(),
            "global lifetime".into(),
        );

        assert_eq!(report.by_mode.len(), 1);
        let encoded = serde_json::to_string(&report).expect("stats JSON");
        assert!(encoded.contains("search_exact"));
        assert!(encoded.contains("final_delivery"));
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
                replacement_capability: ReplacementCapability::Available,
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
            None,
            "a route containing a concrete path is not retained in the privacy-safe report"
        );
        assert_eq!(
            report.bypass.by_tool[0].replacement_capability,
            ReplacementCapability::Available
        );
    }

    /// Distinct privacy-safe command families and capability states must remain distinct.
    #[test]
    fn acceptance_gate_bypass_rows_preserve_truthful_family_identity() {
        let bypass = BypassSummary {
            lifetime: BypassWindow {
                operations: 78,
                total_operations: 100,
                delivered_tokens_estimated: 900,
                total_delivered_tokens_estimated: 1_000,
            },
            by_tool: vec![
                BypassTool {
                    tool: "rg".into(),
                    executions: 23,
                    delivered_tokens_estimated: 300,
                    example_command: "rtk proxy rg -n TODO".into(),
                    replacement: None,
                    replacement_capability: ReplacementCapability::Unknown,
                    rationale: None,
                },
                BypassTool {
                    tool: "grep".into(),
                    executions: 55,
                    delivered_tokens_estimated: 600,
                    example_command: "rtk proxy grep -rn TODO".into(),
                    replacement: Some("hzr search 'TODO' --mode exact".into()),
                    replacement_capability: ReplacementCapability::Available,
                    rationale: Some("hzr search returns ranked matches".into()),
                },
            ],
        };

        let report = build_report(
            EfficiencySummary::default(),
            LedgerSummary::default(),
            "global_lifetime",
            AccountingCoverage::default_complete(),
            bypass,
            "global lifetime".into(),
        );

        assert_eq!(report.bypass.by_tool.len(), 2);
        let grep = report
            .bypass
            .by_tool
            .iter()
            .find(|tool| tool.tool == "grep")
            .expect("grep family");
        assert_eq!(
            grep.replacement_capability,
            ReplacementCapability::Available
        );
        let rg = report
            .bypass
            .by_tool
            .iter()
            .find(|tool| tool.tool == "rg")
            .expect("rg family");
        assert_eq!(rg.replacement_capability, ReplacementCapability::Unknown);
    }

    /// A redacted historical row must remain unknown instead of being guessed from its label.
    #[test]
    fn acceptance_gate_a_redacted_bypass_remains_unknown() {
        let bypass = BypassSummary {
            lifetime: BypassWindow::default(),
            by_tool: vec![BypassTool {
                tool: "search".into(),
                executions: 91,
                delivered_tokens_estimated: 0,
                example_command: "rtk proxy search <redacted>".into(),
                replacement: None,
                replacement_capability: ReplacementCapability::Unknown,
                rationale: None,
            }],
        };

        let report = build_report(
            EfficiencySummary::default(),
            LedgerSummary::default(),
            "global_lifetime",
            AccountingCoverage::default_complete(),
            bypass,
            "global lifetime".into(),
        );

        assert_eq!(report.bypass.by_tool.len(), 1);
        assert_eq!(
            report.bypass.by_tool[0].replacement_capability,
            ReplacementCapability::Unknown
        );
        assert!(report.bypass.by_tool[0].replacement.is_none());
    }

    #[test]
    fn test_default_report_redacts_unbounded_command_details() {
        let sensitive_payload = "secret=value\n".repeat(40);
        let gain = EfficiencySummary {
            by_command: vec![EfficiencyCommandSummary {
                command: format!("rtk rgai {sensitive_payload}"),
                executions: 1,
                baseline_tokens_estimated: 10,
                delivered_tokens_estimated: 5,
                gross_avoided_tokens_estimated: 5,
                regression_tokens_estimated: 0,
                net_avoided_tokens_estimated: 5,
                avg_time_ms: 1,
            }],
            ..EfficiencySummary::default()
        };
        let bypass = BypassSummary {
            lifetime: BypassWindow {
                operations: 1,
                total_operations: 1,
                delivered_tokens_estimated: 5,
                total_delivered_tokens_estimated: 5,
            },
            by_tool: vec![BypassTool {
                tool: "sed".into(),
                executions: 1,
                delivered_tokens_estimated: 5,
                example_command: format!("rtk proxy sed {sensitive_payload}"),
                replacement: Some(format!("hzr rtk -- read {sensitive_payload}")),
                replacement_capability: ReplacementCapability::Available,
                rationale: Some("bounded read".into()),
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

        assert_eq!(
            report.by_command[0].command,
            "hzr search <arguments omitted>"
        );
        assert_eq!(
            report.bypass.by_tool[0].example_command,
            "bypassed sed <arguments omitted>"
        );
        let encoded = serde_json::to_string(&report).expect("report JSON");
        assert!(!encoded.contains("secret=value"));
    }

    #[test]
    fn acceptance_gate_all_json_never_exposes_sensitive_payload_classes() {
        for sentinel in [
            "secret=value",
            "/private/customer/file.rs",
            "SELECT * FROM customer_secrets",
            "python3 -c 'print(credential)'",
            "<<HEREDOC private-body HEREDOC",
        ] {
            let report = build_report_with_command_limit(
                ReportInputs {
                    gain: EfficiencySummary {
                        by_command: vec![EfficiencyCommandSummary {
                            command: format!("rtk raw python3 {sentinel}"),
                            executions: 1,
                            baseline_tokens_estimated: 4,
                            delivered_tokens_estimated: 4,
                            gross_avoided_tokens_estimated: 0,
                            regression_tokens_estimated: 0,
                            net_avoided_tokens_estimated: 0,
                            avg_time_ms: 1,
                        }],
                        ..EfficiencySummary::default()
                    },
                    observed_model_usage: LedgerSummary::default(),
                    observed_model_usage_scope: "global_lifetime",
                    coverage: AccountingCoverage::default_complete(),
                    bypass: BypassSummary {
                        lifetime: BypassWindow {
                            operations: 1,
                            total_operations: 1,
                            delivered_tokens_estimated: 4,
                            total_delivered_tokens_estimated: 4,
                        },
                        by_tool: vec![BypassTool {
                            tool: sentinel.into(),
                            executions: 1,
                            delivered_tokens_estimated: 4,
                            example_command: sentinel.into(),
                            replacement: Some(sentinel.into()),
                            replacement_capability: ReplacementCapability::Available,
                            rationale: Some(sentinel.into()),
                        }],
                    },
                    by_family: Vec::new(),
                    evasion: None,
                    scope: "global lifetime".into(),
                    accounting_version: AccountingVersion::Current,
                },
                ReportOptions {
                    command_limit: None,
                    recovery: None,
                },
            );
            let encoded = serde_json::to_string(&report).expect("--all JSON");
            assert!(!encoded.contains(sentinel), "stats leaked {sentinel}");
        }
    }

    #[test]
    fn acceptance_gate_unbounded_all_json_is_refused_with_bounded_alternatives() {
        let error = super::validate_request_bounds(true, true, false, false)
            .expect_err("unbounded all JSON must be refused");
        let message = error.to_string();
        assert!(message.contains("--since <duration>"));
        assert!(message.contains("--workspace <dir>"));
        super::validate_request_bounds(true, true, true, false).expect("workspace bound");
        super::validate_request_bounds(true, true, false, true).expect("time bound");
        super::validate_request_bounds(false, true, false, false).expect("human view is bounded");
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

    #[test]
    fn test_default_report_bounds_command_history_and_names_recovery() {
        let by_command = (0..75)
            .map(|index| EfficiencyCommandSummary {
                command: format!("rtk command-{index}"),
                executions: 1,
                baseline_tokens_estimated: 10,
                delivered_tokens_estimated: 5,
                gross_avoided_tokens_estimated: 5,
                regression_tokens_estimated: 0,
                net_avoided_tokens_estimated: 5,
                avg_time_ms: 1,
            })
            .collect();
        let report = build_report(
            EfficiencySummary {
                by_command,
                ..EfficiencySummary::default()
            },
            LedgerSummary::default(),
            "global_lifetime",
            AccountingCoverage::default_complete(),
            BypassSummary::default(),
            "global lifetime".into(),
        );

        assert_eq!(report.by_command.len(), DEFAULT_COMMAND_LIMIT);
        assert_eq!(report.by_command_total, 75);
        assert_eq!(report.by_command_omitted, 75 - DEFAULT_COMMAND_LIMIT);
        assert_eq!(
            report.by_command_recovery,
            "hzr stats --json --all --since 7d"
        );
    }

    /// Identities whose privacy-safe labels are all distinct, so truncation is still exercised.
    const NAMED_TOOLS: [&str; 15] = [
        "read", "search", "write", "memory", "codec", "git", "cargo", "sed", "python3", "bash",
        "ssh", "gh", "bun", "docker", "curl",
    ];

    #[test]
    fn test_default_report_bounds_bypass_tools_and_total_json_cost() {
        let bypass = BypassSummary {
            lifetime: BypassWindow {
                operations: 75,
                total_operations: 75,
                delivered_tokens_estimated: 750,
                total_delivered_tokens_estimated: 750,
            },
            // Sixteen identities that carry distinct privacy-safe labels, plus a long tail of
            // unrecognized ones that all share the "other" label.
            by_tool: NAMED_TOOLS
                .iter()
                .map(|tool| (*tool).to_owned())
                .chain((0..59).map(|index| format!("tool-{index}")))
                .map(|tool| BypassTool {
                    executions: 1,
                    delivered_tokens_estimated: 10,
                    example_command: format!("rtk proxy {tool} secret=value"),
                    tool,
                    replacement: None,
                    replacement_capability: ReplacementCapability::Unknown,
                    rationale: None,
                })
                .collect(),
        };
        let report = build_report(
            EfficiencySummary::default(),
            LedgerSummary::default(),
            "global_lifetime",
            AccountingCoverage::default_complete(),
            bypass,
            "global lifetime".into(),
        );

        assert_eq!(report.bypass.by_tool_total, NAMED_TOOLS.len() + 59);
        assert_eq!(report.bypass.by_tool.len(), DEFAULT_BYPASS_TOOL_LIMIT);
        assert_eq!(
            report.bypass.by_tool_omitted,
            NAMED_TOOLS.len() + 59 - DEFAULT_BYPASS_TOOL_LIMIT
        );
        let mut labels = report
            .bypass
            .by_tool
            .iter()
            .map(|tool| tool.tool.as_str())
            .collect::<Vec<_>>();
        labels.sort_unstable();
        let unique = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), unique, "a label may appear at most once");
        assert_eq!(
            report.bypass.by_tool_recovery,
            "hzr stats --json --all --since 7d"
        );
        let encoded = serde_json::to_vec(&report).expect("report JSON");
        assert!(
            encoded.len() / 4 < 4_000,
            "default report exceeded the 4,000-token estimate: {} bytes",
            encoded.len()
        );
        assert!(!encoded.windows(12).any(|window| window == b"secret=value"));
    }
}
