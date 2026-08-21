use crate::discover::registry;
use crate::permissions::{check_command, PermissionVerdict};
use serde::Serialize;
use std::io::Write;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum RewritePlanDecision {
    Rewrite,
    Proxy,
    Ask,
    Deny,
}

#[derive(Serialize)]
struct RewritePlan<'a> {
    decision: RewritePlanDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    proposed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attribution: Option<registry::CanonicalAttribution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
}

/// Run the `rtk rewrite` command.
///
/// Prints the RTK-rewritten command to stdout and exits with a code consumed by
/// the shell hook:
/// - 0: rewrite allowed; hook may auto-allow the rewritten command
/// - 1: no RTK equivalent; hook passes through unchanged
/// - 2: deny rule matched; hook defers to Claude Code native deny
/// - 3: ask/default; hook rewrites but omits permissionDecision
/// - 4: an opaque shell wrapper is ambiguous; hook asks without proposing a rewrite
///
/// Used by shell hooks to rewrite commands transparently:
/// ```bash
/// REWRITTEN=$(rtk rewrite "$CMD") || exit 0
/// [ "$CMD" = "$REWRITTEN" ] && exit 0  # already RTK, skip
/// ```
pub fn run(cmd: &str) -> anyhow::Result<()> {
    let (excluded, transparent_prefixes) = crate::config::Config::load()
        .map(|c| (c.hooks.exclude_commands, c.hooks.transparent_prefixes))
        .unwrap_or_default();

    let verdict = check_command(cmd);
    if verdict == PermissionVerdict::Deny {
        std::process::exit(2);
    }

    let byte_fidelity = std::env::var_os("HZR_INTERNAL_BYTE_FIDELITY").as_deref()
        == Some(std::ffi::OsStr::new("1"));
    match registry::rewrite_command_outcome_with_fidelity(
        cmd,
        &excluded,
        &transparent_prefixes,
        byte_fidelity,
    ) {
        registry::RewriteOutcome::Rewritten(rewritten) => match verdict {
            PermissionVerdict::Allow => {
                print!("{}", rewritten);
                let _ = std::io::stdout().flush();
                Ok(())
            }
            PermissionVerdict::Ask | PermissionVerdict::Default => {
                print!("{}", rewritten);
                let _ = std::io::stdout().flush();
                std::process::exit(3);
            }
            PermissionVerdict::Deny => unreachable!(),
        },
        registry::RewriteOutcome::NoEquivalent => {
            std::process::exit(1);
        }
        registry::RewriteOutcome::ByteFidelityProxy => match verdict {
            PermissionVerdict::Allow => std::process::exit(1),
            PermissionVerdict::Ask | PermissionVerdict::Default => std::process::exit(4),
            PermissionVerdict::Deny => unreachable!(),
        },
        registry::RewriteOutcome::AmbiguousShell | registry::RewriteOutcome::PolicyAsk => {
            std::process::exit(4)
        }
    }
}

/// Emit one typed rewrite decision. The proposed command is ephemeral operational output;
/// attribution is closed, payload-free metadata suitable for accounting.
pub fn run_plan(cmd: &str) -> anyhow::Result<()> {
    let (excluded, transparent_prefixes) = crate::config::Config::load()
        .map(|config| {
            (
                config.hooks.exclude_commands,
                config.hooks.transparent_prefixes,
            )
        })
        .unwrap_or_default();
    let verdict = check_command(cmd);
    let byte_fidelity = std::env::var_os("HZR_INTERNAL_BYTE_FIDELITY").as_deref()
        == Some(std::ffi::OsStr::new("1"));
    let outcome = registry::rewrite_command_outcome_with_fidelity(
        cmd,
        &excluded,
        &transparent_prefixes,
        byte_fidelity,
    );
    let attribution = registry::canonical_attribution(cmd, &outcome);
    let plan = match (verdict, &outcome) {
        (PermissionVerdict::Deny, _) => RewritePlan {
            decision: RewritePlanDecision::Deny,
            proposed: None,
            attribution,
            reason: Some("permission_policy"),
        },
        (PermissionVerdict::Allow, registry::RewriteOutcome::Rewritten(command)) => RewritePlan {
            decision: RewritePlanDecision::Rewrite,
            proposed: Some(command.clone()),
            attribution,
            reason: None,
        },
        (
            PermissionVerdict::Ask | PermissionVerdict::Default,
            registry::RewriteOutcome::Rewritten(command),
        ) => RewritePlan {
            decision: RewritePlanDecision::Ask,
            proposed: Some(command.clone()),
            attribution,
            reason: Some("permission_policy"),
        },
        (PermissionVerdict::Allow, registry::RewriteOutcome::ByteFidelityProxy)
        | (_, registry::RewriteOutcome::NoEquivalent) => RewritePlan {
            decision: RewritePlanDecision::Proxy,
            proposed: None,
            attribution,
            reason: None,
        },
        (_, registry::RewriteOutcome::ByteFidelityProxy)
        | (_, registry::RewriteOutcome::AmbiguousShell)
        | (_, registry::RewriteOutcome::PolicyAsk) => RewritePlan {
            decision: RewritePlanDecision::Ask,
            proposed: None,
            attribution,
            reason: Some("canonical_policy"),
        },
    };
    serde_json::to_writer(std::io::stdout().lock(), &plan)?;
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_supported_command_succeeds() {
        assert!(registry::rewrite_command("git status", &[], &[]).is_some());
    }

    #[test]
    fn test_run_unsupported_returns_none() {
        assert!(registry::rewrite_command("frobnicate --xyz", &[], &[]).is_none());
    }

    #[test]
    fn test_run_already_rtk_returns_some() {
        assert_eq!(
            registry::rewrite_command("rtk git status", &[], &[]),
            Some("rtk git status".into())
        );
    }

    #[test]
    fn test_default_permission_is_not_allow() {
        assert_eq!(
            crate::permissions::check_command_with_rules("git status", &[], &[], &[]),
            crate::permissions::PermissionVerdict::Default
        );
    }
}
