use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use hzr_core::{
    AccountingCoverageStore, AccountingGapEvent, AccountingGapSurface,
    AccountingReceiptContextStore, ActivationMode, Config, FidelityAllowance, FidelityBudget,
    FidelityPreflight, Ledger, OperationRoute, RawFidelityRequest, RawPublicEstimate,
    RawPublicEstimateRequest, SessionEconomicSummary, SessionEfficiencySummary,
    SessionEvasionSummary, fidelity_preflight_required, first_class_replacement,
    load_pricing_catalog, price_avoided_input_tokens, privacy_identity_hash, raw_fidelity_request,
};
use hzr_exec::{
    CanonicalCommand, ForkRuntimePaths, HOST_GRANT_APPLIED_ENV, PinnedRtkAdapter, RewriteDecision,
    RewriteSource, RtkAdapterConfig, RtkRewriteOutcome, host_grant_applied, reconcile_host_grant,
};
use hzr_index::registered_workspaces;
use hzr_protocol::{
    AccountingAttribution, AccountingChannel, AccountingMeasurement, AccountingOperationKind,
    AccountingOperationMode, AccountingRoute, AccountingStage, ContextPlanApiRequest,
    EnforcementTier, EvasionAttribution, EvasionClass, EvasionPathForm, ExecApiRequest,
    FidelityValidation, FilterPlacement, HOST_EXECUTION_GRANT_ENV, HostExecutionGrant,
    HostPermissionMode, OperationApiRequest, PolicyDecision, PolicyEventApiRequest, PrefixEffect,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::time::timeout;

use crate::adoption::NativeToolMode;
use crate::client::DaemonClient;

#[cfg(test)]
#[path = "../../../fork-core/rtk/tests/fixtures/anti_evasion_fixture.rs"]
mod anti_evasion_fixture;

const HOOK_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_HOOK_INPUT_BYTES: u64 = 2 * 1024 * 1024;
const SESSION_CORRECTION_NUDGE: u64 = 3;
const SESSION_BYPASS_COUNT_BUDGET: u64 = 40;
const SESSION_BYPASS_TOKEN_BUDGET: u64 = 250_000;
const SESSION_AVOIDABLE_SHARE_NUDGE: f64 = 10.0;
const STATUSLINE_UPSTREAM_ENV: &str = "HZR_STATUSLINE_UPSTREAM_HEX";
const STATUSLINE_UPSTREAM_TIMEOUT: Duration = Duration::from_millis(500);
const STATUSLINE_UPSTREAM_MAX_BYTES: u64 = 64 * 1024;

pub async fn dispatch(
    config: &Config,
    _native_mode: NativeToolMode,
    host: crate::host_hooks::HookHost,
) {
    let Ok(mut input) = read_input() else {
        return;
    };
    if hook_workspace_root(config, &input).is_none() {
        return;
    }
    if !crate::host_hooks::supports_pre_tool(host, &input) {
        return;
    }
    if host == crate::host_hooks::HookHost::Codex && !host_grants_execution(&input) {
        return;
    }
    input["_hzr_host"] = json!(host.name());
    if host == crate::host_hooks::HookHost::Codex {
        input["agent_type"] = json!("codex");
    }
    let _ = update_session(config, &input, |state| {
        state.operations = state.operations.saturating_add(1);
        state.operations_this_turn = state.operations_this_turn.saturating_add(1);
        state.host_grant_seen |= host_grants_execution(&input);
    });
    let tool_name = input
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match tool_name {
        "Bash" => {
            let _ = rewrite(config, &input).await;
        }
        "Agent" | "Task" => {
            let _ = task(config, &input).await;
        }
        _ => {}
    }
}

/// Observe host-native file tools without steering or blocking them.
///
/// Failures never turn a successful host call into a failed one. Output replacement
/// requires an explicit host capability opt-in and a supported exact response shape.
pub async fn observe(
    config: &Config,
    native_mode: NativeToolMode,
    host: crate::host_hooks::HookHost,
    replace_output: bool,
) {
    let Ok(input) = read_input() else {
        return;
    };
    if hook_workspace_root(config, &input).is_none() {
        return;
    }
    let _ = observe_input(config, &input, native_mode).await;
    if replace_output {
        if let Ok(Some(output)) = timeout(
            HOOK_TIMEOUT,
            crate::host_hooks::replace_tool_output(config, &input, host),
        )
        .await
        {
            // No additionalContext duplicate: updatedToolOutput replaces the structured value.
            let mut stdout = io::stdout().lock();
            let _ = serde_json::to_writer(&mut stdout, &output);
            let _ = stdout.write_all(b"\n");
        }
    }
}

async fn observe_input(config: &Config, input: &Value, native_mode: NativeToolMode) -> Result<()> {
    let Some(request) = native_observation_request(input, native_mode)? else {
        return Ok(());
    };
    let workspace = request.project_path.clone();
    let session = request.session_id.clone();
    let family = request
        .attribution
        .as_ref()
        .map(|attribution| attribution.operation.as_str());
    let recorded = match DaemonClient::from_config(config) {
        Ok(client) => client.record_operation(&request).await.is_ok(),
        Err(_) => false,
    };
    if !recorded {
        record_accounting_gap_now(
            config,
            AccountingGapSurface::Hook,
            Some(&workspace),
            session.as_deref(),
            family,
        )?;
    } else {
        recover_accounting_gap_now(
            config,
            AccountingGapSurface::Hook,
            Some(&workspace),
            session.as_deref(),
            family,
        )?;
    }
    Ok(())
}

fn native_observation_request(
    input: &Value,
    native_mode: NativeToolMode,
) -> Result<Option<OperationApiRequest>> {
    let tool = input
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(tool, "Read" | "Grep" | "Glob" | "Edit" | "Write") {
        return Ok(None);
    }
    let response = input.get("tool_response");
    let response_bytes = response
        .map(serde_json::to_vec)
        .transpose()
        .context("failed to size native tool response")?
        .map_or(0, |bytes| bytes.len());
    let estimated = u64::try_from(response_bytes / 4).unwrap_or(u64::MAX);
    let cwd = input.get("cwd").and_then(Value::as_str).unwrap_or_default();
    let session_id = input.get("session_id").and_then(Value::as_str);
    let agent = agent_attribution(input);
    let (measurement, tokens) = if response.is_some() {
        (AccountingMeasurement::Estimated, estimated)
    } else {
        (AccountingMeasurement::Unmeasured, 0)
    };
    let (route, evasion) = native_observation_policy(tool, native_mode);
    let (operation, mode) = match tool {
        "Read" => (
            AccountingOperationKind::Read,
            AccountingOperationMode::ReadFull,
        ),
        "Grep" | "Glob" => (
            AccountingOperationKind::Search,
            AccountingOperationMode::SearchExact,
        ),
        "Edit" | "Write" => (
            AccountingOperationKind::Write,
            AccountingOperationMode::Write,
        ),
        _ => unreachable!("native tool was validated above"),
    };
    Ok(Some(OperationApiRequest {
        original_command: format!("native {tool}"),
        recorded_command: format!("native {tool}"),
        baseline_tokens_estimated: tokens,
        delivered_tokens_estimated: tokens,
        execution_ms: 0,
        project_path: cwd.to_owned(),
        channel: AccountingChannel::NativeHost,
        measurement,
        route: match route {
            OperationRoute::Optimized => AccountingRoute::Optimized,
            OperationRoute::Bypassed => AccountingRoute::Bypassed,
            OperationRoute::NativeUnaccounted => AccountingRoute::NativeUnaccounted,
        },
        agent: Some(agent),
        session_id: session_id.map(str::to_owned),
        attribution: Some(AccountingAttribution {
            operation,
            mode,
            stage: AccountingStage::FinalDelivery,
            requested_mode: None,
            effective_mode: Some(mode),
            search_strategy: None,
            search_fallback_code: None,
            include_content: None,
            limit: None,
            path_scope_count: None,
            filter_level: None,
            from_line: None,
            to_line: None,
            source_bytes: None,
            evasion: Some(evasion),
        }),
    }))
}

fn read_input() -> Result<Value> {
    let mut bytes = Vec::new();
    io::stdin()
        .take(MAX_HOOK_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("failed to read hook input")?;
    if bytes.len() as u64 > MAX_HOOK_INPUT_BYTES {
        bail!("hook input exceeds {MAX_HOOK_INPUT_BYTES} bytes");
    }
    serde_json::from_slice(&bytes).context("hook input is not valid JSON")
}

async fn rewrite(config: &Config, input: &Value) -> Result<()> {
    let Some(raw) = input.pointer("/tool_input/command").and_then(Value::as_str) else {
        return Ok(());
    };
    if raw.is_empty() {
        return Ok(());
    }
    let cwd = hook_workspace_root(config, input)
        .context("hook input is outside the configured HZR workspace")?;
    let fidelity_evasion = match hook_fidelity_preflight(config, input, raw, &cwd) {
        HookFidelityPreflight::NotRequested => None,
        HookFidelityPreflight::Allow(evasion) => Some(evasion),
        HookFidelityPreflight::Ask { decision, evasion } => {
            let _ = record_local_policy_decision(config, input, &decision, Some(evasion)).await;
            return write_decision(input, decision, None);
        }
    };
    if fidelity_evasion.is_none() && hzr_core::is_direct_hzr_command(raw) {
        return write_decision(
            input,
            RewriteDecision::allow_raw(
                "command already enters the HZR control plane; no fork proxy is needed",
            ),
            None,
        );
    }
    let request = ExecApiRequest {
        channel: None,
        cwd: cwd.to_string_lossy().into_owned(),
        command: raw.to_owned(),
        fidelity_requested: fidelity_evasion.is_some(),
        fidelity_reason: fidelity_evasion
            .as_ref()
            .and_then(|evasion| evasion.fidelity_reason)
            .map(|reason| reason.as_str().to_owned()),
        timeout_ms: Some(HOOK_TIMEOUT.as_millis() as u64),
        caller_path: std::env::var("PATH").ok(),
        agent: Some(agent_attribution(input)),
        session_id: input
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        // The hook holds the host's answer first-hand, so the daemon decides with the same
        // information rather than producing an Ask this hook would immediately override.
        host_execution_grant: host_grant_from_input(input),
    };
    let managed = if let Ok(client) = DaemonClient::from_config(config) {
        timeout(HOOK_TIMEOUT, client.exec_rewrite(&request))
            .await
            .ok()
            .and_then(Result::ok)
    } else {
        None
    };
    let daemon_recorded_policy = managed.is_some();
    let (decision, accounting_notice, managed_evasion) = match managed {
        Some(outcome) => {
            let notice = recover_accounting_for_input(config, input)
                .ok()
                .and_then(|()| accounting_transition(config, input, false));
            (outcome.decision, notice, outcome.evasion)
        }
        None => {
            let _ = record_degraded_rewrite_for_input(config, input);
            let notice = accounting_transition(config, input, true);
            let outcome = fallback_decision(config, raw, &cwd).await;
            // 0.8.1: a daemon-free rewrite still makes fork-core write receipts under a local
            // correlation. Without a context file those receipts stayed undrained forever, so
            // register the same context the daemon would have; the sweeper drains it later.
            if let Some(correlation_id) = outcome.accounting_correlation_id.as_deref() {
                if let Err(error) = AccountingReceiptContextStore::new(&config.data_dir)
                    .register_with_attribution(
                        correlation_id,
                        &cwd,
                        Some(&agent_attribution(input)),
                        input.get("session_id").and_then(Value::as_str),
                        AccountingChannel::HookCli,
                        outcome.evasion, // 0.8.3: classification travels with the registration
                    )
                {
                    eprintln!("HZR daemon-free accounting context was not registered: {error}");
                }
            }
            (outcome.decision, notice, outcome.evasion)
        }
    };
    let decision = steer_to_first_class(raw, decision);
    // Fidelity attribution is authoritative when present; otherwise the daemon's classification
    // of this exact command is what the recording process needs.
    let evasion = fidelity_evasion.or(managed_evasion);
    // A command rewrite shapes the one tool result this call is about to append; it never
    // rewrites content the provider already cached, so it is append-class at every turn position.
    let decision = apply_filter_placement(config, input, PrefixEffect::Append, decision);
    let decision = honor_host_permission_mode(input, decision);
    let decision = attach_hook_evasion(raw, decision, evasion.as_ref());
    let decision = attach_policy_feedback(config, input, decision);
    let decision = attach_session_attribution(input, decision);
    // After the session, because the grant names a digest of it and the reader validates the two
    // together: a grant without its session in the same environment is refused, by design.
    let decision = attach_host_grant(input, decision);
    if !daemon_recorded_policy {
        let _ = record_local_policy_decision(config, input, &decision, None).await;
    }
    write_decision(input, decision, accounting_notice.as_deref())
}

fn accounting_transition(config: &Config, input: &Value, degraded: bool) -> Option<String> {
    accounting_transition_at(config, input, degraded, unix_now())
}

fn accounting_transition_at(
    config: &Config,
    input: &Value,
    degraded: bool,
    now_unix: u64,
) -> Option<String> {
    let mut changed = false;
    update_session_at(config, input, now_unix, |state| {
        changed = state.accounting_degraded != degraded;
        if degraded {
            state.accounting_degraded_operations =
                state.accounting_degraded_operations.saturating_add(1);
        }
        if changed && degraded {
            state.accounting_degraded_started_at_unix = Some(now_unix);
        } else if changed {
            if let Some(started) = state.accounting_degraded_started_at_unix.take() {
                state.accounting_degraded_seconds = state
                    .accounting_degraded_seconds
                    .saturating_add(now_unix.saturating_sub(started));
            }
        }
        state.accounting_degraded = degraded;
        state.accounting_was_degraded |= degraded;
    })
    .ok()?;
    if !changed {
        return None;
    }
    Some(if degraded {
        "HZR ACCOUNTING DEGRADED: the ledger is no longer recording this session's operations; coverage is unknown. Check `hzr daemon service status`.".into()
    } else {
        "HZR ACCOUNTING RECOVERED: managed rewrites are recording again. This session's earlier degraded interval remains partial evidence.".into()
    })
}

async fn record_local_policy_decision(
    config: &Config,
    input: &Value,
    decision: &RewriteDecision,
    explicit_evasion: Option<EvasionAttribution>,
) -> Result<()> {
    let decision = match decision {
        RewriteDecision::Ask { .. } => PolicyDecision::Ask,
        RewriteDecision::Deny { .. } => PolicyDecision::Deny,
        RewriteDecision::AllowRewrite { .. } => PolicyDecision::Correction,
        RewriteDecision::AllowRaw { .. } => return Ok(()),
    };
    let evasion = explicit_evasion.unwrap_or(EvasionAttribution {
        class: EvasionClass::E10CapabilityGap,
        wrapper_depth: 0,
        interpreter: None,
        path_form: EvasionPathForm::Bare,
        stage_count: 1,
        hatch_marker: false,
        avoidable: false,
        tier: EnforcementTier::T0TransparentRewrite,
        fidelity_reason: None,
        fidelity_validation: FidelityValidation::NotRequested,
    });
    let request = PolicyEventApiRequest {
        project_path: input
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        agent: Some(agent_attribution(input)),
        session_id: input
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        evasion,
        decision,
        replacement_family: None,
        command_identity: input
            .pointer("/tool_input/command")
            .and_then(Value::as_str)
            .map(str::to_owned),
    };
    send_hook_policy_event(config, input, "policy", &request).await
}

enum HookFidelityPreflight {
    NotRequested,
    Allow(EvasionAttribution),
    Ask {
        decision: RewriteDecision,
        evasion: EvasionAttribution,
    },
}

fn hook_fidelity_preflight(
    config: &Config,
    input: &Value,
    raw: &str,
    cwd: &Path,
) -> HookFidelityPreflight {
    if !fidelity_preflight_required(raw) {
        return HookFidelityPreflight::NotRequested;
    }
    let budget = input
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .and_then(|session_id| {
            Ledger::open(&config.data_dir.join("ledger/hzr.sqlite"))
                .and_then(|ledger| {
                    ledger.fidelity_session_usage(session_id, FidelityAllowance::default())
                })
                .ok()
        })
        .map(|usage| FidelityBudget {
            remaining_operations: usage.remaining_operations,
            remaining_tokens: usage.remaining_tokens,
            exhausted: usage.exhausted,
        });
    match hzr_core::fidelity_preflight(raw, cwd, budget) {
        FidelityPreflight::NotRequested => HookFidelityPreflight::NotRequested,
        FidelityPreflight::Allow { evasion, .. } => HookFidelityPreflight::Allow(evasion),
        FidelityPreflight::Ask { evasion, reason } => HookFidelityPreflight::Ask {
            decision: RewriteDecision::Ask {
                proposed: Some(CanonicalCommand::shell(raw)),
                reason,
            },
            evasion,
        },
    }
}

/// Carry a T4 fidelity attribution into the process that will record the command.
///
/// 0.8.3: an ordinary command's classification no longer travels here. It is stored with the
/// accounting registration (the daemon stores its own classification when it registers the
/// correlation; the daemon-free path registers with the hook's) and the sweeper copies it onto
/// the receipts. The approved command the host inspects therefore stays free of exported policy
/// JSON: a multi-line script carrying `"evasion"`, `"avoidable": true` and a `PATH` prefix was
/// exactly what a permission classifier reading the tool input could not tell from an actual
/// evasion, and it refused commands as harmless as `df -h | tail -1`.
///
/// The fidelity hatch keeps the environment transport: its command runs raw or enters
/// `hzr exec run` directly, has no engine registration to carry the attribution, and already
/// names the hatch in plain text. There the attribution is a precondition — an unattributed T4
/// execution must not happen silently — so a failure becomes an Ask.
fn attach_hook_evasion(
    raw: &str,
    decision: RewriteDecision,
    evasion: Option<&EvasionAttribution>,
) -> RewriteDecision {
    let Some(evasion) = evasion else {
        return decision;
    };
    if !evasion.hatch_marker {
        return decision; // 0.8.3: attributed through the registration, see above
    }
    let Ok(encoded) = serde_json::to_string(evasion) else {
        return RewriteDecision::Ask {
            proposed: Some(CanonicalCommand::shell(raw)),
            reason: "T4 fidelity attribution could not be serialized".into(),
        };
    };
    match decision {
        RewriteDecision::AllowRaw { .. } => match attributed_hook_command(&encoded, raw) {
            Some(command) => RewriteDecision::AllowRewrite {
                command: CanonicalCommand::shell(command),
                source: RewriteSource::HzrPolicy,
                reason: "T4 fidelity execution carries closed typed attribution".into(),
            },
            None => RewriteDecision::Ask {
                proposed: Some(CanonicalCommand::shell(raw)),
                reason: "T4 fidelity execution needs explicit approval on this host".into(),
            },
        },
        RewriteDecision::AllowRewrite {
            command,
            source,
            reason,
        } => {
            let refuse = || RewriteDecision::Ask {
                proposed: Some(CanonicalCommand::shell(raw)),
                reason: "T4 fidelity execution could not preserve the approved command".into(),
            };
            let Ok(rendered) = render_command(&command) else {
                return refuse();
            };
            match attributed_hook_command(&encoded, &rendered) {
                // The source and reason stay the agent-facing ones the decision already carried;
                // only the environment the command runs in changes.
                Some(attributed) => RewriteDecision::AllowRewrite {
                    command: CanonicalCommand::shell(attributed),
                    source,
                    reason,
                },
                None => refuse(),
            }
        }
        other => other,
    }
}

/// Prefix a managed command with an exported variable.
///
/// A bare `VAR=value <command>` prefix only works when a command follows on the same line. The
/// managed command is a script whose first line is already a run of assignments, so a bare
/// prefix became one more assignment in that run and never reached the engine process — the
/// script's own `export` statement lists only the RTK variables. Exporting explicitly is what
/// actually crosses the process boundary.
#[cfg(unix)]
fn exported_hook_command(variable: &str, value: &str, command: &str) -> String {
    format!("export {variable}={};\n{command}", shell_quote(value))
}

/// The exact prefix `exported_hook_command` writes, for idempotency checks.
///
/// Deriving it from the same formatter is what keeps "did I already attach this?" answerable:
/// a hand-written substring drifts from the emitter and then matches commands that merely
/// reference the variable.
#[cfg(unix)]
fn already_exported(variable: &str) -> String {
    format!("export {variable}=")
}

#[cfg(unix)]
fn attributed_hook_command(encoded: &str, command: &str) -> Option<String> {
    Some(exported_hook_command(
        "HZR_INTERNAL_EVASION_JSON",
        encoded,
        command,
    ))
}

#[cfg(windows)]
fn attributed_hook_command(_encoded: &str, _command: &str) -> Option<String> {
    None
}

/// Carry the session into the process that will record the operation.
///
/// The hook receives the session on stdin, but the command it approves runs in a fresh engine
/// process that can only learn the session from its environment. Without this the executed rows
/// land with a null session, so per-session avoidable operations and tokens read zero however
/// much bypass a session actually performed — the policy events carry the session and the
/// operations do not, which is why a scorecard could show corrections while its budget stayed
/// empty. The engine already reads `HZR_SESSION_ID` and stores only a keyed hash of it.
#[cfg(unix)]
fn attach_session_attribution(input: &Value, decision: RewriteDecision) -> RewriteDecision {
    let Some(session) = input
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|session| !session.is_empty())
    else {
        return decision;
    };
    let RewriteDecision::AllowRewrite {
        command,
        source,
        reason,
    } = decision
    else {
        return decision;
    };
    let Ok(rendered) = render_command(&command) else {
        return RewriteDecision::AllowRewrite {
            command,
            source,
            reason,
        };
    };
    if rendered.contains(&already_exported("HZR_SESSION_ID")) {
        return RewriteDecision::AllowRewrite {
            command,
            source,
            reason,
        };
    }
    RewriteDecision::AllowRewrite {
        command: CanonicalCommand::shell(exported_hook_command(
            "HZR_SESSION_ID",
            session,
            &rendered,
        )),
        source,
        reason,
    }
}

#[cfg(windows)]
fn attach_session_attribution(_input: &Value, decision: RewriteDecision) -> RewriteDecision {
    decision
}

fn native_evasion(
    class: EvasionClass,
    avoidable: bool,
    tier: EnforcementTier,
) -> EvasionAttribution {
    EvasionAttribution {
        class,
        wrapper_depth: 0,
        interpreter: None,
        path_form: EvasionPathForm::Bare,
        stage_count: 1,
        hatch_marker: false,
        avoidable,
        tier,
        fidelity_reason: None,
        fidelity_validation: FidelityValidation::NotRequested,
    }
}

fn native_observation_policy(
    _tool: &str,
    _native_mode: NativeToolMode,
) -> (OperationRoute, EvasionAttribution) {
    // No proven shape-preserving native transform: measurement is not an avoidable bypass.
    (
        OperationRoute::NativeUnaccounted,
        native_evasion(
            EvasionClass::E10CapabilityGap,
            false,
            EnforcementTier::T0TransparentRewrite,
        ),
    )
}

async fn send_hook_policy_event(
    config: &Config,
    input: &Value,
    family: &str,
    request: &PolicyEventApiRequest,
) -> Result<()> {
    let recorded = match DaemonClient::from_config(config) {
        Ok(client) => client.record_policy_event(request).await.is_ok(),
        Err(_) => false,
    };
    let workspace = input.get("cwd").and_then(Value::as_str);
    let session = input.get("session_id").and_then(Value::as_str);
    if recorded {
        recover_accounting_gap_now(
            config,
            AccountingGapSurface::Hook,
            workspace,
            session,
            Some(family),
        )?;
    } else {
        record_accounting_gap_now(
            config,
            AccountingGapSurface::Hook,
            workspace,
            session,
            Some(family),
        )?;
    }
    Ok(())
}

/// Give every agent-visible policy decision the running session cost.
///
/// A correction increments the counter because it is the event being counted. An Ask or a Deny
/// only reports it: those decisions already cost the agent a turn, and an E10 Ask is a genuine
/// capability gap that must never consume the avoidable-bypass budget.
fn attach_policy_feedback(
    config: &Config,
    input: &Value,
    mut decision: RewriteDecision,
) -> RewriteDecision {
    match &mut decision {
        RewriteDecision::AllowRewrite {
            source: RewriteSource::HzrPolicy,
            reason,
            ..
        } => {
            if let Ok(state) = update_session(config, input, |state| {
                state.corrections = state.corrections.saturating_add(1);
            }) {
                reason.push_str(&format!(
                    " T1 named correction class=covered_route; session avoidable-bypass count={}.",
                    state.corrections
                ));
            }
        }
        RewriteDecision::Ask { reason, .. } | RewriteDecision::Deny { reason } => {
            if let Some(state) = read_session(config, input) {
                reason.push_str(&format!(
                    "; session avoidable-bypass count={}",
                    state.corrections
                ));
            }
        }
        RewriteDecision::AllowRaw { .. } | RewriteDecision::AllowRewrite { .. } => {}
    }
    decision
}

/// Enforce the first-class HZR command when the agent is about to reach the shell unfiltered
/// or fork-core selected its tracked raw proxy instead of a safe specialized route.
///
/// Raw remains available when no safe equivalent exists. Once the central operation policy
/// identifies an equivalent, asking leaves the avoidable bypass as the default action.
fn steer_to_first_class(raw: &str, decision: RewriteDecision) -> RewriteDecision {
    if !matches!(
        decision,
        RewriteDecision::AllowRaw { .. }
            | RewriteDecision::AllowRewrite {
                source: RewriteSource::Rtk {
                    route: hzr_exec::RtkRewriteRoute::Proxy,
                    ..
                },
                ..
            }
    ) {
        return decision;
    }
    let Some(replacement) = first_class_replacement(raw) else {
        return decision;
    };
    hzr_policy_rewrite(replacement)
}

/// Whether the host has already decided that commands run without prompting.
///
/// Claude Code reports its permission mode on every hook call. `bypassPermissions` is an explicit
/// operator decision to stop being asked.
fn host_grants_execution(input: &Value) -> bool {
    ["permission_mode", "permissionMode"]
        .into_iter()
        .find_map(|key| input.get(key).and_then(Value::as_str))
        .is_some_and(|mode| mode.eq_ignore_ascii_case("bypassPermissions"))
}

/// Decline a mid-turn *history mutation* when policy says the request prefix must stay put.
///
/// Delivered bytes and billed input are different axes. A harness that caches the request prefix
/// bills a cached read far below a fresh one, and a transform that rewrites content the prefix
/// already carries invalidates everything cached after it. So a route can cut delivered bytes
/// hard and still raise the provider's billed input.
///
/// The causal model matters, though: appending a shaped tool result does not move the cached
/// prefix, however deep into the turn it lands. The earlier version of this policy deferred every
/// mid-turn filter and therefore gave up reduction on operations that could never have invalidated
/// anything. Under `FilterPlacement::Anywhere` — the shipped default — nothing changes. Under
/// `TurnBoundary`, an [`PrefixEffect::Append`] is filtered at any position and counted, while a
/// [`PrefixEffect::HistoryMutation`] is filtered only on the turn's first operation; later ones run
/// raw, tracked, with the deferral counted so the reduction it costs stays visible. A Deny is
/// untouched: prefix stability is not a reason to run something policy forbids.
fn apply_filter_placement(
    config: &Config,
    input: &Value,
    effect: PrefixEffect,
    decision: RewriteDecision,
) -> RewriteDecision {
    let placement = config.policy.filter_placement;
    if placement == FilterPlacement::Anywhere {
        return decision;
    }
    // The current call has already been counted by the dispatcher, so `1` is the turn's first
    // operation. Defaulting to `true` when the session cannot be read keeps an unreadable state
    // from silently disabling filtering altogether.
    let at_boundary = read_session(config, input)
        .map(|state| state.operations_this_turn <= 1)
        .unwrap_or(true);
    if placement.permits(effect, at_boundary) {
        if effect == PrefixEffect::Append
            && !at_boundary
            && matches!(decision, RewriteDecision::AllowRewrite { .. })
        {
            // Counted so the arm comparison can see what the corrected model kept: these are the
            // operations the old turn-boundary rule would have run raw.
            let _ = update_session(config, input, |state| {
                state.placement_append_filtered_operations += 1;
            });
        }
        return decision;
    }
    match decision {
        RewriteDecision::AllowRewrite { .. } => {
            let _ = update_session(config, input, |state| {
                state.placement_deferred_operations += 1;
            });
            RewriteDecision::allow_raw(format!(
                "filter placement policy `{}` defers a `{}` transform mid-turn to keep the cached \
                 request prefix stable; this operation ran unfiltered and earns no savings credit",
                placement.as_str(),
                effect.as_str()
            ))
        }
        other => other,
    }
}

/// Do not re-litigate a permission the operator has already granted.
///
/// The reconciliation itself now lives in `hzr_exec::reconcile_host_grant`, beside the decision
/// type, because the hook was never the only surface that needed it: `hzr exec run` re-derived
/// the same verdict without the host's answer and refused commands this hook had approved.
/// Keeping a private copy here is what allowed those two answers to differ.
fn honor_host_permission_mode(input: &Value, decision: RewriteDecision) -> RewriteDecision {
    reconcile_host_grant(decision, host_grants_execution(input))
}

/// Carry the host's decision to every process the approved command starts.
///
/// The hook already exports the session and the evasion attribution through the approved
/// command's environment; the host's execution grant travels the same way, so a nested
/// `hzr exec run`, the pinned engine, and any agent below them all read one answer instead of
/// inventing three. The grant names only a keyed digest of the session, never the raw
/// identifier, and it is bounded in time — see `HostExecutionGrant::authorize`.
#[cfg(unix)]
fn attach_host_grant(input: &Value, decision: RewriteDecision) -> RewriteDecision {
    if !host_grants_execution(input) {
        return decision;
    }
    let Some(mode) = ["permission_mode", "permissionMode"]
        .into_iter()
        .find_map(|key| input.get(key).and_then(Value::as_str))
        .and_then(HostPermissionMode::parse)
    else {
        return decision;
    };
    let Some(session) = input
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|session| !session.is_empty())
    else {
        // A grant that cannot name its session can never be validated by the reader, and an
        // unvalidatable grant must not be minted at all.
        return decision;
    };
    let applied = host_grant_applied(&decision);
    let RewriteDecision::AllowRewrite {
        command,
        source,
        reason,
    } = decision
    else {
        return decision;
    };
    let Ok(rendered) = render_command(&command) else {
        return RewriteDecision::AllowRewrite {
            command,
            source,
            reason,
        };
    };
    // Match the export statement, not the bare variable name. A command that merely *mentions*
    // the variable — a test asserting on it, a script that reads it — is not a command that
    // already carries it, and treating the two as the same silently skipped the attachment.
    if rendered.contains(&already_exported(HOST_EXECUTION_GRANT_ENV)) {
        return RewriteDecision::AllowRewrite {
            command,
            source,
            reason,
        };
    }
    let grant = HostExecutionGrant {
        mode,
        granted_for_session: privacy_identity_hash("session", session),
        granted_at_ms: unix_millis_now(),
        source: "claude_code_pre_tool_use".into(),
    };
    let Ok(encoded) = serde_json::to_string(&grant) else {
        return RewriteDecision::AllowRewrite {
            command,
            source,
            reason,
        };
    };
    let granted_command = exported_hook_command(HOST_EXECUTION_GRANT_ENV, &encoded, &rendered);
    let granted_command = if applied {
        exported_hook_command(HOST_GRANT_APPLIED_ENV, "1", &granted_command)
    } else {
        granted_command
    };
    RewriteDecision::AllowRewrite {
        command: CanonicalCommand::shell(granted_command),
        source,
        reason,
    }
}

