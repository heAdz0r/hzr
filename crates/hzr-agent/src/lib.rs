mod config;
mod preflight;
mod process;
mod runner;

pub use config::{
    BearerToken, ConfigError, HzrApi, IntegrationLayout, ManagedAgentConfig, ResponseFormat,
};
pub use preflight::{
    CAVEMAN_CODE_NPM_INTEGRITY, CAVEMAN_CODE_NPM_VERSION, NODE_MAXIMUM_VERSION_EXCLUSIVE,
    NODE_MINIMUM_VERSION, NodeVersion, PACKAGE_LOCK_SHA256, PreflightError, PreflightReport,
    RuntimeMetadata, preflight,
};
pub use runner::{AgentEvent, AgentRun, ManagedAgent, RunError};

#[cfg(test)]
mod tests {
    const BRIDGE: &str = include_str!("../../../integrations/caveman-code/bridge.mjs");

    #[test]
    fn test_bridge_contains_fail_closed_ownership_invariants() {
        for required in [
            "settings.setRtkEnabled(false)",
            "settings.setCaveModeEnabled(false)",
            "settings.setCaveModeToolCompression(false)",
            "settings.setCaveModeMLCompression(false)",
            "settings.setTelemetryEnabled(false)",
            "settings.setDisableAllHooks(true)",
            "session.setRepomapEnabled(false)",
            "session.setMemoryEnabled(false)",
            "session.setAutoSnapshotEnabled(false)",
            "process.env.CAVE_OMIT_CLAUDE_MD = \"1\"",
            "process.env.CAVE_MEMORY_AUTO_RECORD = \"0\"",
            "process.env.CAVE_CHAT_MODE = \"auto\"",
            "systemPrompt: \"\"",
            "appendSystemPrompt: responseContract",
            "systemPromptOverride: () => undefined",
            "appendSystemPromptOverride: () => [responseContract]",
            "tools: []",
            "context_prefetched: true",
            "installManagedToolGuard",
            "Caveman native tool execution blocked",
            "session.agent.beforeToolCall = guardedBeforeToolCall",
        ] {
            assert!(BRIDGE.contains(required), "missing invariant: {required}");
        }
        for route in [
            "/v1/health",
            "/v1/search",
            "/v1/context/plan",
            "/v1/fork/run",
            "/v1/memory/recall",
            "/v1/memory/store",
            "/v1/exec/run",
            "/v1/usage",
        ] {
            assert!(BRIDGE.contains(route), "missing managed route: {route}");
        }
        assert!(!BRIDGE.contains("bashTool"));
        assert!(!BRIDGE.contains("grepTool"));
        assert!(!BRIDGE.contains("readTool"));
        assert!(!BRIDGE.contains("editTool"));
        assert!(!BRIDGE.contains("writeTool"));
        for managed_file_tool in ["hzr_read", "hzr_edit", "hzr_write"] {
            assert!(
                BRIDGE.contains(managed_file_tool),
                "missing managed fork-core file tool: {managed_file_tool}"
            );
        }
        assert!(
            BRIDGE.contains("callHzr(\"/v1/memory/recall\", { workspace, ...params }, signal)")
        );
        assert!(BRIDGE.contains("callHzr(\"/v1/memory/store\", { workspace, ...params }, signal)"));
        assert!(BRIDGE.contains("Be concise. Lead with the result."));
        assert!(BRIDGE.contains("session.getSessionStats()"));
        assert!(BRIDGE.contains("response.body.getReader()"));
        assert!(!BRIDGE.contains("await response.text()"));
    }

    #[test]
    fn test_bridge_preflight_requires_compatible_hzr_and_ready_fork_core() {
        for required in [
            "const EXPECTED_HZR_VERSION = \"0.1.0\"",
            "const EXPECTED_PROTOCOL_VERSION = 1",
            "preflightHealth(callHzr)",
            "exactlyOneEngine(health, \"rtk\")",
            "rtk.state !== \"ready\"",
            "exactlyOneEngine(health, \"grepai\")",
            "grepai.state !== \"ready\" && grepai.state !== \"stopped\"",
            "exactlyOneEngine(health, \"icm\")",
            "preflight_warnings: health.warnings",
        ] {
            assert!(
                BRIDGE.contains(required),
                "missing preflight invariant: {required}"
            );
        }
        let health = BRIDGE
            .find("const health = await preflightHealth(callHzr)")
            .expect("health preflight invocation");
        let context = BRIDGE
            .find("const prefetchedContext = await callHzr(")
            .expect("context prefetch invocation");
        assert!(health < context);
    }

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
