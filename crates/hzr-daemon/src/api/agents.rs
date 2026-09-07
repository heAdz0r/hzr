//! HTTP surface for the opt-in agtx Agent Observatory.
//!
//! The split is the point. Public GETs read durable projections and return
//! redacted pseudonyms: no filesystem path is accepted, no raw session id or
//! source title is returned, and none of them can spawn the helper, touch the
//! agtx store or write to the ledger. Everything that changes state —
//! enrollment reload, session links, usage import, a forced sync — sits behind
//! the daemon's existing bearer authentication.

use axum::Json;
use axum::extract::{Path as AxumPath, Query, State};
use hzr_core::{
    AgentEconomicsQuery, AgentSourceIdentity, Ledger, agent_task_label, load_pricing_catalog,
    privacy_identity_hash,
};
use hzr_protocol::agents::{
    AGENT_API_SCHEMA_VERSION, AgentBoardPage, AgentBoardStatus, AgentCoverage, AgentEconomics,
    AgentEdgeView, AgentEnrollmentStatus, AgentEventEntry, AgentEventKind, AgentEventPage,
    AgentFreshness, AgentHookState, AgentIntegrationState, AgentLinkProvenance, AgentLinkRequest,
    AgentLinkResponse, AgentProjectRef, AgentRunView, AgentRuntimePhase, AgentSessionLinkView,
    AgentSyncRequest, AgentSyncResponse, AgentTaskDetail, AgentTaskSummary,
    AgentUsageImportRequest, AgentUsageImportResponse, AgentUsageRejection, AgentsStatusResponse,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::agent_observer::now_ms;
use crate::error::ApiError;
use crate::state::AppState;

const DEFAULT_TASK_LIMIT: usize = 50;
const MAX_TASK_LIMIT: usize = 200;
const DEFAULT_EVENT_LIMIT: usize = 100;
const MAX_EVENT_LIMIT: usize = 500;
const MAX_GRAPH_EDGES: usize = 1_000;
const MAX_WINDOW_MS: i64 = 31 * 24 * 60 * 60 * 1_000;
/// Import batches are bounded independently of the daemon's global body limit.
const MAX_IMPORT_BYTES: usize = 1_048_576;

#[derive(Debug, Deserialize)]
pub struct AgentBoardQuery {
    project_id: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AgentEventQuery {
    project_id: Option<String>,
    task_id: Option<String>,
    cursor: Option<i64>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct AgentEconomicsParams {
    project_id: Option<String>,
    task_id: Option<String>,
    from_ms: Option<i64>,
    to_ms: Option<i64>,
}

/// An opaque id is `sha256:<64 hex>` or a bare 64-hex digest. Anything else is
/// a malformed request, never a path to try opening.
fn valid_opaque_id(value: &str) -> bool {
    let digest = value.strip_prefix("sha256:").unwrap_or(value);
    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Resolve an opaque project id inside the enrolled set only.
///
/// A project HZR is not monitoring is a 404 whether or not it exists on disk;
/// the registry, not the filesystem, decides what this API can name.
async fn resolve_project(
    state: &AppState,
    project_id: Option<&str>,
) -> Result<Option<(String, PathBuf)>, ApiError> {
    let Some(project_id) = project_id else {
        return Ok(None);
    };
    if !valid_opaque_id(project_id) {
        return Err(ApiError::bad_request(
            "project_id must be an opaque sha256 identity",
        ));
    }
    let enrollments = state.agents.enrollments().await;
    for project in &enrollments.projects {
        let hash = privacy_identity_hash("project", &project.project_path.to_string_lossy());
        if hash == project_id {
            return Ok(Some((hash, project.project_path.clone())));
        }
    }
    Ok(None)
}

fn ledger_path(state: &AppState) -> PathBuf {
    state.config.data_dir.join("ledger/hzr.sqlite")
}

fn open_agent_ledger(state: &AppState) -> Result<Option<Ledger>, ApiError> {
    Ledger::agents_read_only(&ledger_path(state))
        .map_err(|error| ApiError::internal(format!("agent projection read failed: {error}")))
}

fn freshness(observed_at_ms: Option<i64>, now: i64, stale_after_ms: u64) -> AgentFreshness {
    match observed_at_ms {
        None => AgentFreshness::Unavailable,
        Some(observed) if now.saturating_sub(observed) <= stale_after_ms as i64 => {
            AgentFreshness::Fresh
        }
        Some(_) => AgentFreshness::Stale,
    }
}

fn empty_board(
    state: AgentIntegrationState,
    project_id: Option<String>,
    projects: Vec<AgentProjectRef>,
) -> AgentBoardPage {
    AgentBoardPage {
        schema_version: AGENT_API_SCHEMA_VERSION,
        generated_at_ms: now_ms(),
        state,
        project_id,
        projects,
        observed_since_ms: None,
        source_observed_at_ms: None,
        lag_ms: None,
        coverage: AgentCoverage::default(),
        tasks: Vec::new(),
        edges: Vec::new(),
        unresolved_task_ids: Vec::new(),
        has_dependency_cycle: false,
        warnings: Vec::new(),
        truncated: false,
        next_cursor: None,
    }
}

/// How a task is named on the public surface.
#[derive(Clone, Copy)]
struct TaskNaming {
    /// False returns the board to pseudonyms only.
    publish_titles: bool,
}

fn task_summary(
    row: &hzr_core::AgentTaskRow,
    now: i64,
    stale_after_ms: u64,
    linked_session_count: u64,
    usage_covered: bool,
    naming: TaskNaming,
) -> AgentTaskSummary {
    AgentTaskSummary {
        task_id: row.task_key.clone(),
        label: agent_task_label(&row.task_key),
        // The pseudonym is always present; the title is what makes the board
        // readable, and it is withheld when the install asks for that.
        title: naming.publish_titles.then(|| row.title.clone()).flatten(),
        branch: naming.publish_titles.then(|| row.branch.clone()).flatten(),
        board_status: AgentBoardStatus::parse(&row.board_status),
        unknown_board_status: row.unknown_board_status.clone(),
        runtime_phase: AgentRuntimePhase::parse(&row.runtime_phase),
        runtime_freshness: freshness(row.runtime_observed_at_ms, now, stale_after_ms),
        runtime_observed_at_ms: row.runtime_observed_at_ms,
        hook_state: AgentHookState::parse(&row.hook_state),
        // An upstream `Working` record past its stale guard is stale evidence
        // however recently HZR read it. `Blocked` never decays.
        hook_freshness: if row.hook_working_stale {
            AgentFreshness::Stale
        } else {
            freshness(row.hook_observed_at_ms, now, stale_after_ms)
        },
        hook_observed_at_ms: row.hook_observed_at_ms,
        agent: (!row.agent.is_empty()).then(|| row.agent.clone()),
        cycle: row.cycle,
        first_observed_at_ms: row.first_observed_at_ms,
        last_observed_at_ms: row.last_observed_at_ms,
        source_updated_at_ms: row.source_updated_at_ms,
        source_age_ms: row
            .source_updated_at_ms
            .map(|updated| now.saturating_sub(updated)),
        linked_session_count,
        usage_covered,
        tombstoned: row.tombstoned,
    }
}

fn event_entry(row: &hzr_core::AgentEventRow) -> AgentEventEntry {
    AgentEventEntry {
        sequence: row.sequence,
        task_id: row.task_key.clone(),
        kind: AgentEventKind::parse(&row.kind).unwrap_or(AgentEventKind::SnapshotGap),
        source_at_ms: row.source_at_ms,
        observed_at_ms: row.observed_at_ms,
        evidence: row.evidence.clone(),
        from_state: row.from_state.clone(),
        to_state: row.to_state.clone(),
    }
}

/// Detect a cycle in the observed dependency edges.
///
/// Reported, not repaired: a cycle is the source's business, and HZR must keep
/// rendering the graph rather than looping or silently dropping an edge.
fn has_cycle(edges: &[hzr_core::AgentEdgeRow]) -> bool {
    let mut outgoing: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in edges {
        outgoing
            .entry(edge.from_task_key.as_str())
            .or_default()
            .push(edge.to_task_key.as_str());
    }
    let mut visiting: BTreeSet<&str> = BTreeSet::new();
    let mut done: BTreeSet<&str> = BTreeSet::new();
    let nodes: Vec<&str> = outgoing.keys().copied().collect();
    for node in nodes {
        if walk(node, &outgoing, &mut visiting, &mut done) {
            return true;
        }
    }
    false
}

fn walk<'a>(
    node: &'a str,
    outgoing: &BTreeMap<&'a str, Vec<&'a str>>,
    visiting: &mut BTreeSet<&'a str>,
    done: &mut BTreeSet<&'a str>,
) -> bool {
    if done.contains(node) {
        return false;
    }
    if !visiting.insert(node) {
        return true;
    }
    for next in outgoing.get(node).into_iter().flatten() {
        if walk(next, outgoing, visiting, done) {
            return true;
        }
    }
    visiting.remove(node);
    done.insert(node);
    false
}

/// Enrolled projects and their integration state, resolved before any ledger
/// handle is taken.
///
/// Split from the counting pass on purpose: a `Ledger` holds a SQLite
/// connection, which is `Send` but not `Sync`, so a borrow of one may not be
/// held across an `await`.
async fn enrolled_project_states(state: &AppState) -> Vec<(String, AgentIntegrationState)> {
    let enrollments = state.agents.enrollments().await;
    let mut entries = Vec::with_capacity(enrollments.projects.len());
    for project in &enrollments.projects {
        let project_hash =
            privacy_identity_hash("project", &project.project_path.to_string_lossy());
        let integration = state.agents.state_for(&project.project_path).await;
        entries.push((project_hash, integration));
    }
    entries
}

/// Every enrolled project as an opaque reference the browser may select.
fn project_refs(
    entries: &[(String, AgentIntegrationState)],
    ledger: Option<&Ledger>,
) -> Result<Vec<AgentProjectRef>, ApiError> {
    let mut refs = Vec::with_capacity(entries.len());
    for (project_hash, integration) in entries {
        let observed_tasks = match ledger {
            Some(ledger) => {
                ledger
                    .agent_coverage_counts(project_hash)
                    .map_err(|error| {
                        ApiError::internal(format!("agent coverage read failed: {error}"))
                    })?
                    .observed_tasks
            }
            None => 0,
        };
        refs.push(AgentProjectRef {
            label: agent_task_label(project_hash).replace("Task", "Project"),
            project_id: project_hash.clone(),
            state: *integration,
            observed_tasks,
        });
    }
    Ok(refs)
}

/// `GET /v1/dashboard/agents`
pub async fn dashboard_agents(
    Query(query): Query<AgentBoardQuery>,
    State(state): State<AppState>,
) -> Result<Json<AgentBoardPage>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_TASK_LIMIT);
    if limit == 0 || limit > MAX_TASK_LIMIT {
        return Err(ApiError::bad_request(format!(
            "limit must be between 1 and {MAX_TASK_LIMIT}"
        )));
    }
    if query
        .cursor
        .as_deref()
        .is_some_and(|cursor| !valid_opaque_id(cursor))
    {
        return Err(ApiError::bad_request("cursor is not a valid page token"));
    }
    let entries = enrolled_project_states(&state).await;
    let Some((project_hash, project_path)) =
        resolve_project(&state, query.project_id.as_deref()).await?
    else {
        // No scope given, or a scope HZR does not monitor. A missing project is
        // a 404; an absent filter returns the picker, not a fleet dump.
        if query.project_id.is_some() {
            return Err(ApiError::not_found(
                "unknown_project",
                "no enrolled agtx project matches that identity",
            ));
        }
        let ledger = open_agent_ledger(&state)?;
        let projects = project_refs(&entries, ledger.as_ref())?;
        let overall = projects
            .first()
            .map_or(AgentIntegrationState::Disabled, |project| project.state);
        return Ok(Json(empty_board(overall, None, projects)));
    };

    let integration_state = state.agents.state_for(&project_path).await;
    let Some(ledger) = open_agent_ledger(&state)? else {
        let projects = project_refs(&entries, None)?;
        return Ok(Json(empty_board(
            integration_state,
            Some(project_hash),
            projects,
        )));
    };
    let projects = project_refs(&entries, Some(&ledger))?;

    let now = now_ms();
    let settings = state.agents.enrollments().await;
    let stale_after_ms = settings.stale_after_ms;
    let naming = TaskNaming {
        publish_titles: settings.publish_task_titles,
    };
    let (rows, next_cursor) = ledger
        .agent_tasks_page(&project_hash, query.cursor.as_deref(), limit)
        .map_err(|error| ApiError::internal(format!("agent task page failed: {error}")))?;
    let edges = ledger
        .agent_edges_for_project(&project_hash, MAX_GRAPH_EDGES)
        .map_err(|error| ApiError::internal(format!("agent edge read failed: {error}")))?;
    let counts = ledger
        .agent_coverage_counts(&project_hash)
        .map_err(|error| ApiError::internal(format!("agent coverage read failed: {error}")))?;

    let mut tasks = Vec::with_capacity(rows.len());
    for row in &rows {
        let links = ledger
            .agent_links_for_task(&row.task_key)
            .map_err(|error| ApiError::internal(format!("agent link read failed: {error}")))?;
        let usage_covered = links.iter().any(|link| link.usage_receipt_count > 0);
        tasks.push(task_summary(
            row,
            now,
            stale_after_ms,
            links.len() as u64,
            usage_covered,
            naming,
        ));
    }

    let known: BTreeSet<&str> = rows.iter().map(|row| row.task_key.as_str()).collect();
    let unresolved: Vec<String> = edges
        .iter()
        .filter(|edge| !edge.resolved || !known.contains(edge.from_task_key.as_str()))
        .map(|edge| edge.from_task_key.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let sources = ledger
        .agent_sources_for_project(&project_hash)
        .map_err(|error| ApiError::internal(format!("agent source read failed: {error}")))?;
    let source_observed_at_ms = sources
        .iter()
        .filter_map(|source| source.last_observed_at_ms)
        .max();
    let runs_total = counts
        .linked_sessions
        .saturating_add(counts.ambiguous_sessions);

    Ok(Json(AgentBoardPage {
        schema_version: AGENT_API_SCHEMA_VERSION,
        generated_at_ms: now,
        state: integration_state,
        project_id: Some(project_hash),
        projects,
        observed_since_ms: sources
            .iter()
            .map(|source| source.first_observed_at_ms)
            .min(),
        source_observed_at_ms,
        lag_ms: source_observed_at_ms.map(|observed| now.saturating_sub(observed)),
        coverage: AgentCoverage {
            observed_tasks: counts.observed_tasks,
            linked_sessions: counts.linked_sessions,
            unlinked_sessions: counts.ambiguous_sessions,
            usage_covered_runs: counts.covered_sessions,
            runs_total,
            gap_count: counts.gaps,
            history_start_ms: counts.history_start_ms,
            // A percentage needs a denominator; without runs there is none.
            usage_coverage_pct: (runs_total > 0)
                .then(|| 100.0 * counts.covered_sessions as f64 / runs_total as f64),
        },
        edges: edges
            .iter()
            .map(|edge| AgentEdgeView {
                from_task_id: edge.from_task_key.clone(),
                to_task_id: edge.to_task_key.clone(),
                kind: AgentEventKind::DependsOn,
                resolved: edge.resolved,
            })
            .collect(),
        has_dependency_cycle: has_cycle(&edges),
        unresolved_task_ids: unresolved,
        tasks,
        warnings: sources
            .iter()
            .filter_map(|source| source.last_error_code.clone())
            .collect(),
        truncated: next_cursor.is_some(),
        next_cursor,
    }))
}

/// `GET /v1/dashboard/agents/tasks/{task_id}`
pub async fn dashboard_agent_task(
    AxumPath(task_id): AxumPath<String>,
    State(state): State<AppState>,
) -> Result<Json<AgentTaskDetail>, ApiError> {
    if !valid_opaque_id(&task_id) {
        return Err(ApiError::bad_request(
            "task_id must be an opaque sha256 identity",
        ));
    }
    let Some(ledger) = open_agent_ledger(&state)? else {
        return Err(ApiError::not_found("unknown_task", "no such observed task"));
    };
    let Some(row) = ledger
        .agent_task(&task_id)
        .map_err(|error| ApiError::internal(format!("agent task read failed: {error}")))?
    else {
        return Err(ApiError::not_found("unknown_task", "no such observed task"));
    };
    // The projection may outlive an enrollment; a task outside the currently
    // enrolled set is out of scope for this API, not merely empty.
    let Some((_, project_path)) = resolve_project(&state, Some(&row.project_hash)).await? else {
        return Err(ApiError::not_found("unknown_task", "no such observed task"));
    };

    let now = now_ms();
    let settings = state.agents.enrollments().await;
    let stale_after_ms = settings.stale_after_ms;
    let naming = TaskNaming {
        publish_titles: settings.publish_task_titles,
    };
    let links = ledger
        .agent_links_for_task(&task_id)
        .map_err(|error| ApiError::internal(format!("agent link read failed: {error}")))?;
    let usage_covered = links.iter().any(|link| link.usage_receipt_count > 0);
    let (events, _) = ledger
        .agent_events_page(&row.project_hash, Some(&task_id), None, DEFAULT_EVENT_LIMIT)
        .map_err(|error| ApiError::internal(format!("agent event read failed: {error}")))?;
    let dependencies = ledger
        .agent_edges_for_project(&row.project_hash, MAX_GRAPH_EDGES)
        .map_err(|error| ApiError::internal(format!("agent edge read failed: {error}")))?
        .into_iter()
        .filter(|edge| edge.to_task_key == task_id || edge.from_task_key == task_id)
        .map(|edge| AgentEdgeView {
            from_task_id: edge.from_task_key,
            to_task_id: edge.to_task_key,
            kind: AgentEventKind::DependsOn,
            resolved: edge.resolved,
        })
        .collect();

    let catalog = load_pricing_catalog(state.config.billing.pricing_file.as_deref())
        .map_err(|error| ApiError::internal(format!("pricing catalog unavailable: {error}")))?;
    let economics = ledger
        .agent_economics(
            AgentEconomicsQuery {
                project_hash: &row.project_hash,
                task_key: Some(&task_id),
                from_ms: 0,
                to_ms: i64::MAX,
            },
            &catalog,
        )
        .map_err(|error| ApiError::internal(format!("agent economics failed: {error}")))?;

    Ok(Json(AgentTaskDetail {
        schema_version: AGENT_API_SCHEMA_VERSION,
        generated_at_ms: now,
        state: state.agents.state_for(&project_path).await,
        task: task_summary(
            &row,
            now,
            stale_after_ms,
            links.len() as u64,
            usage_covered,
            naming,
        ),
        dependencies,
        previous_agents: ledger
            .agent_task_agent_history(&task_id)
            .map_err(|error| ApiError::internal(format!("agent history read failed: {error}")))?,
        runs: links
            .iter()
            .map(|link| AgentRunView {
                run_id: link.session_hash.clone(),
                agent: Some(link.host.clone()),
                model: None,
                host: Some(link.host.clone()),
                observed_start_ms: link.valid_from_ms,
                observed_end_ms: link.valid_to_ms,
                session_linked: !link.conflict,
            })
            .collect(),
        sessions: links
            .iter()
            .map(|link| AgentSessionLinkView {
                session_id: link.session_hash.clone(),
                host: link.host.clone(),
                provenance: AgentLinkProvenance::parse(&link.provenance)
                    .unwrap_or(AgentLinkProvenance::AgtxHook),
                valid_from_ms: link.valid_from_ms,
                valid_to_ms: link.valid_to_ms,
                conflict: link.conflict,
                usage_receipt_count: link.usage_receipt_count,
            })
            .collect(),
        events: events.iter().map(event_entry).collect(),
        economics,
        warnings: Vec::new(),
    }))
}

/// `GET /v1/dashboard/agents/events`
pub async fn dashboard_agent_events(
    Query(query): Query<AgentEventQuery>,
    State(state): State<AppState>,
) -> Result<Json<AgentEventPage>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_EVENT_LIMIT);
    if limit == 0 || limit > MAX_EVENT_LIMIT {
        return Err(ApiError::bad_request(format!(
            "limit must be between 1 and {MAX_EVENT_LIMIT}"
        )));
    }
    let Some((project_hash, project_path)) =
        resolve_project(&state, query.project_id.as_deref()).await?
    else {
        return Err(ApiError::not_found(
            "unknown_project",
            "no enrolled agtx project matches that identity",
        ));
    };
    if query
        .task_id
        .as_deref()
        .is_some_and(|task| !valid_opaque_id(task))
    {
        return Err(ApiError::bad_request("task_id must be an opaque identity"));
    }
    let integration_state = state.agents.state_for(&project_path).await;
    let Some(ledger) = open_agent_ledger(&state)? else {
        return Ok(Json(AgentEventPage {
            schema_version: AGENT_API_SCHEMA_VERSION,
            generated_at_ms: now_ms(),
            state: integration_state,
            events: Vec::new(),
            history_start_ms: None,
            gap_count: 0,
            warnings: Vec::new(),
            truncated: false,
            next_cursor: None,
        }));
    };
    let (rows, next) = ledger
        .agent_events_page(&project_hash, query.task_id.as_deref(), query.cursor, limit)
        .map_err(|error| ApiError::internal(format!("agent event read failed: {error}")))?;
    let counts = ledger
        .agent_coverage_counts(&project_hash)
        .map_err(|error| ApiError::internal(format!("agent coverage read failed: {error}")))?;
    Ok(Json(AgentEventPage {
        schema_version: AGENT_API_SCHEMA_VERSION,
        generated_at_ms: now_ms(),
        state: integration_state,
        events: rows.iter().map(event_entry).collect(),
        history_start_ms: counts.history_start_ms,
        gap_count: counts.gaps,
        warnings: Vec::new(),
        truncated: next.is_some(),
        next_cursor: next.map(|sequence| sequence.to_string()),
    }))
}

