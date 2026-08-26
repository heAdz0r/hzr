use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use hzr_core::{
    Config, DetailedOperationAttribution, FidelityAllowance, FidelityBudget, FidelityPreflight,
    Ledger, OperationAttribution, OperationChannel, OperationMeasurement, OperationRoute,
    PolicyEvent, RawFidelityRequest, efficient_route_replacement, fidelity_preflight_required,
    first_class_replacement, raw_fidelity_request,
};
use hzr_exec::{
    CanonicalCommand, ForkRuntimePaths, PinnedRtkAdapter, RewriteDecision, RewriteSource,
    RtkAdapterConfig,
};
use hzr_protocol::{
    ContextPlanApiRequest, EnforcementTier, EvasionAttribution, EvasionClass, EvasionPathForm,
    ExecApiRequest, FidelityValidation, PolicyDecision,
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

pub async fn dispatch(config: &Config, native_mode: NativeToolMode) {
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    if !crate::activation::is_enabled(config, &cwd)
        .await
        .unwrap_or(false)
    {
        return;
    }
    let Ok(input) = read_input() else {
        return;
    };
    let _ = update_session(config, &input, |state| {
        state.operations = state.operations.saturating_add(1);
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
        "Read" | "Grep" | "Glob" | "Edit" | "Write" => {
            let _ = native_pre_tool(config, &input, native_mode);
        }
        _ => {}
    }
}

/// Observe host-native file tools without steering or blocking them.
///
/// This entry point intentionally returns no error: a measurement failure must never turn a
/// successful host tool call into a failed one. The hook emits no stdout payload.
pub async fn observe(config: &Config, native_mode: NativeToolMode) {
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    if !crate::activation::is_enabled(config, &cwd)
        .await
        .unwrap_or(false)
    {
        return;
    }
    let Ok(input) = read_input() else {
        return;
    };
    let _ = observe_input(config, &input, native_mode);
}

fn observe_input(config: &Config, input: &Value, native_mode: NativeToolMode) -> Result<()> {
    let tool = input
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(tool, "Read" | "Grep" | "Glob" | "Edit" | "Write") {
        return Ok(());
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
        (OperationMeasurement::Estimated, estimated)
    } else {
        (OperationMeasurement::Unmeasured, 0)
    };
    let (route, evasion) = native_observation_policy(tool, native_mode);
    Ledger::open(&config.data_dir.join("ledger/hzr.sqlite"))?
        .record_operation_attributed_with_detail(
            &format!("native {tool}"),
            &format!("native {tool}"),
            tokens,
            tokens,
            0,
            DetailedOperationAttribution {
                attribution: OperationAttribution {
                    project_path: cwd,
                    agent: Some(&agent),
                    session_id,
                    channel: OperationChannel::NativeHost,
                    measurement,
                    route,
                },
                detail: None,
                evasion: Some(&evasion),
            },
        )?;
    Ok(())
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
    let cwd = std::env::current_dir().context("failed to resolve hook working directory")?;
    let fidelity_evasion = match hook_fidelity_preflight(config, input, raw, &cwd) {
        HookFidelityPreflight::NotRequested => None,
        HookFidelityPreflight::Allow(evasion) => Some(evasion),
        HookFidelityPreflight::Ask { decision, evasion } => {
            let _ = record_local_policy_decision(config, input, &decision, Some(evasion));
            return write_decision(input, decision);
        }
    };
    let request = ExecApiRequest {
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
    let mut managed_evasion = None;
    let decision = match managed {
        Some(outcome) => {
            // The daemon answered, so any earlier gap is now behind us: close it instead of
            // leaving `hzr stats` pinned to INCOMPLETE for the rest of the installation.
            let _ = clear_reconciled_rewrites(config);
            managed_evasion = outcome.evasion;
            outcome.decision
        }
        None => {
            let _ = record_degraded_rewrite(config);
            fallback_decision(config, raw, &cwd).await
        }
    };
    let decision = steer_to_first_class(raw, decision);
    // Fidelity attribution is authoritative when present; otherwise the daemon's classification
    // of this exact command is what the recording process needs.
    let evasion = fidelity_evasion.or(managed_evasion);
    let decision = honor_host_permission_mode(input, decision);
    let decision = attach_hook_evasion(raw, decision, evasion.as_ref());
    let decision = attach_policy_feedback(config, input, decision);
    let decision = attach_session_attribution(input, decision);
    if !daemon_recorded_policy {
        let _ = record_local_policy_decision(config, input, &decision, None);
    }
    write_decision(input, decision)
}

fn record_local_policy_decision(
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
    let agent = agent_attribution(input);
    Ledger::open(&config.data_dir.join("ledger/hzr.sqlite"))?.record_policy_event(PolicyEvent {
        project_path: input.get("cwd").and_then(Value::as_str).unwrap_or_default(),
        agent: Some(&agent),
        session_id: input.get("session_id").and_then(Value::as_str),
        evasion,
        decision,
        replacement_family: None,
    })?;
    Ok(())
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

/// Carry the classification into the process that will record the command.
///
/// Two different guarantees share this path. For a fidelity request the attribution is a
/// precondition: an unattributed T4 execution must not happen silently, so a failure becomes an
/// Ask. For every other command the attribution is accounting, and accounting must never turn a
/// working command into a prompt — a failure there leaves the decision exactly as it was.
fn attach_hook_evasion(
    raw: &str,
    decision: RewriteDecision,
    evasion: Option<&EvasionAttribution>,
) -> RewriteDecision {
    let Some(evasion) = evasion else {
        return decision;
    };
    let strict = evasion.hatch_marker;
    let Ok(encoded) = serde_json::to_string(evasion) else {
        return if strict {
            RewriteDecision::Ask {
                proposed: Some(CanonicalCommand::shell(raw)),
                reason: "T4 fidelity attribution could not be serialized".into(),
            }
        } else {
            decision
        };
    };
    match decision {
        RewriteDecision::AllowRaw { .. } if strict => {
            match attributed_hook_command(&encoded, raw) {
                Some(command) => RewriteDecision::AllowRewrite {
                    command: CanonicalCommand::shell(command),
                    source: RewriteSource::HzrPolicy,
                    reason: "T4 fidelity execution carries closed typed attribution".into(),
                },
                None => RewriteDecision::Ask {
                    proposed: Some(CanonicalCommand::shell(raw)),
                    reason: "T4 fidelity execution needs explicit approval on this host".into(),
                },
            }
        }
        RewriteDecision::AllowRewrite {
            command,
            source,
            reason,
        } => {
            let unchanged = |command, source, reason| RewriteDecision::AllowRewrite {
                command,
                source,
                reason,
            };
            let refuse = || RewriteDecision::Ask {
                proposed: Some(CanonicalCommand::shell(raw)),
                reason: "T4 fidelity execution could not preserve the approved command".into(),
            };
            let Ok(rendered) = render_command(&command) else {
                return if strict {
                    refuse()
                } else {
                    unchanged(command, source, reason)
                };
            };
            match attributed_hook_command(&encoded, &rendered) {
                // The source and reason stay the agent-facing ones the decision already carried;
                // only the environment the command runs in changes.
                Some(attributed) => unchanged(CanonicalCommand::shell(attributed), source, reason),
                None if strict => refuse(),
                None => unchanged(command, source, reason),
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
    if rendered.contains("HZR_SESSION_ID=") {
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

fn native_pre_tool(config: &Config, input: &Value, mode: NativeToolMode) -> Result<()> {
    let tool = input
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if mode == NativeToolMode::Observe || tool == "Glob" {
        return Ok(());
    }
    if mode == NativeToolMode::Steer && matches!(tool, "Edit" | "Write") {
        return Ok(());
    }
    let Some(replacement) = native_replacement(input, mode) else {
        return Ok(());
    };
    record_native_correction(config, input, tool)?;
    let count = update_session(config, input, |state| {
        state.corrections = state.corrections.saturating_add(1);
        state.native_denials = state.native_denials.saturating_add(1);
    })
    .map(|state| state.corrections)
    .unwrap_or(0);
    write_hook_json(json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": format!(
                "T1 native-tool correction E8 ({tool}); use `{replacement}`; session avoidable-bypass count={count}"
            ),
        }
    }))
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
    tool: &str,
    native_mode: NativeToolMode,
) -> (OperationRoute, EvasionAttribution) {
    if native_mode == NativeToolMode::Observe {
        return (
            OperationRoute::NativeUnaccounted,
            native_evasion(
                EvasionClass::E8NativeTool,
                false,
                EnforcementTier::T0TransparentRewrite,
            ),
        );
    }
    let allowed_by_policy = tool == "Glob"
        || (native_mode == NativeToolMode::Steer && matches!(tool, "Edit" | "Write"));
    if allowed_by_policy {
        (
            OperationRoute::Bypassed,
            native_evasion(
                EvasionClass::E10CapabilityGap,
                false,
                EnforcementTier::T0TransparentRewrite,
            ),
        )
    } else {
        (
            OperationRoute::Bypassed,
            native_evasion(
                EvasionClass::E8NativeTool,
                true,
                EnforcementTier::T2DenyWithPrescription,
            ),
        )
    }
}

fn record_native_correction(config: &Config, input: &Value, tool: &str) -> Result<()> {
    let cwd = input.get("cwd").and_then(Value::as_str).unwrap_or_default();
    let session_id = input.get("session_id").and_then(Value::as_str);
    let agent = agent_attribution(input);
    let evasion = native_evasion(
        EvasionClass::E8NativeTool,
        true,
        EnforcementTier::T1NamedCorrection,
    );
    Ledger::open(&config.data_dir.join("ledger/hzr.sqlite"))?.record_policy_event(PolicyEvent {
        project_path: cwd,
        agent: Some(&agent),
        session_id,
        evasion,
        decision: PolicyDecision::Deny,
        replacement_family: Some(match tool {
            "Read" => "read",
            "Grep" => "search",
            "Edit" | "Write" => "write",
            _ => "other",
        }),
    })?;
    Ok(())
}

fn native_replacement(input: &Value, mode: NativeToolMode) -> Option<String> {
    let tool = input.get("tool_name")?.as_str()?;
    let arguments = input.get("tool_input")?;
    let path = arguments
        .get("file_path")
        .or_else(|| arguments.get("path"))
        .and_then(Value::as_str);
    match tool {
        "Read" => Some(format!("hzr read {}", shell_quote(path?))),
        "Grep" => {
            let pattern = arguments.get("pattern")?.as_str()?;
            let mut command = format!("hzr search {} --mode exact", shell_quote(pattern));
            if let Some(path) = path {
                command.push_str(" --path ");
                command.push_str(&shell_quote(path));
            }
            Some(command)
        }
        "Edit" if mode == NativeToolMode::Strict => {
            let old = bounded_hook_text(arguments.get("old_string")?.as_str()?)?;
            let new = bounded_hook_text(arguments.get("new_string")?.as_str()?)?;
            Some(format!(
                "hzr write patch {} --old {} --new {} --cas",
                shell_quote(path?),
                shell_quote(old),
                shell_quote(new)
            ))
        }
        "Write" if mode == NativeToolMode::Strict => {
            let content = bounded_hook_text(arguments.get("content")?.as_str()?)?;
            Some(format!(
                "hzr write create {} --content {} --force",
                shell_quote(path?),
                shell_quote(content)
            ))
        }
        _ => None,
    }
}

fn bounded_hook_text(value: &str) -> Option<&str> {
    (value.len() <= 2_048 && !value.contains('\0')).then_some(value)
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
    if matches!(
        decision,
        RewriteDecision::AllowRaw { .. } | RewriteDecision::AllowRewrite { .. }
    ) {
        if let Some(replacement) = efficient_route_replacement(raw) {
            return hzr_policy_rewrite(replacement);
        }
    }
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

/// Do not re-litigate a permission the operator has already granted.
///
/// HZR derives its own verdict from the settings file, which is how a host running in
/// `bypassPermissions` still saw prompts: the hook synthesized an Ask the operator had already
/// answered. Routing and accounting are HZR's job; deciding whether a command may run is not.
/// A Deny still stands — that is an explicit rule, not an absent one — and the decision is still
/// recorded, so the ledger and the scorecard lose nothing.
fn honor_host_permission_mode(input: &Value, decision: RewriteDecision) -> RewriteDecision {
    if !host_grants_execution(input) {
        return decision;
    }
    let RewriteDecision::Ask { proposed, reason } = decision else {
        return decision;
    };
    match proposed {
        Some(command) => RewriteDecision::AllowRewrite {
            command,
            source: RewriteSource::HzrPolicy,
            reason: format!(
                "{reason}; host permission mode grants execution, so HZR recorded it instead of prompting"
            ),
        },
        None => RewriteDecision::allow_raw(
            "host permission mode grants execution; HZR recorded the bypass instead of prompting",
        ),
    }
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

async fn fallback_decision(config: &Config, raw: &str, cwd: &Path) -> RewriteDecision {
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
            return RewriteDecision::Ask {
                proposed: None,
                reason: "HZR_RAW_FIDELITY=1 requires a closed HZR_RAW_FIDELITY_REASON".into(),
            };
        }
        RawFidelityRequest::InvalidReason => {
            return RewriteDecision::Ask {
                proposed: None,
                reason: "HZR_RAW_FIDELITY_REASON is not an allowed fidelity reason".into(),
            };
        }
        RawFidelityRequest::Authorized { payload, .. } => {
            if let Some(replacement) = first_class_replacement(raw) {
                return hzr_policy_rewrite(replacement);
            }
            payload
        }
    };
    let authorized = matches!(fidelity, RawFidelityRequest::Authorized { .. });
    let canonical = CanonicalCommand::shell(command);
    let decision = if authorized {
        adapter.decide_byte_fidelity_in(&canonical, Some(cwd)).await
    } else {
        adapter.decide_in(&canonical, Some(cwd)).await
    };
    if authorized
        && matches!(
            &decision,
            RewriteDecision::AllowRewrite {
                source: RewriteSource::Rtk {
                    route: hzr_exec::RtkRewriteRoute::Proxy,
                    ..
                },
                ..
            }
        )
    {
        return RewriteDecision::allow_raw(
            "authorized raw fidelity request has no byte-faithful managed equivalent",
        );
    }
    decision
}

fn write_decision(input: &Value, decision: RewriteDecision) -> Result<()> {
    let output = match decision {
        RewriteDecision::AllowRaw { .. } => return Ok(()),
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
    serde_json::to_writer(io::stdout().lock(), &output)?;
    io::stdout().lock().write_all(b"\n")?;
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
    let digest = hex::encode(Sha256::digest(format!(
        "hook-session\0{session}\0{}\0{subagent}",
        agent_identity(input)
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
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => SessionFeedback::default(),
        Err(error) => return Err(error.into()),
    };
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

/// Emit bounded feedback for prompt and completion hooks. This hook is deliberately
/// failure-silent: state is advisory and must never block a user prompt or session stop.
pub async fn feedback(config: &Config) {
    let Ok(input) = read_input() else {
        return;
    };
    let event = input
        .get("hook_event_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let state = read_session(config, &input).unwrap_or_default();
    let session_summary = input
        .get("session_id")
        .and_then(Value::as_str)
        .and_then(|session_id| {
            Ledger::open(&config.data_dir.join("ledger/hzr.sqlite"))
                .ok()?
                .session_evasion_summary(session_id, FidelityAllowance::default())
                .ok()
        });
    let crosses_threshold = state.corrections >= SESSION_CORRECTION_NUDGE
        || session_summary.as_ref().is_some_and(|summary| {
            summary.avoidable_operations > 0
                && summary.avoidable_share_pct >= SESSION_AVOIDABLE_SHARE_NUDGE
        });
    if state.operations == 0 && session_summary.is_none() {
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
            let operations = session_summary
                .as_ref()
                .map_or(state.operations, |summary| {
                    summary.operations.max(state.operations)
                });
            // Both halves of the shadow budget come from the ledger, which is the only place
            // that knows what an operation actually delivered. Corrections are reported
            // separately: a corrected command never ran in its bypassed form, so counting it
            // against a budget meant to measure leakage would inflate the very number the
            // shadow window exists to calibrate.
            let recoverable = session_summary
                .as_ref()
                .map_or(0, |summary| summary.recoverable_tokens);
            let avoidable_operations = session_summary
                .as_ref()
                .map_or(0, |summary| summary.avoidable_operations);
            let avoidable_tokens = session_summary
                .as_ref()
                .map_or(0, |summary| summary.avoidable_tokens);
            let top_class = session_summary
                .as_ref()
                .and_then(|summary| summary.top_class)
                .map_or("none", EvasionClass::as_str);
            let _ = write_hook_json(json!({
                "systemMessage": format!(
                    "HZR scorecard: ops={} corrections={} native-denials={} top={} recoverable-tokens={}; shadow-budget avoidable={}/{} ops, {}/{} tokens (T3 measured, not enforced).",
                    operations,
                    state.corrections,
                    state.native_denials,
                    top_class,
                    recoverable,
                    avoidable_operations,
                    SESSION_BYPASS_COUNT_BUDGET,
                    avoidable_tokens,
                    SESSION_BYPASS_TOKEN_BUDGET,
                )
            }));
        }
        _ => {}
    }
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
    pub last_degraded_at_unix: Option<u64>,
}

#[cfg(test)]
impl AccountingCoverage {
    /// The state of an installation that has never lost the daemon. `Default` cannot say
    /// this on its own, because a defaulted `complete: false` would read as a real gap.
    pub fn default_complete() -> Self {
        Self {
            complete: true,
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

pub(crate) fn record_daemon_unavailable_operation(config: &Config) -> Result<()> {
    let ledger = config.data_dir.join("ledger");
    fs::create_dir_all(&ledger)
        .with_context(|| format!("failed to create {}", ledger.display()))?;
    let path = daemon_unavailable_total_path(config);
    let lock_path = path.with_extension("lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("failed to open {}", lock_path.display()))?;
    lock.lock_exclusive()?;
    let current = daemon_unavailable_operations(config)?;
    crate::adoption::atomic_write(&path, format!("{}\n", current.saturating_add(1)).as_bytes())?;
    FileExt::unlock(&lock)?;
    Ok(())
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

fn record_degraded_rewrite(config: &Config) -> Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    record_degraded_rewrite_at(config, timestamp)
}

fn record_degraded_rewrite_at(config: &Config, timestamp: u64) -> Result<()> {
    let ledger = config.data_dir.join("ledger");
    fs::create_dir_all(&ledger)
        .with_context(|| format!("failed to create {}", ledger.display()))?;
    let path = degraded_log_path(config);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    writeln!(file, "{timestamp}")?;
    Ok(())
}

/// Fold the open gap into the lifetime total once the daemon proves it is serving again.
///
/// Called after every successful managed rewrite, so the common case — nothing pending —
/// must not touch the filesystem beyond one `metadata` probe.
fn clear_reconciled_rewrites(config: &Config) -> Result<()> {
    let path = degraded_log_path(config);
    let pending = match fs::metadata(&path) {
        Ok(metadata) if metadata.len() > 0 => read_degraded_log(config)?.len(),
        Ok(_) => 0,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    };
    if pending == 0 {
        return Ok(());
    }
    let total = read_lifetime_total(config)?.unwrap_or(0) + pending;
    let total_path = degraded_total_path(config);
    fs::write(&total_path, format!("{total}\n"))
        .with_context(|| format!("failed to write {}", total_path.display()))?;
    fs::write(&path, "").with_context(|| format!("failed to truncate {}", path.display()))?;
    Ok(())
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
    let pending = read_degraded_log(config)?;
    // A ledger written by an earlier HZR has no lifetime file; its open log *is* the
    // history, so fall back to it instead of reporting a zero total next to a non-zero gap.
    let lifetime = read_lifetime_total(config)?.unwrap_or(0) + pending.len();
    let daemon_unavailable_operations = daemon_unavailable_operations(config)?;
    Ok(AccountingCoverage {
        unreconciled_rewrites: pending.len(),
        lifetime_rewrites: lifetime,
        daemon_unavailable_operations,
        complete: pending.is_empty() && daemon_unavailable_operations == 0,
        last_degraded_at_unix: pending.last().copied(),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use hzr_core::{
        Config, FidelityAllowance, Ledger, OperationAttribution, OperationChannel,
        OperationMeasurement, OperationRoute,
    };
    use hzr_protocol::{
        EnforcementTier, EvasionAttribution, EvasionClass, EvasionPathForm, FidelityValidation,
    };
    use tempfile::tempdir;

    use crate::adoption::NativeToolMode;
    use hzr_exec::{CanonicalCommand, PINNED_RTK_VERSION, RewriteDecision, RewriteSource};

    use super::anti_evasion_fixture::{
        ProbeClass, ProbeDecision, ProbeLayer, ProbeNativeMode, ProbeSurface,
    };
    use super::{
        HookFidelityPreflight, agent_attribution, agent_identity, attach_policy_feedback,
        attach_session_attribution, clear_reconciled_rewrites, context_brief,
        degraded_rewrite_coverage, fallback_decision, honor_host_permission_mode,
        hook_fidelity_preflight, native_observation_policy, native_replacement, observe_input,
        read_session, record_daemon_unavailable_operation, record_degraded_rewrite_at,
        record_local_policy_decision, record_native_correction, render_command,
        steer_to_first_class,
    };

    #[test]
    fn daemon_unavailable_telemetry_is_a_bounded_durable_counter() {
        let directory = tempdir().expect("temporary directory");
        let config = config(directory.path());
        for _ in 0..100 {
            record_daemon_unavailable_operation(&config).expect("record degraded operation");
        }

        let coverage = degraded_rewrite_coverage(&config).expect("coverage");
        assert_eq!(coverage.daemon_unavailable_operations, 100);
        let counter = config
            .data_dir
            .join("ledger/daemon-unavailable-operations.total");
        assert!(fs::metadata(counter).expect("counter metadata").len() <= 32);
    }

    #[test]
    fn test_native_observer_records_coverage_without_claiming_savings() {
        let directory = tempdir().expect("temporary directory");
        let config = config(directory.path());
        observe_input(
            &config,
            &serde_json::json!({
                "tool_name": "Read",
                "tool_input": {"file_path": "/work/src/lib.rs"},
                "tool_response": {"content": "four words of output"},
                "cwd": "/work",
                "session_id": "session-1",
                "agent_type": "claude-code",
                "agent_id": "agent-private-123"
            }),
            NativeToolMode::Observe,
        )
        .expect("native observation");

        let summary = Ledger::open(&config.data_dir.join("ledger/hzr.sqlite"))
            .expect("ledger")
            .efficiency_summary()
            .expect("summary");
        assert_eq!(summary.operations, 0);
        assert_eq!(summary.native_unaccounted_operations, 1);
        assert_eq!(summary.total_observed_operations, 1);
        assert_eq!(summary.by_channel.get("native_host"), Some(&1));
        let correction = serde_json::json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/work/src/lib.rs"},
            "cwd": "/work",
            "session_id": "session-1",
            "agent_type": "claude-code",
            "agent_id": "agent-private-123"
        });
        record_native_correction(&config, &correction, "Read").expect("typed native correction");
        let agent = Ledger::open(&config.data_dir.join("ledger/hzr.sqlite"))
            .expect("ledger")
            .session_evasion_summary("session-1", FidelityAllowance::default())
            .expect("session summary");
        assert_eq!(agent.agent.as_deref(), Some("claude-code"));
        assert!(agent.agent_hash.is_some());
        assert_eq!(agent.avoidable_operations, 0);
        assert_eq!(agent.policy_attempts, 1);
        assert_eq!(agent.policy_denials, 1);
        assert!(
            !serde_json::to_string(&agent)
                .expect("summary JSON")
                .contains("agent-private-123")
        );
    }

    #[test]
    fn acceptance_gate_local_fidelity_ask_is_a_policy_event_not_an_operation() {
        let directory = tempdir().expect("temporary directory");
        let config = config(directory.path());
        let input = serde_json::json!({
            "cwd": "/private/work",
            "session_id": "private-session",
            "agent_type": "claude-code",
            "agent_id": "private-agent"
        });
        let decision = RewriteDecision::Ask {
            proposed: None,
            reason: "bounded audit reason".into(),
        };
        record_local_policy_decision(
            &config,
            &input,
            &decision,
            Some(EvasionAttribution {
                class: EvasionClass::E7FidelityHatch,
                wrapper_depth: 1,
                interpreter: None,
                path_form: EvasionPathForm::Bare,
                stage_count: 1,
                hatch_marker: true,
                avoidable: true,
                tier: EnforcementTier::T4HatchQuarantine,
                fidelity_reason: None,
                fidelity_validation: FidelityValidation::MissingReason,
            }),
        )
        .expect("policy event");
        let ledger = Ledger::open(&config.data_dir.join("ledger/hzr.sqlite")).expect("ledger");
        assert_eq!(
            ledger.efficiency_summary().expect("efficiency").operations,
            0
        );
        let score = ledger
            .session_evasion_summary("private-session", FidelityAllowance::default())
            .expect("score");
        assert_eq!(score.policy_attempts, 1);
        assert_eq!(score.policy_asks, 1);
        assert_eq!(score.top_class, Some(EvasionClass::E7FidelityHatch));
        assert!(
            !serde_json::to_string(&score)
                .expect("JSON")
                .contains("private")
        );
    }

    #[test]
    fn acceptance_gate_steer_allowed_native_tools_are_typed_e10_bypasses() {
        let directory = tempdir().expect("temporary directory");
        let config = config(directory.path());
        for tool in ["Glob", "Edit", "Write"] {
            observe_input(
                &config,
                &serde_json::json!({
                    "tool_name": tool,
                    "tool_response": {"content": "measured native result"},
                    "cwd": "/work",
                    "session_id": "session-native-allowed",
                    "agent_type": "claude-code"
                }),
                NativeToolMode::Steer,
            )
            .expect("policy-allowed native observation");
        }

        let ledger = Ledger::open(&config.data_dir.join("ledger/hzr.sqlite")).expect("ledger");
        let efficiency = ledger.efficiency_summary().expect("efficiency summary");
        assert_eq!(efficiency.native_unaccounted_operations, 0);
        assert_eq!(efficiency.operations, 3);
        assert_eq!(
            efficiency.baseline_tokens_estimated,
            efficiency.delivered_tokens_estimated
        );
        assert_eq!(efficiency.net_avoided_tokens_estimated, 0);
        assert_eq!(efficiency.by_channel.get("native_host"), Some(&3));

        let evasion = ledger
            .session_evasion_summary("session-native-allowed", FidelityAllowance::default())
            .expect("session evasion summary");
        assert_eq!(evasion.top_class, Some(EvasionClass::E10CapabilityGap));
        assert_eq!(evasion.avoidable_operations, 0);
    }

    #[test]
    fn acceptance_gate_native_modes_prescribe_only_proven_surfaces() {
        let read = serde_json::json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/work/file with spaces.md"}
        });
        assert_eq!(
            native_replacement(&read, NativeToolMode::Steer).as_deref(),
            Some("hzr read '/work/file with spaces.md'")
        );
        let grep = serde_json::json!({
            "tool_name": "Grep",
            "tool_input": {"pattern": "two words", "path": "/work/src"}
        });
        assert_eq!(
            native_replacement(&grep, NativeToolMode::Steer).as_deref(),
            Some("hzr search 'two words' --mode exact --path '/work/src'")
        );
        for mode in [
            NativeToolMode::Observe,
            NativeToolMode::Steer,
            NativeToolMode::Strict,
        ] {
            assert_eq!(
                native_replacement(
                    &serde_json::json!({"tool_name": "Glob", "tool_input": {"pattern": "**/*"}}),
                    mode,
                ),
                None,
                "Glob must always remain allowed"
            );
        }
        assert_eq!(
            native_replacement(
                &serde_json::json!({"tool_name": "Edit", "tool_input": {
                    "file_path": "x.rs", "old_string": "old", "new_string": "new"
                }}),
                NativeToolMode::Steer,
            ),
            None,
            "steer must not deny native edits"
        );
        assert!(
            native_replacement(
                &serde_json::json!({"tool_name": "Edit", "tool_input": {
                    "file_path": "x.rs", "old_string": "old", "new_string": "new"
                }}),
                NativeToolMode::Strict,
            )
            .is_some()
        );
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
                // Produced by the native correction and fidelity preflight paths, which already
                // name the fault and the replacement.
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
        assert_eq!(asserted, 31, "every Ask and Deny case carries an assertion");
    }

    #[test]
    fn acceptance_gate_shared_fixture_covers_every_native_mode() {
        let probes = super::anti_evasion_fixture::load_anti_evasion_probes();
        let native = probes
            .iter()
            .filter(|probe| {
                probe.layer == ProbeLayer::Root && probe.surface == ProbeSurface::Native
            })
            .collect::<Vec<_>>();
        assert_eq!(native.len(), 15, "five tools across three modes");

        for probe in native {
            let mode = match probe.mode.expect("validated native mode") {
                ProbeNativeMode::Observe => NativeToolMode::Observe,
                ProbeNativeMode::Steer => NativeToolMode::Steer,
                ProbeNativeMode::Strict => NativeToolMode::Strict,
            };
            let input = serde_json::json!({
                "tool_name": probe.tool.as_deref().expect("native tool"),
                "tool_input": probe.tool_input.as_ref().expect("native tool input"),
            });
            let replacement = native_replacement(&input, mode);
            let tool = probe.tool.as_deref().expect("native tool");
            let would_deny = mode != NativeToolMode::Observe
                && tool != "Glob"
                && !(mode == NativeToolMode::Steer && matches!(tool, "Edit" | "Write"))
                && replacement.is_some();
            match probe.decision {
                ProbeDecision::Deny => {
                    assert!(would_deny, "native probe {} was not denied", probe.id);
                    assert_eq!(
                        replacement.as_deref(),
                        probe.route.as_deref(),
                        "native probe {} did not prescribe its managed route",
                        probe.id
                    );
                }
                ProbeDecision::Allow => {
                    assert!(!would_deny, "native probe {} was denied", probe.id)
                }
                ProbeDecision::Rewrite
                | ProbeDecision::Ask
                | ProbeDecision::Proxy
                | ProbeDecision::Raw => {
                    assert!(
                        matches!(probe.decision, ProbeDecision::Allow | ProbeDecision::Deny),
                        "invalid native decision for {}",
                        probe.id
                    )
                }
            }
            let (_, attribution) = native_observation_policy(tool, mode);
            let class = match probe.class.expect("validated native class") {
                ProbeClass::E8NativeTool => EvasionClass::E8NativeTool,
                ProbeClass::E10CapabilityGap => EvasionClass::E10CapabilityGap,
            };
            assert_eq!(attribution.class, class, "native probe {}", probe.id);
            assert_eq!(
                attribution.avoidable,
                probe.avoidable.expect("native avoidable flag"),
                "native probe {}",
                probe.id
            );
        }
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
        assert!(summary.avoidable_tokens <= super::SESSION_BYPASS_TOKEN_BUDGET);
    }

    #[test]
    fn acceptance_gate_t1_feedback_is_counted_without_raw_identity() {
        let directory = tempdir().expect("temporary directory");
        let config = config(directory.path());
        let input = serde_json::json!({
            "session_id": "private-session",
            "agent_type": "claude-code",
            "agent_id": "private-subagent"
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
    fn acceptance_gate_no_unbounded_exact_read_in_hook() {
        let filtered = RewriteDecision::AllowRewrite {
            command: CanonicalCommand::shell("rtk read src/main.rs --level none"),
            source: RewriteSource::Rtk {
                version: PINNED_RTK_VERSION.into(),
                route: hzr_exec::RtkRewriteRoute::Optimized,
            },
            reason: "fork-core accepted the explicit read".into(),
        };
        let decision =
            steer_to_first_class("hzr rtk -- read src/main.rs --level none", filtered.clone());
        assert_eq!(
            proposed(&decision).as_deref(),
            Some("hzr rtk -- read src/main.rs")
        );

        for command in [
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
        let script = format!(
            r#"#!/bin/sh
if test "${{1:-}}" = --version; then
  printf 'rtk %s\n' '{PINNED_RTK_VERSION}'
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

        for probe in shell_probes {
            let plan = match probe.decision {
                ProbeDecision::Rewrite => serde_json::json!({
                    "decision": "rewrite",
                    "proposed": probe.route.as_deref().expect("rewrite route")
                }),
                ProbeDecision::Ask => {
                    serde_json::json!({"decision": "ask", "reason": "canonical_policy"})
                }
                ProbeDecision::Proxy | ProbeDecision::Raw => {
                    serde_json::json!({"decision": "proxy"})
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
            let decision = steer_to_first_class(command, fallback);
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

    /// The decisive behaviour: once the daemon serves a rewrite again the earlier gap is
    /// reconciled and coverage returns to complete. The lifetime count is kept, because
    /// erasing the history would be the dishonest fix.
    #[test]
    fn test_a_healthy_daemon_reconciles_the_gap_but_keeps_the_lifetime_count() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());
        record_degraded_rewrite_at(&config, 1_785_531_432).expect("record");
        record_degraded_rewrite_at(&config, 1_785_531_500).expect("record");

        clear_reconciled_rewrites(&config).expect("reconcile");
        let coverage = degraded_rewrite_coverage(&config).expect("coverage");

        assert!(coverage.complete, "a healthy daemon closes the gap");
        assert_eq!(coverage.unreconciled_rewrites, 0);
        assert_eq!(
            coverage.lifetime_rewrites, 2,
            "history is retained, only the open gap is closed"
        );
    }

    #[test]
    fn test_reconciling_twice_does_not_double_count_the_lifetime_total() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());
        record_degraded_rewrite_at(&config, 1_785_531_432).expect("record");

        clear_reconciled_rewrites(&config).expect("reconcile");
        clear_reconciled_rewrites(&config).expect("reconcile again");
        record_degraded_rewrite_at(&config, 1_785_600_000).expect("record");
        let coverage = degraded_rewrite_coverage(&config).expect("coverage");

        assert_eq!(coverage.unreconciled_rewrites, 1);
        assert_eq!(coverage.lifetime_rewrites, 2);
    }

    /// Reconciling when nothing is pending must not touch the filesystem: this runs on
    /// every successful managed rewrite, the hottest path HZR has.
    #[test]
    fn test_reconciling_a_clean_ledger_writes_nothing() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());

        clear_reconciled_rewrites(&config).expect("reconcile");

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