#[cfg(windows)]
fn attach_host_grant(_input: &Value, decision: RewriteDecision) -> RewriteDecision {
    decision
}

/// Mint a grant from the live hook payload.
///
/// Returns `None` unless the host both grants execution and names the session, because a grant
/// that cannot be tied to a session can never be validated downstream and must not be created.
fn host_grant_from_input(input: &Value) -> Option<HostExecutionGrant> {
    if !host_grants_execution(input) {
        return None;
    }
    let mode = ["permission_mode", "permissionMode"]
        .into_iter()
        .find_map(|key| input.get(key).and_then(Value::as_str))
        .and_then(HostPermissionMode::parse)?;
    let session = input
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|session| !session.is_empty())?;
    Some(HostExecutionGrant {
        mode,
        granted_for_session: privacy_identity_hash("session", session),
        granted_at_ms: unix_millis_now(),
        source: "claude_code_pre_tool_use".into(),
    })
}

fn unix_millis_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn hzr_policy_rewrite(replacement: hzr_core::RawReplacement) -> RewriteDecision {
    RewriteDecision::AllowRewrite {
        command: CanonicalCommand::shell(replacement.suggestion.clone()),
        source: RewriteSource::HzrPolicy,
        reason: format!(
            "`{}` selected a higher-output route. {}. HZR automatically selected the \
             lower-output first-class route.",
            replacement.tool, replacement.rationale
        ),
    }
}