/// `GET /v1/dashboard/agents/economics`
pub async fn dashboard_agent_economics(
    Query(query): Query<AgentEconomicsParams>,
    State(state): State<AppState>,
) -> Result<Json<AgentEconomics>, ApiError> {
    let Some((project_hash, _)) = resolve_project(&state, query.project_id.as_deref()).await?
    else {
        return Err(ApiError::not_found(
            "unknown_project",
            "no enrolled agtx project matches that identity",
        ));
    };
    let to_ms = query.to_ms.unwrap_or_else(now_ms);
    let from_ms = query.from_ms.unwrap_or(0);
    if from_ms < 0 || to_ms < from_ms {
        return Err(ApiError::bad_request("from_ms must not exceed to_ms"));
    }
    if from_ms > 0 && to_ms.saturating_sub(from_ms) > MAX_WINDOW_MS {
        return Err(ApiError::bad_request("window may not exceed 31 days"));
    }
    if query
        .task_id
        .as_deref()
        .is_some_and(|task| !valid_opaque_id(task))
    {
        return Err(ApiError::bad_request("task_id must be an opaque identity"));
    }
    let Some(ledger) = open_agent_ledger(&state)? else {
        return Ok(Json(AgentEconomics {
            schema_version: AGENT_API_SCHEMA_VERSION,
            ..Default::default()
        }));
    };
    let catalog = load_pricing_catalog(state.config.billing.pricing_file.as_deref())
        .map_err(|error| ApiError::internal(format!("pricing catalog unavailable: {error}")))?;
    let economics = ledger
        .agent_economics(
            AgentEconomicsQuery {
                project_hash: &project_hash,
                task_key: query.task_id.as_deref(),
                from_ms,
                to_ms,
            },
            &catalog,
        )
        .map_err(|error| ApiError::internal(format!("agent economics failed: {error}")))?;
    Ok(Json(economics))
}

