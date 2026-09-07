//! Durable projections for the opt-in agtx Agent Observatory.
//!
//! The daemon's observability store is transient and bounded; task history is
//! neither. These tables are the durable half: one row per observed task, an
//! append-only local event sequence, the session links that make spending
//! attributable, and the normalized one-sided usage receipts that give it a
//! number.
//!
//! Three rules shape everything here.
//!
//! **A snapshot is not an event journal.** Polling sees states, not
//! transitions. A change between two polls is lost, and the only honest thing
//! to do is record what was observed, when it was observed, and where the gaps
//! are. `source_at_ms` and `observed_at_ms` are separate columns for that
//! reason.
//!
//! **Absence is not deletion.** A task missing from one page of one scan means
//! the scan was partial, the store was busy, or the task is gone — and nothing
//! distinguishes those from a single observation. A tombstone therefore needs
//! two consecutive *complete* traversals, and even then it means "no longer
//! present in the source", never Done and never Accepted.
//!
//! **Nothing here fabricates money.** A usage receipt is one-sided by
//! construction: it is priced, never differenced against an invented baseline.
//! Unknown model, missing rate or stale catalog produce an unavailable estimate
//! with a reason, not a zero.

use std::collections::{BTreeMap, BTreeSet};

use hzr_protocol::agents::{
    AgentBoardStatus, AgentEventKind, AgentHookState, AgentLinkProvenance, AgentRuntimePhase,
    AgentSnapshotCapabilities, AgentSnapshotEnvelope, AgentTokenTotals, AgentUsageReceiptV1,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Ledger, LedgerError};

/// Consecutive complete traversals that must miss a task before it is
/// tombstoned. One is not enough: a single truncated scan looks identical.
pub const TOMBSTONE_COMPLETE_SCANS: i64 = 2;

/// Hard caps on a single import batch, mirroring the PRD's transfer bounds.
pub const MAX_USAGE_RECEIPTS_PER_IMPORT: usize = 1_000;
/// Only per-request deltas are accepted. Summing cumulative session snapshots
/// double-counts every earlier request in the session.
pub const SUPPORTED_USAGE_KIND: &str = "request_delta";

/// Longest bounded display string retained for an agent or model label.
const MAX_LABEL_CHARS: usize = 128;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

pub(super) fn init_agent_schema(connection: &Connection) -> Result<(), LedgerError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS agent_sources (
                source_key TEXT PRIMARY KEY,
                enrollment_id TEXT NOT NULL,
                project_hash TEXT NOT NULL,
                source_generation TEXT NOT NULL,
                upstream_version TEXT NOT NULL DEFAULT '',
                upstream_commit TEXT NOT NULL DEFAULT '',
                patch_identity TEXT NOT NULL DEFAULT '',
                cursor TEXT,
                capabilities_json TEXT NOT NULL DEFAULT '{}',
                last_success_ms INTEGER,
                last_error_code TEXT,
                last_observed_at_ms INTEGER,
                complete_scans INTEGER NOT NULL DEFAULT 0,
                first_observed_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_agent_sources_project
                ON agent_sources(project_hash);
             CREATE TABLE IF NOT EXISTS agent_tasks (
                task_key TEXT PRIMARY KEY,
                source_key TEXT NOT NULL,
                project_hash TEXT NOT NULL,
                source_project_id TEXT NOT NULL DEFAULT '',
                source_task_id TEXT NOT NULL DEFAULT '',
                title TEXT,
                branch TEXT,
                board_status TEXT NOT NULL,
                unknown_board_status TEXT,
                runtime_phase TEXT NOT NULL DEFAULT 'unknown',
                runtime_observed_at_ms INTEGER,
                hook_state TEXT NOT NULL DEFAULT 'unknown',
                hook_observed_at_ms INTEGER,
                hook_working_stale INTEGER NOT NULL DEFAULT 0,
                agent TEXT NOT NULL DEFAULT '',
                cycle INTEGER NOT NULL DEFAULT 1,
                first_observed_at_ms INTEGER NOT NULL,
                last_observed_at_ms INTEGER NOT NULL,
                source_updated_at_ms INTEGER,
                revision INTEGER NOT NULL DEFAULT 1,
                state_hash TEXT NOT NULL,
                missing_streak INTEGER NOT NULL DEFAULT 0,
                tombstoned_at_ms INTEGER,
                working_ms INTEGER NOT NULL DEFAULT 0,
                blocked_ms INTEGER NOT NULL DEFAULT 0,
                idle_ms INTEGER NOT NULL DEFAULT 0,
                observed_ms INTEGER NOT NULL DEFAULT 0,
                gap_ms INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS idx_agent_tasks_scope
                ON agent_tasks(project_hash, last_observed_at_ms);
             CREATE INDEX IF NOT EXISTS idx_agent_tasks_source
                ON agent_tasks(source_key);
             CREATE TABLE IF NOT EXISTS agent_task_agents (
                task_key TEXT NOT NULL,
                agent TEXT NOT NULL,
                first_observed_at_ms INTEGER NOT NULL,
                PRIMARY KEY (task_key, agent)
             );
             CREATE TABLE IF NOT EXISTS agent_runs (
                run_id TEXT PRIMARY KEY,
                task_key TEXT NOT NULL,
                project_hash TEXT NOT NULL,
                agent TEXT,
                host TEXT,
                model TEXT,
                session_hash TEXT,
                observed_start_ms INTEGER NOT NULL,
                observed_end_ms INTEGER,
                link_provenance TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_agent_runs_task
                ON agent_runs(task_key, observed_start_ms);
             CREATE TABLE IF NOT EXISTS agent_edges (
                source_key TEXT NOT NULL,
                from_task_key TEXT NOT NULL,
                to_task_key TEXT NOT NULL,
                kind TEXT NOT NULL,
                resolved INTEGER NOT NULL DEFAULT 1,
                from_label TEXT NOT NULL DEFAULT '',
                first_observed_at_ms INTEGER NOT NULL,
                last_observed_at_ms INTEGER NOT NULL,
                PRIMARY KEY (source_key, from_task_key, to_task_key, kind)
             );
             CREATE TABLE IF NOT EXISTS agent_events (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                event_key TEXT NOT NULL UNIQUE,
                source_key TEXT NOT NULL,
                project_hash TEXT NOT NULL,
                task_key TEXT,
                kind TEXT NOT NULL,
                source_at_ms INTEGER,
                observed_at_ms INTEGER NOT NULL,
                evidence TEXT NOT NULL,
                from_state TEXT,
                to_state TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_agent_events_project
                ON agent_events(project_hash, observed_at_ms, sequence);
             CREATE INDEX IF NOT EXISTS idx_agent_events_task
                ON agent_events(task_key, observed_at_ms, sequence);
             CREATE TABLE IF NOT EXISTS agent_session_links (
                link_id TEXT PRIMARY KEY,
                task_key TEXT NOT NULL,
                project_hash TEXT NOT NULL,
                host TEXT NOT NULL,
                session_hash TEXT NOT NULL,
                provenance TEXT NOT NULL,
                valid_from_ms INTEGER NOT NULL,
                valid_to_ms INTEGER,
                conflict INTEGER NOT NULL DEFAULT 0
             );
             CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_links_unique
                ON agent_session_links(task_key, host, session_hash);
             CREATE INDEX IF NOT EXISTS idx_agent_links_session
                ON agent_session_links(session_hash);
             CREATE TABLE IF NOT EXISTS agent_usage_receipts (
                receipt_key TEXT PRIMARY KEY,
                receipt_id TEXT NOT NULL,
                source TEXT NOT NULL,
                source_record_id TEXT NOT NULL,
                payload_hash TEXT NOT NULL,
                observed_at_ms INTEGER NOT NULL,
                project_hash TEXT NOT NULL,
                host TEXT NOT NULL,
                session_hash TEXT NOT NULL,
                source_task_id TEXT,
                request_id TEXT,
                provider TEXT NOT NULL,
                harness TEXT NOT NULL,
                model TEXT NOT NULL,
                billing_method TEXT NOT NULL,
                currency TEXT NOT NULL,
                request_input_tokens INTEGER,
                usage_kind TEXT NOT NULL,
                usage_json TEXT NOT NULL,
                reported_cost_microunits INTEGER,
                original_source_hash TEXT,
                provenance TEXT NOT NULL DEFAULT 'user_supplied',
                externally_verified INTEGER NOT NULL DEFAULT 0,
                imported_at_ms INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_agent_usage_scope
                ON agent_usage_receipts(project_hash, observed_at_ms);
             CREATE INDEX IF NOT EXISTS idx_agent_usage_session
                ON agent_usage_receipts(session_hash, observed_at_ms);
             CREATE TABLE IF NOT EXISTS agent_accounting_sessions (
                project_hash TEXT NOT NULL,
                session_hash TEXT NOT NULL,
                accounting_session_hash TEXT NOT NULL,
                PRIMARY KEY(project_hash, session_hash)
             );",
        )
        .map_err(LedgerError::Database)
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

fn digest_hex(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    hex::encode(hasher.finalize())
}

/// Bounded display validation. An arbitrary source string is never a label.
fn bounded_label(value: &str) -> String {
    let trimmed: String = value
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LABEL_CHARS)
        .collect();
    if trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ' | '/' | '+' | ':'))
    {
        trimmed
    } else {
        String::new()
    }
}

/// Bound one piece of human text from the source.
///
/// A title is prose, so it is sanitised rather than refused the way an
/// identifier is: control characters — ANSI escapes included — are removed and
/// the rest is truncated. It is still untrusted, and every surface renders it
/// as text.
fn bounded_text(value: &str, limit: usize) -> Option<String> {
    let cleaned: String = value
        .chars()
        .filter(|character| !character.is_control())
        .take(limit)
        .collect();
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Longest source title retained.
pub const MAX_TASK_TITLE_CHARS: usize = 160;

/// The pseudonymous label a user sees, e.g. `Task 7ac3`.
#[must_use]
pub fn agent_task_label(task_key: &str) -> String {
    let short: String = task_key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("Task {short}")
}

/// Identity of one enrolled source generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentSourceIdentity {
    /// HZR's own enrollment UUID; stable across store recreation.
    pub enrollment_id: String,
    /// The generation fingerprint the helper reported for the store.
    pub source_generation: String,
    /// Pseudonym for the enrolled worktree.
    pub project_hash: String,
    /// Derived primary key: enrollment plus generation.
    pub source_key: String,
}

impl AgentSourceIdentity {
    #[must_use]
    pub fn new(enrollment_id: &str, source_generation: &str, project_hash: &str) -> Self {
        Self {
            enrollment_id: enrollment_id.to_string(),
            source_generation: source_generation.to_string(),
            project_hash: project_hash.to_string(),
            source_key: digest_hex(&["agent_source", enrollment_id, source_generation]),
        }
    }

    #[must_use]
    pub fn task_key(&self, source_project_id: &str, source_task_id: &str) -> String {
        digest_hex(&[
            "agent_task",
            &self.source_key,
            source_project_id,
            source_task_id,
        ])
    }
}

// ---------------------------------------------------------------------------
// Snapshot application
// ---------------------------------------------------------------------------

/// What applying one page changed. Counts are exact so a replay test can assert
/// zero rather than "about the same".
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentSnapshotApplied {
    pub tasks_seen: u64,
    pub tasks_created: u64,
    pub tasks_updated: u64,
    pub events_recorded: u64,
    pub edges_recorded: u64,
    pub links_created: u64,
    pub tombstoned: u64,
    pub complete: bool,
}

/// A recorded observation gap: a timeout, a refusal, a source reset or a
/// suspension. Written as an event so the timeline can show the hole.
#[derive(Clone, Debug)]
pub struct AgentGap<'a> {
    pub project_hash: &'a str,
    pub source_key: &'a str,
    pub code: &'a str,
    pub observed_at_ms: i64,
}

/// One task's normalized state. Hashing it is how a replay is distinguished
/// from a change without comparing every column by hand.
struct NormalizedTaskState<'a> {
    board_status: &'a str,
    unknown_board_status: Option<&'a str>,
    runtime_phase: &'a str,
    runtime_observed_at_ms: Option<i64>,
    hook_state: &'a str,
    hook_observed_at_ms: Option<i64>,
    agent: &'a str,
    cycle: i64,
    title: Option<&'a str>,
}

impl NormalizedTaskState<'_> {
    fn hash(&self) -> String {
        digest_hex(&[
            self.board_status,
            self.unknown_board_status.unwrap_or(""),
            self.runtime_phase,
            &self.runtime_observed_at_ms.unwrap_or_default().to_string(),
            self.hook_state,
            &self.hook_observed_at_ms.unwrap_or_default().to_string(),
            self.agent,
            &self.cycle.to_string(),
            self.title.unwrap_or(""),
        ])
    }
}