async fn fallback_decision(config: &Config, raw: &str, cwd: &Path) -> RtkRewriteOutcome {
    let adapter = PinnedRtkAdapter::detect(RtkAdapterConfig {
        binary: config.engines.binary("rtk"),
        runtime_paths: Some(ForkRuntimePaths::from_data_root(&config.data_dir)),
        probe_timeout_ms: HOOK_TIMEOUT.as_millis() as u64,
        rewrite_timeout_ms: HOOK_TIMEOUT.as_millis() as u64,
    })
    .await;
    let fidelity = raw_fidelity_request(raw);
    let command = match fidelity {
        RawFidelityRequest::NotRequested => hzr_core::managed_raw_payload(raw).unwrap_or(raw),
        RawFidelityRequest::MissingReason => {
            return RtkRewriteOutcome {
                decision: RewriteDecision::Ask {
                    proposed: None,
                    reason: "HZR_RAW_FIDELITY=1 requires a closed HZR_RAW_FIDELITY_REASON".into(),
                },
                evasion: None,
                accounting_correlation_id: None,
            };
        }
        RawFidelityRequest::InvalidReason => {
            return RtkRewriteOutcome {
                decision: RewriteDecision::Ask {
                    proposed: None,
                    reason: "HZR_RAW_FIDELITY_REASON is not an allowed fidelity reason".into(),
                },
                evasion: None,
                accounting_correlation_id: None,
            };
        }
        RawFidelityRequest::Authorized { payload, .. } => {
            if let Some(replacement) = first_class_replacement(raw) {
                return RtkRewriteOutcome {
                    decision: hzr_policy_rewrite(replacement),
                    evasion: None,
                    accounting_correlation_id: None,
                };
            }
            payload
        }
    };
    let authorized = matches!(fidelity, RawFidelityRequest::Authorized { .. });
    let canonical = CanonicalCommand::shell(command);
    let mut outcome = if authorized {
        adapter
            .decide_byte_fidelity_with_plan_in(&canonical, Some(cwd))
            .await
    } else {
        adapter.decide_with_plan_in(&canonical, Some(cwd)).await
    };
    if authorized
        && matches!(
            &outcome.decision,
            RewriteDecision::AllowRewrite {
                source: RewriteSource::Rtk {
                    route: hzr_exec::RtkRewriteRoute::Proxy,
                    ..
                },
                ..
            }
        )
    {
        outcome.decision = RewriteDecision::allow_raw(
            "authorized raw fidelity request has no byte-faithful managed equivalent",
        );
    }
    outcome
}

fn write_decision(input: &Value, decision: RewriteDecision, notice: Option<&str>) -> Result<()> {
    let mut output = match decision {
        RewriteDecision::AllowRaw { .. } => match notice {
            Some(notice) => json!({"systemMessage": notice}),
            None => return Ok(()),
        },
        RewriteDecision::AllowRewrite {
            command, reason, ..
        } => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason": reason,
                "updatedInput": updated_command(input, &command)?,
            }
        }),
        RewriteDecision::Ask { proposed, reason } => {
            let mut hook = json!({
                "hookEventName": "PreToolUse",
                "permissionDecision": "ask",
                "permissionDecisionReason": reason,
            });
            if let Some(command) = proposed {
                hook["updatedInput"] = updated_command(input, &command)?;
            }
            json!({"hookSpecificOutput": hook})
        }
        RewriteDecision::Deny { reason } => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        }),
    };
    if let Some(notice) = notice {
        output["systemMessage"] = Value::String(notice.to_owned());
    }
    let host = if input["_hzr_host"].as_str() == Some("codex") {
        crate::host_hooks::HookHost::Codex
    } else {
        crate::host_hooks::HookHost::Claude
    };
    if let Some(output) =
        crate::host_hooks::adapt_response(host, input, output, host_grants_execution(input))
    {
        serde_json::to_writer(io::stdout().lock(), &output)?;
        io::stdout().lock().write_all(b"\n")?;
    }
    Ok(())
}

fn updated_command(input: &Value, command: &CanonicalCommand) -> Result<Value> {
    let mut tool_input = input
        .get("tool_input")
        .cloned()
        .context("Bash hook input has no tool_input")?;
    tool_input["command"] = Value::String(render_command(command)?);
    Ok(tool_input)
}

fn render_command(command: &CanonicalCommand) -> Result<String> {
    match command {
        CanonicalCommand::Shell { command, .. } => Ok(command.clone()),
        CanonicalCommand::Argv { program, args } => {
            let mut words = Vec::with_capacity(args.len() + 1);
            words.push(shell_quote(program));
            words.extend(args.iter().map(|argument| shell_quote(argument)));
            Ok(words.join(" "))
        }
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Hook-local session counters.
///
/// These count what the hook itself saw and prevented. Anything measured in tokens comes from
/// the ledger instead: this file cannot observe how much output a command would have delivered,
/// and a second accounting path that guesses is worse than one that abstains. An earlier
/// `avoidable_tokens_estimated` field here had no writer at all, so the shadow budget's token
/// half reported a constant zero and could never calibrate its own threshold.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct SessionFeedback {
    operations: u64,
    corrections: u64,
    native_denials: u64,
    nudged: bool,
    host_grant_seen: bool,
    accounting_degraded: bool,
    accounting_was_degraded: bool,
    session_started_at_unix: Option<u64>,
    accounting_degraded_started_at_unix: Option<u64>,
    accounting_degraded_seconds: u64,
    accounting_degraded_operations: u64,
    /// Tool calls seen since the last user prompt.
    ///
    /// The first call of a turn is the only position at which a filter cannot invalidate a request
    /// prefix that already reached the provider, so this counter is what makes
    /// `FilterPlacement::TurnBoundary` expressible without asking the harness for turn metadata it
    /// does not send.
    operations_this_turn: u64,
    /// History-mutating operations that ran raw because the placement policy declined to filter
    /// them mid-turn.
    ///
    /// Counted so the trade is visible: a policy that protects the cached prefix necessarily gives
    /// up reduction, and an operator comparing arms needs the size of what was given up.
    placement_deferred_operations: u64,
    /// Append-class operations filtered mid-turn under `TurnBoundary` because appending a shaped
    /// tool result cannot invalidate the cached prefix.
    ///
    /// This is the reduction the corrected causal model keeps and the previous
    /// defer-everything rule threw away; the counter is what lets an arm comparison size it.
    #[serde(default)]
    placement_append_filtered_operations: u64,
    /// 0.8.2: model id reported by the host status line (Claude Code sends `model.id`), so the
    /// Stop scorecard prices avoided tokens at the model actually in use instead of the
    /// configured default.
    #[serde(default)]
    observed_model_id: Option<String>,
}

fn agent_identity(input: &Value) -> &'static str {
    let supplied = ["agent_type", "agent", "host"]
        .into_iter()
        .find_map(|key| input.get(key).and_then(Value::as_str))
        .unwrap_or("claude-code")
        .to_ascii_lowercase();
    if supplied.contains("codex") {
        "codex"
    } else if supplied.contains("cursor") {
        "cursor"
    } else {
        "claude-code"
    }
}

fn agent_attribution(input: &Value) -> String {
    let host = agent_identity(input);
    let identity = ["agent_id", "subagent_id"]
        .into_iter()
        .find_map(|key| input.get(key).and_then(Value::as_str))
        .or_else(|| {
            input
                .pointer("/tool_input/agent_id")
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty());
    identity.map_or_else(|| host.to_owned(), |identity| format!("{host}:{identity}"))
}

fn hook_workspace_root(config: &Config, input: &Value) -> Option<std::path::PathBuf> {
    let requested = input.get("cwd").and_then(Value::as_str)?;
    let requested_path = std::path::PathBuf::from(requested);
    let requested = match fs::canonicalize(&requested_path) {
        Ok(requested) => requested,
        Err(_) if config.activation.mode == ActivationMode::All && requested_path.is_absolute() => {
            requested_path
        }
        Err(_) => return None,
    };
    let registry = registered_workspaces(&config.data_dir);
    let registration = registry
        .registrations
        .iter()
        .filter(|registration| requested.starts_with(&registration.root))
        .max_by_key(|registration| registration.root.components().count());
    match config.activation.mode {
        ActivationMode::All => Some(
            registration
                .map(|registration| registration.root.clone())
                .unwrap_or(requested),
        ),
        ActivationMode::Selected => registration
            .filter(|registration| {
                config
                    .activation
                    .allows(&registration.repository_id, &registration.worktree_id)
            })
            .map(|registration| registration.root.clone()),
    }
}

fn session_state_path(config: &Config, input: &Value) -> Option<std::path::PathBuf> {
    let session = input.get("session_id")?.as_str()?.trim();
    if session.is_empty() {
        return None;
    }
    let subagent = ["agent_id", "subagent_id"]
        .into_iter()
        .find_map(|key| input.get(key).and_then(Value::as_str))
        .or_else(|| {
            input
                .pointer("/tool_input/agent_id")
                .and_then(Value::as_str)
        })
        .unwrap_or("root");
    let workspace = hook_workspace_root(config, input)?;
    let workspace_hash = privacy_identity_hash("workspace", workspace.to_string_lossy().as_ref());
    let digest = hex::encode(Sha256::digest(format!(
        "hook-session\0{session}\0{}\0{subagent}\0{}",
        agent_identity(input),
        workspace_hash
    )));
    Some(
        config
            .data_dir
            .join("hook-sessions")
            .join(format!("{digest}.json")),
    )
}

fn update_session(
    config: &Config,
    input: &Value,
    update: impl FnOnce(&mut SessionFeedback),
) -> Result<SessionFeedback> {
    update_session_at(config, input, unix_now(), update)
}

fn update_session_at(
    config: &Config,
    input: &Value,
    now_unix: u64,
    update: impl FnOnce(&mut SessionFeedback),
) -> Result<SessionFeedback> {
    let path = session_state_path(config, input).context("hook input has no session identity")?;
    let parent = path.parent().context("session state has no parent")?;
    fs::create_dir_all(parent)?;
    let lock_path = path.with_extension("lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    lock.lock_exclusive()?;
    let mut state = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to parse {}", path.display()))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => SessionFeedback::default(),
        Err(error) => return Err(error.into()),
    };
    state.session_started_at_unix.get_or_insert(now_unix);
    update(&mut state);
    let mut bytes = serde_json::to_vec(&state)?;
    bytes.push(b'\n');
    crate::adoption::atomic_write(&path, &bytes)?;
    FileExt::unlock(&lock)?;
    Ok(state)
}

fn read_session(config: &Config, input: &Value) -> Option<SessionFeedback> {
    let path = session_state_path(config, input)?;
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn scorecard_message(
    config: &Config,
    state: &SessionFeedback,
    session_summary: Option<&SessionEvasionSummary>,
    efficiency: Option<&SessionEfficiencySummary>,
    economics: Option<&SessionEconomicSummary>,
) -> String {
    // 0.8.2: a degraded interval no longer withholds the measured part of the session. The
    // measured totals are a lower bound and the unmeasured interval is reported beside them.
    let partial = state.accounting_was_degraded;
    let (potential, billed) = economic_message(
        config,
        state.observed_model_id.as_deref(),
        efficiency,
        economics,
        partial,
    );
    let coverage_note = if partial {
        format!(
            " | PARTIAL: {}s across {} operation(s) unmeasured while accounting was degraded ({}% of observed session)",
            session_degraded_seconds(state, unix_now()),
            state.accounting_degraded_operations,
            session_degraded_percent(state, unix_now()),
        )
    } else {
        String::new()
    };
    let (savings, commands) = match efficiency {
        Some(summary) if summary.operations > 0 => {
            let reduction_pct = if summary.baseline_tokens_estimated == 0 {
                0.0
            } else {
                summary.net_avoided_tokens_estimated as f64 * 100.0
                    / summary.baseline_tokens_estimated as f64
            };
            let zero_cause = match crate::stats::classify_zero_reduction_values(
                summary.net_avoided_tokens_estimated,
                summary.operations,
                summary.excluded_legacy_operations,
            ) {
                crate::stats::ZeroReductionCause::OnlyZeroCreditOperations => {
                    "; zero explained: every measured row is zero-credit by policy"
                }
                crate::stats::ZeroReductionCause::ExcludedHistory => {
                    "; zero explained: earlier accounting-policy rows are outside this view"
                }
                crate::stats::ZeroReductionCause::NoOperations => {
                    "; zero explained: no measured command executions"
                }
                crate::stats::ZeroReductionCause::NotZero => "",
            };
            let outcome = if summary.net_avoided_tokens_estimated >= 0 {
                format!(
                    "Saved (estimated net): {} tokens ({reduction_pct:.1}%; gross {}, regression {}; {} -> {}){zero_cause}; {potential}",
                    summary.net_avoided_tokens_estimated,
                    summary.gross_avoided_tokens_estimated,
                    summary.regression_tokens_estimated,
                    summary.baseline_tokens_estimated,
                    summary.delivered_tokens_estimated
                )
            } else {
                format!(
                    "Regression (estimated net): {} tokens ({reduction_pct:.1}%; gross saved {}, regression {}; {} -> {}); {potential}",
                    summary.net_avoided_tokens_estimated.unsigned_abs(),
                    summary.gross_avoided_tokens_estimated,
                    summary.regression_tokens_estimated,
                    summary.baseline_tokens_estimated,
                    summary.delivered_tokens_estimated,
                )
            };
            let top_commands = summary
                .top_commands
                .iter()
                .map(|command| format!("{} x{}", command.command, command.executions))
                .collect::<Vec<_>>()
                .join(", ");
            // The five figures on this line must partition the hook's own event count, or the
            // card repeats the defect it exists to fix: a measured total beside a larger
            // observed total with nothing explaining the difference.
            //
            // `total_observed_operations` counts current-policy rows in the *measured* stages
            // only; `stage_excluded_operations` and `excluded_legacy_operations` are disjoint
            // from it. So the unmeasured/native remainder is what is left inside the measured
            // stages, and the ledger holds the sum of all three buckets.
            let non_ratio = summary
                .total_observed_operations
                .saturating_sub(summary.operations);
            let ledger_rows = summary
                .total_observed_operations
                .max(summary.operations)
                .saturating_add(summary.stage_excluded_operations)
                .saturating_add(summary.excluded_legacy_operations);
            let hook_only = state.operations.saturating_sub(ledger_rows);
            (
                outcome,
                format!(
                    "Measured commands (ratio rows): {} | excluded: {} stage + {} unmeasured/native + {} earlier-policy | hook-only events: {}{coverage_note}\nTop measured: {}",
                    summary.operations,
                    summary.stage_excluded_operations,
                    non_ratio,
                    summary.excluded_legacy_operations,
                    hook_only,
                    if top_commands.is_empty() { "none" } else { &top_commands },
                ),
            )
        }
        Some(summary) => (
            format!(
                "Savings: not measured yet (no session command executions); {potential}"
            ),
            format!(
                "Measured commands (ratio rows): 0 | observed ledger rows: {} | hook-only events: {}{coverage_note}",
                summary.total_observed_operations,
                state.operations.saturating_sub(summary.total_observed_operations),
            ),
        ),
        None if state.accounting_was_degraded => (
            "Savings: unknown (session accounting was degraded; partial ledger totals withheld); potential public-list value unknown".to_owned(),
            format!(
                "Measured commands: partial and withheld | hook events: {} | ACCOUNTING: DEGRADED DURING SESSION | missing coverage: {}s across {} operation(s) ({}% of observed session)",
                state.operations,
                session_degraded_seconds(state, unix_now()),
                state.accounting_degraded_operations,
                session_degraded_percent(state, unix_now()),
            ),
        ),
        None => (
            format!("Savings: unknown (ledger unavailable, not zero); {potential}"),
            "Measured commands: unknown | Top: unknown".to_owned(),
        ),
    };
    let partial_evidence = if partial {
        " | leakage covers measured operations only"
    } else {
        ""
    };
    let policy = match session_summary {
        Some(summary) => {
            let top_class = match summary.top_class {
                Some(EvasionClass::E10CapabilityGap) => "e10-capability-gap",
                Some(class) => class.as_str(),
                None => "none",
            };
            let leakage_meaning = if summary.avoidable_operations == 0 {
                "good: no proven avoidable bypass executed"
            } else {
                "recoverable output escaped the efficient route"
            };
            let prevented = state
                .corrections
                .max(summary.policy_corrections + summary.policy_denials);
            let asked = if state.host_grant_seen && summary.policy_asks > 0 {
                format!(
                    "asked {} (PROPAGATION FAILURE: a host-granted session must ask 0)",
                    summary.policy_asks
                )
            } else {
                format!("asked {}", summary.policy_asks)
            };
            format!(
                "Policy: prevented {} ({} native denial); {asked}; avoidable leakage {} ops / {} tokens ({leakage_meaning})\nEvidence: prevented output not estimated | top evasion {top_class} | grant-applied operations {} | hook events {}{partial_evidence}",
                prevented,
                state.native_denials,
                summary.avoidable_operations,
                summary.avoidable_tokens,
                summary.host_grant_applied_operations,
                state.operations,
            )
        }
        None if state.accounting_was_degraded => format!(
            "Policy: prevented {} ({} native denial); avoidable leakage unknown\nEvidence: session spent time degraded, so ledger-derived leakage is partial and withheld | hook events {}",
            state.corrections, state.native_denials, state.operations,
        ),
        None => format!(
            "Policy: prevented {} ({} native denial); avoidable leakage unknown\nEvidence: ledger unavailable, so leakage is unknown rather than zero | prevented output not estimated | hook events {}",
            state.corrections, state.native_denials, state.operations,
        ),
    };
    format!(
        "HZR session ROI\n{savings}\n{billed}\n{commands}\n{policy}\nShadow guard: T3 observe-only | limit {} ops / {} tokens",
        SESSION_BYPASS_COUNT_BUDGET, SESSION_BYPASS_TOKEN_BUDGET,
    )
}

fn economic_message(
    config: &Config,
    observed_model: Option<&str>,
    efficiency: Option<&SessionEfficiencySummary>,
    economics: Option<&SessionEconomicSummary>,
    partial: bool,
) -> (String, String) {
    let billed = economics
        .and_then(|summary| summary.reported_actual.as_ref())
        .map(|amount| {
            format!(
                "Billed actual (user-supplied, unverified): saved {} {} ({} -> {})",
                amount.currency,
                format_signed_microunits(amount.savings_microunits),
                format_microunits(amount.baseline_microunits),
                format_microunits(amount.delivered_microunits),
            )
        })
        .unwrap_or_else(|| "Billed actual: not measured".to_owned());
    if !config.billing.public_estimate_enabled {
        return (
            "potential public-list value unavailable (opt-in disabled; not an invoice)".into(),
            billed,
        );
    }
    let Some(efficiency) = efficiency else {
        return (
            "potential public-list value unavailable (ledger unavailable; not an invoice)".into(),
            billed,
        );
    };
    let avoided_tokens = efficiency
        .net_avoided_tokens_estimated
        .max(0)
        .unsigned_abs();
    // 0.8.2: the host-reported model wins over the configured default when the catalog knows
    // it; an unknown observed model falls back to the configured selection.
    fn request<'a>(
        config: &'a Config,
        model: &'a str,
        avoided_tokens: u64,
    ) -> RawPublicEstimateRequest<'a> {
        RawPublicEstimateRequest {
            harness: &config.billing.harness,
            provider: &config.billing.provider,
            model,
            method: &config.billing.method,
            request_input_tokens: config.billing.request_input_tokens,
            basis: config.billing.effective_pricing_basis(),
            avoided_tokens,
        }
    }
    let observed = observed_model
        .filter(|_| config.billing.harness == "claude_code")
        .filter(|model| *model != config.billing.model);
    let estimate =
        load_pricing_catalog(config.billing.pricing_file.as_deref()).and_then(|catalog| {
            let configured = || {
                price_avoided_input_tokens(
                    &catalog,
                    request(config, &config.billing.model, avoided_tokens),
                )
            };
            match observed {
                Some(model) => {
                    price_avoided_input_tokens(&catalog, request(config, model, avoided_tokens))
                        .or_else(|_| configured())
                }
                None => configured(),
            }
        });
    let partial_note = if partial {
        " lower bound over measured operations only"
    } else {
        ""
    };
    let potential = match estimate {
        Ok(RawPublicEstimate {
            currency,
            savings_microunits,
            model,
            method,
            pricing_basis,
            price_table_identity,
            ..
        }) => format!(
            "potential public-list value {currency} {} preliminary{partial_note} ({model}/{method}, basis={pricing_basis}, catalog={price_table_identity}; not an invoice)",
            format_microunits(savings_microunits),
        ),
        Err(error) => {
            format!("potential public-list value unavailable ({error}; not an invoice)")
        }
    };
    (potential, billed)
}