// ---------------------------------------------------------------------------
// Authenticated control
// ---------------------------------------------------------------------------

/// `GET /v1/agents/status`
pub async fn agents_status(
    State(state): State<AppState>,
) -> Result<Json<AgentsStatusResponse>, ApiError> {
    let component = state.agents.refresh_component().await;
    let enrollments = state.agents.enrollments().await;
    let runtime = state.agents.runtime_state().await;
    let ledger = open_agent_ledger(&state)?;

    let mut rows = Vec::new();
    for project in &enrollments.projects {
        let project_hash =
            privacy_identity_hash("project", &project.project_path.to_string_lossy());
        let live = runtime.get(&project.project_path);
        let sources = ledger
            .as_ref()
            .map(|ledger| ledger.agent_sources_for_project(&project_hash))
            .transpose()
            .map_err(|error| ApiError::internal(format!("agent source read failed: {error}")))?
            .unwrap_or_default();
        let observed_tasks = ledger
            .as_ref()
            .map(|ledger| ledger.agent_coverage_counts(&project_hash))
            .transpose()
            .map_err(|error| ApiError::internal(format!("agent coverage read failed: {error}")))?
            .map_or(0, |counts| counts.observed_tasks);
        let source_observed_at_ms = sources
            .iter()
            .filter_map(|source| source.last_observed_at_ms)
            .max();
        rows.push(AgentEnrollmentStatus {
            project_id: project_hash,
            enabled: project.enabled && enrollments.enabled,
            state: state.agents.state_for(&project.project_path).await,
            last_success_ms: live.and_then(|live| live.last_success_ms),
            last_error_code: live.and_then(|live| live.last_error_code.clone()),
            source_observed_at_ms,
            lag_ms: source_observed_at_ms.map(|observed| now_ms().saturating_sub(observed)),
            observed_tasks,
            capabilities: sources
                .first()
                .map(|source| source.capabilities.clone())
                .unwrap_or_default(),
        });
    }

    Ok(Json(AgentsStatusResponse {
        schema_version: AGENT_API_SCHEMA_VERSION,
        generated_at_ms: now_ms(),
        enabled: enrollments.enabled,
        poll_interval_ms: enrollments.poll_interval_ms,
        component: component.to_status(),
        enrollments: rows,
    }))
}

