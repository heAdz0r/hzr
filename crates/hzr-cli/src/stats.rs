use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use hzr_core::{
    BypassSummary, CURRENT_ACCOUNTING_POLICY_VERSION, Config, EconomicScopeSummary,
    EfficiencySummary, EvasionSummary, HostVisibleEfficiencySummary, Ledger, LedgerSummary,
    OperationChannel, OperationFamilySummary, OperationModeSummary, OperationRoute, PricingCatalog,
    PrivacySafeOperationKey, RawPublicEstimate, RawPublicEstimateRequest, ReadPipelineSummary,
    ReplacementCapability, StatsQuery, load_pricing_catalog, price_avoided_input_tokens,
    privacy_identity_hash,
};
use hzr_protocol::{AccountingOperationKind, AccountingOperationMode, AccountingStage};
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

/// Money for the two scopes an operator actually compares.
///
/// The block exists because 0.6.3 could price only whatever scope the command happened to ask
/// for, and a single number answers neither "what is this repository costing me" nor "what has
/// HZR saved overall". Both rows are always rendered, including when a scope has nothing to
/// show — an absent row reads as zero, and zero is a claim.
#[derive(Clone, Debug, Serialize)]
pub struct EconomicsReport {
    pub rows: Vec<EconomicScopeRow>,
    /// The exact catalog selection every priced row used, stated once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pricing: Option<PricingIdentity>,
    /// Why no row could be priced. Present exactly when `pricing` is absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    /// Steps that would make pricing available, rendered verbatim next to the reason.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub enable_steps: Vec<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EconomicScopeRow {
    pub scope: &'static str,
    /// False when the scope could not be resolved — a cwd outside any registered worktree.
    pub scope_resolved: bool,
    pub avoided_input_tokens_estimated: u64,
    /// Preliminary public-list value of `avoided_input_tokens_estimated`. Never an invoice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub potential_saved: Option<MoneyAmount>,
    /// Sum of imported provider receipts for this scope. Never summed with `potential_saved`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billed_actual: Option<MoneyAmount>,
    pub billed_receipts: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MoneyAmount {
    pub currency: String,
    pub microunits: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct PricingIdentity {
    pub harness: String,
    pub provider: String,
    pub model: String,
    pub method: String,
    pub pricing_basis: String,
    pub price_table_identity: String,
    pub retrieved_at: String,
}

/// Why a headline of `0` is on screen.
///
/// A reduction of zero has three very different meanings and 0.6.3 rendered all of them as the
/// same `0.0%`. The renderer needs to distinguish them, so the classification is computed once
/// here rather than re-derived from loose fields at print time.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZeroReductionCause {
    /// Not zero at all.
    NotZero,
    /// Recorded history exists but sits outside the selected accounting-policy scope.
    ExcludedHistory,
    /// Every operation in scope belongs to a class that earns no savings credit by policy.
    OnlyZeroCreditOperations,
    /// The ledger genuinely holds nothing for this scope.
    NoOperations,
}

struct ReportInputs {
    gain: EfficiencySummary,
    host_visible_gain: HostVisibleEfficiencySummary,
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
    pub explicit_delivery: hzr_core::DeliverySummary,
    pub hzr_version: &'static str,
    pub scope: String,
    pub direct_savings: DirectSavings,
    pub host_visible_savings: HostVisibleSavings,
    pub by_subsystem: Vec<SubsystemSavings>,
    pub by_mode: Vec<OperationModeSummary>,
    pub read_pipeline: ReadPipelineSummary,
    pub accounting_version_scope: &'static str,
    pub accounting_policy_version: &'static str,
    pub excluded_legacy_operations: u64,
    /// Rows `by_mode` shows and the reduction ratio deliberately does not measure.
    pub stage_exclusion: StageExclusion,
    /// Repeats of an already-filtered command, measured rather than assumed zero.
    pub rerun_tax: RerunTax,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_public_estimate: Option<RawPublicEstimate>,
    pub raw_public_estimate_unavailable_reason: Option<String>,
    /// Per-project and global money, rendered above the reduction headline.
    pub economics: EconomicsReport,
    /// Why the headline reads zero, when it does.
    pub zero_reduction_cause: ZeroReductionCause,
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
pub struct StageExclusion {
    /// Operations visible in `by_mode` that the reduction ratio deliberately does not measure.
    pub operations: u64,
    pub delivered_tokens_estimated: u64,
}

/// The measured cost of a filtered result the model did not accept.
///
/// Reported next to the headline rather than folded into it. Subtracting it from net avoided
/// would silently redefine a metric that shipped in 0.6.3, and a re-run has causes other than
/// filtering. Showing it adjacently is what turns an assumed zero into a number an operator can
/// argue with — `net_avoided_after_rerun_tax_estimated` states the pessimistic reading outright.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct RerunTax {
    pub operations: u64,
    pub tokens_estimated: u64,
    pub net_avoided_after_rerun_tax_estimated: i64,
    /// Operations after which a repeat still counts as a reaction to filtering.
    pub detection_window_operations: u64,
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

#[derive(Clone, Copy, Debug, Serialize)]
pub struct HostVisibleSavings {
    pub operations: u64,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub reduction_pct: f64,
    pub uncapped_operations: u64,
    pub complete: bool,
    pub method: &'static str,
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
    pub key: PrivacySafeOperationKey,
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
    economics_project: Option<&Path>,
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
    // An explicit `--workspace` is also the project the money block prices; otherwise the caller
    // resolves the current worktree. The headline scope is untouched either way.
    let economics_project_text = workspace_text
        .clone()
        .or_else(|| economics_project.map(|path| path.to_string_lossy()));
    let collection = Ledger::stats_collection_read_only(
        &ledger_path,
        StatsQuery {
            project_path: workspace_text.as_deref(),
            since_unix_seconds: cutoff,
            include_legacy_versions: accounting_version == AccountingVersion::All,
            economics_project_path: economics_project_text.as_deref(),
        },
    )?;
    let snapshot = collection.snapshot;
    // Taken before the snapshot is partially moved into the report, so both money rows come from
    // the one read that produced the headline rather than a second look at a moving ledger.
    let snapshot_project_avoided = snapshot.project_avoided_tokens_estimated;
    let snapshot_global_avoided = snapshot.global_avoided_tokens_estimated;
    let snapshot_project_economics = snapshot.project_economics.clone();
    let snapshot_global_economics = snapshot.global_economics.clone();
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
    let mut report = build_report_with_command_limit(
        ReportInputs {
            gain: snapshot.efficiency,
            host_visible_gain: snapshot.host_visible_efficiency,
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
    );
    report.explicit_delivery = snapshot.explicit_delivery;
    let catalog = if !report.explicit_delivery.complete {
        report.raw_public_estimate_unavailable_reason = Some(
            "producer reductions cannot be priced without linked, complete host delivery evidence"
                .into(),
        );
        None
    } else if report.host_visible_savings.complete {
        if config.billing.public_estimate_enabled {
            match load_pricing_catalog(config.billing.pricing_file.as_deref()) {
                Ok(catalog) => Some(catalog),
                Err(error) => {
                    report.raw_public_estimate_unavailable_reason = Some(error.to_string());
                    None
                }
            }
        } else {
            report.raw_public_estimate_unavailable_reason = Some(PRICING_OPT_IN_REASON.to_owned());
            None
        }
    } else {
        report.raw_public_estimate_unavailable_reason = Some(
            "potential pricing is disabled because one or more operation hosts have no validated visible-output cap; raw byte estimates are upper bounds"
                .to_owned(),
        );
        None
    };
    if let Some(catalog) = catalog.as_ref() {
        match price_scope(
            config,
            catalog,
            report.direct_savings.net_avoided_tokens_estimated,
        ) {
            Ok(estimate) => report.raw_public_estimate = Some(estimate),
            Err(error) => report.raw_public_estimate_unavailable_reason = Some(error.to_string()),
        }
    }
    report.economics = build_economics(
        config,
        catalog.as_ref(),
        EconomicsInputs {
            project_resolved: economics_project_text.is_some(),
            project_avoided: snapshot_project_avoided,
            global_avoided: snapshot_global_avoided,
            project_receipts: snapshot_project_economics,
            global_receipts: snapshot_global_economics,
        },
        report.raw_public_estimate_unavailable_reason.clone(),
    );
    Ok(report)
}

const PRICING_OPT_IN_REASON: &str = "public pricing estimate is opt-in; run `hzr billing catalog`, then configure [billing] public_estimate_enabled, harness, provider, model, method, and pricing_basis in the HZR config";

/// The steps that turn `unavailable` into a number, stated where the gap is visible.
///
/// A reason without a remedy is a dead end: the operator learns the feature exists and not how
/// to reach it, which is how the money view stayed switched off through an entire release.
const PRICING_ENABLE_STEPS: [&str; 3] = [
    "1. `hzr billing catalog` — find the exact harness/provider/model/method row",
    "2. set [billing] public_estimate_enabled = true in the HZR config",
    "3. set [billing] harness, provider, model, method and pricing_basis to that exact row",
];

struct EconomicsInputs {
    project_resolved: bool,
    project_avoided: i64,
    global_avoided: i64,
    project_receipts: EconomicScopeSummary,
    global_receipts: EconomicScopeSummary,
}

fn price_scope(
    config: &Config,
    catalog: &PricingCatalog,
    avoided_tokens: i64,
) -> Result<RawPublicEstimate, hzr_core::BillingError> {
    price_avoided_input_tokens(
        catalog,
        RawPublicEstimateRequest {
            harness: &config.billing.harness,
            provider: &config.billing.provider,
            model: &config.billing.model,
            method: &config.billing.method,
            request_input_tokens: config.billing.request_input_tokens,
            basis: config.billing.effective_pricing_basis(),
            avoided_tokens: avoided_tokens.max(0).unsigned_abs(),
        },
    )
}

/// Assemble the two money rows.
///
/// Both rows are always produced, including when a scope is unresolved or holds nothing. An
/// omitted row would be read as a zero, and a zero is a claim about money that no evidence here
/// supports; `scope_resolved` and an absent `billed_actual` say what is actually known.
fn build_economics(
    config: &Config,
    catalog: Option<&PricingCatalog>,
    inputs: EconomicsInputs,
    unavailable_reason: Option<String>,
) -> EconomicsReport {
    let mut pricing = None;
    let mut price = |avoided: i64| -> Option<MoneyAmount> {
        let catalog = catalog?;
        let estimate = price_scope(config, catalog, avoided).ok()?;
        if pricing.is_none() {
            pricing = Some(PricingIdentity {
                harness: estimate.harness.clone(),
                provider: estimate.provider.clone(),
                model: estimate.model.clone(),
                method: estimate.method.clone(),
                pricing_basis: estimate.pricing_basis.clone(),
                price_table_identity: estimate.price_table_identity.clone(),
                retrieved_at: estimate.retrieved_at.clone(),
            });
        }
        Some(MoneyAmount {
            currency: estimate.currency,
            microunits: estimate.savings_microunits,
        })
    };

    let rows = vec![
        economic_row(
            "this project",
            inputs.project_resolved,
            inputs.project_avoided,
            &inputs.project_receipts,
            price(inputs.project_avoided),
        ),
        economic_row(
            "global lifetime",
            true,
            inputs.global_avoided,
            &inputs.global_receipts,
            price(inputs.global_avoided),
        ),
    ];
    let priced = pricing.is_some();
    EconomicsReport {
        rows,
        pricing,
        unavailable_reason: (!priced)
            .then(|| unavailable_reason.unwrap_or_else(|| "no exact pricing evidence".to_owned())),
        enable_steps: if priced {
            Vec::new()
        } else {
            PRICING_ENABLE_STEPS.to_vec()
        },
    }
}

fn economic_row(
    scope: &'static str,
    scope_resolved: bool,
    avoided: i64,
    receipts: &EconomicScopeSummary,
    potential_saved: Option<MoneyAmount>,
) -> EconomicScopeRow {
    // A receipt total is only shown when every receipt in the scope carried both sides of the
    // pair; a partial sum would understate a bill and still look like one.
    let billed_actual = receipts.reported_actual.as_ref().map(|amount| MoneyAmount {
        currency: amount.currency.clone(),
        microunits: amount.savings_microunits.max(0).unsigned_abs(),
    });
    EconomicScopeRow {
        scope,
        scope_resolved,
        avoided_input_tokens_estimated: if scope_resolved {
            avoided.max(0).unsigned_abs()
        } else {
            0
        },
        potential_saved: scope_resolved.then_some(potential_saved).flatten(),
        billed_actual,
        billed_receipts: receipts.paired_receipts,
        notes: if scope_resolved {
            receipts.unavailable_reasons.clone()
        } else {
            vec!["this directory is not inside a resolvable project scope".to_owned()]
        },
    }
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
            host_visible_gain: HostVisibleEfficiencySummary::default(),
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
        host_visible_gain,
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
    // Classified before `gain` is consumed below, so the explanation is derived from the same
    // summary the headline is derived from rather than from whatever survives the moves.
    let zero_reduction_cause = classify_zero_reduction(&gain);
    let by_mode = gain.by_mode.clone();
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
    let mut public_operations = Vec::<hzr_core::EfficiencyOperationSummary>::new();
    for stats in gain.by_command {
        if let Some(aggregate) = public_operations
            .iter_mut()
            .find(|aggregate| aggregate.key == stats.key)
        {
            let total_execution_ms = aggregate
                .avg_time_ms
                .saturating_mul(aggregate.executions)
                .saturating_add(stats.avg_time_ms.saturating_mul(stats.executions));
            aggregate.executions = aggregate.executions.saturating_add(stats.executions);
            aggregate.baseline_tokens_estimated = aggregate
                .baseline_tokens_estimated
                .saturating_add(stats.baseline_tokens_estimated);
            aggregate.delivered_tokens_estimated = aggregate
                .delivered_tokens_estimated
                .saturating_add(stats.delivered_tokens_estimated);
            aggregate.gross_avoided_tokens_estimated = aggregate
                .gross_avoided_tokens_estimated
                .saturating_add(stats.gross_avoided_tokens_estimated);
            aggregate.regression_tokens_estimated = aggregate
                .regression_tokens_estimated
                .saturating_add(stats.regression_tokens_estimated);
            aggregate.net_avoided_tokens_estimated = aggregate
                .net_avoided_tokens_estimated
                .saturating_add(stats.net_avoided_tokens_estimated);
            aggregate.avg_time_ms = total_execution_ms / aggregate.executions.max(1);
        } else {
            public_operations.push(stats);
        }
    }
    let mut commands = public_operations
        .into_iter()
        .map(|stats| CommandSavings {
            subsystem: operation_subsystem(&stats.key),
            command: operation_label(&stats.key),
            key: stats.key,
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
    // 0.8.3: explicitly unmeasured passthrough rows carry no tokens and therefore no per-command
    // aggregate, so the bypass subsystem showed fewer calls than the OPTIMIZER BYPASS panel
    // computed from the same rows (4.2K against 4.5K on one ledger). Count them here so both
    // panels report the same number of operations; they still contribute no tokens.
    if gain.unmeasured_bypass_operations > 0 {
        let totals = subsystems.entry("bypass").or_default();
        totals.0 = totals.0.saturating_add(gain.unmeasured_bypass_operations);
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
                    gain.net_avoided_tokens_estimated.unsigned_abs(),
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
        explicit_delivery: hzr_core::DeliverySummary::default(),
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
        host_visible_savings: HostVisibleSavings {
            operations: host_visible_gain.operations,
            baseline_tokens_estimated: host_visible_gain.baseline_tokens_estimated,
            delivered_tokens_estimated: host_visible_gain.delivered_tokens_estimated,
            net_avoided_tokens_estimated: host_visible_gain.net_avoided_tokens_estimated,
            reduction_pct: signed_percentage(
                host_visible_gain.net_avoided_tokens_estimated,
                host_visible_gain.baseline_tokens_estimated,
            ),
            uncapped_operations: host_visible_gain.uncapped_operations,
            complete: host_visible_gain.uncapped_operations == 0,
            method: "known_host_visible_caps_v1; claude-code=512 tokens; unknown hosts remain raw upper bounds",
        },
        by_subsystem,
        by_mode,
        read_pipeline: gain.read_pipeline,
        accounting_version_scope: match accounting_version {
            AccountingVersion::Current => "typed_v2_plus_aggregate_compatible_v1",
            AccountingVersion::All => "all_versions_compatibility_only",
        },
        accounting_policy_version: CURRENT_ACCOUNTING_POLICY_VERSION,
        excluded_legacy_operations: gain.excluded_legacy_operations,
        stage_exclusion: StageExclusion {
            operations: gain.stage_excluded_operations,
            delivered_tokens_estimated: gain.stage_excluded_delivered_tokens_estimated,
        },
        rerun_tax: RerunTax {
            operations: gain.filter_induced_rerun_operations,
            tokens_estimated: gain.filter_induced_rerun_tokens_estimated,
            net_avoided_after_rerun_tax_estimated: gain
                .net_avoided_tokens_estimated
                .saturating_sub(
                    i64::try_from(gain.filter_induced_rerun_tokens_estimated).unwrap_or(i64::MAX),
                ),
            detection_window_operations: hzr_core::RERUN_DETECTION_WINDOW_OPERATIONS,
        },
        by_family,
        evasion,
        by_command: commands,
        by_command_total,
        by_command_omitted,
        by_command_recovery: by_command_recovery.clone(),
        observed_model_usage,
        observed_model_usage_scope,
        bypass: bypass_report(bypass, false, by_command_recovery.clone()),
        traffic_coverage,
        degraded_rewrites: coverage.unreconciled_rewrites,
        coverage,
        runtime_accounting_complete: traffic_complete,
        economic_claim_ready: false,
        raw_public_estimate: None,
        raw_public_estimate_unavailable_reason: None,
        economics: EconomicsReport {
            rows: Vec::new(),
            pricing: None,
            unavailable_reason: None,
            enable_steps: Vec::new(),
        },
        zero_reduction_cause,
        notes: provider_usage_notes(observed_model_usage_scope),
    }
}

/// Explain a zero headline instead of leaving the reader to guess.
///
/// Order matters. Excluded history is checked first because it is the only cause an operator can
/// act on with one flag, and in the upgrade case it coexists with the others: after a policy
/// bump the surviving rows are typically also zero-credit, and reporting only the second cause
/// would send the reader looking for a defect that is really a scope boundary.
fn classify_zero_reduction(gain: &EfficiencySummary) -> ZeroReductionCause {
    classify_zero_reduction_values(
        gain.net_avoided_tokens_estimated,
        gain.operations,
        gain.excluded_legacy_operations,
    )
}

pub(crate) fn classify_zero_reduction_values(
    net_avoided_tokens_estimated: i64,
    operations: u64,
    excluded_legacy_operations: u64,
) -> ZeroReductionCause {
    if net_avoided_tokens_estimated != 0 {
        return ZeroReductionCause::NotZero;
    }
    if excluded_legacy_operations > 0 {
        return ZeroReductionCause::ExcludedHistory;
    }
    if operations == 0 {
        return ZeroReductionCause::NoOperations;
    }
    ZeroReductionCause::OnlyZeroCreditOperations
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

fn operation_subsystem(key: &PrivacySafeOperationKey) -> &'static str {
    if key.route == OperationRoute::Bypassed {
        return "bypass";
    }
    match key.operation {
        Some(AccountingOperationKind::Read) => "read",
        Some(AccountingOperationKind::Search) => "search",
        Some(AccountingOperationKind::Write) => "write",
        Some(AccountingOperationKind::Memory) => "memory",
        Some(AccountingOperationKind::Codec) => "codec",
        Some(
            AccountingOperationKind::Context
            | AccountingOperationKind::Exec
            | AccountingOperationKind::Observability
            | AccountingOperationKind::Doctor,
        )
        | None => "execution",
    }
}

fn operation_label(key: &PrivacySafeOperationKey) -> String {
    let route = match key.route {
        OperationRoute::Optimized => "opt",
        OperationRoute::Bypassed => "raw",
        OperationRoute::NativeUnaccounted => "native",
    };
    let operation = key.operation.map(AccountingOperationKind::as_str);
    let identity = match operation {
        Some(operation) if operation != key.family => format!("{}>{operation}", key.family),
        _ => key.family.clone(),
    };
    let mode = key.mode.map_or("legacy", |mode: AccountingOperationMode| {
        let full = mode.as_str();
        operation
            .and_then(|operation| full.strip_prefix(operation))
            .and_then(|suffix| suffix.strip_prefix('_'))
            .unwrap_or(full)
    });
    let stage = match key.stage {
        AccountingStage::InternalTransport => "int",
        AccountingStage::FinalDelivery => "final",
        AccountingStage::StandaloneDelivery => "direct",
        AccountingStage::ControlPlane => "control",
    };
    format!("{route} {identity}:{mode}/{stage}")
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
        ReportOptions, build_report, build_report_with_command_limit, operation_label,
        operation_subsystem,
    };
    use crate::hook_runner::AccountingCoverage;
    use hzr_core::{
        BypassSummary, BypassTool, BypassWindow, EfficiencyOperationSummary, EfficiencySummary,
        HostVisibleEfficiencySummary, OperationModeSummary, OperationRoute,
        PrivacySafeOperationKey, ReplacementCapability,
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
                EfficiencyOperationSummary {
                    key: PrivacySafeOperationKey {
                        family: "write".into(),
                        operation: Some(AccountingOperationKind::Write),
                        mode: Some(AccountingOperationMode::Write),
                        stage: AccountingStage::InternalTransport,
                        route: OperationRoute::Optimized,
                    },
                    executions: 1,
                    baseline_tokens_estimated: 400,
                    delivered_tokens_estimated: 100,
                    gross_avoided_tokens_estimated: 300,
                    regression_tokens_estimated: 0,
                    net_avoided_tokens_estimated: 300,
                    avg_time_ms: 4,
                },
                EfficiencyOperationSummary {
                    key: PrivacySafeOperationKey {
                        family: "search".into(),
                        operation: Some(AccountingOperationKind::Search),
                        mode: Some(AccountingOperationMode::SearchAuto),
                        stage: AccountingStage::InternalTransport,
                        route: OperationRoute::Optimized,
                    },
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

    // 0.8.3: the bypass subsystem and the OPTIMIZER BYPASS panel count the same operations.
    #[test]
    fn unmeasured_bypass_operations_count_in_the_bypass_subsystem() {
        let gain = EfficiencySummary {
            operations: 2,
            total_observed_operations: 5,
            unmeasured_bypass_operations: 3,
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

        let bypass = report
            .by_subsystem
            .iter()
            .find(|subsystem| subsystem.subsystem == "bypass")
            .expect("unmeasured passthrough rows appear as bypass operations");
        assert_eq!(bypass.operations, 3);
        assert_eq!(bypass.net_avoided_tokens_estimated, 0);
        assert_eq!(report.traffic_coverage.unmeasured_bypass_operations, 3);
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
    fn typed_public_key_controls_label_and_subsystem() {
        let key = PrivacySafeOperationKey {
            family: "search".into(),
            operation: Some(AccountingOperationKind::Search),
            mode: Some(AccountingOperationMode::SearchExact),
            stage: AccountingStage::InternalTransport,
            route: OperationRoute::Bypassed,
        };

        assert_eq!(operation_subsystem(&key), "bypass");
        assert_eq!(operation_label(&key), "raw search:exact/int");
    }

    #[test]
    fn public_key_is_coalesced_before_report_limit_and_render() {
        let key = PrivacySafeOperationKey {
            family: "search".into(),
            operation: Some(AccountingOperationKind::Search),
            mode: Some(AccountingOperationMode::SearchAuto),
            stage: AccountingStage::InternalTransport,
            route: OperationRoute::Optimized,
        };
        let report = build_report(
            EfficiencySummary {
                by_command: vec![
                    EfficiencyOperationSummary {
                        key: key.clone(),
                        executions: 1,
                        baseline_tokens_estimated: 100,
                        delivered_tokens_estimated: 20,
                        gross_avoided_tokens_estimated: 80,
                        regression_tokens_estimated: 0,
                        net_avoided_tokens_estimated: 80,
                        avg_time_ms: 10,
                    },
                    EfficiencyOperationSummary {
                        key,
                        executions: 3,
                        baseline_tokens_estimated: 300,
                        delivered_tokens_estimated: 90,
                        gross_avoided_tokens_estimated: 210,
                        regression_tokens_estimated: 0,
                        net_avoided_tokens_estimated: 210,
                        avg_time_ms: 30,
                    },
                ],
                ..EfficiencySummary::default()
            },
            LedgerSummary::default(),
            "global_lifetime",
            AccountingCoverage::default_complete(),
            BypassSummary::default(),
            "global lifetime".into(),
        );

        assert_eq!(report.by_command_total, 1);
        assert_eq!(report.by_command[0].executions, 4);
        assert_eq!(report.by_command[0].baseline_tokens_estimated, 400);
        assert_eq!(report.by_command[0].delivered_tokens_estimated, 110);
        assert_eq!(report.by_command[0].net_avoided_tokens_estimated, 290);
        assert_eq!(report.by_command[0].avg_time_ms, 25);
        assert_eq!(report.by_command[0].command, "opt search:auto/int");
        let json = serde_json::to_value(&report).expect("stats JSON");
        assert_eq!(json["by_command"][0]["key"]["mode"], "search_auto");
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
            by_command: vec![EfficiencyOperationSummary {
                key: PrivacySafeOperationKey {
                    family: "search".into(),
                    operation: Some(AccountingOperationKind::Search),
                    mode: Some(AccountingOperationMode::SearchAuto),
                    stage: AccountingStage::InternalTransport,
                    route: OperationRoute::Optimized,
                },
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

        assert_eq!(report.by_command[0].command, "opt search:auto/int");
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
                        by_command: vec![EfficiencyOperationSummary {
                            key: PrivacySafeOperationKey {
                                family: "python".into(),
                                operation: None,
                                mode: None,
                                stage: AccountingStage::InternalTransport,
                                route: OperationRoute::Bypassed,
                            },
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
                    host_visible_gain: HostVisibleEfficiencySummary::default(),
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
            .map(|index| EfficiencyOperationSummary {
                key: PrivacySafeOperationKey {
                    family: format!("route-{index}"),
                    operation: None,
                    mode: None,
                    stage: AccountingStage::InternalTransport,
                    route: OperationRoute::Optimized,
                },
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