fn format_microunits(value: u64) -> String {
    format!("{}.{:06}", value / 1_000_000, value % 1_000_000)
}

fn format_signed_microunits(value: i64) -> String {
    if value < 0 {
        format!("-{}", format_microunits(value.unsigned_abs()))
    } else {
        format_microunits(value.unsigned_abs())
    }
}

/// Emit bounded feedback for prompt and completion hooks. This hook is deliberately
/// failure-silent: state is advisory and must never block a user prompt or session stop.
pub async fn feedback(config: &Config) {
    let Ok(input) = read_input() else {
        return;
    };
    if hook_workspace_root(config, &input).is_none() {
        return;
    }
    let event = input
        .get("hook_event_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let coverage = match session_accounting_coverage(config, &input) {
        Ok(coverage) => coverage,
        Err(_) => {
            let _ = write_hook_json(json!({
                "hookSpecificOutput": {
                    "hookEventName": event,
                    "additionalContext": "HZR ACCOUNTING UNKNOWN: the accounting coverage journal failed integrity validation. Run `hzr doctor` before trusting savings."
                }
            }));
            return;
        }
    };
    let mut state = match read_session(config, &input) {
        Some(state) => state,
        None if session_state_path(config, &input).is_some_and(|path| path.exists()) => {
            let _ = write_hook_json(json!({
                "hookSpecificOutput": {
                    "hookEventName": event,
                    "additionalContext": "HZR ACCOUNTING UNKNOWN: session accounting state failed integrity validation and was not reset. Run `hzr doctor` before trusting savings for this session."
                }
            }));
            return;
        }
        None => SessionFeedback::default(),
    };
    if let Some(coverage) = &coverage {
        let degraded = !coverage.live_complete;
        if state.accounting_degraded != degraded {
            let _ = accounting_transition(config, &input, degraded);
            state = read_session(config, &input).unwrap_or(state);
        }
    }
    let session_summaries =
        input
            .get("session_id")
            .and_then(Value::as_str)
            .and_then(|session_id| {
                let ledger = Ledger::open(&config.data_dir.join("ledger/hzr.sqlite")).ok()?;
                let project_path = input
                    .get("cwd")
                    .and_then(Value::as_str)
                    .and_then(|cwd| registered_workspace_root(config, cwd));
                Some((
                    ledger
                        .session_evasion_summary(session_id, FidelityAllowance::default())
                        .ok(),
                    ledger.session_efficiency_summary(session_id).ok(),
                    project_path.as_deref().and_then(|project_path| {
                        ledger
                            .session_economic_summary(session_id, project_path)
                            .ok()
                    }),
                ))
            });
    let session_summary = session_summaries
        .as_ref()
        .and_then(|(evasion, _, _)| evasion.as_ref());
    let crosses_threshold = state.corrections >= SESSION_CORRECTION_NUDGE
        || session_summary.is_some_and(|summary| {
            summary.avoidable_operations > 0
                && summary.avoidable_share_pct >= SESSION_AVOIDABLE_SHARE_NUDGE
        });
    if state.operations == 0 && session_summary.is_none() {
        return;
    }
    // A prompt boundary is where the operator is actually reading. The transition notice rides a
    // `systemMessage` on the tool call that detected it, which is immediate but easy to scroll
    // past, and a status line has to be configured to exist. Restating the *current* state here
    // is what makes degradation impossible to miss without turning it into per-command noise:
    // a prompt boundary happens once per user turn by construction.
    // A new user prompt starts a new turn, so the next tool call is at a turn boundary.
    if event == "UserPromptSubmit" {
        let _ = update_session(config, &input, |state| state.operations_this_turn = 0);
    }
    if event == "UserPromptSubmit" && state.accounting_degraded {
        let _ = write_hook_json(json!({
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": "HZR ACCOUNTING DEGRADED: the ledger is not recording this session's operations, so savings and leakage for this interval are unknown rather than zero. Check `hzr daemon service status`.",
            }
        }));
        return;
    }
    match event {
        "UserPromptSubmit" if crosses_threshold && !state.nudged => {
            let updated = update_session(config, &input, |state| state.nudged = true);
            if updated.is_err() {
                return;
            }
            let _ = write_hook_json(json!({
                "hookSpecificOutput": {
                    "hookEventName": "UserPromptSubmit",
                    "additionalContext": format!(
                        "HZR: avoidable route share crossed the session threshold ({} corrections); use the prescribed first-class route.",
                        state.corrections,
                    )
                }
            }));
        }
        "Stop" | "SubagentStop" => {
            let efficiency = session_summaries
                .as_ref()
                .and_then(|(_, efficiency, _)| efficiency.as_ref());
            let economics = session_summaries
                .as_ref()
                .and_then(|(_, _, economics)| economics.as_ref());
            let _ = write_hook_json(json!({
                "systemMessage": scorecard_message(config, &state, session_summary, efficiency, economics)
            }));
        }
        _ => {}
    }
}

pub fn statusline(config: &Config) {
    let Ok(input) = read_input() else {
        return;
    };
    if hook_workspace_root(config, &input).is_none() {
        return;
    }
    if let Some(upstream) = statusline_upstream() {
        let bytes = serde_json::to_vec(&input).unwrap_or_default();
        if let Some(rendered) = run_statusline_upstream(&upstream, &bytes) {
            println!("{rendered}");
        }
    }
    remember_observed_model(config, &input); // 0.8.2
    let state = read_session(config, &input);
    let status = match session_accounting_coverage(config, &input) {
        Err(_) => "ACCOUNTING: UNKNOWN",
        Ok(Some(coverage)) if !coverage.live_complete => "ACCOUNTING: DEGRADED",
        Ok(Some(coverage)) if !coverage.historical_complete => {
            "ACCOUNTING: RECOVERED (SESSION PARTIAL)"
        }
        Ok(_) => accounting_statusline(state.as_ref()),
    };
    println!("{status}");
}

/// 0.8.2: the status line is the only host surface that names the model. Record it once per
/// change so the scorecard can price avoided tokens at that model.
fn remember_observed_model(config: &Config, input: &Value) {
    let Some(model) = input.pointer("/model/id").and_then(Value::as_str) else {
        return;
    };
    if model.is_empty() || model.len() > 128 {
        return;
    }
    let known = read_session(config, input)
        .is_some_and(|state| state.observed_model_id.as_deref() == Some(model));
    if known {
        return;
    }
    let model = model.to_owned();
    let _ = update_session_at(config, input, unix_now(), |state| {
        state.observed_model_id = Some(model);
    });
}

fn accounting_statusline(state: Option<&SessionFeedback>) -> &'static str {
    match state {
        Some(state) if state.accounting_degraded => "ACCOUNTING: DEGRADED",
        Some(state) if state.accounting_was_degraded => "ACCOUNTING: RECOVERED (SESSION PARTIAL)",
        Some(_) => "ACCOUNTING: COMPLETE",
        None => "ACCOUNTING: UNKNOWN",
    }
}

fn session_accounting_coverage(
    config: &Config,
    input: &Value,
) -> Result<Option<hzr_core::AccountingCoverageSnapshot>> {
    let Some(session) = input.get("session_id").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(workspace) = hook_workspace_root(config, input) else {
        return Ok(None);
    };
    let session_hash = privacy_identity_hash("session", session);
    let workspace_hash = privacy_identity_hash("workspace", workspace.to_string_lossy().as_ref());
    AccountingCoverageStore::new(&config.data_dir)
        .snapshot_for_context(&session_hash, &workspace_hash, unix_now())
        .map(Some)
        .map_err(anyhow::Error::from)
}

fn statusline_upstream() -> Option<String> {
    let encoded = std::env::var(STATUSLINE_UPSTREAM_ENV).ok()?;
    let bytes = hex::decode(encoded).ok()?;
    let value = serde_json::from_slice::<Value>(&bytes).ok()?;
    value
        .get("command")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn run_statusline_upstream(command: &str, input: &[u8]) -> Option<String> {
    #[cfg(unix)]
    let mut command_process = std::process::Command::new("/bin/sh");
    #[cfg(windows)]
    let mut command_process = std::process::Command::new("cmd.exe");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command_process.arg("-c").arg(command).process_group(0);
    }
    #[cfg(windows)]
    command_process.arg("/C").arg(command);
    let mut child = command_process
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let input = input.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        stdout
            .take(STATUSLINE_UPSTREAM_MAX_BYTES.saturating_add(1))
            .read_to_end(&mut output)
            .map(|_| output)
    });
    let deadline = Instant::now() + STATUSLINE_UPSTREAM_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                terminate_statusline(&mut child);
                let _ = writer.join();
                let _ = reader.join();
                return None;
            }
        }
    };
    writer.join().ok()?.ok()?;
    let output = reader.join().ok()?.ok()?;
    if !status.success() || output.len() as u64 > STATUSLINE_UPSTREAM_MAX_BYTES {
        return None;
    }
    let rendered = String::from_utf8_lossy(&output);
    let rendered = rendered.trim_end();
    (!rendered.is_empty()).then(|| rendered.chars().take(4_096).collect())
}

fn terminate_statusline(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let process_group = format!("-{}", child.id());
        let _ = std::process::Command::new("/bin/kill")
            .args(["-KILL", "--", &process_group])
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn registered_workspace_root(config: &Config, requested: &str) -> Option<String> {
    let requested = fs::canonicalize(requested).ok()?;
    let registry = registered_workspaces(&config.data_dir);
    deepest_registered_root(
        &requested,
        registry
            .registrations
            .iter()
            .map(|registration| registration.root.as_path()),
    )
    .and_then(Path::to_str)
    .map(str::to_owned)
}

fn deepest_registered_root<'a>(
    requested: &Path,
    roots: impl IntoIterator<Item = &'a Path>,
) -> Option<&'a Path> {
    roots
        .into_iter()
        .filter(|root| requested.starts_with(root))
        .max_by_key(|root| root.components().count())
}

async fn task(config: &Config, input: &Value) -> Result<()> {
    let subagent = input
        .pointer("/tool_input/subagent_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if subagent.eq_ignore_ascii_case("explore") {
        return write_hook_json(json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "Native Explore is replaced by the unified HZR planner. Use `hzr context plan <intent>` or `hzr rtk -- memory explore <path>`.",
            }
        }));
    }
    let Some(prompt) = input.pointer("/tool_input/prompt").and_then(Value::as_str) else {
        return Ok(());
    };
    if prompt.is_empty() || prompt.contains("## HZR Unified Context") {
        return Ok(());
    }
    let workspace = std::env::current_dir()
        .context("failed to resolve hook working directory")?
        .canonicalize()
        .context("failed to canonicalize hook working directory")?;
    let client = match DaemonClient::from_config(config) {
        Ok(client) => client,
        Err(_) => return Ok(()),
    };
    let response = match timeout(
        HOOK_TIMEOUT,
        client.context_plan(&ContextPlanApiRequest {
            workspace: workspace.to_string_lossy().into_owned(),
            intent: prompt.chars().take(700).collect(),
            max_tokens: None,
            no_memory: false,
            path: None,
            topic: None,
            search_limit: 10,
            memory_limit: 5,
        }),
    )
    .await
    {
        Ok(Ok(response)) => response,
        _ => return Ok(()),
    };
    let context = context_brief(&serde_json::to_value(&response)?);
    let mut tool_input = input
        .get("tool_input")
        .cloned()
        .context("agent hook input has no tool_input")?;
    tool_input["prompt"] = Value::String(format!(
        "## HZR Unified Context\n{context}\n\n---\n\n{prompt}"
    ));
    write_hook_json(json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "HZR injected one bounded graph-first + ICM context plan",
            "updatedInput": tool_input,
        }
    }))
}

/// Largest number of leads to put in front of a subagent. A plan is a starting point, not a
/// reading list; past a handful the brief competes with the task it was meant to serve.
const MAX_BRIEF_LEADS: usize = 12;

/// Render a context plan as a brief a subagent can act on.
///
/// The plan used to be prepended as a minified JSON envelope with no explanation — no
/// glossary, no statement of what the entries are, no instruction on what to do with them. A
/// subagent that cannot tell what the block is will either ignore it and re-derive everything,
/// or treat it as established fact. The second failure is the worse one: a plan candidate is a
/// ranked guess, so the brief has to say that out loud.
fn context_brief(plan: &Value) -> String {
    let leads: Vec<String> = plan
        .pointer("/pack/selected")
        .and_then(Value::as_array)
        .map(|selected| {
            selected
                .iter()
                .filter(|candidate| {
                    candidate.get("source").and_then(Value::as_str) != Some("memory")
                })
                .filter_map(|candidate| {
                    let path = candidate.get("path").and_then(Value::as_str)?;
                    let span = match (
                        candidate.get("line_start").and_then(Value::as_u64),
                        candidate.get("line_end").and_then(Value::as_u64),
                    ) {
                        (Some(start), Some(end)) => format!(":{start}-{end}"),
                        _ => String::new(),
                    };
                    let symbol = candidate
                        .get("symbol")
                        .and_then(Value::as_str)
                        .map(|symbol| format!(" ({symbol})"))
                        .unwrap_or_default();
                    Some(format!("- {path}{span}{symbol}"))
                })
                .take(MAX_BRIEF_LEADS)
                .collect()
        })
        .unwrap_or_default();

    if leads.is_empty() {
        return "HZR planned this task and found no code leads for it. Start from the task \
                itself; there is nothing here to confirm or rule out."
            .to_owned();
    }

    format!(
        "HZR ranked these as the most likely relevant places for the task below. They are \
         unverified leads, not findings: confirm each one before relying on it, and ignore any \
         that do not fit. Read them with `hzr rtk -- read <path> --from N --to M`, and search \
         with `hzr search \"<pattern>\" --mode exact` when a lead is wrong.\n\n{}",
        leads.join("\n")
    )
}

fn write_hook_json(value: Value) -> Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, &value)?;
    output.write_all(b"\n")?;
    Ok(())
}

/// Rewrites the daemon never saw, and therefore never entered in the usage ledger.
///
/// The distinction between the two counts is the whole point. `unreconciled_rewrites` is
/// an *open* gap — the daemon has not served a rewrite since these happened, so the ledger
/// is still missing them and coverage is genuinely incomplete. `lifetime_rewrites` is the
/// historical total, kept so that closing a gap never looks like erasing it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct AccountingCoverage {
    pub unreconciled_rewrites: usize,
    pub lifetime_rewrites: usize,
    pub daemon_unavailable_operations: usize,
    pub complete: bool,
    pub live_complete: bool,
    pub historical_complete: bool,
    pub open_intervals: usize,
    pub closed_intervals: usize,
    pub last_degraded_at_unix: Option<u64>,
    pub gap_started_at_unix: Option<u64>,
    pub last_recovered_at_unix: Option<u64>,
    pub open_gap_seconds: Option<u64>,
    pub closed_gap_seconds: u64,
    pub hook_missing_operations: u64,
    pub cli_missing_operations: u64,
    pub mcp_missing_operations: u64,
    pub fork_producer_missing_operations: u64,
    /// 0.8.3: rewrites served without the daemon (surface `rewrite_daemon`). Without this the
    /// per-producer breakdown could not be reconciled with the retained intervals or with
    /// `lifetime_rewrites`.
    pub rewrite_missing_operations: u64,
    /// 0.8.3: operations imported from the pre-typed counters; they have no producer surface.
    pub legacy_missing_operations: u64,
    // 0.8.1: receipt journals still waiting for the daemon (inventory, not lifetime totals).
    pub undrained_receipts: usize,
}