/// `POST /v1/agents/reload`
///
/// Re-reads the daemon's own configuration file and adopts the enrollment set.
/// Takes no body and no path: the CLI writes the config atomically and this
/// makes the running daemon honour it without a restart. Every in-flight
/// observation from the previous generation is discarded.
pub async fn agents_reload(
    State(state): State<AppState>,
) -> Result<Json<AgentsStatusResponse>, ApiError> {
    let paths = hzr_core::ConfigPaths::discover();
    let config = hzr_core::Config::load_or_default(&paths.config_file)
        .map_err(|error| ApiError::bad_request(format!("configuration is not usable: {error}")))?;
    state
        .agents
        .set_enrollments(config.integrations.agtx.clone())
        .await;
    state.agents.refresh_component().await;
    state
        .ensure_agent_worker(config.integrations.agtx.enabled)
        .await;
    agents_status(State(state)).await
}

/// `POST /v1/agents/sync`
pub async fn agents_sync(
    State(state): State<AppState>,
    Json(request): Json<AgentSyncRequest>,
) -> Result<Json<AgentSyncResponse>, ApiError> {
    let path = PathBuf::from(&request.project_path);
    if !path.is_absolute() {
        return Err(ApiError::bad_request("project_path must be absolute"));
    }
    let enrollments = state.agents.enrollments().await;
    if enrollments.find(&path).is_none() {
        return Err(ApiError::not_found(
            "unknown_project",
            "that project is not enrolled for agtx monitoring",
        ));
    }
    let outcome = state.agents.observe_once(&path).await;
    Ok(Json(AgentSyncResponse {
        schema_version: AGENT_API_SCHEMA_VERSION,
        state: outcome.state,
        project_id: outcome.project_id,
        pages: outcome.pages,
        tasks_observed: outcome.tasks_observed,
        events_recorded: outcome.events_recorded,
        tombstoned: outcome.tombstoned,
        complete: outcome.complete,
        warnings: outcome.warnings,
        error_code: outcome.error_code,
    }))
}

