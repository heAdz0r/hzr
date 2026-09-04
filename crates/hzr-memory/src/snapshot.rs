use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use rusqlite::types::Type;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{MemoryError, Result, topic_belongs_to_project};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryContent {
    pub id: String,
    pub topic: String,
    pub updated_at: String,
    pub summary: String,
    pub raw_excerpt: Option<String>,
}

pub fn read_memory_by_id(
    path: &Path,
    project: &str,
    id: &str,
    global: bool,
) -> Result<Option<MemoryContent>> {
    if !path.is_file() {
        return Err(MemoryError::SnapshotUnavailable);
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_millis(250))?;
    let record = connection
        .query_row(
            "SELECT id, topic, updated_at, summary, raw_excerpt FROM memories WHERE id = ?1",
            [id],
            |row| {
                Ok(MemoryContent {
                    id: row.get(0)?,
                    topic: row.get(1)?,
                    updated_at: row.get(2)?,
                    summary: row.get(3)?,
                    raw_excerpt: row.get(4)?,
                })
            },
        )
        .optional()?;
    Ok(record.filter(|record| {
        if global {
            crate::topic_is_global(&record.topic)
        } else {
            topic_belongs_to_project(&record.topic, project)
        }
    }))
}

const MAX_MEMORIES: usize = 256;
const MAX_TOPICS: usize = 64;
const MAX_EDGES: usize = 256;
const MAX_TOPIC_MEMORIES: usize = 100;
const MAX_SUMMARY_CHARS: usize = 4_000;
const MAX_EXCERPT_CHARS: usize = 2_000;
const MAX_SOURCE_DATA_CHARS: usize = 1_000;
const MAX_KEYWORDS: usize = 24;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectMemorySnapshot {
    pub memory_count: usize,
    pub visible_memory_count: usize,
    pub hidden_memory_count: usize,
    pub topics: Vec<MemoryTopicSnapshot>,
    pub edges: Vec<MemoryTopicEdge>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryTopicSnapshot {
    pub id: String,
    pub label: String,
    pub memory_count: usize,
    pub average_weight: f64,
    pub newest_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryTopicEdge {
    pub source: String,
    pub target: String,
    pub relationship_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectTopicDetails {
    pub id: String,
    pub label: String,
    pub memory_count: usize,
    pub visible_memory_count: usize,
    pub hidden_memory_count: usize,
    pub truncated: bool,
    pub memories: Vec<ProjectMemoryDetail>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectMemoryDetail {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_accessed: Option<String>,
    pub access_count: u64,
    pub weight: f64,
    pub summary: String,
    pub raw_excerpt: Option<String>,
    pub keywords: Vec<String>,
    pub importance: String,
    pub source_type: Option<String>,
    pub source_data: Option<String>,
    pub related_ids: Vec<String>,
}

struct MemoryRow {
    id: String,
    topic: String,
    updated_at: String,
    weight: f64,
    related_ids: Vec<String>,
}

struct MemoryDetailRow {
    id: String,
    created_at: String,
    updated_at: String,
    last_accessed: Option<String>,
    access_count: u64,
    weight: f64,
    summary: String,
    raw_excerpt: Option<String>,
    keywords: Vec<String>,
    importance: String,
    source_type: Option<String>,
    source_data: Option<String>,
    related_ids: Vec<String>,
}

#[derive(Default)]
struct TopicAccumulator {
    memory_count: usize,
    weight_total: f64,
    newest_at: Option<String>,
}

/// Read a privacy-preserving graph for one repository directly from the managed ICM store.
///
/// The result contains topic aggregates and opaque relationship identifiers only. Memory IDs,
/// summaries, excerpts, keywords, source metadata, database paths, and the repository token are
/// intentionally excluded from the serialized shape.
pub fn read_project_snapshot(path: &Path, repository_id: &str) -> Result<ProjectMemorySnapshot> {
    if !path.is_file() {
        return Err(MemoryError::SnapshotUnavailable);
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_millis(250))?;
    let memory_count = connection.query_row(
        "SELECT COUNT(*) FROM memories WHERE substr(topic, -65) = '-' || ?1",
        [repository_id],
        |row| row.get::<_, usize>(0),
    )?;
    let mut statement = connection.prepare(
        "SELECT id, topic, updated_at, weight, related_ids
         FROM memories
         WHERE substr(topic, -65) = '-' || ?1
         ORDER BY updated_at DESC, id ASC
         LIMIT ?2",
    )?;
    let rows = statement
        .query_map(params![repository_id, (MAX_MEMORIES + 1) as u64], |row| {
            let related_json: String = row.get(4)?;
            Ok(MemoryRow {
                id: row.get(0)?,
                topic: row.get(1)?,
                updated_at: row.get(2)?,
                weight: row.get(3)?,
                related_ids: serde_json::from_str(&related_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(4, Type::Text, Box::new(error))
                })?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut truncated = memory_count > MAX_MEMORIES;
    let rows = rows
        .into_iter()
        .filter(|row| topic_belongs_to_project(&row.topic, repository_id))
        .take(MAX_MEMORIES)
        .collect::<Vec<_>>();

    let mut topic_totals = BTreeMap::<String, TopicAccumulator>::new();
    let mut memory_topics = BTreeMap::<String, String>::new();
    for row in &rows {
        memory_topics.insert(row.id.clone(), row.topic.clone());
        let total = topic_totals.entry(row.topic.clone()).or_default();
        total.memory_count += 1;
        total.weight_total += row.weight;
        if total
            .newest_at
            .as_ref()
            .is_none_or(|newest| row.updated_at > *newest)
        {
            total.newest_at = Some(row.updated_at.clone());
        }
    }

    let topic_ids = topic_totals
        .keys()
        .map(|topic| (topic.clone(), opaque_id(topic)))
        .collect::<BTreeMap<_, _>>();
    let mut topics = topic_totals
        .into_iter()
        .map(|(topic, total)| MemoryTopicSnapshot {
            id: topic_ids[&topic].clone(),
            label: topic
                .strip_suffix(repository_id)
                .and_then(|prefix| prefix.strip_suffix('-'))
                .unwrap_or("topic")
                .to_owned(),
            memory_count: total.memory_count,
            average_weight: total.weight_total / total.memory_count as f64,
            newest_at: total.newest_at,
        })
        .collect::<Vec<_>>();
    topics.sort_by(|left, right| {
        right
            .memory_count
            .cmp(&left.memory_count)
            .then_with(|| left.label.cmp(&right.label))
    });
    if topics.len() > MAX_TOPICS {
        topics.truncate(MAX_TOPICS);
        truncated = true;
    }
    let visible_topics = topics
        .iter()
        .map(|topic| topic.id.as_str())
        .collect::<BTreeSet<_>>();

    let mut relationships = BTreeMap::<(String, String), BTreeSet<(String, String)>>::new();
    for row in &rows {
        for related_id in &row.related_ids {
            let Some(target_topic) = memory_topics.get(related_id) else {
                continue;
            };
            if row.topic == *target_topic {
                continue;
            }
            let pair = ordered_pair(&topic_ids[&row.topic], &topic_ids[target_topic]);
            if !visible_topics.contains(pair.0.as_str())
                || !visible_topics.contains(pair.1.as_str())
            {
                continue;
            }
            let memory_pair = ordered_pair(&row.id, related_id);
            relationships.entry(pair).or_default().insert(memory_pair);
        }
    }
    if relationships.len() > MAX_EDGES {
        truncated = true;
    }
    let edges = relationships
        .into_iter()
        .take(MAX_EDGES)
        .map(|((source, target), relationships)| MemoryTopicEdge {
            source,
            target,
            relationship_count: relationships.len(),
        })
        .collect();

    Ok(ProjectMemorySnapshot {
        memory_count,
        visible_memory_count: rows.len(),
        hidden_memory_count: memory_count.saturating_sub(rows.len()),
        topics,
        edges,
        truncated,
    })
}

/// Resolve one opaque topic identifier into a bounded, read-only project detail response.
///
/// Topic and memory identifiers are hashed before they leave this crate. The raw topic is first
/// positively filtered to the requested repository, so an opaque identifier observed in another
/// project cannot be used to cross the repository boundary.
pub fn read_project_topic_details(
    path: &Path,
    repository_id: &str,
    topic_id: &str,
) -> Result<Option<ProjectTopicDetails>> {
    if !path.is_file() {
        return Err(MemoryError::SnapshotUnavailable);
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_millis(250))?;

    let mut topic_statement = connection.prepare(
        "SELECT DISTINCT topic
         FROM memories
         WHERE substr(topic, -65) = '-' || ?1
         ORDER BY topic ASC",
    )?;
    let topics = topic_statement
        .query_map([repository_id], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let Some(topic) = topics.into_iter().find(|topic| {
        topic_belongs_to_project(topic, repository_id) && opaque_id(topic) == topic_id
    }) else {
        return Ok(None);
    };

    let memory_count = connection.query_row(
        "SELECT COUNT(*) FROM memories WHERE topic = ?1",
        [&topic],
        |row| row.get::<_, usize>(0),
    )?;
    let mut detail_statement = connection.prepare(
        "SELECT id, created_at, updated_at, last_accessed, access_count, weight,
                summary, raw_excerpt, keywords, importance, source_type, source_data, related_ids
         FROM memories
         WHERE topic = ?1
         ORDER BY updated_at DESC, id ASC
         LIMIT ?2",
    )?;
    let rows = detail_statement
        .query_map(params![topic, (MAX_TOPIC_MEMORIES + 1) as u64], |row| {
            let keywords_json: String = row.get(8)?;
            let related_json: String = row.get(12)?;
            Ok(MemoryDetailRow {
                id: row.get(0)?,
                created_at: row.get(1)?,
                updated_at: row.get(2)?,
                last_accessed: row.get(3)?,
                access_count: row.get(4)?,
                weight: row.get(5)?,
                summary: row.get(6)?,
                raw_excerpt: row.get(7)?,
                keywords: serde_json::from_str(&keywords_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(8, Type::Text, Box::new(error))
                })?,
                importance: row.get(9)?,
                source_type: row.get(10)?,
                source_data: row.get(11)?,
                related_ids: serde_json::from_str(&related_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(12, Type::Text, Box::new(error))
                })?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let truncated = rows.len() > MAX_TOPIC_MEMORIES || memory_count > MAX_TOPIC_MEMORIES;
    let memories = rows
        .into_iter()
        .take(MAX_TOPIC_MEMORIES)
        .map(|row| ProjectMemoryDetail {
            id: opaque_id(&row.id),
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_accessed: row.last_accessed,
            access_count: row.access_count,
            weight: row.weight,
            summary: bounded_text(&row.summary, MAX_SUMMARY_CHARS),
            raw_excerpt: row
                .raw_excerpt
                .map(|value| bounded_text(&value, MAX_EXCERPT_CHARS)),
            keywords: row
                .keywords
                .into_iter()
                .take(MAX_KEYWORDS)
                .map(|value| bounded_text(&value, 128))
                .collect(),
            importance: bounded_text(&row.importance, 32),
            source_type: row.source_type.map(|value| bounded_text(&value, 64)),
            source_data: row
                .source_data
                .map(|value| bounded_text(&value, MAX_SOURCE_DATA_CHARS)),
            related_ids: row
                .related_ids
                .into_iter()
                .take(MAX_TOPIC_MEMORIES)
                .map(|value| opaque_id(&value))
                .collect(),
        })
        .collect::<Vec<_>>();
    let visible_memory_count = memories.len();

    Ok(Some(ProjectTopicDetails {
        id: topic_id.to_owned(),
        label: topic
            .strip_suffix(repository_id)
            .and_then(|prefix| prefix.strip_suffix('-'))
            .unwrap_or("topic")
            .to_owned(),
        memory_count,
        visible_memory_count,
        hidden_memory_count: memory_count.saturating_sub(visible_memory_count),
        truncated,
        memories,
    }))
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn opaque_id(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn ordered_pair(left: &str, right: &str) -> (String, String) {
    if left <= right {
        (left.to_owned(), right.to_owned())
    } else {
        (right.to_owned(), left.to_owned())
    }
}