#[cfg(test)]
impl AccountingCoverage {
    /// The state of an installation that has never lost the daemon. `Default` cannot say
    /// this on its own, because a defaulted `complete: false` would read as a real gap.
    pub fn default_complete() -> Self {
        Self {
            complete: true,
            live_complete: true,
            historical_complete: true,
            ..Self::default()
        }
    }
}

fn degraded_log_path(config: &Config) -> std::path::PathBuf {
    config.data_dir.join("ledger/degraded-rewrites.log")
}

fn degraded_total_path(config: &Config) -> std::path::PathBuf {
    config.data_dir.join("ledger/degraded-rewrites.total")
}

fn daemon_unavailable_log_path(config: &Config) -> std::path::PathBuf {
    config
        .data_dir
        .join("ledger/daemon-unavailable-operations.log")
}

fn daemon_unavailable_total_path(config: &Config) -> std::path::PathBuf {
    config
        .data_dir
        .join("ledger/daemon-unavailable-operations.total")
}

pub(crate) fn record_accounting_gap_now(
    config: &Config,
    surface: AccountingGapSurface,
    workspace: Option<&str>,
    session_id: Option<&str>,
    operation_family: Option<&str>,
) -> Result<()> {
    record_accounting_gap(
        config,
        surface,
        workspace,
        session_id,
        operation_family,
        unix_now(),
    )
}

pub(crate) fn recover_accounting_gap_now(
    config: &Config,
    surface: AccountingGapSurface,
    workspace: Option<&str>,
    session_id: Option<&str>,
    operation_family: Option<&str>,
) -> Result<bool> {
    AccountingCoverageStore::new(&config.data_dir)
        .recover(accounting_gap_event(
            surface,
            workspace,
            session_id,
            operation_family,
            unix_now(),
        ))
        .map(|recovered| recovered > 0)
        .map_err(anyhow::Error::from)
}

pub(crate) fn record_accounting_gap(
    config: &Config,
    surface: AccountingGapSurface,
    workspace: Option<&str>,
    session_id: Option<&str>,
    operation_family: Option<&str>,
    at_unix: u64,
) -> Result<()> {
    AccountingCoverageStore::new(&config.data_dir)
        .record_missing(accounting_gap_event(
            surface,
            workspace,
            session_id,
            operation_family,
            at_unix,
        ))
        .map(|_| ())
        .map_err(anyhow::Error::from)
}

fn accounting_gap_event(
    surface: AccountingGapSurface,
    workspace: Option<&str>,
    session_id: Option<&str>,
    operation_family: Option<&str>,
    at_unix: u64,
) -> AccountingGapEvent {
    AccountingGapEvent {
        surface,
        workspace_hash: workspace.map(|value| privacy_identity_hash("workspace", value)),
        session_hash: session_id.map(|value| privacy_identity_hash("session", value)),
        operation_family: operation_family.map(str::to_owned),
        at_unix,
    }
}