struct StoredTask {
    board_status: String,
    agent: String,
    revision: i64,
    state_hash: String,
    first_observed_at_ms: i64,
    last_observed_at_ms: i64,
    runtime_phase: String,
    hook_state: String,
    runtime_observed_at_ms: Option<i64>,
    hook_observed_at_ms: Option<i64>,
    hook_working_stale: bool,
}

/// Everything an ingestion needs that is not in the snapshot itself.
pub struct AgentApplyContext<'a> {
    /// Store-private session pseudonym. Raw session ids never reach a column.
    pub session_pseudonym: &'a dyn Fn(&str) -> String,
    /// Longest gap between two observations that still counts as continuous
    /// time. Anything longer is a hole, and is accumulated as one instead of
    /// being credited to whatever state happened to be showing.
    pub max_observation_interval_ms: i64,
    /// Maximum evidence age for attributing a polling interval to a state.
    pub stale_after_ms: i64,
}

/// Split the interval since the previous observation into a state bucket.
///
/// Runtime phase leads because a live TUI publishes it; the agent's own hook
/// record is the fallback. Neither is guessed from silence.
fn interval_bucket(runtime_phase: &str, hook_state: &str) -> &'static str {
    match runtime_phase {
        "working" => "working",
        "blocked" => "blocked",
        "idle" | "ready" => "idle",
        _ => match hook_state {
            "working" => "working",
            "blocked" => "blocked",
            "waiting" => "idle",
            _ => "unattributed",
        },
    }
}

impl Ledger {
    /// Register or refresh one enrolled source generation.
    ///
    /// A different `source_generation` for the same enrollment produces a new
    /// `source_key` and therefore a new, separate history. That is deliberate:
    /// a store that was deleted and rebuilt shares nothing but a path with the
    /// one before it, and stitching the two together would invent continuity.
    pub fn agent_source_upsert(
        &self,
        identity: &AgentSourceIdentity,
        capabilities: &AgentSnapshotCapabilities,
        upstream_version: &str,
        upstream_commit: &str,
        patch_identity: &str,
        now_ms: i64,
    ) -> Result<bool, LedgerError> {
        let capabilities_json =
            serde_json::to_string(capabilities).unwrap_or_else(|_| "{}".to_string());
        let existed: bool = self
            .connection
            .query_row(
                "SELECT 1 FROM agent_sources WHERE source_key = ?1",
                [&identity.source_key],
                |_| Ok(true),
            )
            .optional()
            .map_err(LedgerError::Database)?
            .unwrap_or(false);
        self.connection
            .execute(
                "INSERT INTO agent_sources (
                    source_key, enrollment_id, project_hash, source_generation,
                    upstream_version, upstream_commit, patch_identity,
                    capabilities_json, first_observed_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
                 ON CONFLICT(source_key) DO UPDATE SET
                    upstream_version = excluded.upstream_version,
                    upstream_commit = excluded.upstream_commit,
                    patch_identity = excluded.patch_identity,
                    capabilities_json = excluded.capabilities_json,
                    updated_at_ms = excluded.updated_at_ms",
                params![
                    identity.source_key,
                    identity.enrollment_id,
                    identity.project_hash,
                    identity.source_generation,
                    upstream_version,
                    upstream_commit,
                    patch_identity,
                    capabilities_json,
                    now_ms
                ],
            )
            .map_err(LedgerError::Database)?;
        Ok(!existed)
    }

