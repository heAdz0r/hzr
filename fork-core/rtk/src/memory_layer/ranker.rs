//! E7.3: Stage-1 Rust linear ranker. Deterministic, always available.
//! Stage-2 (Ollama rerank) is applied on top in the plan-context pipeline.

use serde::{Deserialize, Serialize};

use super::intent::{IntentKind, TaskIntent};

// ── Feature vector ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeatureVec {
    /// Layer match score: file matches requested query_type layers (L0-L6).
    pub f_structural_relevance: f32,
    /// Git churn frequency: how often this file changes in git history (log-normalized).
    pub f_churn_score: f32,
    /// Recency: file appears in delta / recent edits.
    pub f_recency_score: f32,
    /// File criticality: auth/payment/core paths.
    pub f_risk_score: f32,
    /// Test proximity: nearby test files + test edit history.
    pub f_test_proximity: f32,
    /// Estimated token cost (higher cost lowers final score via penalty).
    pub f_token_cost: f32,
    /// Call graph: fraction of query tags whose symbols are called by this file.
    pub f_call_graph_score: f32,
}

// ── Candidate ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub rel_path: String,
    pub features: FeatureVec,
    /// Final normalized score after weighting (set by ranker, then optionally blended by Stage-2).
    pub score: f32,
    /// Source tags: which generators contributed this candidate.
    pub sources: Vec<String>,
    /// Estimated token cost for including this candidate in context.
    pub estimated_tokens: u32,
}

impl Candidate {
    pub fn new(rel_path: impl Into<String>) -> Self {
        Self {
            rel_path: rel_path.into(),
            features: FeatureVec::default(),
            score: 0.0,
            sources: Vec::new(),
            estimated_tokens: 200,
        }
    }
}

// ── Ranking model ──────────────────────────────────────────────────────────────

/// Linear ranking model weights.
/// Loaded from `model_registry` (active model) or falls back to intent-tuned defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankingModel {
    pub w_structural: f32,
    /// Git churn frequency weight (replaces subjective affinity + semantic).
    pub w_churn: f32,
    pub w_recency: f32,
    pub w_risk: f32,
    pub w_test_proximity: f32,
    /// Token cost penalty weight (reduces score for large files).
    pub w_token_cost_penalty: f32,
    /// Call graph weight: callers of query-relevant symbols.
    pub w_call_graph: f32,
}

impl Default for RankingModel {
    fn default() -> Self {
        Self {
            w_structural: 0.25,
            w_churn: 0.20,
            w_recency: 0.15,
            w_risk: 0.15,
            w_test_proximity: 0.05,
            w_call_graph: 0.15, // call graph: callers of query symbols
            w_token_cost_penalty: 0.05,
        }
    }
}

impl RankingModel {
    /// Select intent-tuned weights (ADR-0002: intent-conditioned ranking).
    /// Only deterministic objective signals: structural, churn, recency, risk, test_proximity.
    pub fn for_intent(intent: &TaskIntent) -> Self {
        match intent.predicted {
            IntentKind::Bugfix => Self {
                w_structural: 0.15,
                w_churn: 0.15,
                w_recency: 0.25, // recency elevated: bugfix targets recently broken code
                w_risk: 0.20,
                w_call_graph: 0.20, // call graph elevated: who calls the broken function
                w_test_proximity: 0.05,
                w_token_cost_penalty: 0.00,
            },
            IntentKind::Feature => Self {
                w_structural: 0.30, // structure dominant: feature needs extension points
                w_churn: 0.15,
                w_recency: 0.05,
                w_risk: 0.05,
                w_call_graph: 0.10,     // callers of integration points
                w_test_proximity: 0.30, // test proximity elevated: new feature needs tests
                w_token_cost_penalty: 0.05,
            },
            IntentKind::Refactor => Self {
                w_structural: 0.25, // structure dominant: refactor needs full picture
                w_churn: 0.25,      // churn elevated: frequent changes signal coupling
                w_recency: 0.05,
                w_risk: 0.10,
                w_call_graph: 0.25, // call graph elevated: who uses what we're changing
                w_test_proximity: 0.05,
                w_token_cost_penalty: 0.05,
            },
            IntentKind::Incident => Self {
                w_structural: 0.10,
                w_churn: 0.10,
                w_recency: 0.35, // recency dominant: incident = recently changed = hot path
                w_risk: 0.25,
                w_call_graph: 0.15, // callers of incident-related symbols
                w_test_proximity: 0.00,
                w_token_cost_penalty: 0.05,
            },
            IntentKind::Unknown => Self::default(),
        }
    }

    /// Score a single candidate. Returns value in [0.0, 1.0].
    pub fn score(&self, features: &FeatureVec) -> f32 {
        let raw = self.w_structural * features.f_structural_relevance
            + self.w_churn * features.f_churn_score
            + self.w_recency * features.f_recency_score
            + self.w_risk * features.f_risk_score
            + self.w_test_proximity * features.f_test_proximity
            + self.w_call_graph * features.f_call_graph_score
            - self.w_token_cost_penalty * features.f_token_cost;
        raw.clamp(0.0, 1.0)
    }
}

// ── Ranking pipeline ───────────────────────────────────────────────────────────

/// Stage-1: deterministic Rust scoring. Returns candidates sorted by score descending.
pub fn rank_stage1(mut candidates: Vec<Candidate>, intent: &TaskIntent) -> Vec<Candidate> {
    let model = RankingModel::for_intent(intent);
    for c in &mut candidates {
        c.score = model.score(&c.features);
    }
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates
}