fn daemon_unavailable_operations(config: &Config) -> Result<usize> {
    let total_path = daemon_unavailable_total_path(config);
    match fs::read_to_string(&total_path) {
        Ok(content) => {
            return content
                .trim()
                .parse::<usize>()
                .with_context(|| format!("failed to parse {}", total_path.display()));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", total_path.display()));
        }
    }
    let path = daemon_unavailable_log_path(config);
    match fs::read_to_string(&path) {
        Ok(content) => Ok(content.lines().filter(|line| line.trim() == "1").count()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn record_degraded_rewrite_for_input(config: &Config, input: &Value) -> Result<()> {
    let workspace = hook_workspace_root(config, input);
    record_accounting_gap(
        config,
        AccountingGapSurface::RewriteDaemon,
        workspace.as_deref().and_then(Path::to_str),
        input.get("session_id").and_then(Value::as_str),
        Some("rewrite"),
        unix_now(),
    )
}

#[cfg(test)]
fn record_degraded_rewrite_at(config: &Config, timestamp: u64) -> Result<()> {
    record_accounting_gap(
        config,
        AccountingGapSurface::RewriteDaemon,
        None,
        None,
        Some("rewrite"),
        timestamp,
    )
}

/// Fold the open gap into the lifetime total once the daemon proves it is serving again.
///
/// Called after every successful managed rewrite, so the common case — nothing pending —
/// must not touch the filesystem beyond one `metadata` probe.
#[cfg(test)]
fn close_open_accounting_gaps(config: &Config) -> Result<()> {
    AccountingCoverageStore::new(&config.data_dir)
        .recover(accounting_gap_event(
            AccountingGapSurface::RewriteDaemon,
            None,
            None,
            Some("rewrite"),
            unix_now(),
        ))
        .map(|_| ())
        .map_err(anyhow::Error::from)
}

fn recover_accounting_for_input(config: &Config, input: &Value) -> Result<()> {
    let workspace = hook_workspace_root(config, input);
    AccountingCoverageStore::new(&config.data_dir)
        .recover(accounting_gap_event(
            AccountingGapSurface::RewriteDaemon,
            workspace.as_deref().and_then(Path::to_str),
            input.get("session_id").and_then(Value::as_str),
            Some("rewrite"),
            unix_now(),
        ))
        .map(|_| ())
        .map_err(anyhow::Error::from)
}

fn read_degraded_log(config: &Config) -> Result<Vec<u64>> {
    let path = degraded_log_path(config);
    match fs::read_to_string(&path) {
        Ok(content) => Ok(content
            .lines()
            .filter_map(|line| line.trim().parse::<u64>().ok())
            .collect()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn read_lifetime_total(config: &Config) -> Result<Option<usize>> {
    let path = degraded_total_path(config);
    match fs::read_to_string(&path) {
        Ok(content) => Ok(content.trim().parse::<usize>().ok()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

pub fn degraded_rewrite_coverage(config: &Config) -> Result<AccountingCoverage> {
    degraded_rewrite_coverage_at(config, unix_now())
}

fn degraded_rewrite_coverage_at(config: &Config, now_unix: u64) -> Result<AccountingCoverage> {
    let store = AccountingCoverageStore::new(&config.data_dir);
    if !store.exists() {
        let pending = read_degraded_log(config)?;
        let closed = read_lifetime_total(config)?.unwrap_or(0);
        let daemon_unavailable = daemon_unavailable_operations(config)?;
        store.import_legacy(
            u64::try_from(closed.saturating_add(daemon_unavailable)).unwrap_or(u64::MAX),
        )?;
        for timestamp in pending {
            store.record_missing(AccountingGapEvent {
                surface: AccountingGapSurface::RewriteDaemon,
                workspace_hash: None,
                session_hash: None,
                operation_family: Some("rewrite".into()),
                at_unix: timestamp,
            })?;
        }
    }
    let snapshot = store.snapshot(now_unix)?;
    let inventory = hzr_exec::accounting_journal_inventory(&config.data_dir)?;
    let undrained = inventory.undrained_receipts;
    let lifetime_rewrites = usize::try_from(snapshot.lifetime_missing_operations)
        .unwrap_or(usize::MAX)
        .saturating_add(undrained);
    let unreconciled_rewrites = usize::try_from(snapshot.open_missing_operations)
        .unwrap_or(usize::MAX)
        .saturating_add(undrained);
    let daemon_unavailable_operations = usize::try_from(
        snapshot
            .hook_missing_operations
            .saturating_add(snapshot.cli_missing_operations)
            .saturating_add(snapshot.mcp_missing_operations)
            .saturating_add(snapshot.fork_producer_missing_operations),
    )
    .unwrap_or(usize::MAX);
    let gap_started_at_unix = match (
        snapshot.gap_started_at_unix,
        inventory.oldest_modified_at_unix,
    ) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    };
    Ok(AccountingCoverage {
        unreconciled_rewrites,
        lifetime_rewrites,
        daemon_unavailable_operations,
        complete: snapshot.live_complete && snapshot.historical_complete && undrained == 0,
        live_complete: snapshot.live_complete && undrained == 0,
        historical_complete: snapshot.historical_complete && undrained == 0,
        open_intervals: snapshot.open_intervals + usize::from(undrained > 0),
        closed_intervals: snapshot.closed_intervals,
        last_degraded_at_unix: snapshot.last_failure_at_unix,
        gap_started_at_unix,
        last_recovered_at_unix: snapshot.last_recovered_at_unix,
        open_gap_seconds: gap_started_at_unix.map(|started| now_unix.saturating_sub(started)),
        closed_gap_seconds: snapshot.closed_gap_seconds,
        hook_missing_operations: snapshot.hook_missing_operations,
        cli_missing_operations: snapshot.cli_missing_operations,
        mcp_missing_operations: snapshot.mcp_missing_operations,
        fork_producer_missing_operations: snapshot
            .fork_producer_missing_operations
            .saturating_add(u64::try_from(undrained).unwrap_or(u64::MAX)),
        rewrite_missing_operations: snapshot.rewrite_missing_operations, // 0.8.3
        legacy_missing_operations: snapshot.legacy_missing_operations,   // 0.8.3
        undrained_receipts: undrained,                                   // 0.8.1
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn session_degraded_seconds(state: &SessionFeedback, now_unix: u64) -> u64 {
    state.accounting_degraded_seconds.saturating_add(
        state
            .accounting_degraded_started_at_unix
            .map_or(0, |started| now_unix.saturating_sub(started)),
    )
}

fn session_degraded_percent(state: &SessionFeedback, now_unix: u64) -> u64 {
    let elapsed = state
        .session_started_at_unix
        .map_or(0, |started| now_unix.saturating_sub(started));
    if elapsed == 0 {
        0
    } else {
        session_degraded_seconds(state, now_unix)
            .saturating_mul(100)
            .checked_div(elapsed)
            .unwrap_or(0)
            .min(100)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use hzr_core::{
        AccountingGapSurface, Config, EconomicAmount, FidelityAllowance, Ledger,
        OperationAttribution, OperationChannel, OperationMeasurement, OperationRoute,
        ReceiptProvenance, SessionEconomicSummary, SessionEfficiencySummary, SessionEvasionSummary,
    };
    use hzr_protocol::{
        EnforcementTier, EvasionAttribution, EvasionClass, EvasionPathForm, FidelityValidation,
        PrefixEffect,
    };
    use tempfile::tempdir;

    use crate::adoption::NativeToolMode;
    use hzr_exec::{CanonicalCommand, PINNED_RTK_VERSION, RewriteDecision, RewriteSource};

    use super::anti_evasion_fixture::{ProbeDecision, ProbeLayer, ProbeNativeMode, ProbeSurface};
    use super::{
        HookFidelityPreflight, SessionFeedback, accounting_statusline, accounting_transition,
        accounting_transition_at, agent_attribution, agent_identity, apply_filter_placement,
        attach_hook_evasion, attach_host_grant, attach_policy_feedback, attach_session_attribution,
        close_open_accounting_gaps, context_brief, deepest_registered_root,
        degraded_rewrite_coverage, degraded_rewrite_coverage_at, fallback_decision,
        honor_host_permission_mode, hook_fidelity_preflight, native_observation_policy,
        read_session, reconcile_host_grant, record_accounting_gap_now, record_degraded_rewrite_at,
        remember_observed_model, render_command, run_statusline_upstream, scorecard_message,
        steer_to_first_class, update_session,
    };

    #[cfg(unix)]
    #[test]
    fn upstream_statusline_timeout_is_bounded() {
        let started = std::time::Instant::now();
        assert!(run_statusline_upstream("sleep 30", b"{}").is_none());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn upstream_statusline_output_is_bounded() {
        assert!(run_statusline_upstream("yes x | head -c 2097152", b"{}",).is_none());
    }

    #[test]
    fn deepest_registered_workspace_binds_descendants_without_crossing_nested_roots() {
        let directory = tempdir().expect("temporary directory");
        let parent = directory.path().join("repo");
        let parent_child = parent.join("subdirectory");
        let nested = parent.join("nested");
        let nested_child = nested.join("subdirectory");
        fs::create_dir_all(&parent_child).expect("parent child");
        fs::create_dir_all(&nested_child).expect("nested child");
        let parent = fs::canonicalize(parent).expect("canonical parent");
        let parent_child = fs::canonicalize(parent_child).expect("canonical parent child");
        let nested = fs::canonicalize(nested).expect("canonical nested");
        let nested_child = fs::canonicalize(nested_child).expect("canonical nested child");
        let roots = [parent.as_path(), nested.as_path()];

        assert_eq!(
            deepest_registered_root(&parent_child, roots),
            Some(parent.as_path())
        );
        assert_eq!(
            deepest_registered_root(&nested_child, roots),
            Some(nested.as_path())
        );
        assert_eq!(
            deepest_registered_root(&parent, roots),
            Some(parent.as_path())
        );
    }

    #[test]
    fn daemon_unavailable_telemetry_is_a_bounded_durable_counter() {
        let directory = tempdir().expect("temporary directory");
        let config = config(directory.path());
        for _ in 0..100 {
            record_accounting_gap_now(&config, AccountingGapSurface::Mcp, None, None, Some("test"))
                .expect("record degraded operation");
        }

        let coverage = degraded_rewrite_coverage(&config).expect("coverage");
        assert_eq!(coverage.daemon_unavailable_operations, 100);
        assert_eq!(coverage.mcp_missing_operations, 100);
        let journal = config.data_dir.join("ledger/accounting-coverage.json");
        assert!(fs::metadata(journal).expect("journal metadata").len() <= 1_024);
    }

    #[test]
    fn native_modes_never_classify_exact_native_work_as_avoidable() {
        for tool in ["Read", "Grep", "Glob", "Edit", "Write"] {
            for mode in [
                NativeToolMode::Observe,
                NativeToolMode::Steer,
                NativeToolMode::Strict,
            ] {
                let (route, attribution) = native_observation_policy(tool, mode);
                assert_eq!(route, hzr_core::OperationRoute::NativeUnaccounted);
                assert!(!attribution.avoidable);
                assert_eq!(attribution.tier, EnforcementTier::T0TransparentRewrite);
            }
        }
    }

    /// Tie the matrix's expected wording to the implementation that produces it.
    ///
    /// The matrix used to assert only the verdict, so all 25 Ask cases passed while telling the
    /// agent nothing. Asserting the text alone would drift the moment the wording changed, so
    /// each shell expectation must match a prescription the closed taxonomy actually emits.
    #[test]
    fn acceptance_gate_every_ask_and_deny_asserts_an_implemented_prescription() {
        const CLASSES: [EvasionClass; 10] = [
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
        ];
        let probes = super::anti_evasion_fixture::load_anti_evasion_probes();
        let mut asserted = 0;
        for probe in &probes {
            if !matches!(probe.decision, ProbeDecision::Ask | ProbeDecision::Deny) {
                continue;
            }
            asserted += 1;
            assert!(
                !probe.expect_reason_contains.is_empty(),
                "{} must assert what its reason tells the agent",
                probe.id
            );
            if probe.surface == ProbeSurface::Native || probe.id == "fidelity-missing-reason" {
                // Native entries are historical engine fixtures; current host adapters never
                // deny for optimization. Fidelity preflight has its own regression coverage.
                continue;
            }
            let prescribed = probe.expect_reason_contains.iter().any(|expected| {
                CLASSES
                    .iter()
                    .any(|class| class.prescription().contains(expected.as_str()))
            });
            assert!(
                prescribed,
                "{} expects wording no evasion class prescribes: {:?}",
                probe.id, probe.expect_reason_contains
            );
            assert!(
                probe.expect_reason_contains.iter().any(|expected| CLASSES
                    .iter()
                    .any(|class| class.as_str() == expected.as_str())),
                "{} does not assert its evasion class",
                probe.id
            );
        }
        assert_eq!(asserted, 22, "every Ask and Deny case carries an assertion");
    }

    #[test]
    fn legacy_native_fixture_modes_all_use_nonblocking_observation() {
        // The imported fixture documents the old deny/retry policy, not current host permissions.
        let probes = super::anti_evasion_fixture::load_anti_evasion_probes();
        let native = probes.iter().filter(|probe| {
            probe.layer == ProbeLayer::Root && probe.surface == ProbeSurface::Native
        });
        let mut count = 0;
        for probe in native {
            count += 1;
            let mode = match probe.mode.expect("native mode") {
                ProbeNativeMode::Observe => NativeToolMode::Observe,
                ProbeNativeMode::Steer => NativeToolMode::Steer,
                ProbeNativeMode::Strict => NativeToolMode::Strict,
            };
            let (_, attribution) = native_observation_policy(
                probe.tool.as_deref().expect("native fixture tool"),
                mode,
            );
            assert!(!attribution.avoidable, "{}", probe.id);
            assert_eq!(attribution.class, EvasionClass::E10CapabilityGap);
        }
        assert_eq!(count, 15);
    }

    #[test]
    fn acceptance_gate_shared_fixture_reaches_hook_postprocessing() {
        let probes = super::anti_evasion_fixture::load_anti_evasion_probes();
        let root_shell = probes
            .iter()
            .filter(|probe| probe.layer == ProbeLayer::Root && probe.surface == ProbeSurface::Shell)
            .collect::<Vec<_>>();
        assert_eq!(root_shell.len(), 5, "all root shell probes must execute");

        for probe in root_shell {
            let command = probe.command.as_deref().expect("root shell command");
            let daemon_decision = match probe.decision {
                ProbeDecision::Rewrite
                    if probe
                        .route
                        .as_deref()
                        .is_some_and(|route| route.starts_with("rtk ")) =>
                {
                    RewriteDecision::AllowRewrite {
                        command: CanonicalCommand::shell(
                            probe.route.as_deref().expect("managed rewrite route"),
                        ),
                        source: RewriteSource::Rtk {
                            version: PINNED_RTK_VERSION.into(),
                            route: hzr_exec::RtkRewriteRoute::Optimized,
                        },
                        reason: "typed daemon plan".into(),
                    }
                }
                ProbeDecision::Rewrite | ProbeDecision::Raw => allow_raw(),
                ProbeDecision::Ask => RewriteDecision::Ask {
                    proposed: None,
                    reason: "typed daemon policy".into(),
                },
                ProbeDecision::Proxy | ProbeDecision::Allow | ProbeDecision::Deny => {
                    assert!(
                        matches!(
                            probe.decision,
                            ProbeDecision::Rewrite | ProbeDecision::Ask | ProbeDecision::Raw
                        ),
                        "invalid root decision for {}",
                        probe.id
                    );
                    continue;
                }
            };
            let decision = steer_to_first_class(command, daemon_decision);
            match probe.decision {
                ProbeDecision::Rewrite => assert!(
                    proposed(&decision).is_some_and(
                        |route| route.ends_with(probe.route.as_deref().expect("route"))
                    ),
                    "root probe {} did not reach its hook route: {decision:?}",
                    probe.id
                ),
                ProbeDecision::Ask => assert!(
                    matches!(decision, RewriteDecision::Ask { .. }),
                    "root probe {} was not Ask: {decision:?}",
                    probe.id
                ),
                ProbeDecision::Raw => assert!(
                    matches!(decision, RewriteDecision::AllowRaw { .. }),
                    "root probe {} did not preserve fidelity: {decision:?}",
                    probe.id
                ),
                ProbeDecision::Proxy | ProbeDecision::Allow | ProbeDecision::Deny => {
                    assert!(
                        matches!(
                            probe.decision,
                            ProbeDecision::Rewrite | ProbeDecision::Ask | ProbeDecision::Raw
                        ),
                        "invalid root decision for {}",
                        probe.id
                    )
                }
            }
        }
    }

    /// The shadow budget must measure something.
    ///
    /// Its token half used to read a `SessionFeedback` field that no code path ever wrote, so
    /// every scorecard reported `tokens=0/250000` regardless of what the session did — a shadow
    /// window that can never calibrate the threshold it exists to calibrate. Both halves now
    /// come from the ledger, so an executed avoidable bypass has to move them.
    /// A host that already grants execution must not be prompted by HZR.
    ///
    /// HZR derives its verdict from the settings file, so an operator running in
    /// `bypassPermissions` with no `permissions` block still got an Ask on every rewritten
    /// command — a prompt answering a question they had already answered. Deny is different: it
    /// is an explicit rule and survives.
    #[test]
    fn acceptance_gate_bypass_permissions_is_not_re_litigated() {
        let bypass = serde_json::json!({"permission_mode": "bypassPermissions"});
        let default = serde_json::json!({"permission_mode": "default"});

        let proposed = honor_host_permission_mode(
            &bypass,
            RewriteDecision::Ask {
                proposed: Some(CanonicalCommand::shell("rtk ps aux")),
                reason: "fork-core permission policy requires approval".into(),
            },
        );
        assert!(
            matches!(&proposed, RewriteDecision::AllowRewrite { command, .. }
                if render_command(command).ok().as_deref() == Some("rtk ps aux")),
            "an approved host must run the managed form"
        );

        let unproposed = honor_host_permission_mode(
            &bypass,
            RewriteDecision::Ask {
                proposed: None,
                reason: "opaque wrapper".into(),
            },
        );
        assert!(matches!(unproposed, RewriteDecision::AllowRaw { .. }));

        let denied = honor_host_permission_mode(
            &bypass,
            RewriteDecision::Deny {
                reason: "explicit deny rule".into(),
            },
        );
        assert!(
            matches!(denied, RewriteDecision::Deny { .. }),
            "an explicit deny is a rule, not an absent one"
        );

        let untouched = honor_host_permission_mode(
            &default,
            RewriteDecision::Ask {
                proposed: None,
                reason: "opaque wrapper".into(),
            },
        );
        assert!(matches!(untouched, RewriteDecision::Ask { .. }));
    }

    #[test]
    fn acceptance_gate_host_grant_verdict_is_identical_across_policy_surfaces() {
        let bypass = serde_json::json!({"permission_mode": "bypassPermissions"});
        for probe in super::anti_evasion_fixture::load_anti_evasion_probes() {
            let decision = match probe.decision {
                ProbeDecision::Ask => RewriteDecision::Ask {
                    proposed: probe
                        .route
                        .clone()
                        .or_else(|| probe.command.clone())
                        .map(CanonicalCommand::shell),
                    reason: format!("{} requires approval", probe.id),
                },
                ProbeDecision::Deny => RewriteDecision::Deny {
                    reason: format!("{} is explicitly denied", probe.id),
                },
                _ => continue,
            };
            let hook = honor_host_permission_mode(&bypass, decision.clone());
            let exec = reconcile_host_grant(decision, true);
            assert_eq!(hook, exec, "cross-surface verdict drift for {}", probe.id);
            if probe.decision == ProbeDecision::Deny {
                assert!(
                    matches!(hook, RewriteDecision::Deny { .. }),
                    "grant weakened explicit deny for {}",
                    probe.id
                );
            }
        }
    }

    /// The turn-boundary arm exists so a billed-input benchmark can hold the cached request prefix
    /// still. Its first version deferred *every* mid-turn filter, which modelled a `PreToolUse`
    /// command rewrite as if it rewrote history; it does not — the shaped output is appended after
    /// the cached prefix. Under the default the behaviour must be byte-for-byte what shipped, or
    /// the benchmark comparing the two arms is comparing against something that already moved.
    #[test]
    fn acceptance_gate_filter_placement_defers_only_history_mutations_mid_turn() {
        let directory = tempdir().expect("temporary directory");
        let mut config = Config {
            data_dir: directory.path().to_path_buf(),
            ..Config::default()
        };
        let input = serde_json::json!({"session_id": "placement", "cwd": "/work"});
        let rewrite = || RewriteDecision::AllowRewrite {
            command: CanonicalCommand::shell("rtk read a"),
            source: RewriteSource::HzrPolicy,
            reason: "managed route".into(),
        };

        // Default arm: placement never intervenes, whatever the turn position or effect.
        for effect in [PrefixEffect::Append, PrefixEffect::HistoryMutation] {
            for _ in 0..3 {
                let _ = update_session(&config, &input, |state| {
                    state.operations_this_turn = state.operations_this_turn.saturating_add(1);
                });
                assert!(
                    matches!(
                        apply_filter_placement(&config, &input, effect, rewrite()),
                        RewriteDecision::AllowRewrite { .. }
                    ),
                    "the shipped default must keep filtering wherever the route applies"
                );
            }
        }
        let state = read_session(&config, &input).expect("session state");
        assert_eq!(
            (
                state.placement_deferred_operations,
                state.placement_append_filtered_operations
            ),
            (0, 0),
            "the default arm must not count placement decisions it never made"
        );

        // Turn-boundary arm: the turn's first operation filters regardless of effect.
        config.policy.filter_placement = hzr_protocol::FilterPlacement::TurnBoundary;
        for effect in [PrefixEffect::Append, PrefixEffect::HistoryMutation] {
            let _ = update_session(&config, &input, |state| state.operations_this_turn = 1);
            assert!(matches!(
                apply_filter_placement(&config, &input, effect, rewrite()),
                RewriteDecision::AllowRewrite { .. }
            ));
        }

        // Mid-turn, an appended tool result cannot move the cached prefix: it still filters, and
        // the reduction the old rule would have forgone is counted.
        let _ = update_session(&config, &input, |state| state.operations_this_turn = 2);
        assert!(
            matches!(
                apply_filter_placement(&config, &input, PrefixEffect::Append, rewrite()),
                RewriteDecision::AllowRewrite { .. }
            ),
            "an append-class filter mid-turn does not invalidate the prefix and must not be deferred"
        );

        // A history mutation mid-turn runs raw, with the forgone reduction counted rather than
        // hidden.
        let deferred =
            apply_filter_placement(&config, &input, PrefixEffect::HistoryMutation, rewrite());
        assert!(
            matches!(&deferred, RewriteDecision::AllowRaw { reason }
                if reason.contains("turn_boundary")
                    && reason.contains("history_mutation")
                    && reason.contains("no savings credit")),
            "a deferral must name the policy and the effect and admit it earns no credit: {deferred:?}"
        );
        let state = read_session(&config, &input).expect("session state");
        assert_eq!(
            (
                state.placement_deferred_operations,
                state.placement_append_filtered_operations
            ),
            (1, 1),
            "both sides of the trade must be measurable"
        );

        // A deny is not a prefix question.
        let denied = apply_filter_placement(
            &config,
            &input,
            PrefixEffect::HistoryMutation,
            RewriteDecision::Deny {
                reason: "explicit deny".into(),
            },
        );
        assert!(matches!(denied, RewriteDecision::Deny { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn acceptance_gate_bypass_permissions_returned_command_executes_end_to_end() {
        let input = serde_json::json!({
            "permission_mode": "bypassPermissions",
            "session_id": "live-repro-session"
        });
        let decision = honor_host_permission_mode(
            &input,
            RewriteDecision::Ask {
                proposed: Some(CanonicalCommand::shell(
                    "test \"$HZR_SESSION_ID\" = live-repro-session && \
                     test -n \"$HZR_HOST_EXECUTION_GRANT\" && \
                     test \"$HZR_INTERNAL_HOST_GRANT_APPLIED\" = 1",
                )),
                reason: "nested exec policy requires approval".into(),
            },
        );
        let decision = attach_session_attribution(&input, decision);
        let decision = attach_host_grant(&input, decision);
        let command = match decision {
            RewriteDecision::AllowRewrite { command, .. } => Some(command),
            _ => None,
        }
        .expect("host-granted command was not executable");
        let rendered = render_command(&command).expect("returned shell command");
        let status = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(rendered)
            .status()
            .expect("returned command starts");
        assert_eq!(status.code(), Some(0), "live repro must not exit 77");
    }

    /// 0.8.3: an ordinary classification stays in the accounting registration; only the fidelity
    /// hatch, which has no registration, is still exported into the command it approves.
    #[cfg(unix)]
    #[test]
    fn ordinary_classification_is_not_exported_into_the_approved_command() {
        fn approved(decision: RewriteDecision) -> Option<String> {
            match decision {
                RewriteDecision::AllowRewrite { command, .. } => render_command(&command).ok(),
                _ => None,
            }
        }
        let rewrite = || RewriteDecision::AllowRewrite {
            command: CanonicalCommand::shell("rtk grep -n needle README.md"),
            source: RewriteSource::HzrPolicy,
            reason: "replacement".into(),
        };
        let ordinary = EvasionAttribution {
            class: EvasionClass::E5PipelineOrRedirect,
            wrapper_depth: 0,
            interpreter: None,
            path_form: EvasionPathForm::Bare,
            stage_count: 2,
            hatch_marker: false,
            avoidable: true,
            tier: EnforcementTier::T1NamedCorrection,
            fidelity_reason: None,
            fidelity_validation: FidelityValidation::NotRequested,
        };

        let command = approved(attach_hook_evasion(
            "grep -n needle README.md | head",
            rewrite(),
            Some(&ordinary),
        ))
        .expect("approval must stay an approval");
        assert_eq!(command, "rtk grep -n needle README.md");
        assert!(!command.contains("HZR_INTERNAL_EVASION_JSON"));

        let hatch = EvasionAttribution {
            hatch_marker: true,
            tier: EnforcementTier::T4HatchQuarantine,
            fidelity_validation: FidelityValidation::Valid,
            ..ordinary
        };
        let attributed = approved(attach_hook_evasion(
            "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=checksum hzr exec run 'shasum a'",
            rewrite(),
            Some(&hatch),
        ))
        .expect("approval must stay an approval");
        assert!(
            attributed.starts_with("export HZR_INTERNAL_EVASION_JSON='"),
            "the hatch keeps its attribution: {attributed}"
        );
        assert!(attributed.ends_with("rtk grep -n needle README.md"));
    }

    /// The executed command must carry the session that will be charged for it.
    ///
    /// Policy events are recorded by the hook, which knows the session; operations are recorded
    /// by the engine process the hook approves, which does not. That asymmetry made every
    /// per-session avoidable figure read zero while the same traffic was plainly visible in the
    /// aggregate.
    #[test]
    fn acceptance_gate_an_approved_command_carries_its_session_to_the_engine() {
        fn approved(decision: RewriteDecision) -> Option<String> {
            match decision {
                RewriteDecision::AllowRewrite { command, .. } => render_command(&command).ok(),
                _ => None,
            }
        }
        fn rewrite(command: &str) -> RewriteDecision {
            RewriteDecision::AllowRewrite {
                command: CanonicalCommand::shell(command),
                source: RewriteSource::HzrPolicy,
                reason: "replacement".into(),
            }
        }

        let input = serde_json::json!({"session_id": "private-session"});
        let attributed = approved(attach_session_attribution(
            &input,
            rewrite("rtk proxy /bin/sh -c 'cat a | tail -5'"),
        ))
        .expect("approval must stay an approval");
        assert!(
            attributed.starts_with("export HZR_SESSION_ID='private-session';\n"),
            "unattributed command: {attributed}"
        );
        assert!(attributed.ends_with("rtk proxy /bin/sh -c 'cat a | tail -5'"));

        // A session-less host must not gain an empty attribution.
        let anonymous = approved(attach_session_attribution(
            &serde_json::json!({}),
            rewrite("rtk read README.md"),
        ))
        .expect("approval must stay an approval");
        assert_eq!(anonymous, "rtk read README.md");

        // An already attributed command must not be wrapped twice.
        let twice = approved(attach_session_attribution(
            &input,
            attach_session_attribution(&input, rewrite("rtk read README.md")),
        ))
        .expect("approval must stay an approval");
        assert_eq!(twice.matches("HZR_SESSION_ID=").count(), 1);
    }

    #[test]
    fn acceptance_gate_the_shadow_budget_reflects_recorded_avoidable_bypass() {
        let directory = tempdir().expect("temporary directory");
        let config = config(directory.path());
        let ledger =
            Ledger::open(&config.data_dir.join("ledger/hzr.sqlite")).expect("ledger opens");
        let evasion = EvasionAttribution {
            class: EvasionClass::E5PipelineOrRedirect,
            wrapper_depth: 0,
            interpreter: None,
            path_form: EvasionPathForm::Bare,
            stage_count: 2,
            hatch_marker: false,
            avoidable: true,
            tier: EnforcementTier::T1NamedCorrection,
            fidelity_reason: None,
            fidelity_validation: FidelityValidation::NotRequested,
        };
        ledger
            .record_operation_attributed_with_detail(
                "rtk proxy sh -c 'cat a | tail -5'",
                "rtk proxy sh -c 'cat a | tail -5'",
                4_096,
                4_096,
                1,
                hzr_core::DetailedOperationAttribution {
                    attribution: OperationAttribution {
                        project_path: "/tmp/project",
                        agent: Some("claude-code"),
                        session_id: Some("shadow-session"),
                        channel: OperationChannel::HookCli,
                        measurement: OperationMeasurement::Estimated,
                        route: OperationRoute::Bypassed,
                    },
                    detail: None,
                    evasion: Some(&evasion),
                    host_grant_applied: false,
                },
            )
            .expect("recorded avoidable bypass");

        let summary = ledger
            .session_evasion_summary("shadow-session", FidelityAllowance::default())
            .expect("session summary");

        assert_eq!(summary.avoidable_operations, 1);
        assert_eq!(summary.avoidable_tokens, 4_096);
        assert_eq!(
            summary.recoverable_tokens, 4_096,
            "the scorecard's token figures must move with recorded avoidable bypass"
        );
        let efficiency = ledger
            .session_efficiency_summary("shadow-session")
            .expect("session efficiency");
        let global = ledger.efficiency_summary().expect("global efficiency");
        assert_eq!(
            (
                efficiency.operations,
                efficiency.baseline_tokens_estimated,
                efficiency.delivered_tokens_estimated,
                efficiency.gross_avoided_tokens_estimated,
                efficiency.regression_tokens_estimated,
                efficiency.net_avoided_tokens_estimated,
            ),
            (
                global.operations,
                global.baseline_tokens_estimated,
                global.delivered_tokens_estimated,
                global.gross_avoided_tokens_estimated,
                global.regression_tokens_estimated,
                global.net_avoided_tokens_estimated,
            ),
            "a one-session ledger must use the same token arithmetic as hzr stats"
        );
        assert_eq!(efficiency.top_commands.len(), 1);
        assert!(
            !efficiency.top_commands[0].command.contains("cat a"),
            "session ROI exposed a command payload"
        );
        let message = scorecard_message(
            &Config::default(),
            &SessionFeedback::default(),
            Some(&summary),
            Some(&efficiency),
            None,
        );
        assert!(message.contains("4096 -> 4096"));
        assert!(message.contains("avoidable leakage 1 ops / 4096 tokens"));
        assert!(message.contains("recoverable output escaped the efficient route"));
        ledger
            .record_operation_attributed_with_detail(
                "rtk read private-other-session.txt",
                "rtk read <arguments omitted>",
                9_000,
                900,
                1,
                hzr_core::DetailedOperationAttribution {
                    attribution: OperationAttribution {
                        project_path: "/tmp/project",
                        agent: Some("claude-code"),
                        session_id: Some("other-session"),
                        channel: OperationChannel::HookCli,
                        measurement: OperationMeasurement::Estimated,
                        route: OperationRoute::Optimized,
                    },
                    detail: None,
                    evasion: None,
                    host_grant_applied: false,
                },
            )
            .expect("other session operation");
        assert_eq!(
            ledger
                .session_efficiency_summary("shadow-session")
                .expect("scoped session efficiency"),
            efficiency,
            "another session must not change this session's ROI"
        );
        assert!(summary.avoidable_tokens <= super::SESSION_BYPASS_TOKEN_BUDGET);
    }

    #[test]
    fn acceptance_gate_zero_leakage_explains_prevented_work_and_capability_gaps() {
        let state = SessionFeedback {
            operations: 464,
            corrections: 5,
            native_denials: 1,
            nudged: true,
            ..SessionFeedback::default()
        };
        let summary = SessionEvasionSummary {
            operations: 4,
            delivered_tokens: 8_813,
            top_class: Some(EvasionClass::E10CapabilityGap),
            policy_attempts: 6,
            policy_denials: 1,
            policy_corrections: 4,
            ..SessionEvasionSummary::default()
        };
        let efficiency = SessionEfficiencySummary {
            operations: 7,
            baseline_tokens_estimated: 12_000,
            delivered_tokens_estimated: 4_000,
            gross_avoided_tokens_estimated: 8_000,
            net_avoided_tokens_estimated: 8_000,
            top_commands: vec![hzr_core::EfficiencyCommandSummary {
                command: "hzr read <arguments omitted>".to_owned(),
                executions: 5,
                baseline_tokens_estimated: 10_000,
                delivered_tokens_estimated: 3_000,
                gross_avoided_tokens_estimated: 7_000,
                regression_tokens_estimated: 0,
                net_avoided_tokens_estimated: 7_000,
                avg_time_ms: 1,
            }],
            ..SessionEfficiencySummary::default()
        };

        let message = scorecard_message(
            &Config::default(),
            &state,
            Some(&summary),
            Some(&efficiency),
            None,
        );

        assert!(message.contains("Saved (estimated net): 8000 tokens (66.7%"));
        assert!(message.contains("12000 -> 4000"));
        assert!(message.contains("Top measured: hzr read <arguments omitted> x5"));
        assert!(message.contains("hook-only events: 457"));
        assert!(message.contains("Policy: prevented 5 (1 native denial)"));
        assert!(message.contains("avoidable leakage 0 ops / 0 tokens"));
        assert!(message.contains("good: no proven avoidable bypass executed"));
        assert!(message.contains("top evasion e10-capability-gap"));
        assert!(!message.contains("recoverable-tokens=0"));
    }

    #[test]
    fn acceptance_gate_missing_ledger_is_unknown_instead_of_a_false_zero() {
        let state = SessionFeedback {
            operations: 12,
            corrections: 2,
            native_denials: 0,
            nudged: false,
            ..SessionFeedback::default()
        };

        let message = scorecard_message(&Config::default(), &state, None, None, None);

        assert!(message.contains("Savings: unknown (ledger unavailable, not zero)"));
        assert!(message.contains("avoidable leakage unknown"));
        assert!(!message.contains("avoidable leakage 0"));
    }

    #[test]
    fn billing_opt_out_still_shows_user_supplied_reported_amount() {
        let economics = SessionEconomicSummary {
            paired_receipts: 1,
            reported_actual: Some(EconomicAmount {
                currency: "USD".into(),
                baseline_microunits: 2_000_000,
                delivered_microunits: 750_000,
                savings_microunits: 1_250_000,
            }),
            provenance: Some(ReceiptProvenance::UserSupplied),
            externally_verified: false,
            ..SessionEconomicSummary::default()
        };

        let (potential, billed) =
            super::economic_message(&Config::default(), None, None, Some(&economics), false);
        let message = format!("{potential}\n{billed}");

        assert!(message.contains("opt-in disabled"));
        assert!(message.contains("Billed actual (user-supplied, unverified)"));
        assert!(message.contains("saved USD 1.250000"));
        assert!(!message.contains("Provider invoice"));
    }

    #[test]
    fn acceptance_gate_accounting_transition_notices_are_edge_triggered() {
        let directory = tempdir().expect("temporary directory");
        let config = config(directory.path());
        let input = serde_json::json!({
            "session_id": "transition-session",
            "cwd": directory.path()
        });

        let degraded = accounting_transition(&config, &input, true)
            .expect("first degraded rewrite must announce the transition");
        assert!(degraded.contains("ACCOUNTING DEGRADED"));
        for _ in 0..9 {
            assert_eq!(accounting_transition(&config, &input, true), None);
        }
        let degraded_state = read_session(&config, &input).expect("degraded session state");
        assert_eq!(
            accounting_statusline(Some(&degraded_state)),
            "ACCOUNTING: DEGRADED"
        );

        let recovered = accounting_transition(&config, &input, false)
            .expect("first successful managed rewrite must announce recovery");
        assert!(recovered.contains("ACCOUNTING RECOVERED"));
        assert_eq!(accounting_transition(&config, &input, false), None);
        let recovered_state = read_session(&config, &input).expect("recovered session state");
        assert_eq!(
            accounting_statusline(Some(&recovered_state)),
            "ACCOUNTING: RECOVERED (SESSION PARTIAL)"
        );
        assert_eq!(accounting_statusline(None), "ACCOUNTING: UNKNOWN");
    }

    #[test]
    fn acceptance_gate_degraded_scorecard_reports_measured_part_as_lower_bound() {
        let state = SessionFeedback {
            operations: 10,
            accounting_was_degraded: true,
            ..SessionFeedback::default()
        };
        let efficiency = SessionEfficiencySummary {
            operations: 4,
            baseline_tokens_estimated: 1_000,
            delivered_tokens_estimated: 100,
            gross_avoided_tokens_estimated: 900,
            net_avoided_tokens_estimated: 900,
            ..SessionEfficiencySummary::default()
        };
        let message = scorecard_message(
            &Config::default(),
            &state,
            Some(&SessionEvasionSummary::default()),
            Some(&efficiency),
            None,
        );

        // 0.8.2: the measured part is reported as a lower bound next to the unmeasured interval.
        assert!(message.contains("Saved (estimated net): 900"), "{message}");
        assert!(message.contains("PARTIAL:"), "{message}");
        assert!(
            message.contains("unmeasured while accounting was degraded"),
            "{message}"
        );
        assert!(!message.contains("withheld"), "{message}");
        assert!(message.contains("avoidable leakage 0 ops"), "{message}");
        assert!(
            message.contains("leakage covers measured operations only"),
            "{message}"
        );
    }

    // 0.8.2: the model the host reports through the status line prices the estimate.
    #[test]
    fn observed_status_line_model_prices_the_estimate_over_the_configured_default() {
        let directory = tempdir().expect("temporary directory");
        let mut config = config(directory.path());
        config.billing.public_estimate_enabled = true;
        config.billing.harness = "claude_code".into();
        config.billing.provider = "anthropic".into();
        config.billing.model = "claude-opus-5".into();
        config.billing.method = "standard".into();
        config.billing.pricing_basis = "input".into();
        let input = serde_json::json!({
            "session_id": "statusline-session",
            "cwd": directory.path(),
            "model": {"id": "claude-fable-5-1", "display_name": "Fable 5.1"}
        });
        remember_observed_model(&config, &input);
        let state = read_session(&config, &input).expect("session state");
        assert_eq!(state.observed_model_id.as_deref(), Some("claude-fable-5-1"));
        let efficiency = SessionEfficiencySummary {
            operations: 1,
            baseline_tokens_estimated: 1_000_000,
            delivered_tokens_estimated: 0,
            gross_avoided_tokens_estimated: 1_000_000,
            net_avoided_tokens_estimated: 1_000_000,
            ..SessionEfficiencySummary::default()
        };
        let message = scorecard_message(&config, &state, None, Some(&efficiency), None);
        assert!(message.contains("claude-fable-5-1/standard"), "{message}");
        assert!(
            message.contains("potential public-list value USD 10.000000"),
            "{message}"
        );
        // An unknown observed model falls back to the configured selection.
        let unknown = serde_json::json!({
            "session_id": "statusline-session",
            "cwd": directory.path(),
            "model": {"id": "claude-unreleased-9"}
        });
        remember_observed_model(&config, &unknown);
        let state = read_session(&config, &unknown).expect("session state");
        let message = scorecard_message(&config, &state, None, Some(&efficiency), None);
        assert!(message.contains("claude-opus-5/standard"), "{message}");
    }

    #[test]
    fn acceptance_gate_scorecard_prices_saved_inline_without_merging_billed() {
        let mut config = Config::default();
        config.billing.public_estimate_enabled = true;
        config.billing.harness = "codex".into();
        config.billing.provider = "openai".into();
        config.billing.model = "gpt-5.6-sol".into();
        config.billing.method = "standard_short_context_lte_272k".into();
        config.billing.request_input_tokens = Some(100_000);
        config.billing.pricing_basis = "input".into();
        let efficiency = SessionEfficiencySummary {
            operations: 1,
            total_observed_operations: 1,
            baseline_tokens_estimated: 1_000_000,
            delivered_tokens_estimated: 0,
            gross_avoided_tokens_estimated: 1_000_000,
            net_avoided_tokens_estimated: 1_000_000,
            ..SessionEfficiencySummary::default()
        };
        let economics = SessionEconomicSummary {
            reported_actual: Some(EconomicAmount {
                currency: "USD".into(),
                baseline_microunits: 2_000_000,
                delivered_microunits: 1_000_000,
                savings_microunits: 1_000_000,
            }),
            ..SessionEconomicSummary::default()
        };
        let message = scorecard_message(
            &config,
            &SessionFeedback::default(),
            None,
            Some(&efficiency),
            Some(&economics),
        );
        let mut lines = message.lines();
        assert_eq!(lines.next(), Some("HZR session ROI"));
        let saved = lines.next().expect("saved line");
        let billed = lines.next().expect("billed line");
        assert!(saved.contains("Saved (estimated net): 1000000 tokens"));
        assert!(saved.contains("potential public-list value USD"));
        assert!(!saved.contains("Billed actual"));
        assert!(billed.contains("Billed actual (user-supplied, unverified)"));
        assert!(!billed.contains("potential public-list value"));
    }

    #[test]
    fn acceptance_gate_scorecard_explains_zero_and_granted_asks() {
        let state = SessionFeedback {
            operations: 1,
            host_grant_seen: true,
            ..SessionFeedback::default()
        };
        let efficiency = SessionEfficiencySummary {
            operations: 1,
            total_observed_operations: 1,
            baseline_tokens_estimated: 100,
            delivered_tokens_estimated: 100,
            ..SessionEfficiencySummary::default()
        };
        let evasion = SessionEvasionSummary {
            policy_asks: 21,
            ..SessionEvasionSummary::default()
        };
        let message = scorecard_message(
            &Config::default(),
            &state,
            Some(&evasion),
            Some(&efficiency),
            None,
        );

        assert!(message.contains("zero explained: every measured row is zero-credit by policy"));
        assert!(message.contains("asked 21 (PROPAGATION FAILURE"));
        assert!(!message.contains("; asked 21;"));
    }

    #[test]
    fn acceptance_gate_t1_feedback_is_counted_without_raw_identity() {
        let directory = tempdir().expect("temporary directory");
        let config = config(directory.path());
        let input = serde_json::json!({
            "session_id": "private-session",
            "agent_type": "claude-code",
            "agent_id": "private-subagent",
            "cwd": directory.path()
        });
        let decision = RewriteDecision::AllowRewrite {
            command: CanonicalCommand::shell("hzr read README.md"),
            source: RewriteSource::HzrPolicy,
            reason: "replacement".into(),
        };
        let decision = attach_policy_feedback(&config, &input, decision);
        assert!(matches!(
            decision,
            RewriteDecision::AllowRewrite { ref reason, .. }
                if reason.contains("session avoidable-bypass count=1")
        ));
        let state = read_session(&config, &input).expect("session state");
        assert_eq!(state.corrections, 1);
        let names = fs::read_dir(config.data_dir.join("hook-sessions"))
            .expect("session directory")
            .map(|entry| {
                entry
                    .expect("session entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!names.contains("private-session"));
        assert!(!names.contains("private-subagent"));
        assert_eq!(agent_identity(&input), "claude-code");
        assert_eq!(
            agent_attribution(&input),
            "claude-code:private-subagent",
            "raw identity is supplied transiently for hashing; ledger persists only host + digest"
        );
    }

    #[test]
    fn acceptance_gate_t4_preflights_first_use_fidelity_output() {
        let directory = tempdir().expect("temporary directory");
        let config = config(directory.path());
        let oversized = directory.path().join("oversized.txt");
        fs::write(&oversized, vec![b'x'; 400_001]).expect("oversized fixture");
        let input = serde_json::json!({"session_id": "session-t4"});

        for command in [
            "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=verbatim_source hzr rtk -- raw cat oversized.txt",
            "HZR_EXACT_FIDELITY=1 hzr read oversized.txt --level none",
        ] {
            let preflight = hook_fidelity_preflight(&config, &input, command, directory.path());
            assert!(matches!(preflight, HookFidelityPreflight::Ask { .. }));
            let HookFidelityPreflight::Ask { decision, evasion } = preflight else {
                return;
            };
            assert!(matches!(
                decision,
                RewriteDecision::Ask { ref reason, proposed: Some(_) }
                    if reason.contains("remaining allowance")
            ));
            assert_eq!(
                evasion.fidelity_validation,
                FidelityValidation::BudgetExhausted
            );
        }

        fs::write(directory.path().join("small.txt"), b"bounded").expect("small fixture");
        assert!(matches!(
            hook_fidelity_preflight(
                &config,
                &input,
                "HZR_EXACT_FIDELITY=1 hzr read small.txt --level none",
                directory.path(),
            ),
            HookFidelityPreflight::Allow(_)
        ));
        assert!(matches!(
            hook_fidelity_preflight(
                &config,
                &input,
                "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=complete_log hzr rtk -- raw ssh host docker logs app",
                directory.path(),
            ),
            HookFidelityPreflight::Ask {
                decision: RewriteDecision::Ask { ref reason, .. },
                ..
            } if reason.contains("not statically bounded")
        ));
        assert!(matches!(
            hook_fidelity_preflight(
                &config,
                &input,
                "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=checksum hzr rtk -- raw sha256sum oversized.txt",
                directory.path(),
            ),
            HookFidelityPreflight::Allow(_)
        ));
        assert!(matches!(
            hook_fidelity_preflight(
                &config,
                &serde_json::json!({}),
                "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=checksum hzr rtk -- raw sha256sum oversized.txt",
                directory.path(),
            ),
            HookFidelityPreflight::Ask { .. }
        ));
        assert!(matches!(
            hook_fidelity_preflight(
                &config,
                &input,
                "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=checksum hzr rtk -- raw cat small.txt",
                directory.path(),
            ),
            HookFidelityPreflight::Ask {
                evasion: EvasionAttribution {
                    fidelity_validation: FidelityValidation::Contradicted,
                    ..
                },
                ..
            }
        ));
    }

    fn allow_raw() -> RewriteDecision {
        RewriteDecision::allow_raw("no rule matched")
    }

    /// The plan was prepended to a subagent's prompt as a minified JSON blob with no
    /// explanation: no glossary, no statement that the paths are unverified leads, and no
    /// instruction on what to do with them. A subagent that cannot tell what the block is will
    /// either ignore it and re-derive everything or trust it as fact — and the second failure
    /// is worse, because a plan candidate is a ranked guess, not a finding.
    #[test]
    fn test_the_injected_brief_tells_a_subagent_what_the_evidence_is() {
        let plan = serde_json::json!({
            "pack": {
                "selected": [{
                    "source": "context",
                    "path": "crates/hzr-cli/src/hook_runner.rs",
                    "line_start": 45,
                    "line_end": 79,
                    "relevance": 0.42,
                }],
            },
        });

        let brief = context_brief(&plan);

        assert!(
            brief.contains("crates/hzr-cli/src/hook_runner.rs:45-79"),
            "a lead must be citable as a path and span, got: {brief}"
        );
        assert!(
            brief.contains("unverified"),
            "the brief must say the leads are unverified, got: {brief}"
        );
        assert!(
            !brief.contains("\"pack\""),
            "the raw envelope is noise for a subagent, got: {brief}"
        );
    }

    /// An empty plan must say so rather than prepending an empty block that reads like a
    /// finding of "nothing relevant exists".
    #[test]
    fn test_an_empty_plan_states_that_it_found_nothing() {
        let brief = context_brief(&serde_json::json!({"pack": {"selected": []}}));

        assert!(
            brief.contains("no"),
            "an empty plan must be stated, got: {brief}"
        );
        assert!(!brief.contains("unverified leads:"));
    }

    fn proposed(decision: &RewriteDecision) -> Option<String> {
        match decision {
            RewriteDecision::AllowRewrite {
                command: CanonicalCommand::Shell { command, .. },
                ..
            } => Some(command.clone()),
            _ => None,
        }
    }

    #[test]
    fn test_shell_policy_is_not_reconstructed_after_the_fork_decision() {
        for command in [
            "hzr rtk -- raw sed -n 1030,1105p crates/hzr-core/src/ledger.rs",
            "rg -n RewriteDecision crates/hzr-exec",
            "cat README.md",
        ] {
            assert!(matches!(
                steer_to_first_class(command, allow_raw()),
                RewriteDecision::AllowRaw { .. }
            ));
        }

        let specialized = RewriteDecision::AllowRewrite {
            command: CanonicalCommand::shell("rtk rg -n RewriteDecision crates/hzr-exec"),
            source: RewriteSource::Rtk {
                version: PINNED_RTK_VERSION.into(),
                route: hzr_exec::RtkRewriteRoute::Optimized,
            },
            reason: "fork-core approved and produced the managed command".into(),
        };
        assert_eq!(
            steer_to_first_class(
                "hzr rtk -- raw rg -n RewriteDecision crates/hzr-exec",
                specialized.clone(),
            ),
            specialized,
            "a specialized fork-core filter must not be replaced by indexed search"
        );
    }

    #[test]
    fn acceptance_gate_no_raw_for_optimizable_hook_commands() {
        for command in [
            "hzr rtk -- raw hzr stats",
            "hzr rtk -- raw hzr search \"two words\" --mode exact",
        ] {
            let decision = steer_to_first_class(command, allow_raw());
            assert!(
                matches!(decision, RewriteDecision::AllowRewrite { .. }),
                "{command} remained raw: {decision:?}"
            );
        }

        let proxy = RewriteDecision::AllowRewrite {
            command: CanonicalCommand::shell("rtk proxy nl -ba src/main.rs"),
            source: RewriteSource::Rtk {
                version: PINNED_RTK_VERSION.into(),
                route: hzr_exec::RtkRewriteRoute::Proxy,
            },
            reason: "fork selected tracked raw proxy".into(),
        };
        let decision = steer_to_first_class("hzr rtk -- raw nl -ba src/main.rs", proxy.clone());
        assert_eq!(
            decision, proxy,
            "hook overrode the canonical Proxy decision"
        );
    }

    #[test]
    fn acceptance_gate_no_raw_for_top_level_hzr_file_aliases_in_hook() {
        for command in [
            "hzr read \"docs/file with spaces.md\" --outline",
            "hzr write patch \"docs/file with spaces.md\" --old 'a b' --new 'c d'",
        ] {
            let decision = steer_to_first_class(
                command,
                RewriteDecision::AllowRewrite {
                    command: CanonicalCommand::shell(format!("rtk proxy {command}")),
                    source: RewriteSource::Rtk {
                        version: PINNED_RTK_VERSION.into(),
                        route: hzr_exec::RtkRewriteRoute::Proxy,
                    },
                    reason: "fork selected tracked raw proxy".into(),
                },
            );
            assert_eq!(
                proposed(&decision).as_deref(),
                Some(command),
                "hook changed quoted top-level alias bytes"
            );
            assert!(matches!(
                decision,
                RewriteDecision::AllowRewrite {
                    source: RewriteSource::HzrPolicy,
                    ..
                }
            ));
        }
    }

    #[test]
    fn explicit_full_read_is_preserved_without_a_fidelity_marker() {
        let filtered = RewriteDecision::AllowRewrite {
            command: CanonicalCommand::shell("rtk read src/main.rs --level none"),
            source: RewriteSource::Rtk {
                version: PINNED_RTK_VERSION.into(),
                route: hzr_exec::RtkRewriteRoute::Optimized,
            },
            reason: "fork-core accepted the explicit read".into(),
        };
        for command in [
            "hzr rtk -- read src/main.rs --level none",
            "hzr rtk -- read src/main.rs --from 40 --to 80 --level none",
            "HZR_EXACT_FIDELITY=1 hzr rtk -- read src/main.rs --level none",
        ] {
            assert_eq!(
                steer_to_first_class(command, filtered.clone()),
                filtered,
                "bounded or explicit exact read was changed: {command}"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn acceptance_gate_shared_fixture_reaches_degraded_hook() {
        let probes = super::anti_evasion_fixture::load_anti_evasion_probes();
        let shell_probes = probes
            .iter()
            .filter(|probe| probe.surface == ProbeSurface::Shell)
            .collect::<Vec<_>>();
        assert!(
            shell_probes.len() > 5,
            "both fork and root shell probes must execute"
        );

        let directory = tempdir().expect("temporary directory");
        let engines = directory.path().join("engines");
        fs::create_dir(&engines).expect("engine directory");
        let binary = engines.join("rtk");
        let plan_path = directory.path().join("rewrite-plan.json");
        let contract = serde_json::to_string(
            &hzr_exec::expected_engine_identity().expect("embedded current-engine identity"),
        )
        .expect("contract JSON");
        let script = format!(
            r#"#!/bin/sh
if test "${{1:-}}" = --version; then
  printf 'rtk %s\n' '{PINNED_RTK_VERSION}'
  exit 0
fi
if test "${{1:-}}" = contract && test "${{2:-}}" = --json; then
  printf '%s\n' '{contract}'
  exit 0
fi
if test "${{1:-}}" = rewrite && test "${{2:-}}" = --help; then
  printf 'Usage: rtk rewrite [ARGS]... Raw command to rewrite\n'
  exit 0
fi
if test "${{1:-}}" = proxy && test "${{2:-}}" = --help; then
  printf 'Usage: rtk proxy [ARGS]... Execute command without filtering\n'
  exit 0
fi
if test "${{1:-}}" = rewrite-plan; then
  /bin/cat '{}'
  exit 0
fi
exit 64
"#,
            plan_path.display()
        );
        fs::write(&binary, script).expect("fake fork-core");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))
            .expect("fake fork-core permissions");
        let mut config = config(directory.path());
        config.engines.directory = Some(engines);
        let expected_evasion = EvasionAttribution {
            class: EvasionClass::E10CapabilityGap,
            wrapper_depth: 0,
            interpreter: None,
            path_form: EvasionPathForm::Bare,
            stage_count: 1,
            hatch_marker: false,
            avoidable: false,
            tier: EnforcementTier::T0TransparentRewrite,
            fidelity_reason: None,
            fidelity_validation: FidelityValidation::NotRequested,
        };

        for probe in shell_probes {
            let plan = match probe.decision {
                ProbeDecision::Rewrite => serde_json::json!({
                    "decision": "rewrite",
                    "proposed": probe.route.as_deref().expect("rewrite route"),
                    "attribution": expected_evasion
                }),
                ProbeDecision::Ask => {
                    serde_json::json!({
                        "decision": "ask",
                        "reason": "canonical_policy",
                        "attribution": expected_evasion
                    })
                }
                ProbeDecision::Proxy | ProbeDecision::Raw => {
                    serde_json::json!({
                        "decision": "proxy",
                        "attribution": expected_evasion
                    })
                }
                ProbeDecision::Allow | ProbeDecision::Deny => {
                    assert!(
                        matches!(probe.decision, ProbeDecision::Rewrite),
                        "invalid shell decision for {}",
                        probe.id
                    );
                    continue;
                }
            };
            fs::write(
                &plan_path,
                serde_json::to_vec(&plan).expect("rewrite plan JSON"),
            )
            .expect("rewrite plan fixture");
            let command = probe.command.as_deref().expect("shell command");
            let fallback = fallback_decision(&config, command, directory.path()).await;
            if !probe.id.starts_with("fidelity-") {
                assert_eq!(
                    fallback.evasion,
                    Some(expected_evasion),
                    "daemon-down fallback dropped attribution for {}: {:?}",
                    probe.id,
                    fallback.decision
                );
            }
            let decision = steer_to_first_class(command, fallback.decision);
            match probe.decision {
                ProbeDecision::Rewrite => {
                    let RewriteDecision::AllowRewrite {
                        command: CanonicalCommand::Shell { command, .. },
                        ..
                    } = decision
                    else {
                        assert!(
                            matches!(
                                &decision,
                                RewriteDecision::AllowRewrite {
                                    command: CanonicalCommand::Shell { .. },
                                    ..
                                }
                            ),
                            "shell probe {} was not rewritten: {decision:?}",
                            probe.id
                        );
                        continue;
                    };
                    assert!(
                        command.ends_with(probe.route.as_deref().expect("rewrite route")),
                        "shell probe {} selected {command}",
                        probe.id
                    );
                }
                ProbeDecision::Ask => assert!(
                    matches!(decision, RewriteDecision::Ask { .. }),
                    "shell probe {} was not Ask: {decision:?}",
                    probe.id
                ),
                ProbeDecision::Proxy => assert!(
                    matches!(
                        decision,
                        RewriteDecision::AllowRewrite {
                            source: RewriteSource::Rtk {
                                route: hzr_exec::RtkRewriteRoute::Proxy,
                                ..
                            },
                            ..
                        }
                    ),
                    "shell probe {} was not a tracked Proxy: {decision:?}",
                    probe.id
                ),
                ProbeDecision::Raw => assert!(
                    matches!(decision, RewriteDecision::AllowRaw { .. }),
                    "shell probe {} did not preserve fidelity: {decision:?}",
                    probe.id
                ),
                ProbeDecision::Allow | ProbeDecision::Deny => {
                    assert!(
                        matches!(
                            probe.decision,
                            ProbeDecision::Rewrite
                                | ProbeDecision::Ask
                                | ProbeDecision::Proxy
                                | ProbeDecision::Raw
                        ),
                        "invalid shell decision for {}",
                        probe.id
                    )
                }
            }
        }
    }

    #[test]
    fn test_a_command_without_an_equivalent_is_passed_through_untouched() {
        let decision = steer_to_first_class("cargo clippy --workspace", allow_raw());

        assert!(matches!(decision, RewriteDecision::AllowRaw { .. }));
    }

    #[test]
    fn test_ambiguous_shell_commands_are_not_reconstructed_by_the_hook() {
        for command in [
            "hzr rtk -- raw nl -ba \"src/file with spaces.rs\"",
            "hzr rtk -- raw rg -n \"two words\" src",
            "hzr rtk -- raw rg -n needle src | head -n 20",
        ] {
            let decision = steer_to_first_class(command, allow_raw());
            assert!(
                matches!(decision, RewriteDecision::AllowRaw { .. }),
                "{command} was reconstructed: {decision:?}"
            );
        }
    }

    /// When the pinned engine already rewrote the command there is nothing to steer, and
    /// overriding its decision would replace a filtered command with a guess.
    #[test]
    fn test_an_already_optimized_decision_is_never_overridden() {
        let rewritten = RewriteDecision::AllowRewrite {
            command: CanonicalCommand::shell("rtk read README.md"),
            reason: "fork-core rule".into(),
            source: RewriteSource::HzrPolicy,
        };

        let decision = steer_to_first_class("cat README.md", rewritten);

        assert!(matches!(decision, RewriteDecision::AllowRewrite { .. }));
    }

    #[test]
    fn test_an_ambiguous_shell_wrapper_remains_an_explicit_ask() {
        let ask = RewriteDecision::Ask {
            proposed: None,
            reason: "fork-core could not safely decompose an opaque shell wrapper".into(),
        };

        assert_eq!(steer_to_first_class("sh -c 'git status", ask.clone()), ask);
    }

    fn config(root: &std::path::Path) -> Config {
        Config {
            data_dir: root.to_path_buf(),
            ..Config::default()
        }
    }

    #[test]
    fn test_a_clean_installation_reports_complete_coverage() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());

        let coverage = degraded_rewrite_coverage(&config).expect("coverage");

        assert!(coverage.complete);
        assert_eq!(coverage.unreconciled_rewrites, 0);
        assert_eq!(coverage.lifetime_rewrites, 0);
    }

    #[test]
    fn test_a_daemon_free_rewrite_marks_coverage_incomplete() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());

        record_degraded_rewrite_at(&config, 1_785_531_432).expect("record");
        record_degraded_rewrite_at(&config, 1_785_531_500).expect("record");
        let coverage = degraded_rewrite_coverage(&config).expect("coverage");

        assert!(!coverage.complete);
        assert_eq!(coverage.unreconciled_rewrites, 2);
        assert_eq!(coverage.lifetime_rewrites, 2);
    }

    #[test]
    fn healthy_daemon_closes_live_gap_but_history_stays_partial() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());
        record_degraded_rewrite_at(&config, 1_785_531_432).expect("record");
        record_degraded_rewrite_at(&config, 1_785_531_500).expect("record");

        close_open_accounting_gaps(&config).expect("close gap");
        let coverage = degraded_rewrite_coverage(&config).expect("coverage");

        assert!(
            coverage.live_complete,
            "a healthy daemon closes the live gap"
        );
        assert!(
            !coverage.complete,
            "missing operations remain historical partial evidence"
        );
        assert!(!coverage.historical_complete);
        assert_eq!(coverage.unreconciled_rewrites, 0);
        assert_eq!(
            coverage.lifetime_rewrites, 2,
            "history is retained, only the open gap is closed"
        );
    }

    #[test]
    fn closing_twice_does_not_double_count_the_lifetime_total() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());
        record_degraded_rewrite_at(&config, 1_785_531_432).expect("record");

        close_open_accounting_gaps(&config).expect("close gap");
        close_open_accounting_gaps(&config).expect("close gap again");
        record_degraded_rewrite_at(&config, 1_785_600_000).expect("record");
        let coverage = degraded_rewrite_coverage(&config).expect("coverage");

        assert_eq!(coverage.unreconciled_rewrites, 1);
        assert_eq!(coverage.lifetime_rewrites, 2);
    }

    #[test]
    fn closing_a_clean_coverage_store_writes_nothing() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());

        close_open_accounting_gaps(&config).expect("close clean store");

        assert!(
            !directory.path().join("ledger").exists(),
            "no gap means no bookkeeping"
        );
    }

    #[test]
    fn test_coverage_reports_when_the_last_gap_occurred() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());
        record_degraded_rewrite_at(&config, 1_785_531_432).expect("record");

        let coverage = degraded_rewrite_coverage(&config).expect("coverage");

        assert_eq!(coverage.last_degraded_at_unix, Some(1_785_531_432));
    }

    #[test]
    fn acceptance_gate_accounting_gap_keeps_first_flip_time_and_duration() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());
        record_degraded_rewrite_at(&config, 1_000).expect("first missing rewrite");
        record_degraded_rewrite_at(&config, 1_060).expect("later missing rewrite");

        let coverage = degraded_rewrite_coverage_at(&config, 1_720).expect("coverage");
        assert_eq!(coverage.gap_started_at_unix, Some(1_000));
        assert_eq!(coverage.last_degraded_at_unix, Some(1_060));
        assert_eq!(coverage.open_gap_seconds, Some(720));

        let input = serde_json::json!({
            "session_id": "session-gap",
            "cwd": directory.path(),
        });
        assert!(accounting_transition_at(&config, &input, true, 1_000).is_some());
        assert!(accounting_transition_at(&config, &input, true, 1_060).is_none());
        assert!(accounting_transition_at(&config, &input, false, 1_120).is_some());
        let state = read_session(&config, &input).expect("session state");
        assert_eq!(state.session_started_at_unix, Some(1_000));
        assert_eq!(state.accounting_degraded_started_at_unix, None);
        assert_eq!(state.accounting_degraded_seconds, 120);
        assert_eq!(state.accounting_degraded_operations, 2);
    }

    /// A log left behind by an earlier HZR contains bare timestamps and no lifetime file.
    /// It must still be readable rather than reported as a corrupt ledger.
    #[test]
    fn test_a_legacy_log_without_a_lifetime_file_is_still_counted() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());
        std::fs::create_dir_all(directory.path().join("ledger")).expect("ledger directory");
        std::fs::write(
            directory.path().join("ledger/degraded-rewrites.log"),
            "1785531432\n1785531500\n1785531600\n",
        )
        .expect("legacy log");

        let coverage = degraded_rewrite_coverage(&config).expect("coverage");

        assert_eq!(coverage.unreconciled_rewrites, 3);
        assert_eq!(coverage.lifetime_rewrites, 3);
    }
}
