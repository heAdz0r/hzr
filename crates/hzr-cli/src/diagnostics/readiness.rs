use serde::Serialize;

use super::{CheckStatus, DoctorCheck, ResponseCodecCoverage};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessStatus {
    Ready,
    Degraded,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ReadinessDimension {
    pub status: ReadinessStatus,
    pub evidence_checks: Vec<String>,
    pub meaning: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ReadinessReport {
    pub installation: ReadinessDimension,
    pub process: ReadinessDimension,
    pub retrieval: ReadinessDimension,
    pub accounting: ReadinessDimension,
    pub host_delivery: ReadinessDimension,
    pub economic_claim_ready: bool,
}

impl ReadinessReport {
    pub fn from_checks(checks: &[DoctorCheck], codec: &[ResponseCodecCoverage]) -> Self {
        let installation = dimension(
            checks,
            |name| {
                !matches!(
                    name,
                    "daemon"
                        | "daemon_process"
                        | "daemon_service"
                        | "memory_runtime"
                        | "semantic_runtime"
                        | "index_readiness"
                        | "degraded_rewrites"
                        | "undrained_receipts"
                        | "foreign_engine_processes"
                        | "orphaned_engine_processes" // 0.8.1
                ) && !name.starts_with("global_codec")
            },
            &["hzr_on_path", "hook_ownership"],
            "installation/configuration checks only; does not prove engine readiness",
        );
        let process = dimension(
            checks,
            |name| {
                matches!(
                    name,
                    "daemon_process"
                        | "daemon_service"
                        | "foreign_engine_processes"
                        | "orphaned_engine_processes" // 0.8.1
                )
            },
            &["daemon_process"],
            "authenticated daemon and detected process ownership; not retrieval quality",
        );
        let retrieval = dimension(
            checks,
            |name| {
                matches!(
                    name,
                    "index_readiness" | "semantic_runtime" | "memory_runtime"
                )
            },
            &["index_readiness", "semantic_runtime", "memory_runtime"],
            "current workspace index, semantic runtime and memory readiness; no task-quality claim",
        );
        let accounting = dimension(
            checks,
            |name| matches!(name, "degraded_rewrites" | "undrained_receipts"),
            &["degraded_rewrites"],
            "known receipt gaps only; complete host traffic coverage remains unknown",
        );
        let confirmed = !codec.is_empty()
            && codec
                .iter()
                .all(|coverage| coverage.global_response_replacement_confirmed);
        Self {
            installation, process, retrieval, accounting,
            host_delivery: ReadinessDimension {
                status: if confirmed { ReadinessStatus::Ready } else { ReadinessStatus::Unknown },
                evidence_checks: Vec::new(),
                meaning: if confirmed {
                    "registered response surfaces confirm replacement; arbitrary native tool coverage remains unproven"
                } else {
                    "instructions and codec invocation do not prove actual host delivery or complete native tool interception"
                }.into(),
            },
            economic_claim_ready: false,
        }
    }
}

fn dimension(
    checks: &[DoctorCheck],
    select: impl Fn(&str) -> bool,
    required: &[&str],
    meaning: &str,
) -> ReadinessDimension {
    let selected = checks
        .iter()
        .filter(|check| select(&check.name))
        .collect::<Vec<_>>();
    let status = if selected
        .iter()
        .any(|check| check.status != CheckStatus::Pass)
    {
        ReadinessStatus::Degraded
    } else if required
        .iter()
        .any(|name| !selected.iter().any(|check| check.name == *name))
    {
        ReadinessStatus::Unknown
    } else {
        ReadinessStatus::Ready
    };
    ReadinessDimension {
        status,
        evidence_checks: selected.iter().map(|check| check.name.clone()).collect(),
        meaning: meaning.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_and_missing_probes_do_not_become_readiness_or_economic_proof() {
        let checks = vec![
            DoctorCheck {
                name: "hzr_on_path".into(),
                status: CheckStatus::Pass,
                detail: String::new(),
            },
            DoctorCheck {
                name: "hook_ownership".into(),
                status: CheckStatus::Pass,
                detail: String::new(),
            },
            DoctorCheck {
                name: "daemon_process".into(),
                status: CheckStatus::Pass,
                detail: String::new(),
            },
            DoctorCheck {
                name: "degraded_rewrites".into(),
                status: CheckStatus::Warning,
                detail: String::new(),
            },
        ];
        let report = ReadinessReport::from_checks(&checks, &[]);
        assert_eq!(report.installation.status, ReadinessStatus::Ready);
        assert_eq!(report.process.status, ReadinessStatus::Ready);
        assert_eq!(report.retrieval.status, ReadinessStatus::Unknown);
        assert_eq!(report.accounting.status, ReadinessStatus::Degraded);
        assert_eq!(report.host_delivery.status, ReadinessStatus::Unknown);
        assert!(!report.economic_claim_ready);
        let report = ReadinessReport::from_checks(&[], &[]);
        assert_eq!(report.process.status, ReadinessStatus::Unknown);
        assert_eq!(report.accounting.status, ReadinessStatus::Unknown);
    }
}
