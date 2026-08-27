use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{CodecProfile, ContextPack, FidelityClass, RiskClass, Usage};

fn default_search_limit() -> usize {
    10
}

fn default_memory_limit() -> usize {
    5
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    Exact,
    Semantic,
    #[default]
    Auto,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchStrategy {
    ForkRgaiAdaptive,
    ForkRgaiBuiltin,
    ForkRgaiGrepai,
    ForkRgaiRipgrep,
    ForkRgaiFiles,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchFallbackCode {
    LegacyIndexRequiresMigration,
    SemanticIndexUnavailable,
    GrepaiUnavailable,
    RipgrepUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchLine {
    pub line: u32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchSnippet {
    pub lines: Vec<SearchLine>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_terms: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub path: String,
    pub score: f64,
    pub matched_lines: usize,
    pub snippets: Vec<SearchSnippet>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchApiResponse {
    pub query: String,
    pub path: String,
    pub total_hits: usize,
    pub shown_hits: usize,
    pub scanned_files: usize,
    pub skipped_large: usize,
    pub skipped_binary: usize,
    pub hits: Vec<SearchHit>,
    pub effective_mode: SearchMode,
    pub strategy: SearchStrategy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_code: Option<SearchFallbackCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_generation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchApiRequest {
    pub workspace: String,
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
    #[serde(default)]
    pub mode: SearchMode,
    #[serde(default)]
    pub include_content: bool,
}

/// Authenticated, non-accounted probe used by `hzr doctor` to verify that the managed
/// semantic-search runtime can consume the active fork configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SemanticReadinessApiRequest {
    pub workspace: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SemanticReadinessApiResponse {
    pub ready: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContextPlanApiRequest {
    pub workspace: String,
    pub intent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default = "default_search_limit")]
    pub search_limit: usize,
    #[serde(default = "default_memory_limit")]
    pub memory_limit: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextWarningCode {
    PlannerFallback,
    PlannerUnavailable,
    SearchDegraded,
    SearchUnavailable,
    MemoryUnavailable,
    ContentUnavailable,
    OutlineUnavailable,
    WarningsTruncated,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForkPlannerMetadata {
    pub pipeline_version: Option<String>,
    pub semantic_backend_used: Option<String>,
    pub graph_candidate_count: Option<usize>,
    pub semantic_hit_count: Option<usize>,
    pub candidates_total: usize,
    pub candidates_selected: usize,
    pub estimated_tokens_used: u64,
    pub token_budget: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContextWarning {
    pub code: ContextWarningCode,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextPlanApiResponse {
    pub pack: ContextPack,
    pub contents: BTreeMap<String, String>,
    pub warnings: Vec<ContextWarning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner: Option<ForkPlannerMetadata>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecallApiRequest {
    pub workspace: String,
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
    /// Which namespaces the recall may reach. Defaults to project plus global so an
    /// agent sees standing preferences alongside this repository's history.
    #[serde(default)]
    pub scope: MemoryScopeSelector,
}

/// Wire form of the memory namespace. Kept in the protocol rather than inferred, so a
/// client's scope choice is explicit and auditable in the request.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScopeSelector {
    /// Only the current repository.
    Project,
    /// Only user-global records.
    Global,
    #[default]
    ProjectAndGlobal,
}

/// A write targets exactly one namespace — `project_and_global` is meaningless for a
/// store, and allowing it would duplicate the same fact into two namespaces.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryWriteScope {
    #[default]
    Project,
    Global,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryImportance {
    Critical,
    High,
    #[default]
    Medium,
    Low,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryStoreApiRequest {
    pub workspace: String,
    pub topic: String,
    pub content: String,
    /// Where the record belongs. Project by default: a fact is repository-scoped unless
    /// the caller deliberately states it is a user-wide preference or rule.
    #[serde(default)]
    pub scope: MemoryWriteScope,
    #[serde(default)]
    pub importance: MemoryImportance,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryForgetApiRequest {
    pub workspace: String,
    pub id: String,
    #[serde(default)]
    pub scope: MemoryWriteScope,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryUpdateApiRequest {
    pub workspace: String,
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub scope: MemoryWriteScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importance: Option<MemoryImportance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryPruneApiRequest {
    pub workspace: String,
    pub threshold: f32,
    #[serde(default = "default_true")]
    pub dry_run: bool,
    #[serde(default)]
    pub scope: MemoryWriteScope,
}

const fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryMutationApiResponse {
    pub affected_ids: Vec<String>,
    pub dry_run: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecApiRequest {
    pub cwd: String,
    pub command: String,
    /// The public `HZR_RAW_FIDELITY=1 hzr exec run ...` marker, transported separately from
    /// the command so the daemon never executes a recursive HZR wrapper.
    #[serde(default)]
    pub fidelity_requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fidelity_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The host's standing execution grant, when the harness reported one.
    ///
    /// Without this the daemon re-derives a verdict the host already answered, and a command the
    /// `PreToolUse` hook approved can be refused by the `hzr exec run` that approval launched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_execution_grant: Option<HostExecutionGrant>,
}

/// The harness's own permission posture for the current session.
///
/// A closed enum rather than a string: policy must never branch on an unrecognised value, and an
/// unknown mode has to fail into "no grant" instead of into whatever a substring match returns.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostPermissionMode {
    Default,
    AcceptEdits,
    Plan,
    BypassPermissions,
}

impl HostPermissionMode {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        [
            Self::Default,
            Self::AcceptEdits,
            Self::Plan,
            Self::BypassPermissions,
        ]
        .into_iter()
        .find(|candidate| value.eq_ignore_ascii_case(candidate.as_str()))
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::Plan => "plan",
            Self::BypassPermissions => "bypassPermissions",
        }
    }

    /// Whether the operator has already decided that commands run without prompting.
    #[must_use]
    pub const fn grants_execution(self) -> bool {
        matches!(self, Self::BypassPermissions)
    }
}

/// An output class a filter may never drop, whatever it does to the rest of the stream.
///
/// "Command families need explicit completeness contracts" was documented prose and therefore
/// unenforceable: nothing failed if a filter quietly started swallowing a compiler error. These
/// variants are the closed vocabulary that turns the sentence into a checkable artifact.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MustKeep {
    /// The child's exit status, unaltered — a filter that reports success for a failed run is
    /// worse than no filter at all.
    ExitStatus,
    /// Every failure line: a failing test, a compiler error, a non-zero diagnostic.
    Failures,
    /// Warnings. Droppable-looking and not droppable: a warning ratchet is a real gate.
    Warnings,
    /// The list of files a command changed, which is the whole result of a write-shaped command.
    ChangedFiles,
}

impl MustKeep {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExitStatus => "exit_status",
            Self::Failures => "failures",
            Self::Warnings => "warnings",
            Self::ChangedFiles => "changed_files",
        }
    }
}

/// What one filtered output route promises to preserve.
///
/// Serialize-only: the table is compiled-in truth, so a deserialized contract would be a claim
/// from outside the binary about what the binary guarantees.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct CompletenessContract {
    /// The filtered route, as an agent names it.
    pub route: &'static str,
    /// Whether the route runs a child process.
    ///
    /// `log` filters a stream that someone else produced, so it has no exit status to preserve.
    /// Collapsing this into `carries_failure` would demand a promise it cannot keep, and a
    /// contract nobody can satisfy gets weakened rather than met.
    pub spawns_child: bool,
    /// Whether this route carries failure semantics at all.
    ///
    /// Content-selection routes (`read`, `search`) have no notion of a failing child; holding them
    /// to a failure contract would be theatre. They are bound instead by `raw_pointer_required`.
    pub carries_failure: bool,
    /// Undroppable by rule.
    pub must_keep: &'static [MustKeep],
    /// Whether everything outside `must_keep` may be summarized only alongside an explicit
    /// pointer back to the unfiltered output.
    pub raw_pointer_required: bool,
}

/// Every filtered route HZR owns, with the completeness it guarantees.
///
/// The table is exhaustive on purpose: a new filtered route that reaches an agent without an
/// entry here fails `acceptance_gate_every_filtered_route_declares_its_completeness`, so the
/// contract cannot be an afterthought written down later.
pub const COMPLETENESS_CONTRACTS: &[CompletenessContract] = &[
    CompletenessContract {
        route: "test",
        spawns_child: true,
        carries_failure: true,
        must_keep: &[MustKeep::ExitStatus, MustKeep::Failures, MustKeep::Warnings],
        raw_pointer_required: true,
    },
    CompletenessContract {
        route: "err",
        spawns_child: true,
        carries_failure: true,
        must_keep: &[MustKeep::ExitStatus, MustKeep::Failures],
        raw_pointer_required: true,
    },
    CompletenessContract {
        route: "summary",
        spawns_child: true,
        carries_failure: true,
        must_keep: &[MustKeep::ExitStatus, MustKeep::Failures],
        raw_pointer_required: true,
    },
    CompletenessContract {
        route: "log",
        spawns_child: false,
        carries_failure: true,
        must_keep: &[MustKeep::Failures, MustKeep::Warnings],
        raw_pointer_required: true,
    },
    CompletenessContract {
        route: "build",
        spawns_child: true,
        carries_failure: true,
        must_keep: &[
            MustKeep::ExitStatus,
            MustKeep::Failures,
            MustKeep::Warnings,
            MustKeep::ChangedFiles,
        ],
        raw_pointer_required: true,
    },
    CompletenessContract {
        route: "write",
        spawns_child: true,
        carries_failure: true,
        // A patch whose hunk does not match is a failure, and a write that reports only the files
        // it touched would present a refused edit as a completed one.
        must_keep: &[
            MustKeep::ExitStatus,
            MustKeep::Failures,
            MustKeep::ChangedFiles,
        ],
        raw_pointer_required: false,
    },
    CompletenessContract {
        route: "read",
        spawns_child: false,
        carries_failure: false,
        must_keep: &[],
        raw_pointer_required: true,
    },
    CompletenessContract {
        route: "search",
        spawns_child: false,
        carries_failure: false,
        must_keep: &[],
        raw_pointer_required: true,
    },
];

#[must_use]
pub fn completeness_contract(route: &str) -> Option<&'static CompletenessContract> {
    COMPLETENESS_CONTRACTS
        .iter()
        .find(|contract| contract.route == route)
}

#[cfg(test)]
mod completeness_contract_tests {
    use super::{COMPLETENESS_CONTRACTS, MustKeep, completeness_contract};

    /// Every filtered route an agent can reach declares what it will not drop.
    ///
    /// The list is written here rather than derived from the table, so adding a route to the table
    /// is not enough to satisfy the gate — and shipping a filter without an entry fails it. That
    /// asymmetry is the point: the contract has to be a decision, not a side effect.
    #[test]
    fn acceptance_gate_every_filtered_route_declares_its_completeness() {
        for route in [
            "test", "err", "summary", "log", "build", "write", "read", "search",
        ] {
            let contract = completeness_contract(route).unwrap_or_else(|| {
                unreachable!("filtered route `{route}` ships without a contract")
            });
            assert_eq!(contract.route, route);
        }
        assert_eq!(
            COMPLETENESS_CONTRACTS.len(),
            8,
            "a route was added to the table without being added to this gate"
        );
    }

    /// A route that can fail must promise to say so.
    ///
    /// Exit status and failure lines are the two things a filter can drop that turn a red run
    /// green. Content-selection routes are exempt from the failure clause and bound instead by an
    /// explicit pointer back to unfiltered output, so summarizing can never be a dead end.
    #[test]
    fn acceptance_gate_failure_capable_routes_cannot_drop_failures_or_status() {
        for contract in COMPLETENESS_CONTRACTS {
            if contract.carries_failure {
                assert_eq!(
                    contract.must_keep.contains(&MustKeep::ExitStatus),
                    contract.spawns_child,
                    "`{}` must promise the child's exit status exactly when it runs one",
                    contract.route
                );
                assert!(
                    contract.must_keep.contains(&MustKeep::Failures),
                    "`{}` may swallow failure lines",
                    contract.route
                );
            } else {
                assert!(
                    contract.raw_pointer_required,
                    "`{}` summarizes without promising a route back to the raw output",
                    contract.route
                );
                assert!(
                    contract.must_keep.is_empty(),
                    "`{}` declares failure obligations while claiming no failure semantics",
                    contract.route
                );
            }
        }
    }
}

/// Where a filter is allowed to fire, relative to the model's turn structure.
///
/// This exists because output reduction and provider cost are not the same axis. A harness that
/// caches the request prefix bills a cached read far below a fresh one; a filter that fires in the
/// middle of a turn rewrites content the prefix already contains, invalidating the cached span
/// after it. The delivered-byte saving is real and the *billed* input can still rise.
///
/// Making placement an explicit policy dimension is what lets that be measured instead of
/// assumed. `Anywhere` is the shipped behaviour and stays the default; `TurnBoundary` is the
/// arm a paired billed-input benchmark compares it against.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterPlacement {
    /// Filter wherever the route applies. Maximum delivered-byte reduction, prefix may move.
    #[default]
    Anywhere,
    /// Filter only on the first operation of a turn, leaving a mid-turn prefix untouched.
    TurnBoundary,
}

impl FilterPlacement {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anywhere => "anywhere",
            Self::TurnBoundary => "turn_boundary",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        [Self::Anywhere, Self::TurnBoundary]
            .into_iter()
            .find(|candidate| value.eq_ignore_ascii_case(candidate.as_str()))
    }

    /// Whether a filter may fire at this position in the turn under this policy.
    #[must_use]
    pub const fn permits(self, at_turn_boundary: bool) -> bool {
        match self {
            Self::Anywhere => true,
            Self::TurnBoundary => at_turn_boundary,
        }
    }
}

/// Environment variable carrying a grant to every descendant of an approved command.
pub const HOST_EXECUTION_GRANT_ENV: &str = "HZR_HOST_EXECUTION_GRANT";

/// How long a grant stays usable.
///
/// Long enough to cover a working session, short enough that a value left in an exported shell,
/// a `.env`, or a committed script stops being an approval. A grant is a record of an answer the
/// operator gave a while ago, not a permanent capability.
pub const HOST_EXECUTION_GRANT_MAX_AGE_MS: u64 = 12 * 60 * 60 * 1000;

/// Tolerance for a clock that runs slightly ahead of the process reading the grant.
pub const HOST_EXECUTION_GRANT_MAX_FUTURE_SKEW_MS: u64 = 5 * 60 * 1000;

/// One host decision, carried to every process that would otherwise re-decide it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostExecutionGrant {
    pub mode: HostPermissionMode,
    /// Keyed digest of the session the host granted. Never a raw session identifier.
    pub granted_for_session: String,
    pub granted_at_ms: u64,
    /// Which surface observed the host decision, for audit.
    pub source: String,
}

/// Why a grant was not honoured. Every variant is a refusal to trust, never a downgrade to allow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostGrantRejection {
    /// The mode does not grant execution (`default`, `plan`, `acceptEdits`).
    ModeDoesNotGrantExecution,
    /// The grant names a different session than the one now executing.
    SessionMismatch,
    /// The grant is older than `HOST_EXECUTION_GRANT_MAX_AGE_MS`.
    Expired,
    /// The grant is stamped further in the future than the allowed clock skew.
    FutureTimestamp,
}

impl HostGrantRejection {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ModeDoesNotGrantExecution => "mode_does_not_grant_execution",
            Self::SessionMismatch => "session_mismatch",
            Self::Expired => "expired",
            Self::FutureTimestamp => "future_timestamp",
        }
    }
}