    /// Record a failed or interrupted observation without touching projections.
    pub fn agent_record_gap(&self, gap: AgentGap<'_>) -> Result<(), LedgerError> {
        let event_key = digest_hex(&[
            "gap",
            gap.source_key,
            gap.code,
            &gap.observed_at_ms.to_string(),
        ]);
        self.connection
            .execute(
                "INSERT OR IGNORE INTO agent_events (
                    event_key, source_key, project_hash, task_key, kind,
                    source_at_ms, observed_at_ms, evidence, from_state, to_state)
                 VALUES (?1, ?2, ?3, NULL, 'snapshot_gap', NULL, ?4, 'observed', NULL, ?5)",
                params![
                    event_key,
                    gap.source_key,
                    gap.project_hash,
                    gap.observed_at_ms,
                    gap.code
                ],
            )
            .map_err(LedgerError::Database)?;
        self.connection
            .execute(
                "UPDATE agent_sources
                    SET last_error_code = ?2, updated_at_ms = ?3, complete_scans = 0
                  WHERE source_key = ?1",
                params![gap.source_key, gap.code, gap.observed_at_ms],
            )
            .map_err(LedgerError::Database)?;
        Ok(())
    }

    /// Apply one snapshot page, its deduped events and its continuation cursor
    /// in a single transaction.
    ///
    /// Replaying an identical page changes nothing and records no event: the
    /// idempotency key covers the source generation, the task, the previous
    /// projection revision and the normalized new state. A→B→A still yields two
    /// transition events, because the revision differs each time.
    pub fn agent_apply_snapshot(
        &mut self,
        identity: &AgentSourceIdentity,
        envelope: &AgentSnapshotEnvelope,
        context: &AgentApplyContext<'_>,
    ) -> Result<AgentSnapshotApplied, LedgerError> {
        let observed_at_ms = envelope.observed_at_ms;
        let transaction = self
            .connection
            .transaction()
            .map_err(LedgerError::Database)?;
        let mut applied = AgentSnapshotApplied {
            complete: envelope.complete,
            ..Default::default()
        };

        for task in &envelope.tasks {
            let task_key = identity.task_key(&task.source_project_id, &task.source_task_id);
            let board_status = AgentBoardStatus::parse(&task.board_status).as_str();
            let unknown_board_status = task
                .unknown_board_status
                .as_deref()
                .map(bounded_label)
                .filter(|value| !value.is_empty());
            let runtime = task.runtime.as_ref();
            let runtime_phase = runtime
                .map(|runtime| AgentRuntimePhase::parse(&runtime.phase_status).as_str())
                .unwrap_or("unknown");
            let runtime_observed_at_ms = runtime.and_then(|runtime| runtime.updated_at_ms);
            let hook = task.hook.as_ref();
            let hook_state = hook
                .map(|hook| AgentHookState::parse(&hook.state).as_str())
                .unwrap_or("unknown");
            let hook_observed_at_ms = hook.map(|hook| hook.recorded_at_ms);
            let hook_working_stale = hook.is_some_and(|hook| hook.working_stale);
            let agent = bounded_label(&task.agent);
            let state_hash = NormalizedTaskState {
                board_status,
                unknown_board_status: unknown_board_status.as_deref(),
                runtime_phase,
                runtime_observed_at_ms,
                hook_state,
                hook_observed_at_ms,
                agent: &agent,
                cycle: task.cycle,
                title: task.title.as_deref(),
            }
            .hash();

            let stored = transaction
                .query_row(
                    "SELECT board_status, agent, revision, state_hash, first_observed_at_ms,
                            last_observed_at_ms, runtime_phase, hook_state,
                            runtime_observed_at_ms, hook_observed_at_ms, hook_working_stale
                       FROM agent_tasks WHERE task_key = ?1",
                    [&task_key],
                    |row| {
                        Ok(StoredTask {
                            board_status: row.get(0)?,
                            agent: row.get(1)?,
                            revision: row.get(2)?,
                            state_hash: row.get(3)?,
                            first_observed_at_ms: row.get(4)?,
                            last_observed_at_ms: row.get(5)?,
                            runtime_phase: row.get(6)?,
                            hook_state: row.get(7)?,
                            runtime_observed_at_ms: row.get(8)?,
                            hook_observed_at_ms: row.get(9)?,
                            hook_working_stale: row.get::<_, i64>(10)? != 0,
                        })
                    },
                )
                .optional()
                .map_err(LedgerError::Database)?;

            applied.tasks_seen += 1;
            let changed = stored
                .as_ref()
                .is_none_or(|stored| stored.state_hash != state_hash);
            let revision = stored.as_ref().map_or(1, |stored| {
                if changed {
                    stored.revision + 1
                } else {
                    stored.revision
                }
            });
            let first_observed_at_ms = stored
                .as_ref()
                .map_or(observed_at_ms, |stored| stored.first_observed_at_ms);

            // Attribute the interval since the previous observation to the
            // state that was showing during it, or to the gap column when the
            // interval is longer than one poll can account for.
            let (mut working_delta, mut blocked_delta, mut idle_delta, mut gap_delta) =
                (0_i64, 0_i64, 0_i64, 0_i64);
            if let Some(previous) = stored.as_ref() {
                let delta = observed_at_ms.saturating_sub(previous.last_observed_at_ms);
                if delta > 0 {
                    if delta > context.max_observation_interval_ms {
                        gap_delta = delta;
                    } else {
                        let fresh = |at: Option<i64>| {
                            at.is_some_and(|at| {
                                at <= observed_at_ms
                                    && observed_at_ms.saturating_sub(at) <= context.stale_after_ms
                            })
                        };
                        let runtime = if fresh(previous.runtime_observed_at_ms) {
                            previous.runtime_phase.as_str()
                        } else {
                            "unknown"
                        };
                        let hook = if !previous.hook_working_stale
                            && fresh(previous.hook_observed_at_ms)
                        {
                            previous.hook_state.as_str()
                        } else {
                            "unknown"
                        };
                        match interval_bucket(runtime, hook) {
                            "working" => working_delta = delta,
                            "blocked" => blocked_delta = delta,
                            "idle" => idle_delta = delta,
                            _ => gap_delta = delta,
                        }
                    }
                }
            }

            transaction
                .execute(
                    "INSERT INTO agent_tasks (
                        task_key, source_key, project_hash, source_project_id, source_task_id,
                        title, branch, board_status, unknown_board_status, runtime_phase, runtime_observed_at_ms, hook_state,
                        hook_observed_at_ms, hook_working_stale, agent, cycle,
                        first_observed_at_ms, last_observed_at_ms, source_updated_at_ms,
                        revision, state_hash, missing_streak, tombstoned_at_ms,
                        working_ms, blocked_ms, idle_ms, observed_ms, gap_ms)
                     VALUES (?1, ?2, ?3, ?4, ?24, ?25, ?26, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                             ?13, ?14, ?15, ?16, ?17, ?18, 0, NULL, ?19, ?20, ?21, ?22, ?23)
                     ON CONFLICT(task_key) DO UPDATE SET
                        board_status = excluded.board_status,
                        unknown_board_status = excluded.unknown_board_status,
                        runtime_phase = excluded.runtime_phase,
                        runtime_observed_at_ms = excluded.runtime_observed_at_ms,
                        hook_state = excluded.hook_state,
                        hook_observed_at_ms = excluded.hook_observed_at_ms,
                        hook_working_stale = excluded.hook_working_stale,
                        title = excluded.title,
                        branch = excluded.branch,
                        agent = excluded.agent,
                        cycle = excluded.cycle,
                        last_observed_at_ms = excluded.last_observed_at_ms,
                        source_updated_at_ms = excluded.source_updated_at_ms,
                        revision = excluded.revision,
                        state_hash = excluded.state_hash,
                        missing_streak = 0,
                        tombstoned_at_ms = NULL,
                        working_ms = agent_tasks.working_ms + excluded.working_ms,
                        blocked_ms = agent_tasks.blocked_ms + excluded.blocked_ms,
                        idle_ms = agent_tasks.idle_ms + excluded.idle_ms,
                        observed_ms = agent_tasks.observed_ms + excluded.observed_ms,
                        gap_ms = agent_tasks.gap_ms + excluded.gap_ms",
                    params![
                        task_key,
                        identity.source_key,
                        identity.project_hash,
                        bounded_label(&task.source_project_id),
                        board_status,
                        unknown_board_status,
                        runtime_phase,
                        runtime_observed_at_ms,
                        hook_state,
                        hook_observed_at_ms,
                        i64::from(hook_working_stale),
                        agent,
                        task.cycle,
                        first_observed_at_ms,
                        observed_at_ms,
                        task.source_updated_at_ms,
                        revision,
                        state_hash,
                        working_delta,
                        blocked_delta,
                        idle_delta,
                        working_delta + blocked_delta + idle_delta,
                        gap_delta,
                        task.source_task_id,
                        task.title
                            .as_deref()
                            .and_then(|value| bounded_text(value, MAX_TASK_TITLE_CHARS)),
                        task.branch
                            .as_deref()
                            .and_then(|value| bounded_text(value, MAX_LABEL_CHARS)),
                    ],
                )
                .map_err(LedgerError::Database)?;

            match &stored {
                None => {
                    applied.tasks_created += 1;
                    applied.events_recorded += insert_event(
                        &transaction,
                        identity,
                        Some(&task_key),
                        AgentEventKind::FirstObserved,
                        None,
                        observed_at_ms,
                        "observed",
                        None,
                        Some(board_status),
                        &[&task_key, "0", &state_hash],
                    )?;
                }
                Some(stored) if changed => {
                    applied.tasks_updated += 1;
                    if stored.board_status != board_status {
                        applied.events_recorded += insert_event(
                            &transaction,
                            identity,
                            Some(&task_key),
                            AgentEventKind::PhaseChanged,
                            task.source_updated_at_ms,
                            observed_at_ms,
                            "observed",
                            Some(&stored.board_status),
                            Some(board_status),
                            &[&task_key, &stored.revision.to_string(), &state_hash],
                        )?;
                    }
                    if stored.agent != agent && !agent.is_empty() {
                        applied.events_recorded += insert_event(
                            &transaction,
                            identity,
                            Some(&task_key),
                            AgentEventKind::AgentChanged,
                            task.source_updated_at_ms,
                            observed_at_ms,
                            "observed",
                            Some(&stored.agent),
                            Some(&agent),
                            &[&task_key, &stored.revision.to_string(), &state_hash],
                        )?;
                        // A workflow handoff is an agent change *plus* a phase
                        // change on the same observation. Two agents merely
                        // working in sequence is not one, and is not recorded.
                        if stored.board_status != board_status {
                            applied.events_recorded += insert_event(
                                &transaction,
                                identity,
                                Some(&task_key),
                                AgentEventKind::HandoffObserved,
                                task.source_updated_at_ms,
                                observed_at_ms,
                                "observed",
                                Some(&stored.agent),
                                Some(&agent),
                                &[&task_key, &stored.revision.to_string(), &state_hash],
                            )?;
                        }
                    }
                }
                Some(_) => {}
            }

            if !agent.is_empty() {
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO agent_task_agents (task_key, agent, first_observed_at_ms)
                         VALUES (?1, ?2, ?3)",
                        params![task_key, agent, observed_at_ms],
                    )
                    .map_err(LedgerError::Database)?;
            }

            // Hook evidence is the only automatic link source, and only with a
            // session id the agent itself reported under the enrolled worktree.
            if let Some(session_id) = hook.and_then(|hook| hook.session_id.as_deref()) {
                let host = if agent.is_empty() {
                    "unknown".to_string()
                } else {
                    agent.clone()
                };
                let session_hash = (context.session_pseudonym)(&format!("{host}\u{0}{session_id}"));
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO agent_accounting_sessions VALUES (?1, ?2, ?3)",
                        params![
                            identity.project_hash,
                            session_hash,
                            (context.session_pseudonym)(session_id)
                        ],
                    )
                    .map_err(LedgerError::Database)?;
                applied.links_created += upsert_link(
                    &transaction,
                    identity,
                    &task_key,
                    &host,
                    &session_hash,
                    AgentLinkProvenance::AgtxHook,
                    hook.map_or(observed_at_ms, |hook| hook.recorded_at_ms),
                )?;
                if applied.links_created > 0 {
                    applied.events_recorded += insert_event(
                        &transaction,
                        identity,
                        Some(&task_key),
                        AgentEventKind::SessionLinked,
                        None,
                        observed_at_ms,
                        "observed",
                        None,
                        Some("agtx_hook"),
                        &[&task_key, &session_hash, "agtx_hook"],
                    )?;
                }
            }
        }

        // Edges are keyed by task_key on both ends so an unresolved reference
        // still has a stable identity to render and to resolve later.
        for edge in &envelope.edges {
            let from_key = identity.task_key("", &edge.from_source_task_id);
            let from_key = resolve_edge_endpoint(
                &transaction,
                identity,
                &edge.from_source_task_id,
                &from_key,
            )?;
            let to_key = resolve_edge_endpoint(
                &transaction,
                identity,
                &edge.to_source_task_id,
                &identity.task_key("", &edge.to_source_task_id),
            )?;
            transaction
                .execute(
                    "INSERT INTO agent_edges (
                        source_key, from_task_key, to_task_key, kind, resolved, from_label,
                        first_observed_at_ms, last_observed_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                     ON CONFLICT(source_key, from_task_key, to_task_key, kind) DO UPDATE SET
                        resolved = excluded.resolved,
                        last_observed_at_ms = excluded.last_observed_at_ms",
                    params![
                        identity.source_key,
                        from_key,
                        to_key,
                        "depends_on",
                        i64::from(edge.resolved),
                        agent_task_label(&from_key),
                        observed_at_ms
                    ],
                )
                .map_err(LedgerError::Database)?;
            applied.edges_recorded += 1;
        }

        // Upstream notifications carry stable ids, so they dedupe on their own
        // id rather than on a projection revision. Reading them here does not
        // consume them upstream; the helper never deletes a row.
        for notification in &envelope.notifications {
            let Some(kind) = AgentEventKind::parse(&notification.kind) else {
                continue;
            };
            if !matches!(
                kind,
                AgentEventKind::PhaseCompleted | AgentEventKind::TaskStuck
            ) {
                continue;
            }
            let task_key = notification
                .source_task_id
                .as_deref()
                .map(|id| identity.task_key("", id));
            let task_key = match task_key {
                Some(candidate) => Some(resolve_edge_endpoint(
                    &transaction,
                    identity,
                    notification.source_task_id.as_deref().unwrap_or_default(),
                    &candidate,
                )?),
                None => None,
            };
            applied.events_recorded += insert_event(
                &transaction,
                identity,
                task_key.as_deref(),
                kind,
                notification.created_at_ms,
                observed_at_ms,
                "reported",
                None,
                Some(kind.as_str()),
                &["notification", &notification.source_notification_id],
            )?;
        }

        // A complete traversal is the only thing that licenses reconciliation.
        if envelope.complete {
            let complete_scans: i64 = transaction
                .query_row(
                    "SELECT complete_scans FROM agent_sources WHERE source_key = ?1",
                    [&identity.source_key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(LedgerError::Database)?
                .unwrap_or(0);
            transaction
                .execute(
                    "UPDATE agent_tasks
                        SET missing_streak = missing_streak + 1
                      WHERE source_key = ?1
                        AND last_observed_at_ms < ?2
                        AND tombstoned_at_ms IS NULL",
                    params![identity.source_key, observed_at_ms],
                )
                .map_err(LedgerError::Database)?;
            let mut statement = transaction
                .prepare(
                    "SELECT task_key FROM agent_tasks
                      WHERE source_key = ?1 AND missing_streak >= ?2 AND tombstoned_at_ms IS NULL",
                )
                .map_err(LedgerError::Database)?;
            let missing: Vec<String> = statement
                .query_map(
                    params![identity.source_key, TOMBSTONE_COMPLETE_SCANS],
                    |row| row.get::<_, String>(0),
                )
                .map_err(LedgerError::Database)?
                .filter_map(Result::ok)
                .collect();
            drop(statement);
            for task_key in missing {
                transaction
                    .execute(
                        "UPDATE agent_tasks SET tombstoned_at_ms = ?2 WHERE task_key = ?1",
                        params![task_key, observed_at_ms],
                    )
                    .map_err(LedgerError::Database)?;
                applied.tombstoned += 1;
                applied.events_recorded += insert_event(
                    &transaction,
                    identity,
                    Some(&task_key),
                    AgentEventKind::Tombstoned,
                    None,
                    observed_at_ms,
                    "observed",
                    None,
                    Some("absent_from_source"),
                    &[&task_key, "tombstone", &observed_at_ms.to_string()],
                )?;
            }
            transaction
                .execute(
                    "UPDATE agent_sources
                        SET cursor = NULL, last_success_ms = ?2, last_observed_at_ms = ?2,
                            last_error_code = NULL, complete_scans = ?3, updated_at_ms = ?2
                      WHERE source_key = ?1",
                    params![
                        identity.source_key,
                        observed_at_ms,
                        complete_scans.saturating_add(1)
                    ],
                )
                .map_err(LedgerError::Database)?;
        } else {
            transaction
                .execute(
                    "UPDATE agent_sources
                        SET cursor = ?2, last_success_ms = ?3, last_observed_at_ms = ?3,
                            last_error_code = NULL, updated_at_ms = ?3
                      WHERE source_key = ?1",
                    params![
                        identity.source_key,
                        envelope.next_cursor.as_deref(),
                        observed_at_ms
                    ],
                )
                .map_err(LedgerError::Database)?;
        }

        transaction.commit().map_err(LedgerError::Database)?;
        Ok(applied)
    }

    /// Record an explicit user-supplied link between a task and a host session.
    ///
    /// Explicit is the highest-priority evidence, but it still does not certify
    /// provider billing, and it cannot silently take a session away from
    /// another task: a second claim marks both sides in conflict instead.
    pub fn agent_link_accounting_session(
        &self,
        project_hash: &str,
        session_hash: &str,
        accounting_session_hash: &str,
    ) -> Result<(), LedgerError> {
        self.connection
            .execute(
                "INSERT OR IGNORE INTO agent_accounting_sessions VALUES (?1, ?2, ?3)",
                params![project_hash, session_hash, accounting_session_hash],
            )
            .map_err(LedgerError::Database)?;
        Ok(())
    }

    pub fn agent_link_session(
        &self,
        identity: &AgentSourceIdentity,
        source_task_id: &str,
        source_project_id: &str,
        host: &str,
        session_hash: &str,
        now_ms: i64,
    ) -> Result<(bool, bool), LedgerError> {
        let task_key = identity.task_key(source_project_id, source_task_id);
        let known: bool = self
            .connection
            .query_row(
                "SELECT 1 FROM agent_tasks WHERE task_key = ?1",
                [&task_key],
                |_| Ok(true),
            )
            .optional()
            .map_err(LedgerError::Database)?
            .unwrap_or(false);
        if !known {
            return Ok((false, false));
        }
        let created = upsert_link(
            &self.connection,
            identity,
            &task_key,
            host,
            session_hash,
            AgentLinkProvenance::ExplicitUser,
            now_ms,
        )?;
        let conflict: bool = self
            .connection
            .query_row(
                "SELECT COUNT(DISTINCT task_key) > 1 FROM agent_session_links
                  WHERE session_hash = ?1 AND project_hash = ?2",
                params![session_hash, identity.project_hash],
                |row| row.get::<_, i64>(0),
            )
            .map_err(LedgerError::Database)?
            != 0;
        Ok((created > 0, conflict))
    }
}

