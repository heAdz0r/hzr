use std::collections::HashMap;

use hzr_protocol::{
    CandidateDecision, CandidateSource, ContextCandidate, ContextPack, TokenCount, TokenCountSource,
};

pub struct FusionInput {
    pub source_weight: f32,
    pub candidates: Vec<ContextCandidate>,
}

/// Share of the hard budget durable memory may occupy.
///
/// Memory and code used to compete for one pool. Because a memory body is prose it is
/// routinely an order of magnitude longer than a code candidate, so a single stale fact could
/// consume the plan — observed live at 10.9k of 12k tokens with the answering file not
/// selected at all. Memory is context for the code, so it gets a minority share of the budget.
const MEMORY_BUDGET_SHARE: f32 = 0.35;

/// Multiplier for a code candidate that carries no locatable span.
///
/// A candidate with no line span or symbol could not be summarised at all: a PNG, a lockfile,
/// a binary. It cannot answer a question about code, so it must not outrank evidence that can.
/// It is demoted rather than dropped, because an unsummarisable file is still occasionally the
/// right answer and the planner's job is to rank, not to censor.
const UNLOCATABLE_PENALTY: f32 = 0.4;

pub struct BudgetPlanner {
    hard_limit: u64,
    max_per_path: usize,
    min_relevance: f32,
}

impl BudgetPlanner {
    pub fn new(hard_limit: u64) -> Self {
        Self {
            hard_limit,
            max_per_path: 2,
            min_relevance: 0.05,
        }
    }

    pub fn with_max_per_path(mut self, max_per_path: usize) -> Self {
        self.max_per_path = max_per_path.max(1);
        self
    }

    pub fn with_min_relevance(mut self, min_relevance: f32) -> Self {
        self.min_relevance = min_relevance.clamp(0.0, 1.0);
        self
    }

    pub fn plan(&self, sources: Vec<FusionInput>) -> ContextPack {
        let mut fused = HashMap::<String, (ContextCandidate, f32)>::new();
        for source in sources {
            // Engine scores are only comparable *within* one source — fork rgai returns
            // similarities around 0.1–0.7, the literal engine returns unbounded BM25-like
            // values in the tens, and ICM returns FTS ranks near 0.01. Dividing by the best
            // score in the source puts them on one scale while preserving the spread between
            // them. That spread is the whole signal, and rank-only RRF threw it away: with
            // k=60 over ten candidates every score landed between 1/61 and 1/70, so a live
            // plan reported 0.0123–0.0164 for its best and worst evidence alike.
            let best = source
                .candidates
                .iter()
                .map(|candidate| candidate.relevance)
                .filter(|score| score.is_finite() && *score > 0.0)
                .fold(0.0_f32, f32::max);
            for candidate in source.candidates {
                let normalized = if best > 0.0 {
                    (candidate.relevance.max(0.0) / best).clamp(0.0, 1.0)
                } else {
                    // No usable engine score anywhere in this source: fall back to rank so the
                    // order stays deterministic instead of collapsing to a tie.
                    1.0 / candidate.source_rank.max(1) as f32
                };
                let contribution = source.source_weight * normalized;
                fused
                    .entry(candidate.content_ref.clone())
                    .and_modify(|(_, score)| *score += contribution)
                    .or_insert((candidate, contribution));
            }
        }

        let mut candidates: Vec<_> = fused
            .into_values()
            .map(|(mut candidate, score)| {
                candidate.relevance = score;
                candidate
            })
            .collect();
        candidates.sort_by(|left, right| {
            utility(right)
                .total_cmp(&utility(left))
                .then_with(|| left.content_ref.cmp(&right.content_ref))
        });

        let mut selected = Vec::new();
        let mut rejected = Vec::new();
        let mut used = 0;
        let mut memory_used = 0_u64;
        let memory_limit = (self.hard_limit as f32 * MEMORY_BUDGET_SHARE) as u64;
        let mut paths = HashMap::<String, usize>::new();

        for candidate in candidates {
            if candidate.relevance < self.min_relevance {
                rejected.push(CandidateDecision {
                    candidate_id: candidate.id,
                    reason: "relevance_floor".into(),
                });
                continue;
            }
            let path = candidate.path.clone().unwrap_or_default();
            let path_count = paths.get(&path).copied().unwrap_or_default();
            if !path.is_empty() && path_count >= self.max_per_path {
                rejected.push(CandidateDecision {
                    candidate_id: candidate.id,
                    reason: "path_diversity_limit".into(),
                });
                continue;
            }

            // Memory is capped separately so prose cannot crowd out the code it describes.
            if candidate.source == CandidateSource::Memory
                && memory_used + candidate.tokens.value > memory_limit
            {
                rejected.push(CandidateDecision {
                    candidate_id: candidate.id,
                    reason: "memory_budget_share".into(),
                });
                continue;
            }

            if used + candidate.tokens.value > self.hard_limit {
                rejected.push(CandidateDecision {
                    candidate_id: candidate.id,
                    reason: "hard_token_budget".into(),
                });
                continue;
            }

            used += candidate.tokens.value;
            if candidate.source == CandidateSource::Memory {
                memory_used += candidate.tokens.value;
            }
            if !path.is_empty() {
                paths.insert(path, path_count + 1);
            }
            selected.push(candidate);
        }

        let source = aggregate_token_source(&selected);
        let confidence = confidence(&selected);
        let budget_exceeded = rejected
            .iter()
            .any(|decision| decision.reason == "hard_token_budget");

        ContextPack {
            coverage: coverage(&selected),
            confidence,
            selected,
            rejected,
            used: TokenCount {
                value: used,
                source,
            },
            hard_limit: self.hard_limit,
            budget_exceeded,
        }
    }
}

