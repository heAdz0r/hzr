use std::path::Path;

use hzr_index::Workspace;
use hzr_memory::{MemoryRecord, MemoryScope, MemorySource};
use hzr_protocol::{
    CandidateSource, ContextCandidate, ContextWarning, ContextWarningCode, Provenance,
    SearchApiResponse, SearchStrategy, SymbolUnavailableReason, TokenCount,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{ContextError, Result};

const MAX_MEMORY_CONTENT_BYTES: usize = 4 * 1024;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ForkPlanCandidate {
    pub rel_path: String,
    pub score: f32,
    #[serde(default)]
    pub sources: Vec<String>,
    pub estimated_tokens: u32,
}

/// One symbol from the fork's machine-readable symbol index (`rtk read <file> --symbols`).
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ForkSymbol {
    pub name: String,
    pub kind: String,
    pub span: ForkSymbolSpan,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ForkSymbolSpan {
    pub start_line: u32,
    pub end_line: u32,
}

/// The fork's symbol index for one file.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct ForkSymbolIndex {
    #[serde(default)]
    pub symbols: Vec<ForkSymbol>,
}

/// Symbols per candidate. A plan is a bounded set of leads, so its outline must be bounded
/// too: without a cap a single generated file could spend the whole plan budget on its own
/// symbol list, which is the cost the planner exists to avoid.
const MAX_CANDIDATE_SYMBOLS: usize = 24;

pub(crate) struct RetrievedCandidate {
    pub candidate: ContextCandidate,
    pub content: String,
}

pub(crate) struct NormalizedSource {
    pub candidates: Vec<RetrievedCandidate>,
    pub warnings: Vec<ContextWarning>,
}

/// Render one plan candidate's evidence.
///
/// A candidate used to carry only `{path, score, sources, estimated_tokens}` — nothing an
/// agent could not have got from `ls`, so it opened every file anyway. The symbol outline
/// with line spans is what turns a path into a lead it can act on directly, and it is what
/// the protocol's `symbol`/`line_start`/`line_end` fields were declared for.
fn normalize_plan_evidence(
    path: &str,
    score: f32,
    sources: &[String],
    estimated_tokens: u32,
    outline: &[ForkSymbol],
) -> String {
    let shown: Vec<serde_json::Value> = outline
        .iter()
        .take(MAX_CANDIDATE_SYMBOLS)
        .map(|symbol| {
            serde_json::json!({
                "symbol": symbol.name,
                "kind": symbol.kind,
                "line_start": symbol.span.start_line,
                "line_end": symbol.span.end_line,
            })
        })
        .collect();
    let omitted = outline.len().saturating_sub(shown.len());

    let mut evidence = serde_json::json!({
        "path": path,
        "score": score,
        "sources": sources,
        "estimated_tokens": estimated_tokens,
        "outline": shown,
    });
    if omitted > 0 {
        evidence["outline_omitted"] = serde_json::json!(omitted);
        evidence["outline_recovery"] = serde_json::json!(format!(
            "{omitted} further symbols omitted; run `hzr rtk -- read {path} --outline` for all of them"
        ));
    }
    // A plan is delivered as text, so a stable, compact encoding is what the model reads.
    evidence.to_string()
}

pub(crate) fn normalize_plan(
    selected: Vec<ForkPlanCandidate>,
    workspace: &Workspace,
    generation: &str,
    pipeline_version: Option<&str>,
    outlines: &std::collections::BTreeMap<String, ForkSymbolIndex>,
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
        let outline = outlines
            .get(&path)
            .map(|index| index.symbols.as_slice())
            .unwrap_or_default();
        let content = normalize_plan_evidence(
            &path,
            candidate.score,
            &candidate.sources,
            candidate.estimated_tokens,
            outline,
        );
        // A whole-file candidate spans the file, so report the range the outline covers rather
        // than inventing a single symbol for it; `symbol` stays unset because none is selected.
        let line_start = outline.iter().map(|symbol| symbol.span.start_line).min();
        let line_end = outline.iter().map(|symbol| symbol.span.end_line).max();
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
                symbol_unavailable_reason: Some(if outline.is_empty() {
                    SymbolUnavailableReason::OutlineUnavailable
                } else {
                    SymbolUnavailableReason::WholeFileCandidate
                }),
                line_start,
                line_end,
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

pub(crate) fn normalize_search(
    response: SearchApiResponse,
    generation: &str,
    outlines: &std::collections::BTreeMap<String, ForkSymbolIndex>,
) -> NormalizedSource {
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
        let outline = outlines.get(&hit.path);
        let symbol = line_start.zip(line_end).and_then(|(start, end)| {
            outline.and_then(|index| {
                index
                    .symbols
                    .iter()
                    .filter(|symbol| symbol.span.start_line <= start && symbol.span.end_line >= end)
                    .min_by_key(|symbol| symbol.span.end_line - symbol.span.start_line)
                    .map(|symbol| symbol.name.clone())
            })
        });
        let symbol_unavailable_reason = symbol.is_none().then_some(if outline.is_some() {
            SymbolUnavailableReason::NoEnclosingSymbol
        } else {
            SymbolUnavailableReason::OutlineUnavailable
        });
        candidates.push(RetrievedCandidate {
            candidate: ContextCandidate {
                id: format!("fork-rgai:{content_hash}"),
                source,
                content_ref,
                path: Some(hit.path.clone()),
                symbol,
                symbol_unavailable_reason,
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
        let content = bounded_memory_content(record.summary, &record.id);
        let (content_ref, content_hash) = content_identity(&content);
        let rank = u32::try_from(index + 1).unwrap_or(u32::MAX);
        candidates.push(RetrievedCandidate {
            candidate: ContextCandidate {
                id: format!("icm:{content_hash}"),
                source: CandidateSource::Memory,
                content_ref,
                path,
                symbol: None,
                symbol_unavailable_reason: Some(SymbolUnavailableReason::NotApplicable),
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

fn bounded_memory_content(content: String, memory_id: &str) -> String {
    if content.len() <= MAX_MEMORY_CONTENT_BYTES {
        return content;
    }
    let marker_budget = format!(
        "\n\n[memory content bounded; {} bytes omitted; fetch full memory id {memory_id} with `hzr memory show {memory_id}`]\n\n",
        content.len()
    );
    let available = MAX_MEMORY_CONTENT_BYTES.saturating_sub(marker_budget.len());
    let mut head_end = available / 2;
    while !content.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = content.len() - (available - head_end);
    while !content.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let marker = format!(
        "\n\n[memory content bounded; {} bytes omitted; fetch full memory id {memory_id} with `hzr memory show {memory_id}`]\n\n",
        tail_start.saturating_sub(head_end)
    );
    format!(
        "{}{}{}",
        &content[..head_end],
        marker,
        &content[tail_start..]
    )
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
    use hzr_protocol::{
        SearchApiResponse, SearchHit, SearchLine, SearchMode, SearchSnippet, SearchStrategy,
    };

    use super::{
        ForkSymbol, ForkSymbolIndex, ForkSymbolSpan, MAX_MEMORY_CONTENT_BYTES, content_identity,
        normalize_memory, normalize_plan_evidence, normalize_search,
    };

    fn symbol(name: &str, kind: &str, start_line: u32, end_line: u32) -> ForkSymbol {
        ForkSymbol {
            name: name.into(),
            kind: kind.into(),
            span: ForkSymbolSpan {
                start_line,
                end_line,
            },
        }
    }

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

    /// A plan candidate used to carry no code at all — only `{path, score, sources,
    /// estimated_tokens}`. An agent given that has learned nothing it could not have got from
    /// `ls`, so it opens every file anyway and the plan's whole budget is spent on prose from
    /// memory. Measured on the intent "how does the bash hook decide to rewrite a command":
    /// 11809/16000 tokens, and the file that actually answered it was not even selected.
    ///
    /// The protocol has declared `symbol`, `line_start` and `line_end` since the first
    /// release and the planner never filled them in.
    #[test]
    fn test_a_plan_candidate_carries_the_symbols_an_agent_can_jump_to() {
        let outline = vec![
            symbol("rewrite", "fn", 45, 79),
            symbol("steer_to_first_class", "fn", 88, 103),
        ];
        let normalized = normalize_plan_evidence(
            "crates/hzr-cli/src/hook_runner.rs",
            0.42,
            &["tier_a".into()],
            240,
            &outline,
        );

        assert!(
            normalized.contains("steer_to_first_class"),
            "the outline must reach the agent, got: {normalized}"
        );
        assert!(
            normalized.contains("45") && normalized.contains("79"),
            "a symbol without its line span cannot be read directly, got: {normalized}"
        );
    }

    /// The outline is evidence, not a dump: a large file must not spend the plan's budget on
    /// its own symbol list, and the agent must be told when it was cut.
    #[test]
    fn test_a_long_outline_is_bounded_and_says_so() {
        let outline: Vec<_> = (0..200)
            .map(|index| symbol(&format!("symbol_{index}"), "fn", index * 2, index * 2 + 1))
            .collect();

        let normalized =
            normalize_plan_evidence("src/huge.rs", 0.1, &["tier_a".into()], 9000, &outline);

        assert!(
            normalized.contains("symbol_0"),
            "the first symbols must survive truncation"
        );
        assert!(
            !normalized.contains("symbol_199"),
            "an unbounded outline would reintroduce the cost the planner exists to avoid"
        );
        assert!(
            normalized.contains("omitted"),
            "a truncated outline must say so, got: {normalized}"
        );
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

    #[test]
    fn test_long_memory_is_bounded_without_losing_its_latest_tail() {
        let summary = format!("{}LATEST_DECISION", "x".repeat(10_000));
        let normalized = normalize_memory(vec![memory_record(&summary)]);
        let content = &normalized.candidates[0].content;

        assert!(content.len() <= MAX_MEMORY_CONTENT_BYTES);
        assert!(content.contains("bytes omitted"));
        assert!(content.contains("hzr memory show memory-1"));
        assert!(content.ends_with("LATEST_DECISION"));
    }

    #[test]
    fn test_search_candidate_resolves_the_smallest_enclosing_symbol() {
        let response = SearchApiResponse {
            query: "record_operation".into(),
            path: "crates".into(),
            total_hits: 1,
            shown_hits: 1,
            scanned_files: 1,
            skipped_large: 0,
            skipped_binary: 0,
            hits: vec![SearchHit {
                path: "src/ledger.rs".into(),
                score: 1.0,
                matched_lines: 1,
                snippets: vec![SearchSnippet {
                    lines: vec![SearchLine {
                        line: 55,
                        text: "fn record_operation()".into(),
                    }],
                    matched_terms: vec!["record_operation".into()],
                }],
            }],
            effective_mode: SearchMode::Exact,
            strategy: SearchStrategy::ForkRgaiBuiltin,
            index_generation: Some("generation-1".into()),
            fallback_reason: None,
            next_step: None,
        };
        let outlines = std::collections::BTreeMap::from([(
            "src/ledger.rs".into(),
            ForkSymbolIndex {
                symbols: vec![
                    symbol("impl Ledger", "impl", 1, 100),
                    symbol("record_operation", "fn", 50, 60),
                ],
            },
        )]);

        let normalized = normalize_search(response, "generation-1", &outlines);
        let candidate = &normalized.candidates[0].candidate;

        assert_eq!(candidate.symbol.as_deref(), Some("record_operation"));
        assert!(candidate.symbol_unavailable_reason.is_none());
    }
}