impl HostExecutionGrant {
    /// Whether this grant may stand in for a fresh host answer, right now, for this session.
    ///
    /// Fail-closed on every axis. A grant that cannot be tied to the running session, or that has
    /// aged out, is ignored — it never degrades into a weaker-but-still-permissive verdict,
    /// because a stale approval that still approves is indistinguishable from no policy at all.
    pub fn authorize(
        &self,
        session_digest: Option<&str>,
        now_ms: u64,
    ) -> Result<(), HostGrantRejection> {
        if !self.mode.grants_execution() {
            return Err(HostGrantRejection::ModeDoesNotGrantExecution);
        }
        if session_digest != Some(self.granted_for_session.as_str()) {
            return Err(HostGrantRejection::SessionMismatch);
        }
        if self.granted_at_ms > now_ms.saturating_add(HOST_EXECUTION_GRANT_MAX_FUTURE_SKEW_MS) {
            return Err(HostGrantRejection::FutureTimestamp);
        }
        if now_ms.saturating_sub(self.granted_at_ms) > HOST_EXECUTION_GRANT_MAX_AGE_MS {
            return Err(HostGrantRejection::Expired);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecApprovalApiRequest {
    pub decision_id: String,
    pub approved: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FidelityUnknownResolution {
    AcknowledgeExecuted,
    ProveNotExecuted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FidelityReconcileApiRequest {
    pub reservation_id: String,
    pub resolution: FidelityUnknownResolution,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FidelityReconcileReceipt {
    pub schema_version: u32,
    pub reservation_id: String,
    pub resolution: FidelityUnknownResolution,
    pub operation_recorded: bool,
    pub allowance_released: bool,
    pub cleanup_complete: bool,
    pub idempotent_replay: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForkRunApiRequest {
    pub cwd: String,
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_write: Option<ForkManagedWrite>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum ForkManagedWrite {
    Patch {
        path: String,
        old: String,
        new: String,
    },
    Create {
        path: String,
        content: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandTermination {
    Exited,
    Signaled,
    TimedOut,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForkRunApiResponse {
    pub stdout: String,
    pub stderr: String,
    pub termination: CommandTermination,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub duration_ms: u64,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UsageApiRequest {
    pub trace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub usage: Usage,
    pub turns: u32,
    #[serde(default)]
    pub retries: u32,
    pub latency_ms: u64,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_microusd: Option<u64>,
    /// Канонический workspace root; отсутствует у legacy-чеков — они остаются глобальными.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UsageApiResponse {
    pub recorded: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingChannel {
    HookCli,
    Mcp,
    NativeHost,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingMeasurement {
    Estimated,
    Unmeasured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingRoute {
    Optimized,
    Bypassed,
    NativeUnaccounted,
}

/// Non-sensitive operation family recorded independently from command payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingOperationKind {
    Search,
    Read,
    Write,
    Context,
    Memory,
    Codec,
    Exec,
    Observability,
    Doctor,
}

impl AccountingOperationKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Read => "read",
            Self::Write => "write",
            Self::Context => "context",
            Self::Memory => "memory",
            Self::Codec => "codec",
            Self::Exec => "exec",
            Self::Observability => "observability",
            Self::Doctor => "doctor",
        }
    }
}

/// Requested operation mode. Family-prefixed variants remain unambiguous when persisted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingOperationMode {
    SearchAuto,
    SearchSemantic,
    SearchExact,
    SearchBuiltin,
    ReadFull,
    ReadFiltered,
    ReadRange,
    ReadHead,
    ReadTail,
    ReadOutline,
    ReadSymbols,
    ReadChanged,
    ReadSince,
    Write,
    ContextPlan,
    MemoryRecall,
    MemoryStore,
    MemoryForget,
    MemoryUpdate,
    MemoryPrune,
    CodecCompile,
    ExecRun,
    ObservabilitySnapshot,
    DoctorCheck,
}

impl AccountingOperationMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SearchAuto => "search_auto",
            Self::SearchSemantic => "search_semantic",
            Self::SearchExact => "search_exact",
            Self::SearchBuiltin => "search_builtin",
            Self::ReadFull => "read_full",
            Self::ReadFiltered => "read_filtered",
            Self::ReadRange => "read_range",
            Self::ReadHead => "read_head",
            Self::ReadTail => "read_tail",
            Self::ReadOutline => "read_outline",
            Self::ReadSymbols => "read_symbols",
            Self::ReadChanged => "read_changed",
            Self::ReadSince => "read_since",
            Self::Write => "write",
            Self::ContextPlan => "context_plan",
            Self::MemoryRecall => "memory_recall",
            Self::MemoryStore => "memory_store",
            Self::MemoryForget => "memory_forget",
            Self::MemoryUpdate => "memory_update",
            Self::MemoryPrune => "memory_prune",
            Self::CodecCompile => "codec_compile",
            Self::ExecRun => "exec_run",
            Self::ObservabilitySnapshot => "observability_snapshot",
            Self::DoctorCheck => "doctor_check",
        }
    }

    #[must_use]
    pub const fn operation(self) -> AccountingOperationKind {
        match self {
            Self::SearchAuto | Self::SearchSemantic | Self::SearchExact | Self::SearchBuiltin => {
                AccountingOperationKind::Search
            }
            Self::ReadFull
            | Self::ReadFiltered
            | Self::ReadRange
            | Self::ReadHead
            | Self::ReadTail
            | Self::ReadOutline
            | Self::ReadSymbols
            | Self::ReadChanged
            | Self::ReadSince => AccountingOperationKind::Read,
            Self::Write => AccountingOperationKind::Write,
            Self::ContextPlan => AccountingOperationKind::Context,
            Self::MemoryRecall
            | Self::MemoryStore
            | Self::MemoryForget
            | Self::MemoryUpdate
            | Self::MemoryPrune => AccountingOperationKind::Memory,
            Self::CodecCompile => AccountingOperationKind::Codec,
            Self::ExecRun => AccountingOperationKind::Exec,
            Self::ObservabilitySnapshot => AccountingOperationKind::Observability,
            Self::DoctorCheck => AccountingOperationKind::Doctor,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingStage {
    InternalTransport,
    FinalDelivery,
    StandaloneDelivery,
    ControlPlane,
}

impl AccountingStage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InternalTransport => "internal_transport",
            Self::FinalDelivery => "final_delivery",
            Self::StandaloneDelivery => "standalone_delivery",
            Self::ControlPlane => "control_plane",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingFilterLevel {
    None,
    Minimal,
    Aggressive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingSearchStrategy {
    ForkRgaiAdaptive,
    ForkRgaiBuiltin,
    ForkRgaiGrepai,
    ForkRgaiRipgrep,
    ForkRgaiFiles,
}

impl AccountingSearchStrategy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ForkRgaiAdaptive => "fork_rgai_adaptive",
            Self::ForkRgaiBuiltin => "fork_rgai_builtin",
            Self::ForkRgaiGrepai => "fork_rgai_grepai",
            Self::ForkRgaiRipgrep => "fork_rgai_ripgrep",
            Self::ForkRgaiFiles => "fork_rgai_files",
        }
    }
}

impl SearchFallbackCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LegacyIndexRequiresMigration => "legacy_index_requires_migration",
            Self::SemanticIndexUnavailable => "semantic_index_unavailable",
            Self::GrepaiUnavailable => "grepai_unavailable",
            Self::RipgrepUnavailable => "ripgrep_unavailable",
        }
    }
}

impl AccountingFilterLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Aggressive => "aggressive",
        }
    }
}

/// Typed observability dimensions only. Query text, paths, file contents, and secrets are
/// deliberately absent from this transport contract.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccountingAttribution {
    pub operation: AccountingOperationKind,
    /// Backward-compatible canonical mode. New search producers set this to the effective mode.
    pub mode: AccountingOperationMode,
    pub stage: AccountingStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_mode: Option<AccountingOperationMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_mode: Option<AccountingOperationMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_strategy: Option<AccountingSearchStrategy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_fallback_code: Option<SearchFallbackCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_content: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_scope_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_level: Option<AccountingFilterLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evasion: Option<EvasionAttribution>,
}

/// Closed, privacy-safe taxonomy for command-routing evasion. These values are suitable for
/// persistence and IPC because they describe only the construct, never its payload.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvasionClass {
    E1QuotedCoveredCommand,
    E2ShellWrapper,
    E3InterpreterRead,
    E4ExecutablePath,
    E5PipelineOrRedirect,
    E6NestedUnboundedReader,
    E7FidelityHatch,
    E8NativeTool,
    E9DiagnosticBypass,
    E10CapabilityGap,
    E11PrivilegedPrefix,
}

