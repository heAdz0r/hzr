use crate::error::{MemoryError, Result};
use crate::types::MemoryRecord;
use std::collections::HashSet;

pub const MAX_MEMORY_KIND_BYTES: usize = 64;
pub const PROJECT_TOKEN_BYTES: usize = 64;

/// Namespace segment marking a record as user-global rather than repository-scoped.
///
/// A fixed literal rather than a second SHA-256 token, so the two scopes can never
/// collide: a repository identity is 64 hex characters, and `global` is not hex, so no
/// project can ever masquerade as the global namespace or vice versa.
pub const GLOBAL_SCOPE_TOKEN: &str = "global";

/// Which memories a request may reach.
///
/// Global exists because preferences and architectural rules are properties of the
/// *user*, not of one repository — before this, a preference learned in one project was
/// invisible in every other. Cross-project isolation is unaffected: a record belonging
/// to another repository is never reachable from either scope.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MemoryNamespace {
    /// Only the current repository.
    Project,
    /// Only user-global records.
    Global,
    /// The current repository plus user-global records. Default for recall: an agent
    /// should see your standing preferences alongside this project's history.
    #[default]
    ProjectAndGlobal,
}

pub fn namespaced_topic(kind: &str, project: &str) -> Result<String> {
    validate_memory_kind(kind)?;
    validate_project_token(project)?;
    Ok(format!("{kind}-{project}"))
}

/// Topic for a user-global record: `<kind>-global`.
pub fn global_topic(kind: &str) -> Result<String> {
    validate_memory_kind(kind)?;
    Ok(format!("{kind}-{GLOBAL_SCOPE_TOKEN}"))
}

pub fn topic_is_global(topic: &str) -> bool {
    topic
        .strip_suffix(GLOBAL_SCOPE_TOKEN)
        .and_then(|prefix| prefix.strip_suffix('-'))
        .is_some_and(|kind| validate_memory_kind(kind).is_ok())
}

pub fn validate_memory_kind(kind: &str) -> Result<()> {
    let bytes = kind.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_MEMORY_KIND_BYTES {
        return Err(MemoryError::InvalidRequest(format!(
            "memory topic kind must contain between 1 and {MAX_MEMORY_KIND_BYTES} bytes"
        )));
    }
    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        || !bytes
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || !bytes
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || bytes.windows(2).any(|pair| pair == b"--")
    {
        return Err(MemoryError::InvalidRequest(
            "memory topic kind must use lowercase ASCII letters, digits, and single interior hyphens"
                .into(),
        ));
    }
    Ok(())
}

pub fn topic_belongs_to_project(topic: &str, project: &str) -> bool {
    if validate_project_token(project).is_err() {
        return false;
    }
    topic
        .strip_suffix(project)
        .and_then(|prefix| prefix.strip_suffix('-'))
        .is_some_and(|kind| kind != "legacy-import" && validate_memory_kind(kind).is_ok())
}

pub fn isolate_project_memories(
    records: Vec<MemoryRecord>,
    project: &str,
    exact_topic: Option<&str>,
    limit: usize,
) -> Vec<MemoryRecord> {
    isolate_memories(
        records,
        project,
        MemoryNamespace::Project,
        exact_topic,
        limit,
    )
}

/// Keep only records the requested namespace may reach.
///
/// The filter is positive, never subtractive: a record is kept because it provably
/// belongs to this repository or to the global namespace. Anything else — another
/// repository, or a bare un-namespaced topic written by a tool outside HZR — is dropped.
/// That is what keeps one physical database from leaking between projects.
pub fn isolate_memories(
    records: Vec<MemoryRecord>,
    project: &str,
    namespace: MemoryNamespace,
    exact_topic: Option<&str>,
    limit: usize,
) -> Vec<MemoryRecord> {
    records
        .into_iter()
        .filter(|record| {
            let reachable = match namespace {
                MemoryNamespace::Project => topic_belongs_to_project(&record.topic, project),
                MemoryNamespace::Global => topic_is_global(&record.topic),
                MemoryNamespace::ProjectAndGlobal => {
                    topic_belongs_to_project(&record.topic, project)
                        || topic_is_global(&record.topic)
                }
            };
            reachable && exact_topic.is_none_or(|topic| record.topic == topic)
        })
        .take(limit)
        .collect()
}