fn resolve_edge_endpoint(
    connection: &Connection,
    identity: &AgentSourceIdentity,
    source_task_id: &str,
    fallback: &str,
) -> Result<String, LedgerError> {
    // A task's key includes its source project id, which an edge reference does
    // not carry. Resolve through the stored projection when the task is known
    // and fall back to the project-less key when it is not, so an unresolved
    // reference still renders as a stable node.
    let mut statement = connection
        .prepare("SELECT task_key, source_project_id FROM agent_tasks WHERE source_key = ?1")
        .map_err(LedgerError::Database)?;
    let rows = statement
        .query_map([&identity.source_key], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(LedgerError::Database)?;
    for row in rows.flatten() {
        if row.0 == identity.task_key(&row.1, source_task_id) {
            return Ok(row.0);
        }
    }
    Ok(fallback.to_string())
}

fn upsert_link(
    connection: &Connection,
    identity: &AgentSourceIdentity,
    task_key: &str,
    host: &str,
    session_hash: &str,
    provenance: AgentLinkProvenance,
    valid_from_ms: i64,
) -> Result<u64, LedgerError> {
    let link_id = digest_hex(&["agent_link", task_key, host, session_hash]);
    let changed = connection
        .execute(
            "INSERT INTO agent_session_links (
                link_id, task_key, project_hash, host, session_hash, provenance,
                valid_from_ms, valid_to_ms, conflict)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 0)
             ON CONFLICT(task_key, host, session_hash) DO UPDATE SET
                provenance = CASE
                    WHEN excluded.provenance = 'explicit_user' THEN excluded.provenance
                    ELSE agent_session_links.provenance END",
            params![
                link_id,
                task_key,
                identity.project_hash,
                host,
                session_hash,
                provenance.as_str(),
                valid_from_ms
            ],
        )
        .map_err(LedgerError::Database)?;
    // One session claimed by several tasks stays ambiguous rather than being
    // assigned to whichever page arrived last.
    connection
        .execute(
            "UPDATE agent_session_links SET conflict = 1
              WHERE session_hash = ?1 AND project_hash = ?2
                AND (SELECT COUNT(DISTINCT task_key) FROM agent_session_links
                      WHERE session_hash = ?1 AND project_hash = ?2) > 1",
            params![session_hash, identity.project_hash],
        )
        .map_err(LedgerError::Database)?;
    Ok(u64::from(changed > 0 && changed == 1))
}

#[allow(clippy::too_many_arguments)]
fn insert_event(
    connection: &Connection,
    identity: &AgentSourceIdentity,
    task_key: Option<&str>,
    kind: AgentEventKind,
    source_at_ms: Option<i64>,
    observed_at_ms: i64,
    evidence: &str,
    from_state: Option<&str>,
    to_state: Option<&str>,
    idempotency: &[&str],
) -> Result<u64, LedgerError> {
    let mut parts = vec![identity.source_generation.as_str(), kind.as_str()];
    parts.extend_from_slice(idempotency);
    let event_key = digest_hex(&parts);
    let inserted = connection
        .execute(
            "INSERT OR IGNORE INTO agent_events (
                event_key, source_key, project_hash, task_key, kind,
                source_at_ms, observed_at_ms, evidence, from_state, to_state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                event_key,
                identity.source_key,
                identity.project_hash,
                task_key,
                kind.as_str(),
                source_at_ms,
                observed_at_ms,
                evidence,
                from_state,
                to_state
            ],
        )
        .map_err(LedgerError::Database)?;
    Ok(inserted as u64)
}

// ---------------------------------------------------------------------------
// Normalized one-sided usage import
// ---------------------------------------------------------------------------

/// Outcome of validating and committing one import batch.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentUsageImportOutcome {
    pub accepted: u64,
    pub replayed: u64,
    pub rejected: u64,
    pub conflicts: u64,
    /// `(source_record_id, stable code)`; never the offending payload.
    pub rejections: Vec<(String, String)>,
    pub committed: bool,
}

fn usage_receipt_key(
    receipt: &AgentUsageReceiptV1,
    project_hash: &str,
    session_hash: &str,
) -> String {
    digest_hex(&[
        "agent_usage",
        &receipt.source,
        project_hash,
        &receipt.host,
        session_hash,
        &receipt.source_record_id,
    ])
}