/// `POST /v1/agents/links`
pub async fn agents_link(
    State(state): State<AppState>,
    Json(request): Json<AgentLinkRequest>,
) -> Result<Json<AgentLinkResponse>, ApiError> {
    let path = PathBuf::from(&request.project_path);
    if !path.is_absolute() {
        return Err(ApiError::bad_request("project_path must be absolute"));
    }
    for (name, value, limit) in [
        ("source_task_id", request.source_task_id.as_str(), 256),
        ("session_id", request.session_id.as_str(), 512),
        ("host", request.host.as_str(), 64),
    ] {
        if value.is_empty() || value.len() > limit || !value.is_ascii() {
            return Err(ApiError::bad_request(format!("invalid {name}")));
        }
    }
    let enrollments = state.agents.enrollments().await;
    let Some(project) = enrollments.find(&path).cloned() else {
        return Err(ApiError::not_found(
            "unknown_project",
            "that project is not enrolled for agtx monitoring",
        ));
    };
    let project_hash = privacy_identity_hash("project", &path.to_string_lossy());
    let Some(ledger) = open_agent_ledger(&state)? else {
        return Ok(Json(AgentLinkResponse {
            schema_version: AGENT_API_SCHEMA_VERSION,
            linked: false,
            conflict: false,
            task_id: None,
            session_id: None,
            code: Some("no_observations_yet".into()),
        }));
    };
    // The task's own generation is the one this link belongs to. Linking into a
    // generation that no longer exists would attach spending to a history the
    // source has already replaced.
    let Some(source) = ledger
        .agent_sources_for_project(&project_hash)
        .map_err(|error| ApiError::internal(format!("agent source read failed: {error}")))?
        .into_iter()
        .next()
    else {
        return Ok(Json(AgentLinkResponse {
            schema_version: AGENT_API_SCHEMA_VERSION,
            linked: false,
            conflict: false,
            task_id: None,
            session_id: None,
            code: Some("no_observations_yet".into()),
        }));
    };
    let identity = AgentSourceIdentity::new(
        &crate::agent_observer::enrollment_id(&project),
        &source.source_generation,
        &project_hash,
    );
    let session_hash = state.ledger.privacy_pseudonymizer().hash(
        "session",
        &format!("{}\u{0}{}", request.host, request.session_id),
    );

    // Resolve the task through the stored projection: a caller names the
    // source's task id, and only the projection knows which source project id
    // completes its key.
    let Some((task_key, source_project_id)) = ledger
        .agent_task_by_source_id(&identity.source_key, &request.source_task_id)
        .map_err(|error| ApiError::internal(format!("agent task lookup failed: {error}")))?
    else {
        return Ok(Json(AgentLinkResponse {
            schema_version: AGENT_API_SCHEMA_VERSION,
            linked: false,
            conflict: false,
            task_id: None,
            session_id: None,
            code: Some("unknown_task".into()),
        }));
    };

    let (linked, conflict) = state
        .ledger
        .link_agent_session(
            identity,
            request.source_task_id.clone(),
            source_project_id,
            request.host.clone(),
            session_hash.clone(),
            now_ms(),
        )
        .await
        .map_err(|error| ApiError::internal(format!("agent link write failed: {error}")))?;
    Ok(Json(AgentLinkResponse {
        schema_version: AGENT_API_SCHEMA_VERSION,
        linked,
        conflict,
        task_id: Some(task_key),
        session_id: Some(session_hash),
        code: (!linked).then(|| "already_linked".to_string()),
    }))
}

