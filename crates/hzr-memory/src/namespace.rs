use crate::error::{MemoryError, Result};
use crate::types::MemoryRecord;

pub const MAX_MEMORY_KIND_BYTES: usize = 64;
pub const PROJECT_TOKEN_BYTES: usize = 64;

pub fn namespaced_topic(kind: &str, project: &str) -> Result<String> {
    validate_memory_kind(kind)?;
    validate_project_token(project)?;
    Ok(format!("{kind}-{project}"))
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
        .is_some_and(|kind| validate_memory_kind(kind).is_ok())
}

pub fn isolate_project_memories(
    records: Vec<MemoryRecord>,
    project: &str,
    exact_topic: Option<&str>,
    limit: usize,
) -> Vec<MemoryRecord> {
    records
        .into_iter()
        .filter(|record| {
            topic_belongs_to_project(&record.topic, project)
                && exact_topic.is_none_or(|topic| record.topic == topic)
        })
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
        isolate_project_memories, namespaced_topic, recall_candidate_limit,
        topic_belongs_to_project,
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
