use std::path::Path;

use hzr_index::Workspace;
use hzr_memory::{MemoryRecord, MemoryScope, MemorySource};
use hzr_protocol::{
    CandidateSource, ContextCandidate, ContextWarning, ContextWarningCode, Provenance,
    SearchApiResponse, SearchStrategy, TokenCount,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{ContextError, Result};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ForkPlanCandidate {
    pub rel_path: String,
    pub score: f32,
    #[serde(default)]
    pub sources: Vec<String>,
    pub estimated_tokens: u32,
}

pub(crate) struct RetrievedCandidate {
    pub candidate: ContextCandidate,
    pub content: String,
}

pub(crate) struct NormalizedSource {
    pub candidates: Vec<RetrievedCandidate>,
    pub warnings: Vec<ContextWarning>,
}

pub(crate) fn normalize_plan(
    selected: Vec<ForkPlanCandidate>,
    workspace: &Workspace,
    generation: &str,
    pipeline_version: Option<&str>,
) -> Result<NormalizedSource> {
    let mut candidates = Vec::with_capacity(selected.len());
    for (index, candidate) in selected.into_iter().enumerate() {
        let path = workspace.normalize_result(Path::new(&candidate.rel_path))?;
        let path = path
            .to_str()
            .ok_or_else(|| ContextError::InvalidForkOutput {
                operation: "memory plan",
                detail: "candidate path is not valid UTF-8".into(),
            })?
            .to_owned();
        let content = serde_json::to_string(&serde_json::json!({
            "path": path,
            "score": candidate.score,
            "sources": candidate.sources,
            "estimated_tokens": candidate.estimated_tokens,
        }))
        .map_err(|error| ContextError::InvalidForkOutput {
            operation: "memory plan",
            detail: error.to_string(),
        })?;
        let (content_ref, content_hash) = content_identity(&content);
        let rank = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let evidence_tokens = estimate_tokens(&content).value;
        candidates.push(RetrievedCandidate {
            candidate: ContextCandidate {
                id: format!("fork-plan:{content_hash}"),
                source: CandidateSource::Context,
                content_ref,
                path: Some(path.clone()),
                symbol: None,
                line_start: None,
                line_end: None,
                source_rank: rank,
                relevance: finite_score(candidate.score),
                tokens: TokenCount::estimate(
                    evidence_tokens.max(u64::from(candidate.estimated_tokens)),
                ),
                freshness: generation.to_owned(),
                trust: "workspace:untrusted".into(),
                provenance: Provenance {
                    source: "fork-core/memory-plan".into(),
                    content_hash,
                    generation: Some(generation.to_owned()),
                    canonical_ref: Some(path),
                    derived_by: pipeline_version.map(str::to_owned),
                },
            },
            content,
        });
    }
    Ok(NormalizedSource {
        candidates,
        warnings: Vec::new(),
    })
}

pub(crate) fn normalize_search(response: SearchApiResponse, generation: &str) -> NormalizedSource {
    let source = match response.strategy {
        SearchStrategy::ForkRgaiAdaptive => CandidateSource::Index,
        SearchStrategy::ForkRgaiBuiltin => CandidateSource::Exact,
    };
    let source_name = match response.strategy {
        SearchStrategy::ForkRgaiAdaptive => "fork-core/rgai-adaptive",
        SearchStrategy::ForkRgaiBuiltin => "fork-core/rgai-builtin",
    };
    let mut candidates = Vec::with_capacity(response.hits.len());
    let mut warnings = Vec::new();
    for (index, hit) in response.hits.into_iter().enumerate() {
        let content = hit
            .snippets
            .iter()
            .flat_map(|snippet| &snippet.lines)
            .map(|line| format!("L{} {}", line.line, line.text))
            .collect::<Vec<_>>()
            .join("\n");
        if content.is_empty() {
            warnings.push(ContextWarning {
                code: ContextWarningCode::ContentUnavailable,
                message: format!("fork rgai result {} omitted snippet content", hit.path),
            });
            continue;
        }
        let (content_ref, content_hash) = content_identity(&content);
        let rank = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let line_start = hit
            .snippets
            .iter()
            .flat_map(|snippet| &snippet.lines)
            .map(|line| line.line)
            .min();
        let line_end = hit
            .snippets
            .iter()
            .flat_map(|snippet| &snippet.lines)
            .map(|line| line.line)
            .max();
        candidates.push(RetrievedCandidate {
            candidate: ContextCandidate {
                id: format!("fork-rgai:{content_hash}"),
                source,
                content_ref,
                path: Some(hit.path.clone()),
                symbol: None,
                line_start,
                line_end,
                source_rank: rank,
                relevance: finite_score(hit.score as f32),
                tokens: estimate_tokens(&content),
                freshness: generation.to_owned(),
                trust: "workspace:untrusted".into(),
                provenance: Provenance {
                    source: source_name.into(),
                    content_hash,
                    generation: Some(generation.to_owned()),
                    canonical_ref: Some(format!(
                        "{}#L{}-L{}",
                        hit.path,
                        line_start.unwrap_or_default(),
                        line_end.unwrap_or_default()
                    )),
                    derived_by: Some("rtk-rgai-0.44.1-fork.1".into()),
                },
            },
            content,
        });
    }
    NormalizedSource {
        candidates,
        warnings,
    }
}

pub(crate) fn normalize_memory(records: Vec<MemoryRecord>) -> NormalizedSource {
    let mut candidates = Vec::with_capacity(records.len());
    let mut warnings = Vec::new();
    for (index, record) in records.into_iter().enumerate() {
        if record.summary.is_empty() {
            warnings.push(ContextWarning {
                code: ContextWarningCode::ContentUnavailable,
                message: format!("ICM memory {} has no summary content", record.id),
            });
            continue;
        }
        let path = match &record.source {
            MemorySource::ClaudeCode { file_path, .. } => file_path.clone(),
            MemorySource::Conversation { .. } | MemorySource::Manual => None,
        };
        let trust = match record.scope {
            MemoryScope::User => "icm:user",
            MemoryScope::Project => "icm:project",
            MemoryScope::Org => "icm:org",
        };
        let content = record.summary;
        let (content_ref, content_hash) = content_identity(&content);
        let rank = u32::try_from(index + 1).unwrap_or(u32::MAX);
        candidates.push(RetrievedCandidate {
            candidate: ContextCandidate {
                id: format!("icm:{content_hash}"),
                source: CandidateSource::Memory,
                content_ref,
                path,
                symbol: None,
                line_start: None,
                line_end: None,
                source_rank: rank,
                relevance: record
                    .score
                    .filter(|score| score.is_finite())
                    .unwrap_or(record.weight),
                tokens: estimate_tokens(&content),
                freshness: record.updated_at.clone(),
                trust: trust.into(),
                provenance: Provenance {
                    source: "icm".into(),
                    content_hash,
                    generation: Some(format!("icm:{}", record.updated_at)),
                    canonical_ref: Some(format!("icm:memory:{}", record.id)),
                    derived_by: None,
                },
            },
            content,
        });
    }
    NormalizedSource {
        candidates,
        warnings,
    }
}

fn content_identity(content: &str) -> (String, String) {
    let content_hash = hex::encode(Sha256::digest(content.as_bytes()));
    (format!("sha256:{content_hash}"), content_hash)
}

fn estimate_tokens(content: &str) -> TokenCount {
    let bytes = u64::try_from(content.len()).unwrap_or(u64::MAX);
    TokenCount::estimate(bytes.div_ceil(4).max(1))
}

fn finite_score(score: f32) -> f32 {
    if score.is_finite() { score } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use hzr_memory::{Importance, MemoryRecord, MemoryScope, MemorySource};

    use super::{content_identity, normalize_memory};

    fn memory_record(summary: &str) -> MemoryRecord {
        MemoryRecord {
            score: Some(0.8),
            id: "memory-1".into(),
            created_at: "2026-07-01T00:00:00Z".into(),
            updated_at: "2026-07-02T00:00:00Z".into(),
            last_accessed: "2026-07-03T00:00:00Z".into(),
            access_count: 1,
            weight: 0.7,
            topic: "hzr:test".into(),
            summary: summary.into(),
            raw_excerpt: None,
            keywords: vec!["test".into()],
            importance: Importance::High,
            source: MemorySource::Manual,
            related_ids: Vec::new(),
            scope: MemoryScope::Project,
        }
    }

    #[test]
    fn test_equal_content_has_stable_reference() {
        let content = "same canonical content";
        let memory = normalize_memory(vec![memory_record(content)]);
        assert_eq!(
            memory.candidates[0].candidate.content_ref,
            content_identity(content).0
        );
    }

    #[test]
    fn test_missing_memory_content_becomes_warning() {
        let normalized = normalize_memory(vec![memory_record("")]);
        assert!(normalized.candidates.is_empty());
        assert_eq!(normalized.warnings.len(), 1);
    }
}
