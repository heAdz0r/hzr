//! Redacted, bounded read-only projection of managed delegation sessions.
use crate::state::AppState;
use axum::{Json, extract::State, http::StatusCode};
use hzr_core::{privacy_identity_hash, read_bounded_regular_file};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Deserialize)]
struct Snapshot {
    schema_version: u32,
    run_id: String,
    provider: String,
    model: String,
    status: String,
    started_at_ms: u64,
    updated_at_ms: u64,
    tool_calls: u64,
    turns: u64,
    current_tool: Option<String>,
    actual_input_tokens: u64,
    actual_output_tokens: u64,
    actual_cache_read_tokens: u64,
    usage_observed: bool,
}

#[derive(Deserialize)]
struct Outcome {
    schema_version: u32,
    status: String,
    finished_at_ms: u64,
}

#[derive(Serialize)]
pub struct RunView {
    id: String,
    provider: String,
    model: String,
    status: String,
    started_at_ms: u64,
    updated_at_ms: u64,
    tool_calls: u64,
    turns: u64,
    current_tool: Option<String>,
    actual_input_tokens: Option<u64>,
    actual_output_tokens: Option<u64>,
    actual_cache_read_tokens: Option<u64>,
    execution: &'static str,
    parent_acceptance: &'static str,
}

fn read_runs(root: &Path, now: u64) -> Vec<RunView> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    // Keep the newest UUIDv7 session names without retaining an unbounded directory listing.
    let mut recent = std::collections::BTreeSet::new();
    for entry in entries.flatten().take(100_000) {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            recent.insert(entry.path());
            if recent.len() > 200 {
                recent.pop_first();
            }
        }
    }
    let mut runs = Vec::new();
    for directory in recent.into_iter().rev() {
        let Ok(bytes) = read_bounded_regular_file(&directory.join("delegation.json"), 16_384)
        else {
            continue;
        };
        let Ok(mut s) = serde_json::from_slice::<Snapshot>(&bytes) else {
            continue;
        };
        if s.schema_version != 1
            || s.run_id.len() > 128
            || !matches!(
                s.provider.as_str(),
                "opencode-go" | "openrouter" | "deepseek"
            )
            || s.model.is_empty()
            || s.model.len() > 160
            || s.started_at_ms > now.saturating_add(60_000)
            || s.updated_at_ms < s.started_at_ms
            || s.updated_at_ms > now.saturating_add(60_000)
            || !s
                .model
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_./:".contains(&b))
            || !matches!(
                s.status.as_str(),
                "starting" | "running" | "completed" | "failed"
            )
            || s.current_tool.as_ref().is_some_and(|tool| {
                tool.len() > 48
                    || !tool.starts_with("hzr_")
                    || !tool.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
            })
            || now.saturating_sub(s.updated_at_ms) > 30 * 24 * 60 * 60 * 1000
        {
            continue;
        }
        // Parent-owned outcome wins over any bridge heartbeat, including one
        // written while the process group was being terminated.
        let outcome = read_bounded_regular_file(&directory.join("delegation-outcome.json"), 1024)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Outcome>(&bytes).ok())
            .filter(|outcome| {
                outcome.schema_version == 1
                    && matches!(
                        outcome.status.as_str(),
                        "completed" | "failed" | "timed_out" | "cancelled"
                    )
                    && outcome.finished_at_ms >= s.started_at_ms
                    && outcome.finished_at_ms <= now.saturating_add(60_000)
            });
        if let Some(outcome) = outcome {
            s.status = outcome.status;
            s.updated_at_ms = outcome.finished_at_ms;
            s.current_tool = None;
        }
        let status = if matches!(s.status.as_str(), "starting" | "running")
            && now.saturating_sub(s.updated_at_ms) > 30_000
        {
            "interrupted".to_owned()
        } else {
            s.status
        };
        runs.push(RunView {
            id: privacy_identity_hash("delegation", &s.run_id),
            provider: s.provider,
            model: s.model,
            status: status.clone(),
            started_at_ms: s.started_at_ms,
            updated_at_ms: s.updated_at_ms,
            tool_calls: s.tool_calls,
            turns: s.turns,
            current_tool: if status == "interrupted" {
                None
            } else {
                s.current_tool
            },
            actual_input_tokens: s.usage_observed.then_some(s.actual_input_tokens),
            actual_output_tokens: s.usage_observed.then_some(s.actual_output_tokens),
            actual_cache_read_tokens: s.usage_observed.then_some(s.actual_cache_read_tokens),
            execution: "hzr-managed",
            parent_acceptance: "not_recorded",
        });
        if runs.len() == 50 {
            break;
        }
    }
    runs
}

pub async fn dashboard(State(state): State<AppState>) -> Result<Json<Vec<RunView>>, StatusCode> {
    let root = state.config.data_dir.join("sessions");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    tokio::task::spawn_blocking(move || read_runs(&root, now))
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redacts_paths_prompts_and_marks_stale_runs() {
        let temp = tempfile::tempdir().expect("test fixture");
        let directory = temp.path().join("session");
        std::fs::create_dir(&directory).expect("test fixture");
        let snapshot = serde_json::json!({
            "schema_version":1, "run_id":"private-session", "provider":"opencode-go",
            "model":"deepseek-v4.1-flash", "status":"running",
            "started_at_ms":1, "updated_at_ms":2, "tool_calls":1, "turns":1,
            "current_tool":"hzr_read", "actual_input_tokens":20, "actual_output_tokens":3,
            "actual_cache_read_tokens":0, "usage_observed":true,
            "workspace":"/private/workspace", "prompt":"private-task", "key":"private-secret"
        });
        std::fs::write(directory.join("delegation.json"), snapshot.to_string())
            .expect("test fixture");
        let runs = read_runs(temp.path(), 40_000);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "interrupted");
        assert!(runs[0].current_tool.is_none());
        // A terminal receipt wins even if the child wrote a later running heartbeat.
        std::fs::write(
            directory.join("delegation-outcome.json"),
            r#"{"schema_version":1,"status":"timed_out","finished_at_ms":3}"#,
        )
        .expect("receipt");
        let terminated = read_runs(temp.path(), 40_000);
        assert_eq!(terminated[0].status, "timed_out");
        assert!(terminated[0].current_tool.is_none());
        let public = serde_json::to_string(&runs).expect("test fixture");
        assert!(!public.contains("private-"));
        assert!(!public.contains("/private"));
        assert!(public.contains("not_recorded"));
    }
}
