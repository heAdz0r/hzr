mod config;
mod delegation;
mod preflight;
mod process;
mod runner;

pub use config::{
    BearerToken, ConfigError, HzrApi, IntegrationLayout, ManagedAgentConfig, ResponseFormat,
    WorkerConfig,
};
pub use preflight::{
    CAVEMAN_CODE_NPM_INTEGRITY, CAVEMAN_CODE_NPM_VERSION, NODE_MAXIMUM_VERSION_EXCLUSIVE,
    NODE_MINIMUM_VERSION, NodeVersion, PACKAGE_LOCK_SHA256, PreflightError, PreflightReport,
    RuntimeMetadata, preflight,
};
pub use runner::{AgentEvent, AgentRun, ManagedAgent, RunError};

/// How many bytes of `AGENTS.md` plus `CLAUDE.md` the managed worker preloads.
///
/// A preload budget, not an admission rule. A repository whose rules exceed it still
/// delegates: the bridge preloads the head of each file, tells the worker which sections
/// it did not preload, and the worker reads the rest with `hzr_read`. Refusing to run at
/// all left `hzr delegate` unusable in exactly the repositories whose rules matter most.
/// The bridge constant `PROJECT_INSTRUCTIONS_BUDGET_BYTES` must carry the same value;
/// `test_bridge_budgets_project_instructions_instead_of_refusing_them` asserts it.
pub const PROJECT_INSTRUCTIONS_BUDGET_BYTES: usize = 24 * 1024;

#[cfg(test)]
mod tests {
    const BRIDGE: &str = include_str!("../../../integrations/caveman-code/bridge.mjs");

    #[test]
    fn test_bridge_accounts_provider_usage_once_for_every_terminal_outcome() {
        assert_eq!(BRIDGE.matches("\"/v1/usage\"").count(), 1);
        for outcome in ["completed", "invalid_response", "failed"] {
            assert!(
                BRIDGE.contains(&format!("\"{outcome}\"")),
                "missing bounded usage outcome: {outcome}"
            );
        }
        for required in [
            "usage = await recordUsage(",
            "actual: {",
            "estimated: {",
            "session.getSessionStats()",
            "stats.tokens.input",
            "stats.tokens.output",
            "event.type === \"auto_retry_start\"",
            "usage_recorded: usage.recorded",
            "usage_warning: usage.warning",
            "project_path: workspace",
        ] {
            assert!(
                BRIDGE.contains(required),
                "missing usage invariant: {required}"
            );
        }
        assert!(!BRIDGE.contains("cost_microusd"));
        assert!(!BRIDGE.contains("\"accepted\""));

        let prompt = BRIDGE
            .find("await session.prompt(")
            .expect("prompt invocation");
        let validation = BRIDGE
            .rfind("validateAssistantOutput(text")
            .expect("output validation invocation");
        let accounting = BRIDGE
            .find("usage = await recordUsage(")
            .expect("usage accounting invocation");
        assert!(prompt < validation);
        assert!(validation < accounting);
    }

    #[test]
    fn test_bridge_budgets_project_instructions_instead_of_refusing_them() {
        assert!(
            BRIDGE.contains(&format!(
                "const PROJECT_INSTRUCTIONS_BUDGET_BYTES = {} * 1024;",
                super::PROJECT_INSTRUCTIONS_BUDGET_BYTES / 1024
            )),
            "the bridge budget must equal PROJECT_INSTRUCTIONS_BUDGET_BYTES"
        );
        for required in [
            "export function fitProjectInstructions(",
            "hzr_instructions_truncated",
            "health.warnings.push(...projectInstructions.warnings)",
            "must not be a symlink",
        ] {
            assert!(
                BRIDGE.contains(required),
                "missing instruction budget invariant: {required}"
            );
        }
        assert!(
            !BRIDGE.contains("project instructions exceed"),
            "oversized repository rules must be budgeted, never a refusal to run"
        );
    }

    #[test]
    fn test_bridge_enforces_response_quality_before_and_after_generation() {
        for required in [
            "assertResourceInvariants(resourceLoader, responseContract)",
            "session.systemPrompt.split(responseContract).length !== 2",
            "model response is empty",
            "model response is not valid JSON",
            "JSON.parse(text)",
        ] {
            assert!(
                BRIDGE.contains(required),
                "missing quality invariant: {required}"
            );
        }
        assert!(
            BRIDGE
                .matches("assertSessionInvariants(\n      session,")
                .count()
                >= 2,
            "session invariants must be checked before and after generation"
        );
    }
}
