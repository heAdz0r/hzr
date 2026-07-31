//! E7.1: Intent Parser — normalize task text, classify intent, build stable fingerprint.
//! All classification is Rust rule-based (no ML dep). Ollama classify is optional in ollama.rs.

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

// ── Public types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IntentKind {
    Bugfix,
    Feature,
    Refactor,
    Incident,
    #[default]
    Unknown,
}

impl std::fmt::Display for IntentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bugfix => write!(f, "bugfix"),
            Self::Feature => write!(f, "feature"),
            Self::Refactor => write!(f, "refactor"),
            Self::Incident => write!(f, "incident"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    #[default]
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskIntent {
    /// Classified intent kind.
    pub predicted: IntentKind,
    /// Confidence in [0.0, 1.0]. 0.0 for Unknown.
    pub confidence: f32,
    /// Stable hash: normalize(task) + "|" + project_id + "|" + intent.
    pub task_fingerprint: String,
    /// Significant tokens extracted from task (stopwords removed, min 3 chars).
    pub extracted_tags: Vec<String>,
    /// Risk class based on sensitive-domain signal matches.
    pub risk_class: RiskClass,
}

// ── Stopwords ──────────────────────────────────────────────────────────────────

const STOPWORDS: &[&str] = &[
    "a", "an", "the", "is", "in", "at", "of", "on", "to", "do", "be", "we", "it", "as", "by", "or",
    "and", "for", "with", "not", "are", "was", "were", "this", "that", "has", "have", "had",
    "will", "can", "should", "would", "when", "what", "how", "why", "who", "which",
];

// ── Intent signals ─────────────────────────────────────────────────────────────

const BUGFIX_SIGNALS: &[&str] = &[
    "bug",
    "fix",
    "broken",
    "crash",
    "panic",
    "error",
    "fail",
    "failure",
    "regression",
    "incorrect",
    "wrong",
    "unexpected",
    "issue",
    "problem",
    "not working",
    "nil pointer",
    "null pointer",
    "exception",
    "stack trace",
    "traceback",
    "segfault",
    "oom",
    "out of memory",
    "timeout",
    "deadlock",
    "race condition",
    "undefined",
    "404",
    "500",
    "401",
    "403",
    "not found",
    "unauthorized",
];

const FEATURE_SIGNALS: &[&str] = &[
    "add",
    "implement",
    "create",
    "new",
    "feature",
    "support",
    "enable",
    "introduce",
    "build",
    "develop",
    "write",
    "make",
    "allow",
    "provide",
    "extend",
    "enhance",
    "integration",
    "endpoint",
    "api",
    "command",
    "cli",
    "module",
];

const REFACTOR_SIGNALS: &[&str] = &[
    "refactor",
    "refactoring",
    "restructure",
    "reorganize",
    "cleanup",
    "clean up",
    "simplify",
    "extract",
    "rename",
    "move",
    "split",
    "merge",
    "consolidate",
    "reduce",
    "eliminate",
    "modernize",
    "upgrade",
];

const INCIDENT_SIGNALS: &[&str] = &[
    "incident",
    "production",
    "prod",
    "outage",
    "degraded",
    "sev1",
    "sev2",
    "critical",
    "hotfix",
    "hot fix",
    "urgent",
    "emergency",
    "down",
    "alert",
    "alarm",
    "on-call",
    "oncall",
    "postmortem",
    "post-mortem",
];

// ── Risk domain signals ────────────────────────────────────────────────────────

const HIGH_RISK_SIGNALS: &[&str] = &[
    "auth",
    "authentication",
    "authorization",
    "password",
    "secret",
    "token",
    "key",
    "payment",
    "billing",
    "credit card",
    "stripe",
    "checkout",
    "order",
    "transaction",
    "migration",
    "database",
    "schema",
    "production",
    "deploy",
    "release",
    "admin",
    "permission",
    "role",
    "privilege",
    "access control",
];

const MEDIUM_RISK_SIGNALS: &[&str] = &[
    "api",
    "endpoint",
    "service",
    "middleware",
    "routing",
    "network",
    "cache",
    "session",
    "cookie",
    "storage",
    "file",
    "upload",
    "test",
    "integration",
    "config",
    "environment",
];

// ── Main entry point ───────────────────────────────────────────────────────────

/// Parse task text into a `TaskIntent`.
///
/// `project_id` is the project cache key (hex string) used in fingerprint calculation,
/// ensuring the same task text maps to different fingerprints across projects.
pub fn parse_intent(task: &str, project_id: &str) -> TaskIntent {
    let normalized = normalize_task(task);
    let tags = extract_tags(&normalized);
    let (predicted, confidence) = classify_intent(&normalized);
    let risk_class = classify_risk(&normalized);
    let fingerprint = build_fingerprint(&normalized, project_id, &predicted);
    TaskIntent {
        predicted,
        confidence,
        task_fingerprint: fingerprint,
        extracted_tags: tags,
        risk_class,
    }
}

/// Normalize: lowercase, keep alphanumeric + space/colon/underscore/dash, collapse whitespace.
pub fn normalize_task(task: &str) -> String {
    let lower = task.to_lowercase();
    let cleaned: String = lower
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " :/_-".contains(c) {
                c
            } else {
                ' '
            }
        })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Extract entity tags: significant tokens not in stopwords, min 3 chars, capped at 20.