fn usage_payload_hash(receipt: &AgentUsageReceiptV1) -> String {
    let canonical = serde_json::to_string(receipt).unwrap_or_default();
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

fn validate_usage_receipt(receipt: &AgentUsageReceiptV1, now_ms: u64) -> Result<(), String> {
    if receipt.schema_version != 1 {
        return Err("unsupported_schema_version".into());
    }
    if receipt.usage_kind != SUPPORTED_USAGE_KIND {
        // A cumulative session snapshot summed with its predecessors counts
        // every earlier request again. Refuse rather than silently overstate.
        return Err("unsupported_usage_kind".into());
    }
    for (value, limit) in [
        (receipt.receipt_id.as_str(), 256),
        (receipt.source.as_str(), 128),
        (receipt.source_record_id.as_str(), 256),
        (receipt.host.as_str(), 64),
        (receipt.session_id.as_str(), 512),
        (receipt.provider.as_str(), 64),
        (receipt.harness.as_str(), 64),
        (receipt.model.as_str(), 128),
        (receipt.billing_method.as_str(), 64),
        (receipt.currency.as_str(), 8),
    ] {
        if value.is_empty()
            || value.len() > limit
            || !value.is_ascii()
            || value.chars().any(char::is_control)
        {
            return Err("invalid_field".into());
        }
    }
    if receipt.project_path.is_empty() || receipt.project_path.len() > 4096 {
        return Err("invalid_project_path".into());
    }
    if receipt.usage.reasoning_tokens > receipt.usage.output_tokens {
        // Reasoning is already inside output; a larger value means the exporter
        // added it twice.
        return Err("reasoning_exceeds_output".into());
    }
    if receipt
        .request_input_tokens
        .is_some_and(|tokens| tokens == 0)
    {
        return Err("invalid_request_input_tokens".into());
    }
    if receipt.observed_at_ms == 0 || receipt.observed_at_ms > now_ms.saturating_add(5 * 60 * 1_000)
    {
        return Err("invalid_observed_at".into());
    }
    Ok(())
}

impl Ledger {
    /// Validate an entire batch, then commit it atomically.
    ///
    /// Nothing is written unless every receipt passes: a partially applied
    /// import leaves a project's spending half-true, and an exporter cannot
    /// tell which half. A replay of the same `source_record_id` with identical
    /// content is a no-op; the same id with different content is a conflict and
    /// fails the batch.
    pub fn agent_import_usage(
        &mut self,
        receipts: &[AgentUsageReceiptV1],
        project_hash: &dyn Fn(&str) -> String,
        session_pseudonym: &dyn Fn(&str) -> String,
        now_ms: u64,
    ) -> Result<AgentUsageImportOutcome, LedgerError> {
        let mut outcome = AgentUsageImportOutcome::default();
        if receipts.len() > MAX_USAGE_RECEIPTS_PER_IMPORT {
            outcome.rejected = receipts.len() as u64;
            outcome
                .rejections
                .push((String::new(), "batch_too_large".into()));
            return Ok(outcome);
        }

        struct Prepared<'a> {
            receipt: &'a AgentUsageReceiptV1,
            key: String,
            payload_hash: String,
            project_hash: String,
            session_hash: String,
        }

        let mut prepared = Vec::with_capacity(receipts.len());
        let mut seen_keys = BTreeSet::new();
        for receipt in receipts {
            if let Err(code) = validate_usage_receipt(receipt, now_ms) {
                outcome.rejected += 1;
                outcome
                    .rejections
                    .push((receipt.source_record_id.clone(), code));
                continue;
            }
            let project = project_hash(&receipt.project_path);
            let session =
                session_pseudonym(&format!("{}\u{0}{}", receipt.host, receipt.session_id));
            let key = usage_receipt_key(receipt, &project, &session);
            if !seen_keys.insert(key.clone()) {
                outcome.conflicts += 1;
                outcome.rejections.push((
                    receipt.source_record_id.clone(),
                    "duplicate_in_batch".into(),
                ));
                continue;
            }
            prepared.push(Prepared {
                receipt,
                payload_hash: usage_payload_hash(receipt),
                key,
                project_hash: project,
                session_hash: session,
            });
        }

        for entry in &prepared {
            let existing: Option<String> = self
                .connection
                .query_row(
                    "SELECT payload_hash FROM agent_usage_receipts WHERE receipt_key = ?1",
                    [&entry.key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(LedgerError::Database)?;
            match existing {
                Some(hash) if hash == entry.payload_hash => outcome.replayed += 1,
                Some(_) => {
                    outcome.conflicts += 1;
                    outcome.rejections.push((
                        entry.receipt.source_record_id.clone(),
                        "source_record_conflict".into(),
                    ));
                }
                None => outcome.accepted += 1,
            }
        }

        if outcome.rejected > 0 || outcome.conflicts > 0 {
            outcome.accepted = 0;
            outcome.replayed = 0;
            outcome.committed = false;
            return Ok(outcome);
        }

        let transaction = self
            .connection
            .transaction()
            .map_err(LedgerError::Database)?;
        for entry in &prepared {
            transaction
                .execute(
                    "INSERT OR IGNORE INTO agent_accounting_sessions VALUES (?1, ?2, ?3)",
                    params![
                        entry.project_hash,
                        entry.session_hash,
                        session_pseudonym(&entry.receipt.session_id)
                    ],
                )
                .map_err(LedgerError::Database)?;
            let usage_json = serde_json::to_string(&entry.receipt.usage)
                .map_err(|error| LedgerError::InvalidOperation(error.to_string()))?;
            transaction
                .execute(
                    "INSERT OR IGNORE INTO agent_usage_receipts (
                        receipt_key, receipt_id, source, source_record_id, payload_hash,
                        observed_at_ms, project_hash, host, session_hash, source_task_id,
                        request_id, provider, harness, model, billing_method, currency,
                        request_input_tokens, usage_kind, usage_json, reported_cost_microunits,
                        original_source_hash, provenance, externally_verified, imported_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                             ?16, ?17, ?18, ?19, ?20, ?21, 'user_supplied', 0, ?22)",
                    params![
                        entry.key,
                        entry.receipt.receipt_id,
                        entry.receipt.source,
                        entry.receipt.source_record_id,
                        entry.payload_hash,
                        i64::try_from(entry.receipt.observed_at_ms).unwrap_or(i64::MAX),
                        entry.project_hash,
                        entry.receipt.host,
                        entry.session_hash,
                        entry.receipt.task_id,
                        entry.receipt.request_id,
                        entry.receipt.provider,
                        entry.receipt.harness,
                        entry.receipt.model,
                        entry.receipt.billing_method,
                        entry.receipt.currency,
                        entry.receipt.request_input_tokens,
                        entry.receipt.usage_kind,
                        usage_json,
                        entry.receipt.reported_cost_microunits,
                        entry.receipt.original_source_hash,
                        i64::try_from(now_ms).unwrap_or(i64::MAX),
                    ],
                )
                .map_err(LedgerError::Database)?;
        }
        transaction.commit().map_err(LedgerError::Database)?;
        outcome.committed = true;
        Ok(outcome)
    }
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

/// One task projection, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentTaskRow {
    pub task_key: String,
    pub project_hash: String,
    /// The source's own title, when the enrollment publishes titles.
    pub title: Option<String>,
    pub branch: Option<String>,
    pub board_status: String,
    pub unknown_board_status: Option<String>,
    pub runtime_phase: String,
    pub runtime_observed_at_ms: Option<i64>,
    pub hook_state: String,
    pub hook_observed_at_ms: Option<i64>,
    pub hook_working_stale: bool,
    pub agent: String,
    pub cycle: i64,
    pub first_observed_at_ms: i64,
    pub last_observed_at_ms: i64,
    pub source_updated_at_ms: Option<i64>,
    pub tombstoned: bool,
    /// Time attributed to each state over *observed* intervals only, plus the
    /// time that fell into holes between observations.
    pub working_ms: i64,
    pub blocked_ms: i64,
    pub idle_ms: i64,
    pub observed_ms: i64,
    pub gap_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentEdgeRow {
    pub from_task_key: String,
    pub to_task_key: String,
    pub kind: String,
    pub resolved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentEventRow {
    pub sequence: i64,
    pub task_key: Option<String>,
    pub kind: String,
    pub source_at_ms: Option<i64>,
    pub observed_at_ms: i64,
    pub evidence: String,
    pub from_state: Option<String>,
    pub to_state: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSessionLinkRow {
    pub task_key: String,
    pub host: String,
    pub session_hash: String,
    pub provenance: String,
    pub valid_from_ms: i64,
    pub valid_to_ms: Option<i64>,
    pub conflict: bool,
    pub usage_receipt_count: u64,
}

/// One stored usage receipt, decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentUsageRow {
    pub session_hash: String,
    pub provider: String,
    pub harness: String,
    pub model: String,
    pub billing_method: String,
    pub currency: String,
    pub request_input_tokens: Option<u64>,
    pub usage: AgentTokenTotals,
    pub reported_cost_microunits: Option<u64>,
    pub observed_at_ms: i64,
}

/// Source-level state for one enrollment generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSourceRow {
    pub source_key: String,
    pub project_hash: String,
    pub source_generation: String,
    pub capabilities: AgentSnapshotCapabilities,
    pub cursor: Option<String>,
    pub last_success_ms: Option<i64>,
    pub last_error_code: Option<String>,
    pub last_observed_at_ms: Option<i64>,
    pub first_observed_at_ms: i64,
    pub complete_scans: i64,
}

impl Ledger {
    /// Open the ledger read-only for a dashboard GET.
    ///
    /// Returns `None` when there is no ledger yet or the agent tables have not
    /// been created — a public read must never create a file, run a migration
    /// or contend on DDL with the daemon's single writer.
    pub fn agents_read_only(path: &std::path::Path) -> Result<Option<Self>, LedgerError> {
        if !path.is_file() {
            return Ok(None);
        }
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(LedgerError::Database)?;
        connection
            .busy_timeout(std::time::Duration::from_millis(250))
            .map_err(LedgerError::Database)?;
        let present: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type = 'table' AND name = 'agent_tasks'",
                [],
                |row| row.get(0),
            )
            .map_err(LedgerError::Database)?;
        if present == 0 {
            return Ok(None);
        }
        Ok(Some(Self { connection }))
    }

    /// Every source generation recorded for one enrolled project, newest first.
    pub fn agent_sources_for_project(
        &self,
        project_hash: &str,
    ) -> Result<Vec<AgentSourceRow>, LedgerError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT source_key, project_hash, source_generation, capabilities_json, cursor,
                        last_success_ms, last_error_code, last_observed_at_ms,
                        first_observed_at_ms, complete_scans
                   FROM agent_sources
                  WHERE project_hash = ?1
                  ORDER BY updated_at_ms DESC",
            )
            .map_err(LedgerError::Database)?;
        let rows = statement
            .query_map([project_hash], |row| {
                let capabilities_json: String = row.get(3)?;
                Ok(AgentSourceRow {
                    source_key: row.get(0)?,
                    project_hash: row.get(1)?,
                    source_generation: row.get(2)?,
                    capabilities: serde_json::from_str(&capabilities_json).unwrap_or_default(),
                    cursor: row.get(4)?,
                    last_success_ms: row.get(5)?,
                    last_error_code: row.get(6)?,
                    last_observed_at_ms: row.get(7)?,
                    first_observed_at_ms: row.get(8)?,
                    complete_scans: row.get(9)?,
                })
            })
            .map_err(LedgerError::Database)?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Find one task by the source's own id inside one enrollment generation.
    ///
    /// A task key mixes in the source project id, which a caller naming only a
    /// task cannot supply, so the lookup goes through the stored projection
    /// rather than guessing at the missing half.
    pub fn agent_task_by_source_id(
        &self,
        source_key: &str,
        source_task_id: &str,
    ) -> Result<Option<(String, String)>, LedgerError> {
        let row = self
            .connection
            .query_row(
                "SELECT task_key, source_project_id FROM agent_tasks
                  WHERE source_key = ?1 AND source_task_id = ?2",
                params![source_key, source_task_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(LedgerError::Database)?;
        Ok(row)
    }

    /// One bounded page of tasks. The cursor is the last `task_key` returned.
    pub fn agent_tasks_page(
        &self,
        project_hash: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<AgentTaskRow>, Option<String>), LedgerError> {
        let limit = limit.clamp(1, 200);
        let mut statement = self
            .connection
            .prepare(
                "SELECT task_key, project_hash, board_status, unknown_board_status, runtime_phase,
                        runtime_observed_at_ms, hook_state, hook_observed_at_ms,
                        hook_working_stale, agent, cycle, first_observed_at_ms,
                        last_observed_at_ms, source_updated_at_ms, tombstoned_at_ms,
                        working_ms, blocked_ms, idle_ms, observed_ms, gap_ms, title, branch
                   FROM agent_tasks
                  WHERE project_hash = ?1 AND task_key > ?2
                  ORDER BY task_key ASC
                  LIMIT ?3",
            )
            .map_err(LedgerError::Database)?;
        let rows = statement
            .query_map(
                params![
                    project_hash,
                    cursor.unwrap_or(""),
                    i64::try_from(limit + 1).unwrap_or(i64::MAX)
                ],
                |row| {
                    Ok(AgentTaskRow {
                        task_key: row.get(0)?,
                        project_hash: row.get(1)?,
                        board_status: row.get(2)?,
                        unknown_board_status: row.get(3)?,
                        runtime_phase: row.get(4)?,
                        runtime_observed_at_ms: row.get(5)?,
                        hook_state: row.get(6)?,
                        hook_observed_at_ms: row.get(7)?,
                        hook_working_stale: row.get::<_, i64>(8)? != 0,
                        agent: row.get(9)?,
                        cycle: row.get(10)?,
                        first_observed_at_ms: row.get(11)?,
                        last_observed_at_ms: row.get(12)?,
                        source_updated_at_ms: row.get(13)?,
                        tombstoned: row.get::<_, Option<i64>>(14)?.is_some(),
                        working_ms: row.get(15)?,
                        blocked_ms: row.get(16)?,
                        idle_ms: row.get(17)?,
                        observed_ms: row.get(18)?,
                        gap_ms: row.get(19)?,
                        title: row.get(20)?,
                        branch: row.get(21)?,
                    })
                },
            )
            .map_err(LedgerError::Database)?;
        let mut tasks: Vec<AgentTaskRow> = rows.filter_map(Result::ok).collect();
        let next_cursor = if tasks.len() > limit {
            tasks.truncate(limit);
            tasks.last().map(|task| task.task_key.clone())
        } else {
            None
        };
        Ok((tasks, next_cursor))
    }

    pub fn agent_task(&self, task_key: &str) -> Result<Option<AgentTaskRow>, LedgerError> {
        let (tasks, _) = self.agent_tasks_page_by_keys(&[task_key.to_string()])?;
        Ok(tasks.into_iter().next())
    }

    fn agent_tasks_page_by_keys(
        &self,
        keys: &[String],
    ) -> Result<(Vec<AgentTaskRow>, Option<String>), LedgerError> {
        let mut found = Vec::new();
        for key in keys {
            let mut statement = self
                .connection
                .prepare(
                    "SELECT task_key, project_hash, board_status, unknown_board_status,
                            runtime_phase, runtime_observed_at_ms, hook_state,
                            hook_observed_at_ms, hook_working_stale, agent, cycle,
                            first_observed_at_ms, last_observed_at_ms, source_updated_at_ms,
                            tombstoned_at_ms, working_ms, blocked_ms, idle_ms, observed_ms,
                            gap_ms, title, branch
                       FROM agent_tasks WHERE task_key = ?1",
                )
                .map_err(LedgerError::Database)?;
            let row = statement
                .query_row([key], |row| {
                    Ok(AgentTaskRow {
                        task_key: row.get(0)?,
                        project_hash: row.get(1)?,
                        board_status: row.get(2)?,
                        unknown_board_status: row.get(3)?,
                        runtime_phase: row.get(4)?,
                        runtime_observed_at_ms: row.get(5)?,
                        hook_state: row.get(6)?,
                        hook_observed_at_ms: row.get(7)?,
                        hook_working_stale: row.get::<_, i64>(8)? != 0,
                        agent: row.get(9)?,
                        cycle: row.get(10)?,
                        first_observed_at_ms: row.get(11)?,
                        last_observed_at_ms: row.get(12)?,
                        source_updated_at_ms: row.get(13)?,
                        tombstoned: row.get::<_, Option<i64>>(14)?.is_some(),
                        working_ms: row.get(15)?,
                        blocked_ms: row.get(16)?,
                        idle_ms: row.get(17)?,
                        observed_ms: row.get(18)?,
                        gap_ms: row.get(19)?,
                        title: row.get(20)?,
                        branch: row.get(21)?,
                    })
                })
                .optional()
                .map_err(LedgerError::Database)?;
            if let Some(row) = row {
                found.push(row);
            }
        }
        Ok((found, None))
    }

    pub fn agent_edges_for_project(
        &self,
        project_hash: &str,
        limit: usize,
    ) -> Result<Vec<AgentEdgeRow>, LedgerError> {
        let limit = limit.clamp(1, 1_000);
        let mut statement = self
            .connection
            .prepare(
                "SELECT e.from_task_key, e.to_task_key, e.kind, e.resolved
                   FROM agent_edges e
                   JOIN agent_sources s ON s.source_key = e.source_key
                  WHERE s.project_hash = ?1
                  ORDER BY e.from_task_key, e.to_task_key
                  LIMIT ?2",
            )
            .map_err(LedgerError::Database)?;
        let rows = statement
            .query_map(
                params![project_hash, i64::try_from(limit).unwrap_or(i64::MAX)],
                |row| {
                    Ok(AgentEdgeRow {
                        from_task_key: row.get(0)?,
                        to_task_key: row.get(1)?,
                        kind: row.get(2)?,
                        resolved: row.get::<_, i64>(3)? != 0,
                    })
                },
            )
            .map_err(LedgerError::Database)?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Events for a project, or for one task inside it, ordered by observation.
    pub fn agent_events_page(
        &self,
        project_hash: &str,
        task_key: Option<&str>,
        cursor: Option<i64>,
        limit: usize,
    ) -> Result<(Vec<AgentEventRow>, Option<i64>), LedgerError> {
        let limit = limit.clamp(1, 500);
        let mut statement = self
            .connection
            .prepare(
                "SELECT sequence, task_key, kind, source_at_ms, observed_at_ms, evidence,
                        from_state, to_state
                   FROM agent_events
                  WHERE project_hash = ?1
                    AND (?2 IS NULL OR task_key = ?2)
                    AND sequence > ?3
                  ORDER BY sequence ASC
                  LIMIT ?4",
            )
            .map_err(LedgerError::Database)?;
        let rows = statement
            .query_map(
                params![
                    project_hash,
                    task_key,
                    cursor.unwrap_or(0),
                    i64::try_from(limit + 1).unwrap_or(i64::MAX)
                ],
                |row| {
                    Ok(AgentEventRow {
                        sequence: row.get(0)?,
                        task_key: row.get(1)?,
                        kind: row.get(2)?,
                        source_at_ms: row.get(3)?,
                        observed_at_ms: row.get(4)?,
                        evidence: row.get(5)?,
                        from_state: row.get(6)?,
                        to_state: row.get(7)?,
                    })
                },
            )
            .map_err(LedgerError::Database)?;
        let mut events: Vec<AgentEventRow> = rows.filter_map(Result::ok).collect();
        let next = if events.len() > limit {
            events.truncate(limit);
            events.last().map(|event| event.sequence)
        } else {
            None
        };
        Ok((events, next))
    }

    pub fn agent_links_for_task(
        &self,
        task_key: &str,
    ) -> Result<Vec<AgentSessionLinkRow>, LedgerError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT l.task_key, l.host, l.session_hash, l.provenance, l.valid_from_ms,
                        l.valid_to_ms, l.conflict,
                        (SELECT COUNT(*) FROM agent_usage_receipts u
                          WHERE u.session_hash = l.session_hash)
                   FROM agent_session_links l
                  WHERE l.task_key = ?1
                  ORDER BY l.valid_from_ms ASC",
            )
            .map_err(LedgerError::Database)?;
        let rows = statement
            .query_map([task_key], |row| {
                Ok(AgentSessionLinkRow {
                    task_key: row.get(0)?,
                    host: row.get(1)?,
                    session_hash: row.get(2)?,
                    provenance: row.get(3)?,
                    valid_from_ms: row.get(4)?,
                    valid_to_ms: row.get(5)?,
                    conflict: row.get::<_, i64>(6)? != 0,
                    usage_receipt_count: row.get::<_, i64>(7)?.max(0) as u64,
                })
            })
            .map_err(LedgerError::Database)?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Usage receipts for a set of session pseudonyms inside a project window.
    pub fn agent_usage_rows(
        &self,
        project_hash: &str,
        sessions: &BTreeSet<String>,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<AgentUsageRow>, LedgerError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT session_hash, provider, harness, model, billing_method, currency,
                        request_input_tokens, usage_json, reported_cost_microunits, observed_at_ms
                   FROM agent_usage_receipts
                  WHERE project_hash = ?1 AND observed_at_ms >= ?2 AND observed_at_ms <= ?3
                  ORDER BY observed_at_ms ASC",
            )
            .map_err(LedgerError::Database)?;
        let rows = statement
            .query_map(params![project_hash, from_ms, to_ms], |row| {
                let usage_json: String = row.get(7)?;
                Ok(AgentUsageRow {
                    session_hash: row.get(0)?,
                    provider: row.get(1)?,
                    harness: row.get(2)?,
                    model: row.get(3)?,
                    billing_method: row.get(4)?,
                    currency: row.get(5)?,
                    request_input_tokens: row
                        .get::<_, Option<i64>>(6)?
                        .and_then(|value| u64::try_from(value).ok()),
                    usage: serde_json::from_str(&usage_json).unwrap_or_default(),
                    reported_cost_microunits: row
                        .get::<_, Option<i64>>(8)?
                        .and_then(|value| u64::try_from(value).ok()),
                    observed_at_ms: row.get(9)?,
                })
            })
            .map_err(LedgerError::Database)?;
        // An empty set means "no session is linked here", which is exactly zero
        // attributable receipts — never "no filter, take everything".
        Ok(rows
            .filter_map(Result::ok)
            .filter(|row| sessions.contains(&row.session_hash))
            .collect())
    }

    /// Session pseudonyms linked to a task, and whether each is ambiguous.
    pub fn agent_task_sessions(
        &self,
        task_key: &str,
    ) -> Result<(BTreeSet<String>, BTreeSet<String>), LedgerError> {
        let mut exclusive = BTreeSet::new();
        let mut ambiguous = BTreeSet::new();
        for link in self.agent_links_for_task(task_key)? {
            if link.conflict {
                ambiguous.insert(link.session_hash);
            } else {
                exclusive.insert(link.session_hash);
            }
        }
        Ok((exclusive, ambiguous))
    }

    /// Distinct agents observed on a task, oldest first.
    pub fn agent_task_agent_history(&self, task_key: &str) -> Result<Vec<String>, LedgerError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT agent FROM agent_task_agents
                  WHERE task_key = ?1 ORDER BY first_observed_at_ms ASC",
            )
            .map_err(LedgerError::Database)?;
        let rows = statement
            .query_map([task_key], |row| row.get::<_, String>(0))
            .map_err(LedgerError::Database)?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Existing HZR operation accounting for a set of linked sessions.
    ///
    /// Reuses the session efficiency summary the rest of HZR already publishes,
    /// so an agent task never gets a second, differently-computed reduction.
    pub fn agent_operation_reduction(
        &self,
        project_hash: &str,
        sessions: &BTreeSet<String>,
    ) -> Result<(u64, u64), LedgerError> {
        let mut baseline = 0_u64;
        let mut delivered = 0_u64;
        for session in sessions {
            let summary =
                self.session_efficiency_summary_for_hashes(session, session, Some(project_hash))?;
            baseline = baseline.saturating_add(summary.baseline_tokens_estimated);
            delivered = delivered.saturating_add(summary.delivered_tokens_estimated);
        }
        Ok((baseline, delivered))
    }

    /// Provider receipts already recorded through the legacy path for these
    /// sessions, which the agent projection must not add on top of its own.
    pub fn agent_unreconciled_receipt_count(
        &self,
        project_hash: &str,
        sessions: &BTreeSet<String>,
    ) -> Result<u64, LedgerError> {
        if sessions.is_empty() {
            return Ok(0);
        }
        let mut total = 0_u64;
        for session in sessions {
            let count: i64 = self
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM provider_economic_receipts
                      WHERE session_hash = ?1 AND project_hash = ?2",
                    params![session, project_hash],
                    |row| row.get(0),
                )
                .map_err(LedgerError::Database)?;
            total = total.saturating_add(count.max(0) as u64);
        }
        Ok(total)
    }

    /// Counts behind the coverage block on every response.
    pub fn agent_coverage_counts(
        &self,
        project_hash: &str,
    ) -> Result<AgentCoverageCounts, LedgerError> {
        let observed_tasks: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM agent_tasks WHERE project_hash = ?1",
                [project_hash],
                |row| row.get(0),
            )
            .map_err(LedgerError::Database)?;
        let linked_sessions: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(DISTINCT session_hash) FROM agent_session_links
                  WHERE project_hash = ?1 AND conflict = 0",
                [project_hash],
                |row| row.get(0),
            )
            .map_err(LedgerError::Database)?;
        let ambiguous_sessions: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(DISTINCT session_hash) FROM agent_session_links
                  WHERE project_hash = ?1 AND conflict = 1",
                [project_hash],
                |row| row.get(0),
            )
            .map_err(LedgerError::Database)?;
        let covered_sessions: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(DISTINCT l.session_hash)
                   FROM agent_session_links l
                   JOIN agent_usage_receipts u ON u.session_hash = l.session_hash
                  WHERE l.project_hash = ?1",
                [project_hash],
                |row| row.get(0),
            )
            .map_err(LedgerError::Database)?;
        let gaps: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM agent_events
                  WHERE project_hash = ?1 AND kind = 'snapshot_gap'",
                [project_hash],
                |row| row.get(0),
            )
            .map_err(LedgerError::Database)?;
        let history_start: Option<i64> = self
            .connection
            .query_row(
                "SELECT MIN(observed_at_ms) FROM agent_events WHERE project_hash = ?1",
                [project_hash],
                |row| row.get(0),
            )
            .optional()
            .map_err(LedgerError::Database)?
            .flatten();
        Ok(AgentCoverageCounts {
            observed_tasks: observed_tasks.max(0) as u64,
            linked_sessions: linked_sessions.max(0) as u64,
            ambiguous_sessions: ambiguous_sessions.max(0) as u64,
            covered_sessions: covered_sessions.max(0) as u64,
            gaps: gaps.max(0) as u64,
            history_start_ms: history_start,
        })
    }

    /// Prune observed events past the retention window.
    ///
    /// Task projections, session links and usage receipt deduplication keys are
    /// deliberately untouched: dropping a receipt key would let the same import
    /// bill a second time, and dropping a projection would resurrect a task as
    /// newly discovered.
    pub fn agent_prune_events(&self, retention_days: u32, now_ms: i64) -> Result<u64, LedgerError> {
        let retention_days = i64::from(retention_days.clamp(1, 365));
        let cutoff = now_ms.saturating_sub(retention_days * 24 * 60 * 60 * 1_000);
        let removed = self
            .connection
            .execute(
                "DELETE FROM agent_events WHERE observed_at_ms < ?1",
                [cutoff],
            )
            .map_err(LedgerError::Database)?;
        Ok(removed as u64)
    }

    /// Remove every projection for one enrolled project.
    ///
    /// Explicit, separate from `disable`: disabling stops observation and keeps
    /// history, and conflating the two would make "pause monitoring" delete a
    /// month of evidence.
    pub fn agent_purge_project(&mut self, project_hash: &str) -> Result<u64, LedgerError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(LedgerError::Database)?;
        let mut removed = 0_u64;
        for statement in [
            "DELETE FROM agent_task_agents WHERE task_key IN
                (SELECT task_key FROM agent_tasks WHERE project_hash = ?1)",
            "DELETE FROM agent_edges WHERE source_key IN
                (SELECT source_key FROM agent_sources WHERE project_hash = ?1)",
            "DELETE FROM agent_runs WHERE project_hash = ?1",
            "DELETE FROM agent_session_links WHERE project_hash = ?1",
            "DELETE FROM agent_events WHERE project_hash = ?1",
            "DELETE FROM agent_tasks WHERE project_hash = ?1",
            "DELETE FROM agent_sources WHERE project_hash = ?1",
        ] {
            removed = removed.saturating_add(
                transaction
                    .execute(statement, [project_hash])
                    .map_err(LedgerError::Database)? as u64,
            );
        }
        transaction.commit().map_err(LedgerError::Database)?;
        Ok(removed)
    }
}