/// Merge independently ranked project and global recalls without allowing one
/// namespace's oversampling window to starve the other.
pub fn merge_memories(
    project: Vec<MemoryRecord>,
    global: Vec<MemoryRecord>,
    limit: usize,
) -> Vec<MemoryRecord> {
    let mut records = project;
    records.extend(global);
    records.sort_by(|left, right| {
        right
            .score
            .unwrap_or(f32::NEG_INFINITY)
            .total_cmp(&left.score.unwrap_or(f32::NEG_INFINITY))
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut seen = HashSet::new();
    records
        .into_iter()
        .filter(|record| seen.insert(record.id.clone()))
        .take(limit)
        .collect()
}

/// ICM filters after ranking, so bounded oversampling prevents foreign candidates
/// from consuming the caller's entire result window before HZR's exact filter.
#[must_use]
pub fn recall_candidate_limit(requested: usize) -> usize {
    requested.saturating_mul(10).min(100)
}

fn validate_project_token(project: &str) -> Result<()> {
    if project.len() != PROJECT_TOKEN_BYTES
        || !project
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(MemoryError::InvalidRequest(
            "memory project token must be a lowercase SHA-256 repository identity".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::types::{Importance, MemoryRecord, MemoryScope, MemorySource};

    use super::{
        GLOBAL_SCOPE_TOKEN, MemoryNamespace, global_topic, isolate_memories,
        isolate_project_memories, merge_memories, namespaced_topic, recall_candidate_limit,
        topic_belongs_to_project, topic_is_global,
    };

    const PROJECT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const PROJECT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn test_namespaced_topic_is_deterministic_and_segment_safe() {
        assert_eq!(
            namespaced_topic("architecture-decisions", PROJECT_A).expect("valid topic"),
            format!("architecture-decisions-{PROJECT_A}")
        );
    }

    #[test]
    fn test_namespaced_topic_rejects_lossy_or_ambiguous_kinds() {
        for kind in ["Architecture", "foo_bar", "foo--bar", "-foo", "foo-", ""] {
            assert!(
                namespaced_topic(kind, PROJECT_A).is_err(),
                "accepted {kind:?}"
            );
        }
    }

    #[test]
    fn test_topic_belongs_to_project_never_accepts_global_or_foreign_topics() {
        assert!(topic_belongs_to_project(
            &format!("context-{PROJECT_A}"),
            PROJECT_A
        ));
        assert!(!topic_belongs_to_project("preferences", PROJECT_A));
        assert!(!topic_belongs_to_project(
            &format!("context-{PROJECT_B}"),
            PROJECT_A
        ));
        assert!(!topic_belongs_to_project(
            &format!("context-{PROJECT_A}-suffix"),
            PROJECT_A
        ));
        assert!(
            !topic_belongs_to_project(&format!("legacy-import-{PROJECT_A}"), PROJECT_A),
            "unclassified legacy imports must stay quarantined from project recall"
        );
    }

    #[test]
    fn test_recall_candidate_limit_is_bounded() {
        assert_eq!(recall_candidate_limit(1), 10);
        assert_eq!(recall_candidate_limit(5), 50);
        assert_eq!(recall_candidate_limit(100), 100);
    }

    #[test]
    fn test_isolate_project_memories_removes_global_and_cross_repo_records() {
        let records = vec![
            record("preferences"),
            record(&format!("context-{PROJECT_B}")),
            record(&format!("context-{PROJECT_A}")),
            record(&format!("decision-{PROJECT_A}")),
        ];

        let scoped = isolate_project_memories(
            records,
            PROJECT_A,
            Some(&format!("context-{PROJECT_A}")),
            10,
        );

        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].topic, format!("context-{PROJECT_A}"));
    }

    #[test]
    fn test_global_and_project_namespaces_cannot_collide() {
        // A repository identity is 64 hex chars; `global` is not hex, so neither
        // namespace can ever be mistaken for the other.
        assert!(!topic_is_global(&format!("context-{PROJECT_A}")));
        assert!(!topic_belongs_to_project(
            &global_topic("preferences").expect("global topic"),
            PROJECT_A
        ));
        assert!(
            GLOBAL_SCOPE_TOKEN.len() != super::PROJECT_TOKEN_BYTES,
            "the global token must not be shaped like a project identity"
        );
    }

    #[test]
    fn test_global_topic_rejects_the_same_lossy_kinds_as_project_topics() {
        for kind in ["Preferences", "foo_bar", "foo--bar", "-foo", "foo-", ""] {
            assert!(global_topic(kind).is_err(), "accepted {kind:?}");
        }
    }

    #[test]
    fn test_project_and_global_recall_never_reaches_another_repository() {
        let records = vec![
            record("preferences"),                   // un-namespaced: not reachable
            record(&format!("context-{PROJECT_B}")), // foreign project
            record(&format!("context-{PROJECT_A}")), // this project
            record(&format!("preferences-{GLOBAL_SCOPE_TOKEN}")), // user-global
        ];

        let reachable = isolate_memories(
            records,
            PROJECT_A,
            MemoryNamespace::ProjectAndGlobal,
            None,
            10,
        );

        let topics: Vec<&str> = reachable.iter().map(|r| r.topic.as_str()).collect();
        assert_eq!(
            topics.len(),
            2,
            "only this project plus global are reachable"
        );
        assert!(topics.contains(&format!("context-{PROJECT_A}").as_str()));
        assert!(topics.contains(&format!("preferences-{GLOBAL_SCOPE_TOKEN}").as_str()));
        assert!(
            !topics.iter().any(|topic| topic.contains(PROJECT_B)),
            "cross-project isolation must survive the global scope"
        );
    }

    #[test]
    fn test_global_only_scope_excludes_this_project() {
        let records = vec![
            record(&format!("context-{PROJECT_A}")),
            record(&format!("preferences-{GLOBAL_SCOPE_TOKEN}")),
        ];
        let reachable = isolate_memories(records, PROJECT_A, MemoryNamespace::Global, None, 10);
        assert_eq!(reachable.len(), 1);
        assert!(topic_is_global(&reachable[0].topic));
    }

    #[test]
    fn test_project_only_scope_is_unchanged_by_the_new_namespace() {
        let records = vec![
            record(&format!("context-{PROJECT_A}")),
            record(&format!("preferences-{GLOBAL_SCOPE_TOKEN}")),
        ];
        let reachable = isolate_memories(records, PROJECT_A, MemoryNamespace::Project, None, 10);
        assert_eq!(
            reachable.len(),
            1,
            "project scope must not leak global records"
        );
        assert_eq!(reachable[0].topic, format!("context-{PROJECT_A}"));
    }

    #[test]
    fn test_merge_preserves_cross_namespace_ranking_and_deduplicates() {
        let mut project = record(&format!("context-{PROJECT_A}"));
        project.score = Some(0.8);
        let mut global = record(&format!("preferences-{GLOBAL_SCOPE_TOKEN}"));
        global.score = Some(0.9);
        let duplicate = global.clone();

        let merged = merge_memories(vec![project, duplicate], vec![global], 10);

        assert_eq!(merged.len(), 2);
        assert!(topic_is_global(&merged[0].topic));
        assert_eq!(merged[1].topic, format!("context-{PROJECT_A}"));
    }

    fn record(topic: &str) -> MemoryRecord {
        MemoryRecord {
            score: Some(1.0),
            id: topic.into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            last_accessed: "2026-01-01T00:00:00Z".into(),
            access_count: 0,
            weight: 1.0,
            topic: topic.into(),
            summary: "fixture".into(),
            raw_excerpt: None,
            keywords: Vec::new(),
            importance: Importance::Medium,
            source: MemorySource::Manual,
            related_ids: Vec::new(),
            scope: MemoryScope::Project,
        }
    }
}
