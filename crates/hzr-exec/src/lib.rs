//! HZR's typed command decision and execution boundary.
//!
//! The crate keeps user commands canonical, treats shell syntax conservatively,
//! and never exposes upstream adapter exit codes as policy decisions.

mod adapter;
mod capture;
mod error;
mod executor;
mod model;
mod shell;

pub use adapter::{
    ForkCoreConfig, ForkCoreInvocation, ForkCoreRunner, ForkRuntimePaths, PINNED_RTK_VERSION,
    PinnedRtkAdapter, RtkAdapterConfig, RtkCapabilities, RtkRewriteInterface,
};
pub use error::ExecError;
pub use executor::{ExecutionHandle, ExecutionPipeline};
pub use model::{
    CanonicalCommand, CaptureConfig, CaptureOverflow, CapturedContent, CapturedStream, Environment,
    ExecutionEnvelope, ExecutionEvent, ExecutionOutcome, ExecutionResult, ExecutionStream,
    NeverWorseChoice, NotStarted, RewriteDecision, RewriteSource, RtkRewriteRoute, StdinSpec,
    Termination, TerminationCause,
};
pub use shell::{ShellSafety, analyze_shell, parse_simple_shell};

/// Selects a transformed representation only when it is strictly smaller.
///
/// Execution status is deliberately outside this function: callers must retain
/// the canonical [`ExecutionResult`] and apply the selected view only for display.
#[must_use]
pub fn choose_never_worse(raw: &[u8], candidate: &[u8]) -> NeverWorseChoice {
    if candidate.len() < raw.len() {
        NeverWorseChoice::Candidate
    } else {
        NeverWorseChoice::Raw
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        NeverWorseChoice, RewriteDecision, RewriteSource, RtkRewriteRoute, choose_never_worse,
    };

    #[test]
    fn test_choose_never_worse_selects_strictly_smaller_candidate() {
        assert_eq!(
            choose_never_worse(b"long raw output", b"short"),
            NeverWorseChoice::Candidate
        );
    }

    #[test]
    fn test_choose_never_worse_keeps_raw_on_tie() {
        assert_eq!(choose_never_worse(b"same", b"size"), NeverWorseChoice::Raw);
    }

    #[test]
    fn test_choose_never_worse_keeps_raw_when_candidate_grows() {
        assert_eq!(
            choose_never_worse(b"short", b"long candidate"),
            NeverWorseChoice::Raw
        );
    }

    #[test]
    fn test_rewrite_decision_serializes_without_exit_code_protocol() -> serde_json::Result<()> {
        let serialized = serde_json::to_value(RewriteDecision::allow_raw("exact fallback"))?;
        assert_eq!(
            serialized,
            json!({
                "decision": "allow_raw",
                "reason": "exact fallback"
            })
        );
        Ok(())
    }

    #[test]
    fn test_legacy_rtk_decision_without_route_defaults_to_optimized() -> serde_json::Result<()> {
        let decision: RewriteDecision = serde_json::from_value(json!({
            "decision": "allow_rewrite",
            "command": {"kind": "shell", "shell": "/bin/sh", "command": "rtk rg needle"},
            "source": {"source": "rtk", "version": "0.44.1-fork.1"},
            "reason": "legacy payload"
        }))?;

        assert!(matches!(
            decision,
            RewriteDecision::AllowRewrite {
                source: RewriteSource::Rtk {
                    route: RtkRewriteRoute::Optimized,
                    ..
                },
                ..
            }
        ));
        Ok(())
    }
}
