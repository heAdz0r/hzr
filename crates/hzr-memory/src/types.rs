use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Importance {
    Critical,
    High,
    Medium,
    Low,
}

impl fmt::Display for Importance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecallRequest {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    pub limit: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl RecallRequest {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            topic: None,
            limit: 5,
            keyword: None,
            project: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoreRequest {
    pub topic: String,
    pub content: String,
    pub importance: Importance,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

impl StoreRequest {
    pub fn new(topic: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            content: content.into(),
            importance: Importance::Medium,
            keywords: Vec::new(),
            raw: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct MemoryRecord {
    #[serde(default)]
    pub score: Option<f32>,
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_accessed: String,
    pub access_count: u32,
    pub weight: f32,
    pub topic: String,
    pub summary: String,
    #[serde(default)]
    pub raw_excerpt: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub importance: Importance,
    pub source: MemorySource,
    #[serde(default)]
    pub related_ids: Vec<String>,
    #[serde(default)]
    pub scope: MemoryScope,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct MemoryRecallResponse {
    pub count: usize,
    pub total_matches: usize,
    pub memories: Vec<MemoryRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MemorySource {
    ClaudeCode {
        session_id: String,
        file_path: Option<String>,
    },
    Conversation {
        thread_id: String,
    },
    Manual,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryScope {
    #[default]
    User,
    Project,
    Org,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IcmTransport {
    StdioMcp,
    Http,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTransport {
    Http,
    StdioMcp,
    Cli,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct StoreReceipt {
    pub transport: MemoryTransport,
    pub memory: Option<MemoryRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ServiceHealth {
    pub status: String,
    pub has_embedder: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct MemoryStats {
    pub total_memories: usize,
    pub total_topics: usize,
    pub avg_weight: f32,
    pub oldest_memory: Option<String>,
    pub newest_memory: Option<String>,
}