impl EvasionClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::E1QuotedCoveredCommand => "e1",
            Self::E2ShellWrapper => "e2",
            Self::E3InterpreterRead => "e3",
            Self::E4ExecutablePath => "e4",
            Self::E5PipelineOrRedirect => "e5",
            Self::E6NestedUnboundedReader => "e6",
            Self::E7FidelityHatch => "e7",
            Self::E8NativeTool => "e8",
            Self::E9DiagnosticBypass => "e9",
            Self::E10CapabilityGap => "e10",
            Self::E11PrivilegedPrefix => "e11",
        }
    }

    /// Short human-readable name of the construct, for agent-facing policy messages.
    #[must_use]
    pub const fn construct(self) -> &'static str {
        match self {
            Self::E1QuotedCoveredCommand => "quoted covered command",
            Self::E2ShellWrapper => "shell wrapper",
            Self::E3InterpreterRead => "interpreter file read",
            Self::E4ExecutablePath => "executable path form",
            Self::E5PipelineOrRedirect => "pipeline or redirect",
            Self::E6NestedUnboundedReader => "nested unbounded reader",
            Self::E7FidelityHatch => "raw fidelity request",
            Self::E8NativeTool => "host-native file tool",
            Self::E9DiagnosticBypass => "direct HZR diagnostic access",
            Self::E10CapabilityGap => "no first-class route",
            Self::E11PrivilegedPrefix => "privilege elevation prefix",
        }
    }

    /// The route an agent should reach for instead.
    ///
    /// An Ask that does not say what to run instead costs a turn and teaches nothing, so every
    /// class carries its prescription here rather than leaving the caller to invent one. E10 is
    /// deliberately not a correction: no route exists, so approval is the honest outcome.
    #[must_use]
    pub const fn prescription(self) -> &'static str {
        match self {
            Self::E1QuotedCoveredCommand | Self::E2ShellWrapper | Self::E4ExecutablePath => {
                "re-issue the inner command directly so HZR can route it"
            }
            Self::E3InterpreterRead => {
                "read files with `hzr read <file>` and scan them with `hzr search '<pattern>'`; \
                 keep the interpreter for computation that has no managed route"
            }
            Self::E5PipelineOrRedirect => {
                "run each stage through its own managed route, for example \
                 `hzr read <file> --max-lines N` instead of piping a full read into a limiter"
            }
            Self::E6NestedUnboundedReader => {
                "read the matched files with `hzr read <file>` instead of embedding an unbounded \
                 reader in the command"
            }
            Self::E7FidelityHatch => {
                "supply a fidelity reason that matches the command, or use the bounded managed \
                 route when exact bytes are not required"
            }
            Self::E8NativeTool => "use the equivalent `hzr` file operation",
            Self::E9DiagnosticBypass => {
                "use `hzr stats` (add `--since` or `--workspace`) instead of reading HZR state \
                 directly; this is an HZR policy decision surfaced through the harness approval \
                 channel, not a harness permission"
            }
            Self::E10CapabilityGap => {
                "no managed route covers this command; approving runs it as tracked raw"
            }
            Self::E11PrivilegedPrefix => {
                "elevation was granted to one binary; HZR stays out of it and runs the command \
                 verbatim — drop the elevation to get a managed route"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvasionInterpreter {
    Shell,
    Python,
    Javascript,
    Ruby,
    Perl,
    Awk,
    Sed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvasionPathForm {
    Bare,
    AbsoluteSystem,
    Relative,
    ResolvedAlias,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementTier {
    T0TransparentRewrite,
    T1NamedCorrection,
    T2DenyWithPrescription,
    T3BudgetExhaustion,
    T4HatchQuarantine,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FidelityReason {
    Binary,
    Checksum,
    MachineProtocol,
    CompleteLog,
    FullPatch,
    VerbatimSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FidelityValidation {
    NotRequested,
    Valid,
    MissingReason,
    InvalidReason,
    Contradicted,
    ProvenEquivalent,
    BudgetExhausted,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    Ask,
    Deny,
    Correction,
}

impl PolicyDecision {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Deny => "deny",
            Self::Correction => "correction",
        }
    }
}

impl EnforcementTier {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::T0TransparentRewrite => "t0",
            Self::T1NamedCorrection => "t1",
            Self::T2DenyWithPrescription => "t2",
            Self::T3BudgetExhaustion => "t3",
            Self::T4HatchQuarantine => "t4",
        }
    }
}

impl FidelityReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::Checksum => "checksum",
            Self::MachineProtocol => "machine_protocol",
            Self::CompleteLog => "complete_log",
            Self::FullPatch => "full_patch",
            Self::VerbatimSource => "verbatim_source",
        }
    }
}

