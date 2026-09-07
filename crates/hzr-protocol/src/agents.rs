//! Typed DTOs for the opt-in agtx Agent Observatory.
//!
//! Three surfaces live here and are deliberately kept apart:
//!
//! 1. the **helper wire** types (`AgentSnapshot*`), which mirror exactly what
//!    the pinned `hzr-agtx-observer` emits;
//! 2. the **public dashboard** types (`AgentBoard*`, `AgentTask*`,
//!    `AgentEconomics`), which are redacted and carry no raw identity, path or
//!    free text; and
//! 3. the **authenticated mutation** types (enrollment, session links,
//!    normalized usage import).
//!
//! A field that exists on the wire and not in the public view is not an
//! oversight: raw session ids, source task ids, worktree paths and titles are
//! correlation input, never dashboard output.

use serde::{Deserialize, Serialize};

/// Helper protocol major understood by this build.
pub const AGENT_SNAPSHOT_SCHEMA_VERSION: u32 = 1;
/// Patch identity this build will accept from the helper. Anything else is a
/// different producer, and a different producer is refused rather than trusted.
pub const AGENT_OBSERVER_PATCH_IDENTITY: &str = "hzr-agtx-readonly-observer-2";
/// Public API payload version, independent of the helper protocol.
pub const AGENT_API_SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Helper wire protocol
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentSnapshotRequest {
    pub schema_version: u32,
    pub request_id: String,
    pub data_root: String,
    pub project_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    pub task_limit: usize,
    pub edge_limit: usize,
    #[serde(default)]
    pub include_titles: bool,
    pub include_hook_status: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSnapshotCapabilities {
    #[serde(default)]
    pub tasks: bool,
    #[serde(default)]
    pub task_runtime: bool,
    #[serde(default)]
    pub notifications: bool,
    #[serde(default)]
    pub running_agents: bool,
    #[serde(default)]
    pub hook_status: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSnapshotProject {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_project_id: Option<String>,
    pub project_path_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSnapshotRuntime {
    pub phase_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_phase_status: Option<String>,
    #[serde(default)]
    pub updated_at_ms: Option<i64>,
    #[serde(default)]
    pub pane_changed_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSnapshotHook {
    pub state: String,
    pub recorded_at_ms: i64,
    #[serde(default)]
    pub agent: String,
    /// Raw agent-reported session id. Private correlation input: it is hashed
    /// before storage and never appears in a public response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub working_stale: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSnapshotTask {
    pub source_task_id: String,
    #[serde(default)]
    pub source_project_id: String,
    /// The source's own task title, when the observer was asked for titles.
    /// Bounded and control-character-free at the source; still untrusted text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub board_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_board_status: Option<String>,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub cycle: i64,
    #[serde(default)]
    pub referenced_task_ids: Vec<String>,
    #[serde(default)]
    pub has_worktree: bool,
    #[serde(default)]
    pub source_created_at_ms: Option<i64>,
    #[serde(default)]
    pub source_updated_at_ms: Option<i64>,
    #[serde(default)]
    pub runtime: Option<AgentSnapshotRuntime>,
    #[serde(default)]
    pub hook: Option<AgentSnapshotHook>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSnapshotEdge {
    pub from_source_task_id: String,
    pub to_source_task_id: String,
    pub kind: String,
    #[serde(default)]
    pub resolved: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSnapshotNotification {
    pub source_notification_id: String,
    #[serde(default)]
    pub source_task_id: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub created_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSnapshotRunningAgent {
    pub source_task_id: String,
    #[serde(default)]
    pub agent_name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub started_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSnapshotWarning {
    pub code: String,
    #[serde(default)]
    pub count: u32,
}

/// One page of one observation, exactly as the helper produced it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentSnapshotEnvelope {
    pub schema_version: u32,
    #[serde(default)]
    pub request_id: String,
    #[serde(default)]
    pub upstream_version: String,
    #[serde(default)]
    pub upstream_commit: String,
    pub patch_identity: String,
    pub source_instance_id: String,
    pub observed_at_ms: i64,
    #[serde(default)]
    pub snapshot_id: String,
    #[serde(default)]
    pub snapshot_consistency: String,
    pub project: AgentSnapshotProject,
    #[serde(default)]
    pub capabilities: AgentSnapshotCapabilities,
    #[serde(default)]
    pub tasks: Vec<AgentSnapshotTask>,
    #[serde(default)]
    pub edges: Vec<AgentSnapshotEdge>,
    #[serde(default)]
    pub notifications: Vec<AgentSnapshotNotification>,
    #[serde(default)]
    pub running_agents: Vec<AgentSnapshotRunningAgent>,
    #[serde(default)]
    pub next_cursor: Option<String>,
    #[serde(default)]
    pub complete: bool,
    #[serde(default)]
    pub warnings: Vec<AgentSnapshotWarning>,
}

/// A typed refusal from the helper.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentSnapshotFailure {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub request_id: String,
    pub error: String,
    #[serde(default)]
    pub detail: String,
}

// ---------------------------------------------------------------------------
// Observed vocabularies, kept separate on purpose
// ---------------------------------------------------------------------------

/// The board column. Distinct from runtime phase and from hook state: a task in
/// `Running` whose runtime went stale is not a confirmed live agent.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentBoardStatus {
    Backlog,
    Planning,
    Running,
    Review,
    Done,
    #[default]
    Unknown,
}

impl AgentBoardStatus {
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "backlog" => Self::Backlog,
            "planning" => Self::Planning,
            "running" => Self::Running,
            "review" => Self::Review,
            "done" => Self::Done,
            _ => Self::Unknown,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Planning => "planning",
            Self::Running => "running",
            Self::Review => "review",
            Self::Done => "done",
            Self::Unknown => "unknown",
        }
    }
}

/// The runtime phase a live agtx TUI published, if one is running.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRuntimePhase {
    Working,
    Blocked,
    Idle,
    Ready,
    Exited,
    #[default]
    Unknown,
}

impl AgentRuntimePhase {
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "working" => Self::Working,
            "blocked" => Self::Blocked,
            "idle" => Self::Idle,
            "ready" => Self::Ready,
            "exited" => Self::Exited,
            _ => Self::Unknown,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Idle => "idle",
            Self::Ready => "ready",
            Self::Exited => "exited",
            Self::Unknown => "unknown",
        }
    }
}