/// The raw counts behind a coverage block.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AgentCoverageCounts {
    pub observed_tasks: u64,
    /// Sessions linked to exactly one task.
    pub linked_sessions: u64,
    /// Sessions claimed by more than one task, so unattributable.
    pub ambiguous_sessions: u64,
    pub covered_sessions: u64,
    pub gaps: u64,
    pub history_start_ms: Option<i64>,
}

/// Scope of one economics query: a whole project, or one task inside it.
#[derive(Clone, Copy, Debug)]
pub struct AgentEconomicsQuery<'a> {
    pub project_hash: &'a str,
    /// `None` aggregates the project; `Some` restricts to one task's sessions.
    pub task_key: Option<&'a str>,
    pub from_ms: i64,
    pub to_ms: i64,
}

impl Ledger {
    /// Build the economics block for one scope.
    ///
    /// Four evidence layers stay apart and are never added together:
    /// reported tokens, the amounts the source itself stated, the catalog
    /// estimate of the same requests, and HZR's own operation reduction. A
    /// reported cost and an estimate for the *same* request are alternatives —
    /// summing them would bill it twice.
    pub fn agent_economics(
        &self,
        query: AgentEconomicsQuery<'_>,
        catalog: &crate::billing::PricingCatalog,
    ) -> Result<hzr_protocol::agents::AgentEconomics, LedgerError> {
        use hzr_protocol::agents::{AgentEconomics, AgentMoneySubtotal, AgentUnavailable};

        if let Some(task_key) = query.task_key {
            if self
                .agent_task(task_key)?
                .is_none_or(|task| task.project_hash != query.project_hash)
            {
                return Err(LedgerError::InvalidOperation(
                    "task outside project scope".into(),
                ));
            }
        }
        let (exclusive, ambiguous) = match query.task_key {
            Some(task_key) => self.agent_task_sessions(task_key)?,
            None => self.agent_project_sessions(query.project_hash)?,
        };

        let rows =
            self.agent_usage_rows(query.project_hash, &exclusive, query.from_ms, query.to_ms)?;
        let aggregate = AgentUsageAggregate::from_rows(&rows)?;
        let mut economics = AgentEconomics {
            schema_version: hzr_protocol::agents::AGENT_API_SCHEMA_VERSION,
            reported_tokens: aggregate.tokens,
            reported_receipt_count: aggregate.receipt_count,
            ..Default::default()
        };
        economics.reported_cost = aggregate
            .reported_cost
            .iter()
            .map(
                |(currency, (microunits, covered, total))| AgentMoneySubtotal {
                    currency: currency.clone(),
                    microunits: *microunits,
                    covered_receipts: *covered,
                    total_receipts: *total,
                },
            )
            .collect();

        let (estimated, identity, versions, unavailable) = price_rows(catalog, &rows)?;
        economics.estimated_api_cost = estimated;
        economics.price_table_identity = identity;
        economics.price_entry_versions = versions;
        economics.unavailable = unavailable;

        if !ambiguous.is_empty() {
            let shared =
                self.agent_usage_rows(query.project_hash, &ambiguous, query.from_ms, query.to_ms)?;
            let (shared_cost, _, _, mut shared_unavailable) = price_rows(catalog, &shared)?;
            economics.shared_unallocated_cost = shared_cost;
            economics.unavailable.append(&mut shared_unavailable);
        }

        let mut accounting_sessions = BTreeSet::new();
        for session in &exclusive {
            let mapped: Option<String> = self.connection.query_row(
                "SELECT accounting_session_hash FROM agent_accounting_sessions WHERE project_hash = ?1 AND session_hash = ?2",
                params![query.project_hash, session], |row| row.get(0),
            ).optional().map_err(LedgerError::Database)?;
            if let Some(mapped) = mapped {
                accounting_sessions.insert(mapped);
            } else {
                economics.unavailable.push(AgentUnavailable {
                    metric: "hzr_operation_reduction".into(),
                    reason: "session predates accounting linkage; re-link or re-import its usage"
                        .into(),
                });
            }
        }
        let (baseline, delivered) =
            self.agent_operation_reduction(query.project_hash, &accounting_sessions)?;
        economics.hzr_baseline_tokens_estimated = baseline;
        economics.hzr_delivered_tokens_estimated = delivered;
        economics.net_avoided_tokens_estimated = i64::try_from(baseline).unwrap_or(i64::MAX)
            - i64::try_from(delivered).unwrap_or(i64::MAX);
        // Null, not zero, when there is no baseline to be a percentage of.
        // Negative reduction is a real result and is kept as one.
        economics.reduction_pct = (baseline > 0)
            .then(|| 100.0 * economics.net_avoided_tokens_estimated as f64 / baseline as f64);
        economics.unreconciled_existing_receipts =
            self.agent_unreconciled_receipt_count(query.project_hash, &accounting_sessions)?;

        if let Some(task_key) = query.task_key {
            if let Some(task) = self.agent_task(task_key)? {
                economics.observed_wall_time_ms = task.observed_ms.max(0) as u64;
                economics.observed_blocked_time_ms = task.blocked_ms.max(0) as u64;
                economics.observed_idle_time_ms = task.idle_ms.max(0) as u64;
            }
        }

        // Done, Review, a Ready phase and a process exit are all *not*
        // acceptance. Without explicit acceptance evidence the denominator does
        // not exist, so neither does the metric.
        economics.accepted_task_count = None;
        economics.cost_per_accepted_task = Vec::new();
        economics.unavailable.push(AgentUnavailable {
            metric: "cost_per_accepted_task".into(),
            reason: "no explicit acceptance evidence is linked to these tasks".into(),
        });
        Ok(economics)
    }

