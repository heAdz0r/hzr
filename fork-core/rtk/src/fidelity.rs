use std::ffi::OsStr;

use anyhow::{bail, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FidelityReason {
    Binary,
    Checksum,
    MachineProtocol,
    CompleteLog,
    FullPatch,
    VerbatimSource,
}

pub fn exact_requested(allowed: &[FidelityReason]) -> Result<bool> {
    validate_request(
        std::env::var_os("HZR_RAW_FIDELITY").as_deref(),
        std::env::var_os("HZR_RAW_FIDELITY_REASON").as_deref(),
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
            .and_then(parse_reason)
            .is_some_and(|reason| allowed.contains(&reason));
    if !authorized {
        bail!(
            "exact fidelity refused: HZR_RAW_FIDELITY=1 requires one route-compatible closed HZR_RAW_FIDELITY_REASON"
        );
    }
    Ok(true)
}

fn parse_reason(reason: &str) -> Option<FidelityReason> {
    match reason {
        "binary" => Some(FidelityReason::Binary),
        "checksum" => Some(FidelityReason::Checksum),
        "machine_protocol" => Some(FidelityReason::MachineProtocol),
        "complete_log" => Some(FidelityReason::CompleteLog),
        "full_patch" => Some(FidelityReason::FullPatch),
        "verbatim_source" => Some(FidelityReason::VerbatimSource),
        _ => None,
    }
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