fn aggregate_token_source(candidates: &[ContextCandidate]) -> TokenCountSource {
    let Some(first) = candidates.first() else {
        return TokenCountSource::Estimate;
    };
    if candidates
        .iter()
        .any(|candidate| candidate.tokens.source == TokenCountSource::Estimate)
    {
        return TokenCountSource::Estimate;
    }
    if candidates
        .iter()
        .all(|candidate| candidate.tokens.source == first.tokens.source)
    {
        first.tokens.source
    } else {
        TokenCountSource::ModelTokenizer
    }
}

/// Whether a candidate points at a place an agent can open.
///
/// Memory is exempt: a durable fact legitimately has no line span, and penalising it for that
/// would demote the whole source rather than the unsummarisable files this is aimed at.
fn is_locatable(candidate: &ContextCandidate) -> bool {
    if candidate.source == CandidateSource::Memory {
        return true;
    }
    candidate.symbol.is_some() || candidate.line_start.is_some() || candidate.line_end.is_some()
}

/// Ordering score.
///
/// Deliberately free of any token term. Dividing by `sqrt(tokens)` turned this into a brevity
/// prize: with `relevance` pinned in a 15% band by rank-only fusion, the expression degenerated
/// into `boost / sqrt(tokens)`, so a 30-token `Cargo.toml` beat a 3000-token file that answered
/// the question by an order of magnitude. Length is a budget constraint, enforced when filling
/// the budget — it is not evidence of relevance.
fn utility(candidate: &ContextCandidate) -> f32 {
    let source_boost = match candidate.source {
        CandidateSource::Exact => 1.5,
        CandidateSource::Context => 1.2,
        CandidateSource::Index => 1.0,
        CandidateSource::Memory => 0.9,
    };
    let locatable = if is_locatable(candidate) {
        1.0
    } else {
        UNLOCATABLE_PENALTY
    };
    source_boost * candidate.relevance * locatable
}

/// Share of the selected evidence an agent can actually open.
///
/// This used to be "how many of the four source kinds appeared", which is a property of the
/// retrieval wiring rather than of the answer: a plan that missed the answering file entirely
/// still reported 0.50 because two kinds were present. What a caller needs to know is whether
/// the evidence is addressable.
fn coverage(candidates: &[ContextCandidate]) -> f32 {
    if candidates.is_empty() {
        return 0.0;
    }
    let locatable = candidates
        .iter()
        .filter(|candidate| is_locatable(candidate))
        .count() as f32;
    locatable / candidates.len() as f32
}