    /// Every session pseudonym linked anywhere in a project, split by ambiguity.
    pub fn agent_project_sessions(
        &self,
        project_hash: &str,
    ) -> Result<(BTreeSet<String>, BTreeSet<String>), LedgerError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT session_hash, MAX(conflict) FROM agent_session_links
                  WHERE project_hash = ?1 GROUP BY session_hash",
            )
            .map_err(LedgerError::Database)?;
        let rows = statement
            .query_map([project_hash], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0))
            })
            .map_err(LedgerError::Database)?;
        let mut exclusive = BTreeSet::new();
        let mut ambiguous = BTreeSet::new();
        for (session, conflict) in rows.flatten() {
            if conflict {
                ambiguous.insert(session);
            } else {
                exclusive.insert(session);
            }
        }
        Ok((exclusive, ambiguous))
    }
}

/// Price a set of receipts, keeping per-currency subtotals apart and turning
/// every pricing failure into a named unavailability rather than a zero.
type PricedAgentRows = (
    Vec<hzr_protocol::agents::AgentMoneySubtotal>,
    Option<String>,
    Vec<String>,
    Vec<hzr_protocol::agents::AgentUnavailable>,
);

fn price_rows(
    catalog: &crate::billing::PricingCatalog,
    rows: &[AgentUsageRow],
) -> Result<PricedAgentRows, LedgerError> {
    use hzr_protocol::agents::{AgentMoneySubtotal, AgentUnavailable};
    let mut totals: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
    let mut identity = None;
    let mut versions = BTreeSet::new();
    let mut reasons: BTreeMap<String, String> = BTreeMap::new();
    for row in rows {
        let entry = totals.entry(row.currency.clone()).or_insert((0, 0, 0));
        entry.2 += 1;
        let usage = crate::billing::ProviderTokenUsage {
            input_tokens: row.usage.input_tokens,
            output_tokens: row.usage.output_tokens,
            reasoning_tokens: row.usage.reasoning_tokens,
            cache_read_tokens: row.usage.cache_read_tokens,
            cache_write_tokens: row.usage.cache_write_tokens,
            cache_write_5m_tokens: row.usage.cache_write_5m_tokens,
            cache_write_1h_tokens: row.usage.cache_write_1h_tokens,
        };
        match crate::billing::price_single_usage(
            catalog,
            crate::billing::SingleUsageRequest {
                harness: &row.harness,
                provider: &row.provider,
                model: &row.model,
                method: &row.billing_method,
                request_input_tokens: row.request_input_tokens,
                usage,
                currency: &row.currency,
            },
        ) {
            Ok(price) => {
                entry.0 = entry.0.checked_add(price.microunits).ok_or_else(|| {
                    LedgerError::InvalidOperation("economic arithmetic overflow".into())
                })?;
                entry.1 += 1;
                identity.get_or_insert(price.price_table_identity);
                versions.insert(price.entry_version);
            }
            Err(error) => {
                reasons
                    .entry(format!("estimated_api_cost:{}", row.model))
                    .or_insert_with(|| error.to_string());
            }
        }
    }
    let subtotals = totals
        .into_iter()
        .map(
            |(currency, (microunits, covered, total))| AgentMoneySubtotal {
                currency,
                microunits,
                covered_receipts: covered,
                total_receipts: total,
            },
        )
        .collect();
    let unavailable = reasons
        .into_iter()
        .map(|(metric, reason)| AgentUnavailable { metric, reason })
        .collect();
    Ok((
        subtotals,
        identity,
        versions.into_iter().collect(),
        unavailable,
    ))
}

