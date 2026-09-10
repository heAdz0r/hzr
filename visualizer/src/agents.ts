/**
 * Types and pure helpers for the Agents workspace.
 *
 * Everything here is deliberately free of Vue and of `fetch`, so the rules that
 * matter — which states are distinct, what counts as stale, when a number is
 * unknown rather than zero — are unit-testable on their own.
 */

export type AgentIntegrationState =
  | "disabled"
  | "missing_component"
  | "incompatible"
  | "connecting"
  | "ready"
  | "partial"
  | "stale"
  | "error";

export type AgentBoardStatus =
  | "backlog"
  | "planning"
  | "running"
  | "review"
  | "done"
  | "unknown";

export type AgentRuntimePhase =
  | "working"
  | "blocked"
  | "idle"
  | "ready"
  | "exited"
  | "unknown";

export type AgentHookState = "working" | "blocked" | "waiting" | "ended" | "unknown";

export type AgentFreshness = "fresh" | "stale" | "unavailable";

export type AgentEventKind =
  | "depends_on"
  | "session_linked"
  | "agent_changed"
  | "phase_changed"
  | "phase_completed"
  | "task_stuck"
  | "handoff_observed"
  | "snapshot_gap"
  | "first_observed"
  | "tombstoned";

export interface AgentProjectRef {
  project_id: string;
  label: string;
  state: AgentIntegrationState;
  observed_tasks: number;
}

export interface AgentCoverage {
  observed_tasks: number;
  linked_sessions: number;
  unlinked_sessions: number;
  usage_covered_runs: number;
  runs_total: number;
  gap_count: number;
  history_start_ms: number | null;
  usage_coverage_pct: number | null;
}

export interface AgentTaskSummary {
  task_id: string;
  /** Stable pseudonym, e.g. `Task 7ac3`. Always present. */
  label: string;
  /** The source's own title, when the install publishes titles. */
  title: string | null;
  branch: string | null;
  board_status: AgentBoardStatus;
  unknown_board_status: string | null;
  runtime_phase: AgentRuntimePhase;
  runtime_freshness: AgentFreshness;
  runtime_observed_at_ms: number | null;
  hook_state: AgentHookState;
  hook_freshness: AgentFreshness;
  hook_observed_at_ms: number | null;
  agent: string | null;
  cycle: number;
  first_observed_at_ms: number;
  last_observed_at_ms: number;
  source_updated_at_ms: number | null;
  source_age_ms: number | null;
  linked_session_count: number;
  usage_covered: boolean;
  tombstoned: boolean;
}

export interface AgentEdgeView {
  from_task_id: string;
  to_task_id: string;
  kind: AgentEventKind;
  resolved: boolean;
}

export interface AgentBoardPage {
  schema_version: number;
  generated_at_ms: number;
  state: AgentIntegrationState;
  project_id: string | null;
  projects: AgentProjectRef[];
  observed_since_ms: number | null;
  source_observed_at_ms: number | null;
  lag_ms: number | null;
  coverage: AgentCoverage;
  tasks: AgentTaskSummary[];
  edges: AgentEdgeView[];
  unresolved_task_ids: string[];
  has_dependency_cycle: boolean;
  warnings: string[];
  truncated: boolean;
  next_cursor: string | null;
}

export interface AgentEventEntry {
  sequence: number;
  task_id: string | null;
  kind: AgentEventKind;
  source_at_ms: number | null;
  observed_at_ms: number;
  evidence: string;
  from_state: string | null;
  to_state: string | null;
}

export interface AgentEventPage {
  schema_version: number;
  generated_at_ms: number;
  state: AgentIntegrationState;
  events: AgentEventEntry[];
  history_start_ms: number | null;
  gap_count: number;
  warnings: string[];
  truncated: boolean;
  next_cursor: string | null;
}

export interface AgentMoneySubtotal {
  currency: string;
  microunits: number;
  covered_receipts: number;
  total_receipts: number;
}

export interface AgentTokenTotals {
  input_tokens: number;
  output_tokens: number;
  reasoning_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  cache_write_5m_tokens: number;
  cache_write_1h_tokens: number;
}

export interface AgentUnavailable {
  metric: string;
  reason: string;
}