/// How clearly the top candidate stands out from the field.
///
/// This used to be `(exact + n*0.5)/n`, which is *exactly* 0.5 whenever no exact-mode candidate
/// is present — a constant printed beside real measurements. A plan cannot know in absolute
/// terms whether its best hit is correct, but it can report whether one candidate separated
/// itself from the rest, which is what distinguishes a pinpointed answer from a flat list of
/// guesses. A single candidate has no field to stand out from, so it reports its locatability.
fn confidence(candidates: &[ContextCandidate]) -> f32 {
    let Some(top) = candidates.first() else {
        return 0.0;
    };
    let rest = &candidates[1..];
    if rest.is_empty() {
        return if is_locatable(top) { 1.0 } else { 0.0 };
    }
    let best = utility(top);
    if best <= 0.0 {
        return 0.0;
    }
    let mean_rest = rest.iter().map(utility).sum::<f32>() / rest.len() as f32;
    ((best - mean_rest) / best).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use hzr_protocol::{
        CandidateSource, ContextCandidate, Provenance, TokenCount, TokenCountSource,
    };

    use super::{BudgetPlanner, FusionInput, aggregate_token_source};

    fn candidate(id: &str, path: &str, tokens: u64, rank: u32) -> ContextCandidate {
        ContextCandidate {
            id: id.into(),
            source: CandidateSource::Index,
            content_ref: id.into(),
            path: Some(path.into()),
            symbol: None,
            line_start: None,
            line_end: None,
            source_rank: rank,
            relevance: 0.8,
            tokens: TokenCount::estimate(tokens),
            freshness: "fresh".into(),
            trust: "workspace".into(),
            provenance: Provenance {
                source: "test".into(),
                content_hash: id.into(),
                generation: None,
                canonical_ref: None,
                derived_by: None,
            },
        }
    }

    fn scored(id: &str, path: &str, tokens: u64, rank: u32, score: f32) -> ContextCandidate {
        let mut candidate = candidate(id, path, tokens, rank);
        candidate.relevance = score;
        candidate.line_start = Some(1);
        candidate.line_end = Some(10);
        candidate
    }

    /// The reported `relevance` was rank-only RRF with k=60. Over lists of ten that pins every
    /// candidate between 1/61 and 1/70, so a live plan reported 0.0123–0.0164 for everything
    /// while discarding each engine's own magnitude. An agent — and any threshold — then sees
    /// the best and worst evidence as equally weak.
    #[test]
    fn test_relevance_preserves_engine_magnitude_instead_of_collapsing_to_rrf() {
        let planner = BudgetPlanner::new(10_000);
        let pack = planner.plan(vec![FusionInput {
            source_weight: 1.0,
            candidates: vec![
                scored("strong", "hit.rs", 100, 1, 0.90),
                scored("weak", "miss.rs", 100, 2, 0.05),
            ],
        }]);

        let strong = pack.selected[0].relevance;
        let weak = pack.selected[1].relevance;
        assert!(
            strong >= weak * 4.0,
            "a 18x engine-score gap must survive fusion, got {strong} vs {weak}"
        );
        assert!(
            strong > 0.5,
            "the top candidate must not be reported as weak evidence, got {strong}"
        );
    }

    /// With `relevance` nearly constant, `utility = boost * relevance / sqrt(tokens)` degenerated
    /// into a brevity prize: a 200-token file beat a 5000-token one 6.5x on length alone. That is
    /// why `Cargo.toml` and a PNG outranked the file that answered the question. Length is a
    /// budget constraint, not evidence of relevance.
    #[test]
    fn test_selection_does_not_reward_brevity_over_relevance() {
        let planner = BudgetPlanner::new(10_000);
        let pack = planner.plan(vec![FusionInput {
            source_weight: 1.0,
            candidates: vec![
                scored("tiny_irrelevant", "Cargo.toml", 30, 2, 0.05),
                scored("long_relevant", "answer.rs", 3_000, 1, 0.95),
            ],
        }]);

        assert_eq!(
            pack.selected[0].path.as_deref(),
            Some("answer.rs"),
            "the relevant candidate must win regardless of being longer"
        );
    }

    /// A candidate with no locatable span could not be summarised at all — a PNG, a lockfile, a
    /// binary. It cannot answer a question about code, so it must not outrank evidence that can.
    #[test]
    fn test_a_candidate_without_a_locatable_span_is_demoted() {
        let mut opaque = scored("opaque", "logo.png", 40, 1, 0.60);
        opaque.line_start = None;
        opaque.line_end = None;
        let locatable = scored("locatable", "answer.rs", 40, 2, 0.55);

        let planner = BudgetPlanner::new(10_000);
        let pack = planner.plan(vec![FusionInput {
            source_weight: 1.0,
            candidates: vec![opaque, locatable],
        }]);

        assert_eq!(
            pack.selected[0].path.as_deref(),
            Some("answer.rs"),
            "an unlocatable candidate must not outrank locatable evidence at a similar score"
        );
    }

    /// Memory competed with code for one budget, so a single stale multi-kilobyte fact could
    /// consume the plan. Observed live: 10.9k of 12k tokens spent, and the file that answered
    /// the question was not selected at all.
    #[test]
    fn test_memory_cannot_consume_the_whole_budget() {
        let mut huge = scored("stale", "", 900, 1, 0.99);
        huge.source = CandidateSource::Memory;
        huge.path = None;
        let mut second = scored("stale2", "", 900, 2, 0.98);
        second.source = CandidateSource::Memory;
        second.path = None;
        let code = scored("code", "answer.rs", 400, 1, 0.40);

        let planner = BudgetPlanner::new(1_000);
        let pack = planner.plan(vec![FusionInput {
            source_weight: 1.0,
            candidates: vec![huge, second, code],
        }]);

        assert!(
            pack.selected
                .iter()
                .any(|candidate| candidate.path.as_deref() == Some("answer.rs")),
            "code must still fit after memory takes its share"
        );
        let memory_tokens: u64 = pack
            .selected
            .iter()
            .filter(|candidate| candidate.source == CandidateSource::Memory)
            .map(|candidate| candidate.tokens.value)
            .sum();
        assert!(
            memory_tokens < 1_000,
            "memory must not spend the entire budget, spent {memory_tokens}"
        );
    }

    /// `confidence` was `(exact + n*0.5)/n`, which is exactly 0.5 whenever no exact-mode
    /// candidate is present, and `coverage` was source-kinds-present/4. Both were reported next
    /// to real measurements while being structural constants — a live plan showed 0.50/0.50 for
    /// a result that had in fact missed the answer.
    #[test]
    fn test_confidence_and_coverage_are_measurements_not_constants() {
        let planner = BudgetPlanner::new(10_000);

        // A plan cannot know in absolute terms whether its best hit is correct, but it can
        // report whether one candidate separated itself from the field. That is what tells a
        // pinpointed answer apart from a flat list of equally weak guesses — which is exactly
        // what the live plan produced while reporting a confident-looking 0.50.
        let pinpointed = planner.plan(vec![FusionInput {
            source_weight: 1.0,
            candidates: vec![
                scored("a", "a.rs", 50, 1, 0.95),
                scored("a2", "a2.rs", 50, 2, 0.06),
            ],
        }]);
        let flat = planner.plan(vec![FusionInput {
            source_weight: 1.0,
            candidates: vec![
                scored("b", "b.rs", 50, 1, 0.20),
                scored("b2", "b2.rs", 50, 2, 0.19),
            ],
        }]);

        assert!(
            pinpointed.confidence > flat.confidence + 0.5,
            "confidence must follow separation, got {} vs {}",
            pinpointed.confidence,
            flat.confidence
        );

        let strong = pinpointed;

        let mut opaque = scored("c", "c.png", 50, 1, 0.95);
        opaque.line_start = None;
        opaque.line_end = None;
        let unlocatable = planner.plan(vec![FusionInput {
            source_weight: 1.0,
            candidates: vec![opaque],
        }]);
        assert!(
            strong.coverage > unlocatable.coverage,
            "coverage must reflect whether evidence is locatable, got {} vs {}",
            strong.coverage,
            unlocatable.coverage
        );
    }

    #[test]
    fn test_plan_never_exceeds_hard_budget() {
        let planner = BudgetPlanner::new(100);
        let pack = planner.plan(vec![FusionInput {
            source_weight: 1.0,
            candidates: vec![candidate("a", "a.rs", 70, 1), candidate("b", "b.rs", 60, 2)],
        }]);

        assert!(pack.used.value <= 100);
        assert_eq!(pack.selected.len(), 1);
        assert_eq!(pack.used.source, TokenCountSource::Estimate);
    }

    #[test]
    fn test_plan_deduplicates_same_content_reference() {
        let same = candidate("same", "a.rs", 20, 1);
        let planner = BudgetPlanner::new(100);
        let pack = planner.plan(vec![
            FusionInput {
                source_weight: 1.0,
                candidates: vec![same.clone()],
            },
            FusionInput {
                source_weight: 0.8,
                candidates: vec![same],
            },
        ]);

        assert_eq!(pack.selected.len(), 1);
    }

    #[test]
    fn test_token_source_preserves_provider_only_counts() {
        let mut provider = candidate("provider", "a.rs", 20, 1);
        provider.tokens = TokenCount::provider(20);

        assert_eq!(
            aggregate_token_source(&[provider]),
            TokenCountSource::Provider
        );
    }

    #[test]
    fn test_plan_preserves_calibrated_relevance_over_adjacent_rank() {
        let mut weak = candidate("weak", "weak.rs", 20, 1);
        weak.relevance = 0.05;
        let mut strong = candidate("strong", "strong.rs", 20, 2);
        strong.relevance = 0.9;

        let pack = BudgetPlanner::new(100).plan(vec![FusionInput {
            source_weight: 1.0,
            candidates: vec![weak, strong],
        }]);

        assert_eq!(pack.selected[0].id, "strong");
        assert!(pack.selected[0].relevance > pack.selected[1].relevance);
    }

    #[test]
    fn test_plan_rejects_candidates_below_relevance_floor() {
        let mut noise = candidate("noise", "noise.rs", 20, 1);
        noise.relevance = 0.01;
        let signal = candidate("signal", "signal.rs", 20, 2);

        let pack = BudgetPlanner::new(100)
            .with_min_relevance(0.1)
            .plan(vec![FusionInput {
                source_weight: 1.0,
                candidates: vec![noise, signal],
            }]);

        assert_eq!(pack.selected.len(), 1);
        assert_eq!(pack.selected[0].id, "signal");
        assert!(
            pack.rejected
                .iter()
                .any(|decision| decision.reason == "relevance_floor")
        );
    }
}