/// What the agent itself last reported through its lifecycle hook.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHookState {
    Working,
    Blocked,
    Waiting,
    Ended,
    #[default]
    Unknown,
}

impl AgentHookState {
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "working" => Self::Working,
            "blocked" => Self::Blocked,
            "waiting" => Self::Waiting,
            "ended" => Self::Ended,
            _ => Self::Unknown,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Waiting => "waiting",
            Self::Ended => "ended",
            Self::Unknown => "unknown",
        }
    }
}

/// How old the evidence behind a state is.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentFreshness {
    Fresh,
    Stale,
    #[default]
    Unavailable,
}

impl AgentFreshness {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Where the integration itself stands, independent of any evidence coverage.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentIntegrationState {
    #[default]
    Disabled,
    MissingComponent,
    Incompatible,
    Connecting,
    Ready,
    Partial,
    Stale,
    Error,
}

impl AgentIntegrationState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::MissingComponent => "missing_component",
            Self::Incompatible => "incompatible",
            Self::Connecting => "connecting",
            Self::Ready => "ready",
            Self::Partial => "partial",
            Self::Stale => "stale",
            Self::Error => "error",
        }
    }
}

/// Why a session link is believed. Neither value certifies provider billing.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentLinkProvenance {
    ExplicitUser,
    AgtxHook,
}

impl AgentLinkProvenance {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitUser => "explicit_user",
            Self::AgtxHook => "agtx_hook",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "explicit_user" => Some(Self::ExplicitUser),
            "agtx_hook" => Some(Self::AgtxHook),
            _ => None,
        }
    }
}

/// Observed interaction kinds. Every one requires evidence; none of them is
/// "these two agents talked to each other".
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEventKind {
    DependsOn,
    SessionLinked,
    AgentChanged,
    PhaseChanged,
    PhaseCompleted,
    TaskStuck,
    HandoffObserved,
    SnapshotGap,
    FirstObserved,
    Tombstoned,
}