export interface AgentEconomics {
  schema_version: number;
  reported_tokens: AgentTokenTotals;
  reported_receipt_count: number;
  reported_cost: AgentMoneySubtotal[];
  estimated_api_cost: AgentMoneySubtotal[];
  price_table_identity: string | null;
  price_entry_versions: string[];
  hzr_baseline_tokens_estimated: number;
  hzr_delivered_tokens_estimated: number;
  net_avoided_tokens_estimated: number;
  reduction_pct: number | null;
  shared_unallocated_cost: AgentMoneySubtotal[];
  unreconciled_existing_receipts: number;
  observed_wall_time_ms: number;
  observed_blocked_time_ms: number;
  observed_idle_time_ms: number;
  accepted_task_count: number | null;
  cost_per_accepted_task: AgentMoneySubtotal[];
  unavailable: AgentUnavailable[];
}

export interface AgentSessionLinkView {
  session_id: string;
  host: string;
  provenance: "explicit_user" | "agtx_hook";
  valid_from_ms: number;
  valid_to_ms: number | null;
  conflict: boolean;
  usage_receipt_count: number;
}

export interface AgentRunView {
  run_id: string;
  agent: string | null;
  model: string | null;
  host: string | null;
  observed_start_ms: number;
  observed_end_ms: number | null;
  session_linked: boolean;
}

export interface AgentTaskDetail {
  schema_version: number;
  generated_at_ms: number;
  state: AgentIntegrationState;
  task: AgentTaskSummary;
  dependencies: AgentEdgeView[];
  previous_agents: string[];
  runs: AgentRunView[];
  sessions: AgentSessionLinkView[];
  events: AgentEventEntry[];
  economics: AgentEconomics;
  warnings: string[];
}

/** The five upstream columns, plus a separate group for unrecognised states. */
export const AGENT_BOARD_COLUMNS: readonly AgentBoardStatus[] = [
  "backlog",
  "planning",
  "running",
  "review",
  "done",
  "unknown",
] as const;

export const boardColumnLabel: Record<AgentBoardStatus, string> = {
  backlog: "Backlog / research",
  planning: "Planning",
  running: "Running",
  review: "Review",
  done: "Done",
  unknown: "Unrecognised state",
};

export const integrationStateLabel: Record<AgentIntegrationState, string> = {
  disabled: "Monitoring off",
  missing_component: "Component not installed",
  incompatible: "Component incompatible",
  connecting: "First observation pending",
  ready: "Observing",
  partial: "Partial page",
  stale: "Source stale",
  error: "Observation failed",
};

/**
 * Every status colour has words behind it.
 *
 * A board column says where a task sits; a runtime phase says what a live agtx
 * TUI last published; a hook state says what the agent said about itself. They
 * are three different claims and the UI never merges them into one dot.
 */
export const runtimePhaseLabel: Record<AgentRuntimePhase, string> = {
  working: "Working",
  blocked: "Waiting for input",
  idle: "Idle (heuristic)",
  ready: "Artifact ready",
  exited: "Process exited",
  unknown: "Unknown",
};

export const hookStateLabel: Record<AgentHookState, string> = {
  working: "Agent reported working",
  blocked: "Agent reported blocked",
  waiting: "Turn ended",
  ended: "Session ended",
  unknown: "No hook evidence",
};

export const eventKindLabel: Record<AgentEventKind, string> = {
  depends_on: "Depends on",
  session_linked: "Session linked",
  agent_changed: "Agent assignment changed",
  phase_changed: "Observed phase change",
  phase_completed: "Source reported phase complete",
  task_stuck: "Source reported task stuck",
  handoff_observed: "Workflow handoff observed",
  snapshot_gap: "Observation gap",
  first_observed: "First observed",
  tombstoned: "No longer present in source",
};

/** Em dash, never a zero, for a value that was never observed. */
export const UNKNOWN = "—";

/**
 * What to call a task on screen.
 *
 * An opaque id identifies a task but does not let anyone recognise it, so the
 * source's own title leads and the pseudonym follows as the stable handle. With
 * titles withheld, the pseudonym is all there is and stands alone.
 */
export function taskDisplayName(task: {
  label: string;
  title: string | null;
}): string {
  return task.title?.trim() ? task.title : task.label;
}

