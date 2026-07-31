use std::collections::HashMap;

use hzr_protocol::{
    CandidateDecision, CandidateSource, ContextCandidate, ContextPack, TokenCount, TokenCountSource,
};

pub struct FusionInput {
    pub source_weight: f32,
    pub candidates: Vec<ContextCandidate>,
}

pub struct BudgetPlanner {
    hard_limit: u64,
    max_per_path: usize,
}

impl BudgetPlanner {
    pub fn new(hard_limit: u64) -> Self {
        Self {
            hard_limit,
            max_per_path: 2,
        }
    }

    pub fn with_max_per_path(mut self, max_per_path: usize) -> Self {
        self.max_per_path = max_per_path.max(1);
        self
    }

    pub fn plan(&self, sources: Vec<FusionInput>) -> ContextPack {
        let mut fused = HashMap::<String, (ContextCandidate, f32)>::new();
        for source in sources {
            for candidate in source.candidates {
                let contribution =
                    source.source_weight / (60.0 + candidate.source_rank.max(1) as f32);
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
        let mut paths = HashMap::<String, usize>::new();

        for candidate in candidates {
            let path = candidate.path.clone().unwrap_or_default();
            let path_count = paths.get(&path).copied().unwrap_or_default();
            if !path.is_empty() && path_count >= self.max_per_path {
                rejected.push(CandidateDecision {
                    candidate_id: candidate.id,
                    reason: "path_diversity_limit".into(),
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

fn utility(candidate: &ContextCandidate) -> f32 {
    let source_boost = match candidate.source {
        CandidateSource::Exact => 1.5,
        CandidateSource::Context => 1.2,
        CandidateSource::Index => 1.0,
        CandidateSource::Memory => 0.9,
    };
    source_boost * candidate.relevance / (candidate.tokens.value.max(1) as f32).sqrt()
}

fn coverage(candidates: &[ContextCandidate]) -> f32 {
    let kinds = candidates.iter().fold([false; 4], |mut kinds, candidate| {
        let index = match candidate.source {
            CandidateSource::Exact => 0,
            CandidateSource::Index => 1,
            CandidateSource::Context => 2,
            CandidateSource::Memory => 3,
        };
        kinds[index] = true;
        kinds
    });
    kinds.into_iter().filter(|present| *present).count() as f32 / 4.0
}

fn confidence(candidates: &[ContextCandidate]) -> f32 {
    if candidates.is_empty() {
        return 0.0;
    }
    let exact = candidates
        .iter()
        .filter(|candidate| candidate.source == CandidateSource::Exact)
        .count() as f32;
    ((exact + candidates.len() as f32 * 0.5) / candidates.len() as f32).min(1.0)
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
            relevance: 0.0,
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
}