impl AgentEventKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DependsOn => "depends_on",
            Self::SessionLinked => "session_linked",
            Self::AgentChanged => "agent_changed",
            Self::PhaseChanged => "phase_changed",
            Self::PhaseCompleted => "phase_completed",
            Self::TaskStuck => "task_stuck",
            Self::HandoffObserved => "handoff_observed",
            Self::SnapshotGap => "snapshot_gap",
            Self::FirstObserved => "first_observed",
            Self::Tombstoned => "tombstoned",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "depends_on" => Self::DependsOn,
            "session_linked" => Self::SessionLinked,
            "agent_changed" => Self::AgentChanged,
            "phase_changed" => Self::PhaseChanged,
            "phase_completed" => Self::PhaseCompleted,
            "task_stuck" => Self::TaskStuck,
            "handoff_observed" => Self::HandoffObserved,
            "snapshot_gap" => Self::SnapshotGap,
            "first_observed" => Self::FirstObserved,
            "tombstoned" => Self::Tombstoned,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// Public dashboard payloads
// ---------------------------------------------------------------------------

/// What was and was not observed. Every ratio here carries both halves, because
/// a percentage with no denominator is not evidence.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentCoverage {
    pub observed_tasks: u64,
    pub linked_sessions: u64,
    pub unlinked_sessions: u64,
    pub usage_covered_runs: u64,
    pub runs_total: u64,
    pub gap_count: u64,
    /// Earliest observation retained after pruning; null before the first scan.
    pub history_start_ms: Option<i64>,
    /// Null unless `runs_total` is nonzero.
    pub usage_coverage_pct: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTaskSummary {
    /// Opaque, store-private pseudonym. Resolves only inside HZR's registry.
    pub task_id: String,
    /// Short pseudonym, e.g. `Task 7ac3`. Always present, and always the
    /// identity a support conversation can safely quote.
    pub label: String,
    /// The source's own title, when the enrollment publishes titles.
    ///
    /// A board labelled only by pseudonyms is unreadable — nobody can tell
    /// which task is which — so this is on by default for a locally enrolled
    /// project and can be turned off per install. It is untrusted text and is
    /// rendered as text, never as markup.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    pub board_status: AgentBoardStatus,
    /// Present only when the source string matched no board column.
    pub unknown_board_status: Option<String>,
    pub runtime_phase: AgentRuntimePhase,
    pub runtime_freshness: AgentFreshness,
    pub runtime_observed_at_ms: Option<i64>,
    pub hook_state: AgentHookState,
    pub hook_freshness: AgentFreshness,
    pub hook_observed_at_ms: Option<i64>,
    pub agent: Option<String>,
    pub cycle: i64,
    pub first_observed_at_ms: i64,
    pub last_observed_at_ms: i64,
    pub source_updated_at_ms: Option<i64>,
    pub source_age_ms: Option<i64>,
    pub linked_session_count: u64,
    /// True when at least one linked session has an imported usage receipt.
    pub usage_covered: bool,
    pub tombstoned: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentEdgeView {
    pub from_task_id: String,
    pub to_task_id: String,
    pub kind: AgentEventKind,
    /// False when the dependency names a task that is not in the source.
    pub resolved: bool,
}

/// One enrolled project, named only by its pseudonym.
///
/// The board needs a picker, and the public API has no business handing the
/// browser a filesystem path to choose from.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentProjectRef {
    pub project_id: String,
    /// Short pseudonymous label, e.g. `Project 4f2a`.
    pub label: String,
    pub state: AgentIntegrationState,
    pub observed_tasks: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AgentBoardPage {
    pub schema_version: u32,
    pub generated_at_ms: i64,
    pub state: AgentIntegrationState,
    pub project_id: Option<String>,
    /// Every enrolled project, so the UI can offer a scope without ever seeing
    /// a path. Present on every response, including the unscoped one.
    pub projects: Vec<AgentProjectRef>,
    pub observed_since_ms: Option<i64>,
    pub source_observed_at_ms: Option<i64>,
    pub lag_ms: Option<i64>,
    pub coverage: AgentCoverage,
    pub tasks: Vec<AgentTaskSummary>,
    pub edges: Vec<AgentEdgeView>,
    /// Referenced task ids present as edge endpoints but absent from `tasks`.
    pub unresolved_task_ids: Vec<String>,
    /// True when the dependency graph contains at least one cycle.
    pub has_dependency_cycle: bool,
    pub warnings: Vec<String>,
    pub truncated: bool,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSessionLinkView {
    /// Pseudonymous session identity. Never the raw provider session id.
    pub session_id: String,
    pub host: String,
    pub provenance: AgentLinkProvenance,
    pub valid_from_ms: i64,
    pub valid_to_ms: Option<i64>,
    /// True when the same session is claimed by more than one task without an
    /// explicit non-overlapping allocation.
    pub conflict: bool,
    pub usage_receipt_count: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunView {
    pub run_id: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub host: Option<String>,
    pub observed_start_ms: i64,
    pub observed_end_ms: Option<i64>,
    pub session_linked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentEventEntry {
    /// Monotonic local ordering. Observation order, not source order.
    pub sequence: i64,
    pub task_id: Option<String>,
    pub kind: AgentEventKind,
    /// Source-reported time, when the source had one.
    pub source_at_ms: Option<i64>,
    pub observed_at_ms: i64,
    /// `observed` (HZR saw the change) or `reported` (the source said so).
    pub evidence: String,
    /// Bounded, enumerated detail: previous and next state names only.
    pub from_state: Option<String>,
    pub to_state: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentEventPage {
    pub schema_version: u32,
    pub generated_at_ms: i64,
    pub state: AgentIntegrationState,
    pub events: Vec<AgentEventEntry>,
    pub history_start_ms: Option<i64>,
    pub gap_count: u64,
    pub warnings: Vec<String>,
    pub truncated: bool,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AgentTaskDetail {
    pub schema_version: u32,
    pub generated_at_ms: i64,
    pub state: AgentIntegrationState,
    pub task: AgentTaskSummary,
    pub dependencies: Vec<AgentEdgeView>,
    pub previous_agents: Vec<String>,
    pub runs: Vec<AgentRunView>,
    pub sessions: Vec<AgentSessionLinkView>,
    pub events: Vec<AgentEventEntry>,
    pub economics: AgentEconomics,
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Economics
// ---------------------------------------------------------------------------

/// Reported token totals, by normalized dimension.
///
/// These are the same dimensions [`crate::Usage`]'s provider counterpart uses:
/// non-cached input excludes cache reads and writes, and reasoning is already
/// inside output and must never be added to it again.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTokenTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_write_5m_tokens: u64,
    pub cache_write_1h_tokens: u64,
}

/// One currency's subtotal with its own coverage. Currencies are never summed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentMoneySubtotal {
    pub currency: String,
    pub microunits: u64,
    /// Receipts contributing to this subtotal.
    pub covered_receipts: u64,
    /// Receipts in scope, whether or not they contributed.
    pub total_receipts: u64,
}

/// A metric that is unavailable, with the reason it is unavailable. Never a
/// zero standing in for "we do not know".
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentUnavailable {
    pub metric: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentEconomics {
    pub schema_version: u32,
    /// Token totals across receipts that could be attributed to this scope.
    pub reported_tokens: AgentTokenTotals,
    pub reported_receipt_count: u64,
    /// Amounts the imported source itself stated. Not verified invoices.
    pub reported_cost: Vec<AgentMoneySubtotal>,
    /// Catalog-priced estimate of the reported usage. An alternative to
    /// `reported_cost` for the same request, never additive to it.
    pub estimated_api_cost: Vec<AgentMoneySubtotal>,
    /// Identity of the current catalog used for this response. Historical
    /// estimates are recalculated; this is not a persisted pricing snapshot.
    pub price_table_identity: Option<String>,
    pub price_entry_versions: Vec<String>,
    /// Existing HZR operation accounting for the linked sessions.
    pub hzr_baseline_tokens_estimated: u64,
    pub hzr_delivered_tokens_estimated: u64,
    /// baseline − delivered. Negative values are retained, not clamped.
    pub net_avoided_tokens_estimated: i64,
    /// Null when the baseline is zero.
    pub reduction_pct: Option<f64>,
    /// Spend that could not be allocated to a single task because a session
    /// ambiguously covers several. Shown once at project scope, never doubled.
    pub shared_unallocated_cost: Vec<AgentMoneySubtotal>,
    /// Receipts that exist in the legacy provider-receipt table with no join
    /// evidence to an agent run. Excluded from every combined total above.
    pub unreconciled_existing_receipts: u64,
    pub observed_wall_time_ms: u64,
    pub observed_blocked_time_ms: u64,
    pub observed_idle_time_ms: u64,
    pub accepted_task_count: Option<u64>,
    pub cost_per_accepted_task: Vec<AgentMoneySubtotal>,
    pub unavailable: Vec<AgentUnavailable>,
}

// ---------------------------------------------------------------------------
// Normalized one-sided usage import
// ---------------------------------------------------------------------------

/// A single provider request's usage, as reported by an exporter.
///
/// Deliberately **not** [`crate::api::ProviderEconomicReceiptRequest`]'s shape:
/// that type requires a baseline/delivered pair, and inventing a baseline so a
/// single observation fits it would fabricate savings. This is one-sided by
/// design and is priced, never differenced.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentUsageReceiptV1 {
    pub schema_version: u32,
    pub receipt_id: String,
    /// Exporter identity, e.g. `fixture-export`.
    pub source: String,
    /// The exporter's own id for this record. Deduplication key, and a new
    /// `receipt_id` must never let the same one back in.
    pub source_record_id: String,
    pub observed_at_ms: u64,
    pub project_path: String,
    pub host: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub provider: String,
    pub harness: String,
    pub model: String,
    pub billing_method: String,
    pub currency: String,
    /// Actual priced-request input, not the model's context capacity. Required
    /// by catalog entries that are tiered by context size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_input_tokens: Option<u64>,
    /// Only `request_delta` is accepted in this release.
    pub usage_kind: String,
    pub usage: AgentTokenTotals,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_cost_microunits: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_source_hash: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentUsageImportRequest {
    pub schema_version: u32,
    pub source: String,
    pub receipts: Vec<AgentUsageReceiptV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentUsageRejection {
    pub source_record_id: String,
    /// Stable machine code, never the offending payload.
    pub code: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentUsageImportResponse {
    pub schema_version: u32,
    pub accepted: u64,
    pub replayed: u64,
    pub rejected: u64,
    pub conflicts: u64,
    pub rejections: Vec<AgentUsageRejection>,
    /// True only when the batch was written. A validation or conflict failure
    /// writes nothing at all.
    pub committed: bool,
}

// ---------------------------------------------------------------------------
// Authenticated control payloads
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentEnrollRequest {
    pub project_path: String,
    pub data_dir: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentDisableRequest {
    pub project_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentSyncRequest {
    pub project_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentLinkRequest {
    pub project_path: String,
    pub source_task_id: String,
    pub session_id: String,
    pub host: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentComponentStatus {
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub patch_identity: Option<String>,
    pub protocol_schema_version: Option<u32>,
    /// Set when the platform has no verified observer build.
    pub unsupported_platform: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentEnrollmentStatus {
    /// Opaque project pseudonym. The enrolled path is never returned here.
    pub project_id: String,
    pub enabled: bool,
    pub state: AgentIntegrationState,
    pub last_success_ms: Option<i64>,
    pub last_error_code: Option<String>,
    pub source_observed_at_ms: Option<i64>,
    pub lag_ms: Option<i64>,
    pub observed_tasks: u64,
    pub capabilities: AgentSnapshotCapabilities,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentsStatusResponse {
    pub schema_version: u32,
    pub generated_at_ms: i64,
    pub enabled: bool,
    pub poll_interval_ms: u64,
    pub component: AgentComponentStatus,
    pub enrollments: Vec<AgentEnrollmentStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSyncResponse {
    pub schema_version: u32,
    pub state: AgentIntegrationState,
    pub project_id: Option<String>,
    pub pages: u32,
    pub tasks_observed: u64,
    pub events_recorded: u64,
    pub tombstoned: u64,
    pub complete: bool,
    pub warnings: Vec<String>,
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentLinkResponse {
    pub schema_version: u32,
    pub linked: bool,
    pub conflict: bool,
    pub task_id: Option<String>,
    pub session_id: Option<String>,
    pub code: Option<String>,
}