/** The short, quotable handle shown beside a title. */
export function taskHandle(task: { label: string; title: string | null }): string {
  return task.title?.trim() ? task.label : "";
}

export function freshnessLabel(freshness: AgentFreshness, ageMs: number | null): string {
  if (freshness === "unavailable") return `${UNKNOWN} no evidence`;
  const age = ageMs === null ? UNKNOWN : formatAge(ageMs);
  return freshness === "fresh" ? `Fresh · ${age}` : `Stale · ${age}`;
}

export function formatAge(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return UNKNOWN;
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

/** Microunits are millionths of a currency unit; never floated into a total. */
export function formatMicrounits(subtotal: AgentMoneySubtotal): string {
  const whole = Math.trunc(subtotal.microunits / 1_000_000);
  const fraction = subtotal.microunits % 1_000_000;
  return `${subtotal.currency} ${whole}.${String(fraction).padStart(6, "0")}`;
}

export function coverageLabel(subtotal: AgentMoneySubtotal): string {
  return `${subtotal.covered_receipts}/${subtotal.total_receipts} receipts`;
}

export function groupByColumn(
  tasks: readonly AgentTaskSummary[],
): Array<{ status: AgentBoardStatus; label: string; tasks: AgentTaskSummary[] }> {
  return AGENT_BOARD_COLUMNS.map((status) => ({
    status,
    label: boardColumnLabel[status],
    tasks: tasks.filter((task) => task.board_status === status),
  }));
}

export interface AgentGraphElement {
  data: Record<string, string | number>;
  classes?: string;
}

/**
 * Build the dependency graph.
 *
 * A referenced task that is not in the page still gets a node: an unresolved
 * dependency is evidence of a gap, and dropping it would make the graph look
 * complete when it is not.
 */
export function graphElements(
  tasks: readonly AgentTaskSummary[],
  edges: readonly AgentEdgeView[],
): AgentGraphElement[] {
  const known = new Map(tasks.map((task) => [task.task_id, task]));
  const elements: AgentGraphElement[] = tasks.map((task) => ({
    data: {
      id: task.task_id,
      label: taskDisplayName(task),
      status: task.board_status,
    },
    classes: task.tombstoned ? "task tombstoned" : "task",
  }));
  const unresolved = new Set<string>();
  for (const edge of edges) {
    for (const endpoint of [edge.from_task_id, edge.to_task_id]) {
      if (!known.has(endpoint) && !unresolved.has(endpoint)) {
        unresolved.add(endpoint);
        elements.push({
          data: { id: endpoint, label: "Unresolved", status: "unknown" },
          classes: "task unresolved",
        });
      }
    }
    elements.push({
      data: {
        id: `${edge.from_task_id}->${edge.to_task_id}`,
        source: edge.from_task_id,
        target: edge.to_task_id,
        kind: edge.kind,
      },
      classes: edge.resolved ? "dependency" : "dependency unresolved",
    });
  }
  return elements;
}

/**
 * A signature that changes only when the rendered graph would change.
 *
 * Layout is expensive and re-running it on an unchanged snapshot makes a board
 * that is merely being polled look like a board that is moving.
 */
export function graphSignature(
  tasks: readonly AgentTaskSummary[],
  edges: readonly AgentEdgeView[],
): string {
  const nodes = tasks
    .map((task) => `${task.task_id}:${task.board_status}:${task.tombstoned ? 1 : 0}`)
    .sort()
    .join("|");
  const links = edges
    .map((edge) => `${edge.from_task_id}>${edge.to_task_id}:${edge.resolved ? 1 : 0}`)
    .sort()
    .join("|");
  return `${nodes}#${links}`;
}

/** One-command opt-in. `onboard` verifies the bundled observer itself, so the
 * component install is no longer a step the user runs by hand. */
export const AGENT_ONBOARD_COMMAND =
  "hzr agents onboard --project <absolute-worktree> --agtx-data-dir <absolute-agtx-data-dir>";

/** Only a brand-new board needs this once: it opens the bundled agtx app and
 * creates the store `onboard` then enrolls. Skip it for an existing board. */
export const AGENT_BOARD_COMMAND =
  "hzr agents board --project <absolute-worktree> --agtx-data-dir <absolute-agtx-data-dir>";