fn extract_tags(normalized: &str) -> Vec<String> {
    let stopset: std::collections::HashSet<&str> = STOPWORDS.iter().copied().collect();
    normalized
        .split_whitespace()
        .filter(|t| t.len() >= 3 && !stopset.contains(*t))
        .map(|t| t.to_string())
        .take(20)
        .collect()
}

/// Rule-based intent classification. Returns (kind, confidence).
fn classify_intent(normalized: &str) -> (IntentKind, f32) {
    let scores = [
        (
            IntentKind::Bugfix,
            score_signals(normalized, BUGFIX_SIGNALS),
        ),
        (
            IntentKind::Feature,
            score_signals(normalized, FEATURE_SIGNALS),
        ),
        (
            IntentKind::Refactor,
            score_signals(normalized, REFACTOR_SIGNALS),
        ),
        (
            IntentKind::Incident,
            score_signals(normalized, INCIDENT_SIGNALS),
        ),
    ];
    let total: f32 = scores.iter().map(|(_, s)| s).sum();
    let best = scores
        .iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    match best {
        Some((kind, score)) if *score > 0.0 => {
            let raw = if total > 0.0 { score / total } else { 0.0 };
            (kind.clone(), raw.clamp(0.4, 0.95)) // changed: .clamp() replaces .max().min() chain
        }
        _ => (IntentKind::Unknown, 0.0),
    }
}

/// Count signals from the list that appear as substrings in normalized text.
fn score_signals(normalized: &str, signals: &[&str]) -> f32 {
    signals.iter().filter(|&&s| normalized.contains(s)).count() as f32
}

/// Risk classification based on domain-signal matches.
fn classify_risk(normalized: &str) -> RiskClass {
    if score_signals(normalized, HIGH_RISK_SIGNALS) >= 1.0 {
        RiskClass::High
    } else if score_signals(normalized, MEDIUM_RISK_SIGNALS) >= 1.0 {
        RiskClass::Medium
    } else {
        RiskClass::Low
    }
}

/// Build a stable fingerprint: xxh3_64(normalized + "|" + project_id + "|" + intent).
fn build_fingerprint(normalized: &str, project_id: &str, intent: &IntentKind) -> String {
    let input = format!("{}|{}|{}", normalized, project_id, intent);
    format!("{:016x}", xxh3_64(input.as_bytes()))
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bugfix_classification() {
        let i = parse_intent("bug: jwt token not refreshing on 401", "proj1");
        assert_eq!(i.predicted, IntentKind::Bugfix);
        assert!(i.confidence >= 0.4);
    }

    #[test]
    fn test_feature_classification() {
        let i = parse_intent("add support for oauth2 authentication endpoint", "proj1");
        assert_eq!(i.predicted, IntentKind::Feature);
    }

    #[test]
    fn test_refactor_classification() {
        let i = parse_intent("refactor the auth module to reduce duplication", "proj1");
        assert_eq!(i.predicted, IntentKind::Refactor);
    }

    #[test]
    fn test_incident_classification() {
        let i = parse_intent("production outage: payments service down sev1", "proj1");
        assert_eq!(i.predicted, IntentKind::Incident);
        assert_eq!(i.risk_class, RiskClass::High);
    }

    #[test]
    fn test_stable_fingerprint_same_project() {
        let a = parse_intent("fix the login bug", "proj1");
        let b = parse_intent("fix the login bug", "proj1");
        assert_eq!(a.task_fingerprint, b.task_fingerprint);
    }

    #[test]
    fn test_different_projects_different_fingerprint() {
        let a = parse_intent("fix the login bug", "proj1");
        let b = parse_intent("fix the login bug", "proj2");
        assert_ne!(a.task_fingerprint, b.task_fingerprint);
    }

    #[test]
    fn test_high_risk_auth_task() {
        let i = parse_intent("token validation broken in auth middleware", "proj1");
        assert_eq!(i.risk_class, RiskClass::High);
    }

    #[test]
    fn test_unknown_intent_empty_input() {
        let i = parse_intent("", "proj1");
        assert_eq!(i.predicted, IntentKind::Unknown);
        assert_eq!(i.confidence, 0.0);
    }

    #[test]
    fn test_tags_strip_stopwords() {
        let i = parse_intent("fix the login issue", "proj1");
        assert!(!i.extracted_tags.contains(&"the".to_string()));
        assert!(i.extracted_tags.contains(&"fix".to_string()));
        assert!(i.extracted_tags.contains(&"login".to_string()));
    }

    #[test]
    fn test_normalize_strips_punctuation() {
        let n = normalize_task("fix bug: auth.token[refresh]!!");
        assert!(!n.contains('.'));
        assert!(!n.contains('['));
        assert!(!n.contains('!'));
    }

    #[test]
    fn test_normalize_preserves_colon_slash() {
        let n = normalize_task("bug: src/auth.rs line 42");
        assert!(n.contains(':'));
        assert!(n.contains('/'));
    }
}