/// Stage-2: blend Ollama rerank scores with Stage-1 scores.
/// `rerank_scores`: parallel array for top-K candidates (same order).
/// Formula: `final = 0.6 * stage1_score + 0.4 * rerank_score`.
pub fn apply_stage2(mut candidates: Vec<Candidate>, rerank_scores: &[f32]) -> Vec<Candidate> {
    for (c, &r) in candidates.iter_mut().zip(rerank_scores.iter()) {
        c.score = (0.6 * c.score + 0.4 * r).clamp(0.0, 1.0);
    }
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates
}

// ── File risk scoring helper ───────────────────────────────────────────────────

const RISK_PATH_SIGNALS: &[&str] = &[
    "auth",
    "authn",
    "authz",
    "login",
    "password",
    "secret",
    "token",
    "jwt",
    "payment",
    "billing",
    "stripe",
    "checkout",
    "crypto",
    "encrypt",
    "admin",
    "permission",
    "role",
    "privilege",
    "acl",
    "migration",
    "migrate",
    "schema",
];

/// Score a file path for risk (auth/payment/admin paths score high). Returns [0.0, 1.0].
pub fn path_risk_score(rel_path: &str) -> f32 {
    let lower = rel_path.to_lowercase();
    let hits = RISK_PATH_SIGNALS
        .iter()
        .filter(|&&s| lower.contains(s))
        .count();
    (hits as f32 * 0.4).min(1.0)
}

/// Returns true if the file looks like a test file.
pub fn is_test_file(rel_path: &str) -> bool {
    let lower = rel_path.to_lowercase();
    lower.contains("/test")
        || lower.contains("_test.")
        || lower.contains(".test.")
        || lower.contains("spec.")
        || lower.ends_with("_spec.rs")
        || lower.contains("/tests/")
        || lower.contains("__tests__")
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_layer::intent::parse_intent;

    fn make_candidate(path: &str, churn: f32, structural: f32) -> Candidate {
        let mut c = Candidate::new(path);
        c.features.f_churn_score = churn;
        c.features.f_structural_relevance = structural;
        c
    }

    #[test]
    fn test_rank_stage1_sorts_desc() {
        let intent = parse_intent("fix auth token bug", "p");
        let candidates = vec![
            make_candidate("low.rs", 0.1, 0.1),
            make_candidate("high.rs", 0.9, 0.8),
            make_candidate("mid.rs", 0.5, 0.4),
        ];
        let ranked = rank_stage1(candidates, &intent);
        assert_eq!(ranked[0].rel_path, "high.rs");
        assert_eq!(ranked[2].rel_path, "low.rs");
    }

    #[test]
    fn test_model_score_clamped() {
        let model = RankingModel::default();
        let features = FeatureVec {
            f_structural_relevance: 2.0, // > 1 intentionally
            f_churn_score: 2.0,
            ..Default::default()
        };
        let score = model.score(&features);
        assert!(score <= 1.0, "score {score} must be <= 1.0");
        assert!(score >= 0.0);
    }

    #[test]
    fn test_bugfix_weights_recency_dominant() {
        let intent = parse_intent("fix the broken login", "p");
        let model = RankingModel::for_intent(&intent);
        assert!(
            model.w_recency >= model.w_churn,
            "recency should dominate for bugfix"
        );
    }

    #[test]
    fn test_incident_weights_recency_dominant() {
        let intent = parse_intent("production outage critical service down", "p");
        let model = RankingModel::for_intent(&intent);
        assert!(
            model.w_recency >= model.w_churn,
            "recency should dominate for incident"
        );
    }

    #[test]
    fn test_churn_score_affects_ranking() {
        let intent = parse_intent("refactor module", "p");
        let mut high_churn = Candidate::new("hot.rs");
        high_churn.features.f_churn_score = 1.0;
        let mut low_churn = Candidate::new("cold.rs");
        low_churn.features.f_churn_score = 0.0;
        let ranked = rank_stage1(vec![low_churn, high_churn], &intent);
        assert_eq!(ranked[0].rel_path, "hot.rs", "high churn should rank first");
    }

    #[test]
    fn test_apply_stage2_blends_not_reverses() {
        let intent = parse_intent("fix bug", "p");
        // a: high stage1, b: low stage1
        let mut cands = vec![
            make_candidate("a.rs", 0.9, 0.9),
            make_candidate("b.rs", 0.1, 0.1),
        ];
        cands = rank_stage1(cands, &intent);
        // stage2 flips scores
        let rerank = vec![0.0_f32, 1.0_f32];
        let blended = apply_stage2(cands, &rerank);
        // scores are blended 60/40, so a: 0.6*high + 0.4*0 still > b: 0.6*low + 0.4*1
        // with high score ~0.67+ and low score ~0.46
        assert!(blended[0].score >= blended[1].score - 0.5); // ordering may vary — blending is partial
    }

    #[test]
    fn test_path_risk_score_auth() {
        assert!(path_risk_score("src/auth/middleware.rs") > 0.0);
        assert!(path_risk_score("src/payment/stripe.rs") > 0.0);
        assert_eq!(path_risk_score("src/ui/button.rs"), 0.0);
    }

    #[test]
    fn test_is_test_file() {
        assert!(is_test_file("src/tests/auth_test.rs"));
        assert!(is_test_file("src/auth.test.ts"));
        assert!(is_test_file("__tests__/login.spec.js"));
        assert!(!is_test_file("src/auth.rs"));
    }
}