/// `POST /v1/agents/usage/import`
pub async fn agents_usage_import(
    State(state): State<AppState>,
    Json(request): Json<AgentUsageImportRequest>,
) -> Result<Json<AgentUsageImportResponse>, ApiError> {
    if request.schema_version != 1 {
        return Err(ApiError::bad_request(
            "unsupported usage import schema_version",
        ));
    }
    if request.receipts.len() > hzr_core::MAX_USAGE_RECEIPTS_PER_IMPORT {
        return Err(ApiError::bad_request(format!(
            "at most {} receipts per import",
            hzr_core::MAX_USAGE_RECEIPTS_PER_IMPORT
        )));
    }
    let encoded = serde_json::to_vec(&request.receipts).unwrap_or_default();
    if encoded.len() > MAX_IMPORT_BYTES {
        return Err(ApiError::bad_request("import payload exceeds 1 MiB"));
    }
    let outcome = state
        .ledger
        .import_agent_usage(request.receipts, now_ms().max(0) as u64)
        .await
        .map_err(|error| ApiError::internal(format!("usage import failed: {error}")))?;
    Ok(Json(AgentUsageImportResponse {
        schema_version: AGENT_API_SCHEMA_VERSION,
        accepted: outcome.accepted,
        replayed: outcome.replayed,
        rejected: outcome.rejected,
        conflicts: outcome.conflicts,
        rejections: outcome
            .rejections
            .into_iter()
            .map(|(source_record_id, code)| AgentUsageRejection {
                source_record_id,
                code,
            })
            .collect(),
        committed: outcome.committed,
    }))
}

#[cfg(test)]
mod tests;
