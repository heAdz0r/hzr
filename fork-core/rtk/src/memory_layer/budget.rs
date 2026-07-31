//! E7.4: Budget-aware context assembler (ADR-0004).
//! Greedy knapsack: maximize sum(utility_i) s.t. sum(tokens_i) <= budget.

use serde::{Deserialize, Serialize};

use super::ranker::Candidate;

// ── Types ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DroppedCandidate {
    pub rel_path: String,
    pub reason: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetReport {
    pub token_budget: u32,
    pub estimated_used: u32,
    pub candidates_total: usize,
    pub candidates_selected: usize,
    /// Fraction of budget consumed: estimated_used / token_budget.
    pub efficiency_score: f32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AssemblyResult {
    pub selected: Vec<Candidate>,
    pub dropped: Vec<DroppedCandidate>,
    pub budget_report: BudgetReport,
    /// Human-readable trace of why each selected candidate was chosen.
    pub decision_trace: Vec<String>,
    /// PRD R4: pipeline version identifier (additive, old consumers ignore it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipeline_version: Option<String>, // ADDED: PRD additive field
    /// PRD R4: semantic backend used in this run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_backend_used: Option<String>, // ADDED: PRD additive field
    /// PRD R4: number of graph-first candidates built.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph_candidate_count: Option<usize>, // ADDED: PRD additive field
    /// PRD R4: number of candidates with semantic evidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_hit_count: Option<usize>, // ADDED: PRD additive field
}

// ── Token estimator ────────────────────────────────────────────────────────────

const BASE_TOKENS_PER_FILE: u32 = 40; // path + metadata overhead
const TOKENS_PER_CHAR: f32 = 0.28; // Calibrated on cl100k_base tokenizer across mixed Rust/TS/Python corpus (~3.57 chars/token avg)

/// Estimate token cost for a file. Uses actual line_count when available (T4.1).
/// Falls back to extension-based median estimates.
pub fn estimate_tokens_for_path(rel_path: &str, line_count: Option<u32>) -> u32 {
    // T4.1: line_count param
    let base = BASE_TOKENS_PER_FILE;
    let path_tokens = (rel_path.len() as f32 * TOKENS_PER_CHAR) as u32;
    let content_tokens = if let Some(lines) = line_count {
        // T4.1: use actual line count
        (lines as f32 * 14.0) as u32 // L4: ~55 chars/line median × 0.25 tok/char (cl100k_base); validated on 500-file RTK + T3 corpus
    } else {
        match rel_path.rsplit('.').next().unwrap_or("") {
            "rs" | "ts" | "tsx" | "java" | "go" | "cpp" | "c" => 350,
            "py" | "js" | "jsx" | "swift" | "kt" => 280,
            "md" | "toml" | "yaml" | "yml" => 150,
            "json" | "lock" => 120,
            _ => 200,
        }
    };
    base + path_tokens + content_tokens
}

/// Utility of a candidate: score / sqrt(normalized_token_cost).
/// CHANGED: use sqrt to reduce cost penalty — prevents cheap low-relevance files
/// from displacing expensive high-relevance source files in greedy selection.
fn utility(c: &Candidate) -> f32 {
    // CHANGED: score / sqrt(cost) instead of score / cost
    let cost_normalized = (c.estimated_tokens.max(1) as f32 / 100.0).max(0.1);
    c.score / cost_normalized.sqrt()
}

// ── Assembly ───────────────────────────────────────────────────────────────────

/// Greedy budget-aware assembly (ADR-0004 §7.5).
/// Candidates must already be ranked (score set by ranker). Selects greedily by
/// utility-per-token, respecting the hard `token_budget` cap.
pub fn assemble(candidates: Vec<Candidate>, token_budget: u32) -> AssemblyResult {
    let total = candidates.len();
    let mut selected: Vec<Candidate> = Vec::new();
    let mut dropped: Vec<DroppedCandidate> = Vec::new();
    let mut tokens_used: u32 = 0;
    let mut trace: Vec<String> = Vec::new();

    // Sort by utility descending (greedy)
    let mut ordered = candidates;
    ordered.sort_by(|a, b| {
        utility(b)
            .partial_cmp(&utility(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // CHANGED: collect over-budget candidates for min-1 guarantee
    let mut over_budget: Vec<Candidate> = Vec::new();

    for candidate in ordered {
        let cost = candidate.estimated_tokens;
        if tokens_used + cost <= token_budget {
            trace.push(format!(
                "{} selected (score={:.2}, est_tokens={}, utility={:.4}, sources=[{}])",
                candidate.rel_path,
                candidate.score,
                cost,
                utility(&candidate),
                candidate.sources.join(","),
            ));
            tokens_used += cost;
            selected.push(candidate);
        } else {
            dropped.push(DroppedCandidate {
                rel_path: candidate.rel_path.clone(),
                reason: format!(
                    "budget_exceeded (needs {}, available {})",
                    cost,
                    token_budget.saturating_sub(tokens_used),
                ),
                score: candidate.score,
            });
            over_budget.push(candidate);
        }
    }

    // CHANGED: min-1 guarantee — returning 0 files is worse than slightly exceeding budget.
    // When greedy produces nothing, force-select the highest-scoring candidate.
    if selected.is_empty() && !over_budget.is_empty() {
        over_budget.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let best = over_budget.remove(0);
        dropped.retain(|d| d.rel_path != best.rel_path);
        let capped_cost = best.estimated_tokens.min(token_budget.max(1));
        trace.push(format!(
            "{} FORCED min-1 (score={:.2}, est_tokens={}, capped_to={})",
            best.rel_path, best.score, best.estimated_tokens, capped_cost,
        ));
        tokens_used = capped_cost;
        selected.push(best);
    }

    let efficiency = if token_budget > 0 {
        tokens_used as f32 / token_budget as f32
    } else {
        0.0
    };

    AssemblyResult {
        budget_report: BudgetReport {
            token_budget,
            estimated_used: tokens_used,
            candidates_total: total,
            candidates_selected: selected.len(),
            efficiency_score: efficiency,
        },
        decision_trace: trace,
        selected,
        dropped,
        // ADDED: PRD additive fields — None by default (set by graph-first pipeline)
        pipeline_version: None,
        semantic_backend_used: None,
        graph_candidate_count: None,
        semantic_hit_count: None,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_layer::ranker::Candidate;

    fn make_scored(path: &str, score: f32, tokens: u32) -> Candidate {
        let mut c = Candidate::new(path);
        c.score = score;
        c.estimated_tokens = tokens;
        c
    }

    #[test]
    fn test_assemble_respects_budget() {
        let candidates = vec![
            make_scored("a.rs", 0.9, 500),
            make_scored("b.rs", 0.8, 500),
            make_scored("c.rs", 0.7, 500),
        ];
        let result = assemble(candidates, 900);
        assert!(result.budget_report.estimated_used <= 900);
        // Only 1 can fit (each is 500 tokens, budget 900)
        assert_eq!(result.budget_report.candidates_selected, 1);
        assert_eq!(result.dropped.len(), 2);
    }

    #[test]
    fn test_assemble_maximizes_utility_not_just_score() {
        // cheap high-utility should beat expensive higher-score
        let candidates = vec![
            make_scored("cheap.rs", 0.8, 100),     // utility = 0.8 / 1.0 = 0.80
            make_scored("expensive.rs", 0.9, 900), // utility = 0.9 / 9.0 = 0.10
        ];
        let result = assemble(candidates, 500);
        assert_eq!(result.selected[0].rel_path, "cheap.rs");
    }

    #[test]
    fn test_budget_report_efficiency() {
        let candidates = vec![make_scored("a.rs", 0.9, 500)];
        let result = assemble(candidates, 1000);
        assert!(result.budget_report.efficiency_score <= 1.0);
        assert_eq!(result.budget_report.estimated_used, 500);
        assert!((result.budget_report.efficiency_score - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_dropped_candidates_have_reason() {
        let candidates = vec![
            make_scored("big.rs", 0.9, 9000),
            make_scored("small.rs", 0.5, 50),
        ];
        let result = assemble(candidates, 100);
        assert!(!result.dropped.is_empty());
        assert!(result
            .dropped
            .iter()
            .any(|d| d.reason.contains("budget_exceeded")));
    }

    #[test]
    fn test_empty_candidates() {
        let result = assemble(vec![], 3000);
        assert_eq!(result.selected.len(), 0);
        assert_eq!(result.budget_report.estimated_used, 0);
        assert_eq!(result.budget_report.efficiency_score, 0.0);
    }

    #[test]
    fn test_decision_trace_populated() {
        let candidates = vec![make_scored("a.rs", 0.8, 100)];
        let result = assemble(candidates, 500);
        assert!(!result.decision_trace.is_empty());
        assert!(result.decision_trace[0].contains("a.rs"));
    }

    #[test]
    fn test_min1_guarantee_when_all_exceed_budget() {
        // All candidates exceed budget → min-1 forces the best one
        let candidates = vec![
            make_scored("big.rs", 0.9, 2000),
            make_scored("bigger.rs", 0.7, 3000),
        ];
        let result = assemble(candidates, 1800);
        assert_eq!(
            result.budget_report.candidates_selected, 1,
            "min-1 guarantee: must select at least 1 candidate"
        );
        assert_eq!(result.selected[0].rel_path, "big.rs");
        assert!(
            result.decision_trace[0].contains("FORCED min-1"),
            "trace should note forced selection"
        );
    }

    #[test]
    fn test_min1_not_triggered_when_greedy_succeeds() {
        let candidates = vec![
            make_scored("small.rs", 0.8, 100),
            make_scored("big.rs", 0.9, 5000),
        ];
        let result = assemble(candidates, 500);
        assert_eq!(result.budget_report.candidates_selected, 1);
        assert_eq!(result.selected[0].rel_path, "small.rs");
        // Should NOT have FORCED in trace
        assert!(
            !result.decision_trace[0].contains("FORCED"),
            "min-1 should not trigger when greedy found candidates"
        );
    }

    #[test]
    fn test_estimate_tokens_by_extension() {
        assert!(
            estimate_tokens_for_path("src/main.rs", None)
                > estimate_tokens_for_path("config.json", None)
        ); // T4.3: None as second arg
        assert!(
            estimate_tokens_for_path("README.md", None)
                < estimate_tokens_for_path("src/lib.rs", None)
        );
    }
}
