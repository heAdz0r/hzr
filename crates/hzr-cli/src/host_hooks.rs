mod post;
pub use post::replace_tool_output;

use clap::ValueEnum;
use serde::Serialize;
use serde_json::{Value, json};

/// Explicit host selection avoids inferring a permission model from tool names.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum HookHost {
    #[default]
    Claude,
    Codex,
}

impl HookHost {
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

/// A fixture probe proves adapter behavior, never installation or model delivery.
pub fn capabilities(host: HookHost, probe: bool) -> Value {
    let manifest: Value =
        serde_json::from_str(include_str!("../../../contracts/agent-capabilities.json"))
            .expect("embedded agent capabilities");
    let mut result = json!({
        "adapter_contract": "hzr_host_hooks_v1",
        "host": host,
        "declared": manifest["harnesses"][host.name()]["hooks"],
        "installation": "not_probed",
        "trusted": "not_probed",
        "observed": "not_probed",
        "delivery": "unverified",
        "economic_credit": false
    });
    if probe {
        let input = json!({"hook_event_name":"PreToolUse","tool_name":"Bash",
            "tool_input":{"command":"printf hzr-hook-probe","timeout":1000}});
        let rewrite = json!({"hookSpecificOutput":{"hookEventName":"PreToolUse",
            "permissionDecision":"allow","updatedInput":input["tool_input"]}});
        let transformed = adapt_response(host, &input, rewrite.clone(), false);
        result["fixture_probe"] = json!({
            "supported_shape": supports_pre_tool(host, &input),
            "unsupported_shape_passes_through": !supports_pre_tool(host, &json!({"tool_name":"Bash"})),
            "preserves_permission_boundary": match host {
                HookHost::Claude => transformed.as_ref().is_some_and(|v| v["hookSpecificOutput"].get("permissionDecision").is_none()),
                HookHost::Codex => transformed.is_none(),
            },
            "explicit_host_grant_rewrite": adapt_response(host, &input, rewrite, true).is_some(),
            "scope": "local_adapter_fixture_only"
        });
    }
    result
}

pub fn supports_pre_tool(host: HookHost, input: &Value) -> bool {
    if input["hook_event_name"].as_str() != Some("PreToolUse") {
        return false;
    }
    match input["tool_name"].as_str() {
        Some("Bash") => {
            input["tool_input"].is_object() && input["tool_input"]["command"].is_string()
        }
        Some("Agent" | "Task") if host == HookHost::Claude => input["tool_input"].is_object(),
        _ => false,
    }
}

/// Unsupported optimization emits nothing; the host retains its normal permissions.
/// Claude accepts argument updates without auto-approval. Codex requires allow, so
/// its adapter only rewrites when the host explicitly reports bypassPermissions.
pub fn adapt_response(
    host: HookHost,
    input: &Value,
    mut output: Value,
    host_granted: bool,
) -> Option<Value> {
    if !supports_pre_tool(host, input) {
        return None;
    }
    let Some(hook) = output
        .get_mut("hookSpecificOutput")
        .and_then(Value::as_object_mut)
    else {
        return (host == HookHost::Claude).then_some(output);
    };
    match hook.get("permissionDecision").and_then(Value::as_str) {
        Some("allow") if !host_granted => match host {
            HookHost::Claude => {
                hook.remove("permissionDecision");
                hook.remove("permissionDecisionReason");
            }
            HookHost::Codex => return None,
        },
        Some("ask") if host == HookHost::Codex => {
            // Codex ignores "ask": retain a real policy boundary instead of emitting it.
            hook.insert("permissionDecision".into(), json!("deny"));
            hook.remove("updatedInput");
        }
        _ => {}
    }
    if host == HookHost::Codex {
        output.as_object_mut()?.remove("systemMessage");
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> Value {
        json!({"hook_event_name":"PreToolUse","tool_name":"Bash",
            "tool_input":{"command":"cargo test","timeout":1000}})
    }

    #[test]
    fn claude_argument_rewrite_does_not_grant_permissions() {
        let output = json!({"hookSpecificOutput":{"hookEventName":"PreToolUse",
            "permissionDecision":"allow","updatedInput": input()["tool_input"]}});
        let adapted = adapt_response(HookHost::Claude, &input(), output, false)
            .expect("supported host response fixture");
        assert!(
            adapted["hookSpecificOutput"]
                .get("permissionDecision")
                .is_none()
        );
        assert_eq!(
            adapted["hookSpecificOutput"]["updatedInput"]["timeout"],
            1000
        );
    }

    #[test]
    fn codex_rewrite_requires_explicit_host_grant_and_never_emits_ask() {
        let output = json!({"hookSpecificOutput":{"permissionDecision":"allow","updatedInput":input()["tool_input"]}});
        assert!(adapt_response(HookHost::Codex, &input(), output.clone(), false).is_none());
        assert!(adapt_response(HookHost::Codex, &input(), output, true).is_some());
        let ask = json!({"hookSpecificOutput":{"permissionDecision":"ask","updatedInput":{}}});
        let result = adapt_response(HookHost::Codex, &input(), ask, true)
            .expect("supported host response fixture");
        assert_eq!(result["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(result["hookSpecificOutput"].get("updatedInput").is_none());
    }

    #[test]
    fn native_semantics_and_unknown_shapes_pass_to_host_unchanged() {
        for name in [
            "Read",
            "Grep",
            "Glob",
            "Edit",
            "Write",
            "apply_patch",
            "exec_command",
        ] {
            let input = json!({"hook_event_name":"PreToolUse","tool_name":name,
                "tool_input":{"offset":100,"limit":20,"pattern":"a.*b","output_mode":"count"}});
            for host in [HookHost::Claude, HookHost::Codex] {
                assert!(!supports_pre_tool(host, &input));
                assert!(adapt_response(host, &input, json!({}), false).is_none());
            }
        }
        assert!(!supports_pre_tool(
            HookHost::Claude,
            &json!({"tool_name":"Bash","tool_input":{"command":1}})
        ));
        assert!(!supports_pre_tool(
            HookHost::Claude,
            &json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"x"}})
        ));
    }

    #[test]
    fn security_denial_survives_both_host_adapters() {
        for host in [HookHost::Claude, HookHost::Codex] {
            let denial = json!({"hookSpecificOutput":{"hookEventName":"PreToolUse",
                "permissionDecision":"deny","permissionDecisionReason":"outside allowed workspace"}});
            let result = adapt_response(host, &input(), denial.clone(), false)
                .expect("supported host response fixture");
            assert_eq!(result, denial);
        }
    }

    #[test]
    fn probes_do_not_claim_live_delivery_or_economic_credit() {
        for host in [HookHost::Claude, HookHost::Codex] {
            let report = capabilities(host, true);
            assert_eq!(report["delivery"], "unverified");
            assert_eq!(report["economic_credit"], false);
            for check in [
                "supported_shape",
                "unsupported_shape_passes_through",
                "preserves_permission_boundary",
                "explicit_host_grant_rewrite",
            ] {
                assert_eq!(report["fixture_probe"][check], true, "{host:?}: {check}");
            }
        }
    }
}