impl FidelityValidation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotRequested => "not_requested",
            Self::Valid => "valid",
            Self::MissingReason => "missing_reason",
            Self::InvalidReason => "invalid_reason",
            Self::Contradicted => "contradicted",
            Self::ProvenEquivalent => "proven_equivalent",
            Self::BudgetExhausted => "budget_exhausted",
        }
    }
}

impl EvasionInterpreter {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Python => "python",
            Self::Javascript => "javascript",
            Self::Ruby => "ruby",
            Self::Perl => "perl",
            Self::Awk => "awk",
            Self::Sed => "sed",
        }
    }
}

impl EvasionPathForm {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bare => "bare",
            Self::AbsoluteSystem => "absolute_system",
            Self::Relative => "relative",
            Self::ResolvedAlias => "resolved_alias",
        }
    }
}

/// Payload-free accounting evidence. Producers may populate this after the shared normalizer;
/// consumers can aggregate it without learning the original command, path, query, or content.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvasionAttribution {
    pub class: EvasionClass,
    pub wrapper_depth: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpreter: Option<EvasionInterpreter>,
    pub path_form: EvasionPathForm,
    pub stage_count: u16,
    pub hatch_marker: bool,
    pub avoidable: bool,
    pub tier: EnforcementTier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fidelity_reason: Option<FidelityReason>,
    pub fidelity_validation: FidelityValidation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationApiRequest {
    pub original_command: String,
    pub recorded_command: String,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub execution_ms: u64,
    pub project_path: String,
    pub channel: AccountingChannel,
    pub measurement: AccountingMeasurement,
    pub route: AccountingRoute,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<AccountingAttribution>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationApiResponse {
    pub recorded: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardState {
    Ready,
    Degraded,
    Rebuilding,
    Standby,
    Stopped,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardService {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub state: DashboardState,
    pub detail: String,
    pub command: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardProjectState {
    Ready,
    Warming,
    Registered,
    Unavailable,
    Degraded,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardProjectArtifacts {
    pub config_present: bool,
    pub vectors_present: bool,
    pub symbols_present: bool,
    pub repository_graph_present: bool,
    pub size_bytes: u64,
    pub modified_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardProject {
    pub name: String,
    pub root: String,
    pub repository_id: String,
    pub worktree_id: String,
    pub git_backed: bool,
    pub linked_worktree: bool,
    pub state: DashboardProjectState,
    pub registered_at_ms: u64,
    pub last_seen_at_ms: u64,
    pub artifacts: DashboardProjectArtifacts,
    pub command: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardProjectPage {
    pub projects: Vec<DashboardProject>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub next_offset: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardObservedUsage {
    pub tasks: u64,
    pub accepted: u64,
    pub actual_input_tokens: u64,
    pub actual_output_tokens: u64,
    pub estimated_input_tokens: u64,
    pub cost_microusd: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DashboardEstimatedEfficiency {
    pub accounting_policy_version: String,
    pub excluded_legacy_operations: u64,
    pub operations: u64,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub gross_avoided_tokens_estimated: u64,
    pub regression_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub reduction_pct: f64,
    pub total_execution_ms: u64,
    pub measurement: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardMemoryRetrieval {
    Hybrid,
    Fts5,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DashboardMemoryTopic {
    pub id: String,
    pub label: String,
    pub memory_count: usize,
    pub average_weight: f64,
    pub newest_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardMemoryEdge {
    pub source: String,
    pub target: String,
    pub relationship_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DashboardMemoryObservatory {
    pub state: DashboardState,
    pub project: Option<String>,
    pub retrieval: DashboardMemoryRetrieval,
    pub observed_at_ms: u64,
    pub latency_ms: u64,
    pub transport: String,
    pub source: String,
    pub memory_count: usize,
    pub visible_memory_count: usize,
    pub hidden_memory_count: usize,
    pub topics: Vec<DashboardMemoryTopic>,
    pub edges: Vec<DashboardMemoryEdge>,
    pub truncated: bool,
    pub diagnostic_command: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DashboardMemoryTopicDetails {
    pub id: String,
    pub label: String,
    pub memory_count: usize,
    pub visible_memory_count: usize,
    pub hidden_memory_count: usize,
    pub truncated: bool,
    pub memories: Vec<DashboardMemoryDetail>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DashboardMemoryDetail {
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

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardIndexArtifacts {
    pub initialized: bool,
    pub vectors_present: bool,
    pub symbols_present: bool,
    pub repository_graph_present: bool,
    pub size_bytes: u64,
    pub modified_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardIndexWatcher {
    pub state: DashboardState,
    pub pid: Option<u32>,
    pub uptime_ms: Option<u64>,
    pub owned_by_hzr: bool,
    pub ready_marker_observed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardSearchActivity {
    pub state: DashboardState,
    pub ledger_id: Option<u64>,
    pub observed_at: Option<String>,
    pub operation: Option<String>,
    pub command_hash: Option<String>,
    pub project_hash: Option<String>,
    pub agent: Option<String>,
    pub session_hash: Option<String>,
    pub route: Option<DashboardOperationRoute>,
    pub execution_ms: Option<u64>,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardIndexObservatory {
    pub state: DashboardState,
    pub project: Option<String>,
    pub observed_at_ms: u64,
    pub generation: Option<String>,
    pub config_fingerprint: Option<String>,
    pub artifacts: DashboardIndexArtifacts,
    pub watcher: DashboardIndexWatcher,
    pub search_activity: DashboardSearchActivity,
    pub diagnostic_command: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardLocalActivity {
    pub project: Option<String>,
    pub accounting_policy_version: String,
    pub excluded_legacy_operations: u64,
    pub operations: u64,
    pub optimized_operations: u64,
    pub raw_operations: u64,
    pub native_unaccounted_operations: u64,
    pub unmeasured_bypass_operations: u64,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub gross_avoided_tokens_estimated: u64,
    pub regression_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub total_execution_ms: u64,
    pub first_record_at: Option<String>,
    pub last_record_at: Option<String>,
    pub unscoped_operations: u64,
    pub measurement: String,
    pub recent_operations: Vec<DashboardLocalOperation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardOperationRoute {
    Optimized,
    Raw,
    NativeUnaccounted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardLocalOperation {
    pub ledger_id: u64,
    pub timestamp: String,
    pub operation: String,
    pub route: DashboardOperationRoute,
    pub command_hash: String,
    pub project_hash: String,
    pub agent: Option<String>,
    pub session_hash: Option<String>,
    pub producer_version: Option<String>,
    pub policy_version: Option<String>,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub execution_ms: u64,
    pub replacement: Option<String>,
    pub rationale: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardTraceStage {
    Request,
    Policy,
    Engine,
    Ledger,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardTraceState {
    Completed,
    ApprovalRequired,
    Denied,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardTraceSpan {
    pub sequence: u64,
    pub trace_hash: String,
    pub linked_trace_hash: Option<String>,
    pub span_id: u64,
    pub parent_span_id: Option<u64>,
    pub stage: DashboardTraceStage,
    pub state: DashboardTraceState,
    pub engine: String,
    pub observed_at_ms: u64,
    pub duration_ms: u64,
    pub project_hash: Option<String>,
    pub session_hash: Option<String>,
    pub route: Option<String>,
    pub error_code: Option<String>,
    pub producer_version: String,
    pub policy_version: String,
    pub generation: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardLifecycleKind {
    Starting,
    Ready,
    Degraded,
    RestartScheduled,
    Reaped,
    Stopped,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardLifecycleEvent {
    pub sequence: u64,
    pub observed_at_ms: u64,
    pub engine: String,
    pub kind: DashboardLifecycleKind,
    pub project_hash: Option<String>,
    pub detail_code: String,
    pub producer_version: String,
    pub generation: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardObservability {
    pub trace_spans: Vec<DashboardTraceSpan>,
    pub lifecycle_events: Vec<DashboardLifecycleEvent>,
    pub next_cursor: Option<u64>,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardProviderReceiptState {
    Available,
    NoReceipts,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardProviderReceipts {
    pub state: DashboardProviderReceiptState,
    pub records: u64,
    pub accepted: u64,
    pub actual_input_tokens: u64,
    pub actual_output_tokens: u64,
    pub cost_microusd: u64,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardEconomicAmount {
    pub currency: String,
    pub baseline_microunits: u64,
    pub delivered_microunits: u64,
    pub savings_microunits: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardRawPublicEstimate {
    pub currency: String,
    pub savings_microunits: u64,
    pub avoided_input_tokens_estimated: u64,
    pub pricing_basis: String,
    pub catalog_identity: String,
    pub entry_version: String,
    pub preliminary: bool,
    pub disclaimer: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardSessionCommand {
    pub command_family: String,
    pub executions: u64,
    pub net_avoided_tokens_estimated: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardSessionRoi {
    pub session_hash: Option<String>,
    pub operations: u64,
    pub baseline_tokens_estimated: u64,
    pub delivered_tokens_estimated: u64,
    pub net_avoided_tokens_estimated: i64,
    pub top_commands: Vec<DashboardSessionCommand>,
    pub selected_harness: String,
    pub selected_provider: String,
    pub selected_model: String,
    pub selected_method: String,
    pub selected_request_input_tokens: Option<u64>,
    pub selected_pricing_basis: String,
    pub catalog_identity: Option<String>,
    pub raw_public_estimate: Option<DashboardRawPublicEstimate>,
    pub raw_public_estimate_unavailable_reason: Option<String>,
    pub imported_claim_records: u64,
    pub reported_actual: Option<DashboardEconomicAmount>,
    pub receipt_provenance: Option<String>,
    pub receipt_externally_verified: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DashboardHelpCommand {
    pub label: String,
    pub description: String,
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DashboardResponse {
    pub protocol_version: u16,
    pub hzr_version: String,
    pub visualizer_version: String,
    pub generated_at_ms: u64,
    pub uptime_ms: u64,
    pub daemon_endpoint: String,
    pub overall_state: DashboardState,
    pub services: Vec<DashboardService>,
    pub projects: Vec<DashboardProject>,
    pub projects_total: usize,
    pub projects_next_offset: Option<usize>,
    pub selected_worktree_id: Option<String>,
    pub registry_warnings: usize,
    pub observed_usage: DashboardObservedUsage,
    pub estimated_efficiency: DashboardEstimatedEfficiency,
    pub memory_observatory: DashboardMemoryObservatory,
    pub index_observatory: DashboardIndexObservatory,
    pub local_activity: DashboardLocalActivity,
    pub observability: DashboardObservability,
    pub provider_receipts: DashboardProviderReceipts,
    pub session_roi: DashboardSessionRoi,
    pub help: Vec<DashboardHelpCommand>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodecApiRequest {
    pub content: String,
    #[serde(default)]
    pub project_path: String,
    #[serde(default)]
    pub fidelity: FidelityClass,
    #[serde(default)]
    pub risk: RiskClass,
    #[serde(default)]
    pub profile: CodecProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<AccountingChannel>,
}

#[cfg(test)]
mod tests {
    use super::{
        AccountingAttribution, ContextPlanApiRequest, ExecApiRequest, MemoryImportance,
        MemoryPruneApiRequest, MemoryRecallApiRequest, MemoryStoreApiRequest, OperationApiRequest,
        SearchApiRequest, SearchMode,
    };

    #[test]
    fn test_search_defaults_are_bounded_and_auto() {
        let request: SearchApiRequest =
            serde_json::from_str(r#"{"workspace":"/repo","query":"authentication flow"}"#)
                .expect("search request parses");

        assert_eq!(request.limit, 10);
        assert_eq!(request.mode, SearchMode::Auto);
        assert!(!request.include_content);
        assert_eq!(MemoryImportance::default(), MemoryImportance::Medium);
    }

    #[test]
    fn test_context_plan_defaults_bound_both_retrieval_sources() {
        let request: ContextPlanApiRequest =
            serde_json::from_str(r#"{"workspace":"/repo","intent":"authentication flow"}"#)
                .expect("context plan request parses");

        assert_eq!(request.search_limit, 10);
        assert_eq!(request.memory_limit, 5);
        assert!(request.path.is_none());
        assert!(request.topic.is_none());
    }

    #[test]
    fn test_memory_requests_require_workspace_and_reject_project_override() {
        assert!(serde_json::from_str::<MemoryRecallApiRequest>(r#"{"query":"decision"}"#).is_err());
        assert!(
            serde_json::from_str::<MemoryStoreApiRequest>(
                r#"{"topic":"context","content":"decision"}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<MemoryRecallApiRequest>(
                r#"{"workspace":"/repo","query":"decision","project":"foreign"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn test_memory_prune_defaults_to_preview() {
        let request: MemoryPruneApiRequest =
            serde_json::from_str(r#"{"workspace":"/repo","threshold":0.1}"#)
                .expect("memory prune request parses");

        assert!(request.dry_run);
    }

    #[test]
    fn exec_request_accepts_legacy_payloads_and_carries_caller_path() {
        let legacy: ExecApiRequest =
            serde_json::from_str(r#"{"cwd":"/repo","command":"cargo test"}"#)
                .expect("legacy exec request parses");
        assert!(legacy.caller_path.is_none());
        assert!(!legacy.fidelity_requested);
        assert!(legacy.fidelity_reason.is_none());

        let request = ExecApiRequest {
            cwd: "/repo".into(),
            command: "cargo test".into(),
            fidelity_requested: true,
            fidelity_reason: Some("checksum".into()),
            timeout_ms: Some(1_000),
            caller_path: Some("/toolchain/bin:/usr/bin".into()),
            agent: Some("cli".into()),
            session_id: None,
            host_execution_grant: None,
        };
        let value = serde_json::to_value(request).expect("exec request serializes");
        assert_eq!(value["caller_path"], "/toolchain/bin:/usr/bin");
        assert_eq!(value["fidelity_requested"], true);
        assert_eq!(value["fidelity_reason"], "checksum");
    }

    #[test]
    fn operation_request_accepts_legacy_payload_without_attribution() {
        let request: OperationApiRequest = serde_json::from_str(
            r#"{"original_command":"hzr search","recorded_command":"hzr search","baseline_tokens_estimated":1,"delivered_tokens_estimated":1,"execution_ms":0,"project_path":"/repo","channel":"mcp","measurement":"estimated","route":"optimized"}"#,
        )
        .expect("legacy operation request parses");
        assert!(request.attribution.is_none());
    }

    #[test]
    fn acceptance_gate_operation_attribution_rejects_free_form_dimensions() {
        for request in [
            r#"{"operation":"read","mode":"read_filtered","stage":"internal_transport","filter_level":"secret=value"}"#,
            r#"{"operation":"search","mode":"search_exact","stage":"final_delivery","search_strategy":"private/path"}"#,
            r#"{"operation":"search","mode":"search_exact","stage":"final_delivery","search_fallback_code":"token=secret"}"#,
        ] {
            assert!(serde_json::from_str::<AccountingAttribution>(request).is_err());
        }
    }
}
