use crate::discover::registry;
use crate::permissions::{check_command, PermissionVerdict};
use std::io::Write;

/// Run the `rtk rewrite` command.
///
/// Prints the RTK-rewritten command to stdout and exits with a code consumed by
/// the shell hook:
/// - 0: rewrite allowed; hook may auto-allow the rewritten command
/// - 1: no RTK equivalent; hook passes through unchanged
/// - 2: deny rule matched; hook defers to Claude Code native deny
/// - 3: ask/default; hook rewrites but omits permissionDecision
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

    match registry::rewrite_command(cmd, &excluded, &transparent_prefixes) {
        Some(rewritten) => match verdict {
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
        None => {
            std::process::exit(1);
        }
    }
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
