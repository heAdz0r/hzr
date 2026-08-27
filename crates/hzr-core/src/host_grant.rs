//! The host's execution grant, as every HZR process below an approved command sees it.
//!
//! HZR used to answer the same policy question twice. The `PreToolUse` hook received the
//! harness's permission mode and allowed a command; the `hzr exec run` that the approval had just
//! launched re-derived the verdict with no knowledge of that answer and refused it. One intent,
//! two processes, two verdicts — and an operator watching an approved command exit 77.
//!
//! This module is the reader half of the fix. The hook mints a grant and exports it onto the
//! command it approves; everything below reads it here, through one validation, so a second
//! opinion is not merely discouraged but unavailable.

use std::time::{SystemTime, UNIX_EPOCH};

use hzr_protocol::{HOST_EXECUTION_GRANT_ENV, HostExecutionGrant, HostGrantRejection};

use crate::privacy_identity_hash;

/// Environment variables that can name the current session, in priority order.
///
/// `HZR_SESSION_ID` is first because the hook exports it onto the command it approves. Every call
/// site used to read only the harness-native variables, so an operation started *by* an approved
/// command recorded no session — per-session attribution was blank for exactly the traffic HZR
/// had just routed.
const SESSION_ENV_KEYS: [&str; 4] = [
    "HZR_SESSION_ID",
    "CODEX_THREAD_ID",
    "CLAUDE_SESSION_ID",
    "CURSOR_TRACE_ID",
];

#[must_use]
pub fn ambient_session_id() -> Option<String> {
    SESSION_ENV_KEYS.into_iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

/// The grant this process inherited.
///
/// * `None` — no grant is present; decide as though the host never answered.
/// * `Some(Err(_))` — a grant is present and was refused. The reason is returned so diagnostics
///   can report the drift instead of silently behaving as if nothing was carried.
/// * `Some(Ok(grant))` — the host's answer stands for this session, right now.
#[must_use]
pub fn inspect_ambient_host_grant() -> Option<Result<HostExecutionGrant, HostGrantRejection>> {
    let encoded = std::env::var(HOST_EXECUTION_GRANT_ENV).ok()?;
    let encoded = encoded.trim();
    if encoded.is_empty() {
        return None;
    }
    // A malformed grant is refused, never partially trusted: the only safe reading of a value we
    // cannot parse is that no host answer reached us.
    let Ok(grant) = serde_json::from_str::<HostExecutionGrant>(encoded) else {
        return Some(Err(HostGrantRejection::SessionMismatch));
    };
    let session_digest = ambient_session_id().map(|value| privacy_identity_hash("session", &value));
    Some(
        grant
            .authorize(session_digest.as_deref(), unix_millis_now())
            .map(|()| grant),
    )
}

/// Whether the host has already granted execution for this session.
#[must_use]
pub fn ambient_host_grants_execution() -> bool {
    matches!(inspect_ambient_host_grant(), Some(Ok(_)))
}

fn unix_millis_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use hzr_protocol::{HOST_EXECUTION_GRANT_MAX_AGE_MS, HostPermissionMode};

    use super::*;

    fn grant(session: &str, granted_at_ms: u64) -> HostExecutionGrant {
        HostExecutionGrant {
            mode: HostPermissionMode::BypassPermissions,
            granted_for_session: privacy_identity_hash("session", session),
            granted_at_ms,
            source: "test".into(),
        }
    }

    /// A grant is an answer about one session at one time, not a capability.
    ///
    /// Each rejection is asserted separately because they close different holes: session binding
    /// stops a value copied into another context from approving anything, and the age bound stops
    /// a grant left in an exported shell or a committed script from approving anything later.
    #[test]
    fn acceptance_gate_a_grant_is_bound_to_its_session_and_its_moment() {
        let now = 1_000_000_000_000;
        let digest = privacy_identity_hash("session", "live-session");

        assert_eq!(
            grant("live-session", now).authorize(Some(&digest), now),
            Ok(())
        );

        assert_eq!(
            grant("other-session", now).authorize(Some(&digest), now),
            Err(HostGrantRejection::SessionMismatch),
            "a grant minted for another session must never approve this one"
        );
        assert_eq!(
            grant("live-session", now).authorize(None, now),
            Err(HostGrantRejection::SessionMismatch),
            "a process with no session cannot claim a session-bound grant"
        );
        assert_eq!(
            grant("live-session", now - HOST_EXECUTION_GRANT_MAX_AGE_MS - 1)
                .authorize(Some(&digest), now),
            Err(HostGrantRejection::Expired),
            "an approval does not survive indefinitely in an environment variable"
        );
        assert_eq!(
            grant("live-session", now + HOST_EXECUTION_GRANT_MAX_AGE_MS)
                .authorize(Some(&digest), now),
            Err(HostGrantRejection::FutureTimestamp),
            "a grant stamped in the future is evidence of tampering or a broken clock"
        );

        let mut refusing = grant("live-session", now);
        refusing.mode = HostPermissionMode::Default;
        assert_eq!(
            refusing.authorize(Some(&digest), now),
            Err(HostGrantRejection::ModeDoesNotGrantExecution),
            "only a mode that actually grants execution may stand in for a prompt"
        );
    }
}
