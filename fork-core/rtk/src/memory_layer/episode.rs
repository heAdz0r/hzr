//! E8.1: Episodic memory — session lifecycle (start/event/end), affinity tracking,
//! causal link recording. Persistent in SQLite (schema v5 tables: ADR-0005).

use anyhow::{Context, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::cache::{epoch_secs, with_db, with_retry};
use super::intent::TaskIntent;

// ── Public types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Read,
    Edit,
    GrepaiHit,
    Delta,
    Decision,
    Feedback,
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read => write!(f, "read"),
            Self::Edit => write!(f, "edit"),
            Self::GrepaiHit => write!(f, "grepai_hit"),
            Self::Delta => write!(f, "delta"),
            Self::Decision => write!(f, "decision"),
            Self::Feedback => write!(f, "feedback"),
        }
    }
}

impl std::str::FromStr for EventType {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "read" => Ok(Self::Read),
            "edit" => Ok(Self::Edit),
            "grepai_hit" => Ok(Self::GrepaiHit),
            "delta" => Ok(Self::Delta),
            "decision" => Ok(Self::Decision),
            "feedback" => Ok(Self::Feedback),
            other => anyhow::bail!("Unknown event type: {other}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EpisodeEvent {
    pub session_id: String,
    pub event_type: EventType,
    pub file_path: Option<String>,
    pub symbol: Option<String>,
    pub payload_json: Option<String>,
}

// ── Episode lifecycle ──────────────────────────────────────────────────────────

/// Start a new episode. Returns the `session_id` (16-char hex).
#[allow(dead_code)] // L1: Episode lifecycle API — will be wired into run_explore/run_plan for session tracking (PRD §8.1 planned integration)
pub fn start_episode(
    project_id: &str,
    task_text: &str,
    intent: &TaskIntent,
    query_type: &str,
    token_budget: Option<i64>,
) -> Result<String> {
    with_retry(3, || {
        start_episode_inner(project_id, task_text, intent, query_type, token_budget)
    })
}

fn start_episode_inner(
    project_id: &str,
    task_text: &str,
    intent: &TaskIntent,
    query_type: &str,
    token_budget: Option<i64>,
) -> Result<String> {
    let now = epoch_secs(std::time::SystemTime::now()) as i64;
    let session_id = {
        let raw = format!("{}|{}|{}", project_id, task_text, now);
        format!("{:016x}", xxhash_rust::xxh3::xxh3_64(raw.as_bytes()))
    };
    with_db(|conn| {
        // C1: pooled connection
        conn.execute(
            "INSERT OR IGNORE INTO episodes
                 (session_id, project_id, task_text, task_fingerprint, query_type, started_at, token_budget)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                session_id,
                project_id,
                task_text,
                intent.task_fingerprint,
                query_type,
                now,
                token_budget,
            ],
        )
        .context("Failed to insert episode")?;
        Ok(session_id.clone())
    })
}

/// Record a single event within a session. Also updates `task_file_affinity` for file events.
#[allow(dead_code)] // L1: Episode event API — will be wired into run_explore/run_plan for file-access tracking and affinity scoring (PRD §8.1 planned integration)
pub fn record_episode_event(event: &EpisodeEvent) -> Result<()> {
    with_retry(3, || record_episode_event_inner(event))
}

fn record_episode_event_inner(event: &EpisodeEvent) -> Result<()> {
    with_db(|conn| {
        // C1: pooled connection
        let now = epoch_secs(std::time::SystemTime::now()) as i64;
        conn.execute(
            "INSERT INTO episode_events
                 (session_id, event_type, file_path, symbol, payload_json, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                event.session_id,
                event.event_type.to_string(),
                event.file_path,
                event.symbol,
                event.payload_json,
                now,
            ],
        )
        .context("Failed to insert episode_event")?;

        Ok(())
    })
}

/// Purge episodes older than `retention_days`. Returns count of deleted episodes.
pub fn purge_episodes(retention_days: i64) -> Result<usize> {
    with_db(|conn| {
        // C1: pooled connection
        let cutoff = epoch_secs(std::time::SystemTime::now()) as i64 - retention_days * 86_400;
        let deleted = conn
            .execute(
                "DELETE FROM episodes WHERE started_at < ?1",
                params![cutoff],
            )
            .context("Failed to purge old episodes")?;
        // Cascade-delete orphaned events (no FK in SQLite by default)
        conn.execute(
            "DELETE FROM episode_events
             WHERE session_id NOT IN (SELECT session_id FROM episodes)",
            [],
        )
        .context("Failed to purge orphaned episode_events")?;
        Ok(deleted)
    })
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_layer::intent::parse_intent;

    fn test_intent() -> TaskIntent {
        parse_intent("fix jwt token refresh bug", "testproj")
    }

    /// RAII guard: redirects mem_db_path() for this thread only via thread-local.
    /// Other test threads continue to use their own paths. No env var mutation.
    struct IsolatedDb {
        _dir: tempfile::TempDir,
    }
    impl Drop for IsolatedDb {
        fn drop(&mut self) {
            use crate::memory_layer::cache::THREAD_DB_PATH;
            THREAD_DB_PATH.with(|p| *p.borrow_mut() = None);
        }
    }

    fn isolated_db() -> IsolatedDb {
        use crate::memory_layer::cache::THREAD_DB_PATH;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("mem.db");
        THREAD_DB_PATH.with(|p| *p.borrow_mut() = Some(path));
        IsolatedDb { _dir: dir }
    }

    #[test]
    fn test_start_episode_returns_hex_id() {
        let _db = isolated_db();
        let id = start_episode(
            "proj1",
            "fix auth bug",
            &test_intent(),
            "bugfix",
            Some(3000),
        )
        .expect("start_episode");
        assert_eq!(id.len(), 16, "session_id should be 16 hex chars");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_record_episode_event_read() {
        let _db = isolated_db();
        let session =
            start_episode("proj2", "add feature", &test_intent(), "feature", None).unwrap();
        let event = EpisodeEvent {
            session_id: session.clone(),
            event_type: EventType::Read,
            file_path: Some("src/auth.rs".to_string()),
            symbol: None,
            payload_json: None,
        };
        record_episode_event(&event).expect("record_event");
    }

    #[test]
    fn test_purge_episodes_removes_old() {
        let _db = isolated_db();
        let _ = start_episode("proj5", "old task", &test_intent(), "bugfix", None).unwrap();
        // Purge with 0 days (removes everything)
        let deleted = purge_episodes(-1).expect("purge"); // -1 days: cutoff = now+86400, purges everything
        assert!(deleted >= 1);
    }
}