/// Group token totals and money by currency without ever crossing currencies.
#[derive(Clone, Debug, Default)]
pub struct AgentUsageAggregate {
    pub tokens: AgentTokenTotals,
    pub receipt_count: u64,
    /// currency -> (microunits, receipts contributing, receipts in scope)
    pub reported_cost: BTreeMap<String, (u64, u64, u64)>,
}

impl AgentUsageAggregate {
    pub fn from_rows(rows: &[AgentUsageRow]) -> Result<Self, LedgerError> {
        let mut aggregate = Self::default();
        for row in rows {
            aggregate.receipt_count += 1;
            aggregate.tokens.input_tokens = aggregate
                .tokens
                .input_tokens
                .checked_add(row.usage.input_tokens)
                .ok_or_else(|| LedgerError::InvalidOperation("token arithmetic overflow".into()))?;
            aggregate.tokens.output_tokens = aggregate
                .tokens
                .output_tokens
                .checked_add(row.usage.output_tokens)
                .ok_or_else(|| LedgerError::InvalidOperation("token arithmetic overflow".into()))?;
            aggregate.tokens.reasoning_tokens = aggregate
                .tokens
                .reasoning_tokens
                .checked_add(row.usage.reasoning_tokens)
                .ok_or_else(|| LedgerError::InvalidOperation("token arithmetic overflow".into()))?;
            aggregate.tokens.cache_read_tokens = aggregate
                .tokens
                .cache_read_tokens
                .checked_add(row.usage.cache_read_tokens)
                .ok_or_else(|| LedgerError::InvalidOperation("token arithmetic overflow".into()))?;
            aggregate.tokens.cache_write_tokens = aggregate
                .tokens
                .cache_write_tokens
                .checked_add(row.usage.cache_write_tokens)
                .ok_or_else(|| LedgerError::InvalidOperation("token arithmetic overflow".into()))?;
            aggregate.tokens.cache_write_5m_tokens = aggregate
                .tokens
                .cache_write_5m_tokens
                .checked_add(row.usage.cache_write_5m_tokens)
                .ok_or_else(|| LedgerError::InvalidOperation("token arithmetic overflow".into()))?;
            aggregate.tokens.cache_write_1h_tokens = aggregate
                .tokens
                .cache_write_1h_tokens
                .checked_add(row.usage.cache_write_1h_tokens)
                .ok_or_else(|| LedgerError::InvalidOperation("token arithmetic overflow".into()))?;
            let entry = aggregate
                .reported_cost
                .entry(row.currency.clone())
                .or_insert((0, 0, 0));
            entry.2 += 1;
            if let Some(cost) = row.reported_cost_microunits {
                entry.0 = entry.0.checked_add(cost).ok_or_else(|| {
                    LedgerError::InvalidOperation("economic arithmetic overflow".into())
                })?;
                entry.1 += 1;
            }
        }
        Ok(aggregate)
    }
}

#[cfg(test)]
mod tests;
