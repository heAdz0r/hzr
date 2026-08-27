use std::ffi::OsStr;

use anyhow::{bail, Result};
pub use hzr_engine_contract::FidelityReason;
use hzr_engine_contract::{RAW_FIDELITY_ENV, RAW_FIDELITY_REASON_ENV};

pub fn exact_requested(allowed: &[FidelityReason]) -> Result<bool> {
    validate_request(
        std::env::var_os(RAW_FIDELITY_ENV).as_deref(),
        std::env::var_os(RAW_FIDELITY_REASON_ENV).as_deref(),
        allowed,
    )
}

pub(crate) fn validate_request(
    marker: Option<&OsStr>,
    reason: Option<&OsStr>,
    allowed: &[FidelityReason],
) -> Result<bool> {
    if marker.is_none() && reason.is_none() {
        return Ok(false);
    }
    let authorized = marker == Some(OsStr::new("1"))
        && reason
            .and_then(OsStr::to_str)
            .and_then(FidelityReason::parse)
            .is_some_and(|reason| allowed.contains(&reason));
    if !authorized {
        bail!(
            "exact fidelity refused: HZR_RAW_FIDELITY=1 requires one route-compatible closed HZR_RAW_FIDELITY_REASON"
        );
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_reason_must_be_present_and_route_compatible() {
        let allowed = [FidelityReason::MachineProtocol];
        assert!(!validate_request(None, None, &allowed).unwrap());
        assert!(validate_request(Some(OsStr::new("1")), None, &allowed).is_err());
        assert!(
            validate_request(Some(OsStr::new("1")), Some(OsStr::new("unknown")), &allowed,)
                .is_err()
        );
        assert!(validate_request(
            Some(OsStr::new("1")),
            Some(OsStr::new("complete_log")),
            &allowed,
        )
        .is_err());
        assert!(validate_request(None, Some(OsStr::new("machine_protocol")), &allowed).is_err());
        assert!(validate_request(
            Some(OsStr::new("1")),
            Some(OsStr::new("machine_protocol")),
            &allowed,
        )
        .unwrap());
    }

    #[test]
    fn refusal_does_not_echo_unknown_reason() {
        let error = validate_request(
            Some(OsStr::new("1")),
            Some(OsStr::new("user-secret-sentinel")),
            &[FidelityReason::CompleteLog],
        )
        .unwrap_err()
        .to_string();

        assert!(!error.contains("user-secret-sentinel"));
        assert!(error.contains("exact fidelity refused"));
    }
}
