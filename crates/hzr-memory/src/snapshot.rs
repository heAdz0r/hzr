use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use rusqlite::types::Type;
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Result, topic_belongs_to_project};

const MAX_MEMORIES: usize = 256;
const MAX_TOPICS: usize = 64;
const MAX_EDGES: usize = 256;

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

struct MemoryRow {
    id: String,
    topic: String,
    updated_at: String,
    weight: f64,
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
        return Ok(ProjectMemorySnapshot::default());
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
