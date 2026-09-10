<script setup lang="ts">
/**
 * The Agents workspace: a read-only view of an enrolled agtx board.
 *
 * Three things this deliberately does not do. It has no drag affordance,
 * because HZR cannot move a task. It never draws a flowing message animation,
 * because polling observes states and not messages. And it never renders a zero
 * where evidence is missing — an unobserved value is an em dash with a reason.
 */
import type { Core, ElementDefinition } from "cytoscape";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import AppIcon from "./AppIcon.vue";
import { DetailRequestCoordinator } from "../detail-request";
import {
  AGENT_ONBOARDING_COMMANDS,
  UNKNOWN,
  coverageLabel,
  eventKindLabel,
  formatAge,
  formatMicrounits,
  freshnessLabel,
  graphElements,
  graphSignature,
  groupByColumn,
  hookStateLabel,
  integrationStateLabel,
  runtimePhaseLabel,
  taskDisplayName,
  taskHandle,
  type AgentBoardPage,
  type AgentEventPage,
  type AgentTaskDetail,
} from "../agents";

const REFRESH_INTERVAL_MS = 5_000;

const board = ref<AgentBoardPage | null>(null);
const events = ref<AgentEventPage | null>(null);
const detail = ref<AgentTaskDetail | null>(null);
const error = ref<string | null>(null);
const detailError = ref<string | null>(null);
const projectId = ref<string | null>(null);
const selectedTaskId = ref<string | null>(null);
const view = ref<"board" | "graph" | "timeline">("board");
const graphElement = ref<HTMLDivElement | null>(null);
const copied = ref<string | null>(null);

let graph: Core | null = null;
let graphResizeObserver: ResizeObserver | null = null;
let renderedSignature = "";
let timer: number | undefined;
let mounted = false;
const boardRequests = new DetailRequestCoordinator();
const detailRequests = new DetailRequestCoordinator();

const columns = computed(() => groupByColumn(board.value?.tasks ?? []));
const state = computed(() => board.value?.state ?? "disabled");
const stateLabel = computed(() => integrationStateLabel[state.value]);
const enrolled = computed(() => (board.value?.projects.length ?? 0) > 0);
const coverage = computed(() => board.value?.coverage ?? null);
/** True once any observed task carries a title, i.e. the install publishes them. */
const titlesPublished = computed(() =>
  (board.value?.tasks ?? []).some((task) => Boolean(task.title)),
);
const selectedTask = computed(
  () => board.value?.tasks.find((task) => task.task_id === selectedTaskId.value) ?? null,
);

/**
 * Name any task id that appears as a graph endpoint.
 *
 * An id fragment identifies a row but tells a reader nothing, so an endpoint
 * resolves to its display name; only a reference to a task the source no longer
 * has stays anonymous, and it says so.
 */
const nameForTaskId = computed(() => {
  const known = new Map(
    (board.value?.tasks ?? []).map((task) => [task.task_id, taskDisplayName(task)]),
  );
  return (taskId: string) => known.get(taskId) ?? `Unresolved reference (${taskId.slice(0, 8)})`;
});

/** A percentage without a denominator is not a number we are allowed to show. */
const usageCoverageLabel = computed(() => {
  const value = coverage.value;
  if (!value || value.usage_coverage_pct === null) return UNKNOWN;
  return `${value.usage_coverage_pct.toFixed(0)}% (${value.usage_covered_runs}/${value.runs_total})`;
});

async function loadBoard(): Promise<void> {
  const requested = projectId.value;
  const ticket = boardRequests.begin(requested);
  try {
    const query = requested ? `?project_id=${encodeURIComponent(requested)}&limit=200` : "";
    const response = await fetch(`/v1/dashboard/agents${query}`, {
      cache: "no-store",
      signal: ticket.signal,
    });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const page = (await response.json()) as AgentBoardPage;
    // A response for a scope the user has since left may not replace the view.
    if (!boardRequests.isCurrent(ticket, requested)) return;
    board.value = page;
    error.value = null;
    if (projectId.value === null && page.projects.length > 0) {
      projectId.value = page.projects[0].project_id;
      await loadBoard();
      return;
    }
    if (view.value === "timeline") await loadEvents();
    if (view.value === "graph") await renderGraph();
  } catch (cause) {
    if ((cause as Error).name === "AbortError") return;
    if (!boardRequests.isCurrent(ticket, requested)) return;
    error.value = (cause as Error).message;
  } finally {
    boardRequests.finish(ticket);
  }
}

async function loadEvents(): Promise<void> {
  const requested = projectId.value;
  if (!requested) return;
  try {
    const response = await fetch(
      `/v1/dashboard/agents/events?project_id=${encodeURIComponent(requested)}&limit=200`,
      { cache: "no-store" },
    );
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const page = (await response.json()) as AgentEventPage;
    if (projectId.value !== requested) return;
    events.value = page;
  } catch (cause) {
    if ((cause as Error).name !== "AbortError") error.value = (cause as Error).message;
  }
}

async function selectTask(taskId: string | null): Promise<void> {
  selectedTaskId.value = taskId;
  detail.value = null;
  detailError.value = null;
  if (!taskId) {
    detailRequests.abort();
    return;
  }
  const ticket = detailRequests.begin(taskId);
  try {
    const response = await fetch(
      `/v1/dashboard/agents/tasks/${encodeURIComponent(taskId)}`,
      { cache: "no-store", signal: ticket.signal },
    );
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const payload = (await response.json()) as AgentTaskDetail;
    if (!detailRequests.isCurrent(ticket, taskId)) return;
    detail.value = payload;
  } catch (cause) {
    if ((cause as Error).name === "AbortError") return;
    if (!detailRequests.isCurrent(ticket, taskId)) return;
    detailError.value = (cause as Error).message;
  } finally {
    detailRequests.finish(ticket);
  }
}

async function selectProject(next: string | null): Promise<void> {
  if (next === projectId.value) return;
  boardRequests.switchProject(next);
  projectId.value = next;
  await selectTask(null);
  board.value = null;
  events.value = null;
  renderedSignature = "";
  await loadBoard();
}

async function setView(next: "board" | "graph" | "timeline"): Promise<void> {
  view.value = next;
  if (next === "timeline") await loadEvents();
  if (next === "graph") await renderGraph();
}

async function renderGraph(): Promise<void> {
  await nextTick();
  const container = graphElement.value;
  const page = board.value;
  if (!container || !page) return;
  const signature = graphSignature(page.tasks, page.edges);
  // An unchanged snapshot must not relayout: a board that is merely polled
  // should not look like a board that is moving. Re-fitting is still needed,
  // because the container has no size until this view is the visible one.
  if (graph && signature === renderedSignature) {
    graph.resize();
    graph.fit(undefined, 24);
    return;
  }
  renderedSignature = signature;
  // Loaded on demand, the same way the memory graph does: a static import here
  // would pull cytoscape into the main chunk for every visitor, including the
  // ones who never open this view.
  const { default: cytoscape } = await import("cytoscape");
  const elements = graphElements(page.tasks, page.edges) as ElementDefinition[];
  const reducedMotion = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
  graph?.destroy();
  graph = cytoscape({
    container,
    elements,
    minZoom: 0.4,
    maxZoom: 2.2,
    boxSelectionEnabled: false,
    style: [
      {
        selector: "node.task",
        style: {
          shape: "round-rectangle",
          width: 96,
          height: 34,
          label: "data(label)",
          "font-family": "Inter, SF Pro Display, sans-serif",
          "font-size": 10,
          color: "#d8d5cd",
          "text-valign": "center",
          "background-color": "#242831",
          "border-width": 1.5,
          "border-color": "#667085",
        },
      },
      {
        selector: "node.unresolved",
        style: { "border-style": "dashed", "border-color": "#c98a2b", shape: "diamond" },
      },
      {
        selector: "node.tombstoned",
        style: { opacity: 0.45, "border-style": "dotted" },
      },
      {
        selector: "edge.dependency",
        style: {
          width: 1.4,
          "line-color": "#667085",
          "target-arrow-color": "#667085",
          "target-arrow-shape": "triangle",
          "curve-style": "bezier",
        },
      },
      {
        selector: "edge.unresolved",
        style: { "line-style": "dashed", "line-color": "#c98a2b" },
      },
    ],
    layout: { name: "breadthfirst", directed: true, spacingFactor: 1.15, animate: !reducedMotion },
  });
  // The container has no measured size until this view is the visible one, and
  // cytoscape keeps whatever viewport it saw at construction — which draws the
  // whole graph into one corner. Observing the box is the only thing that
  // reliably catches the first real measurement, on tab switch and on resize
  // alike.
  const fitToContainer = () => {
    graph?.resize();
    graph?.fit(undefined, 24);
  };
  graph.one("layoutstop", fitToContainer);
  graphResizeObserver?.disconnect();
  graphResizeObserver = new ResizeObserver(fitToContainer);
  graphResizeObserver.observe(container);
  graph.on("tap", "node.task", (event) => {
    const id = String(event.target.id());
    if (page.tasks.some((task) => task.task_id === id)) void selectTask(id);
  });
}

async function copyCommand(command: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(command);
    copied.value = command;
  } catch {
    copied.value = null;
  }
}

function scheduleRefresh(): void {
  window.clearInterval(timer);
  timer = window.setInterval(() => {
    // Browser polling suspends in the background; the daemon keeps observing on
    // its own schedule either way.
    if (document.visibilityState === "visible" && mounted) void loadBoard();
  }, REFRESH_INTERVAL_MS);
}

watch(view, () => {
  if (view.value === "graph") void renderGraph();
});

onMounted(() => {
  mounted = true;
  void loadBoard();
  scheduleRefresh();
});

onBeforeUnmount(() => {
  mounted = false;
  window.clearInterval(timer);
  boardRequests.abort();
  detailRequests.abort();
  graphResizeObserver?.disconnect();
  graphResizeObserver = null;
  graph?.destroy();
  graph = null;
});
</script>

<template>
  <section class="section-block agents-section" aria-labelledby="agents-title">
    <div class="section-heading">
      <div>
        <span class="eyebrow">agtx Agent Observatory</span>
        <h2 id="agents-title">Observed agent work</h2>
      </div>
      <p>
        Read-only. HZR observes an enrolled agtx board; it never creates a task, starts an
        agent, advances a phase, or answers a permission prompt.
      </p>
    </div>

    <div v-if="!enrolled" class="empty-state agents-empty" role="status">
      <span class="empty-icon"><AppIcon name="warning" :size="24" /></span>
      <span class="eyebrow">{{ stateLabel }}</span>
      <h3>No agtx project is enrolled.</h3>
      <p>
        The pinned agtx runtime and observer are included in HZR; no Cargo or separate
        agtx installation is needed. Verify the component, open a board to initialize
        its data directory if needed, then enroll it for read-only monitoring.
        The board command opens the interactive application; skip it for an existing board.
        Installing or enrolling alone never starts an agent.
      </p>
      <ol class="agents-onboarding">
        <li v-for="command in AGENT_ONBOARDING_COMMANDS" :key="command">
          <code>{{ command }}</code>
          <button type="button" @click="copyCommand(command)">
            {{ copied === command ? "Copied" : "Copy" }}
          </button>
        </li>
      </ol>
      <p class="health-boundary">
        Installing and enrolling from the browser is deliberately unavailable: these are
        authenticated actions, and the daemon secret never reaches this page.
      </p>
    </div>

    <template v-else>
      <div class="agents-toolbar">
        <label class="workspace-select">
          <span>agtx project</span>
          <select
            :value="projectId ?? ''"
            @change="selectProject(($event.target as HTMLSelectElement).value || null)"
          >
            <option v-for="project in board?.projects ?? []" :key="project.project_id" :value="project.project_id">
              {{ project.label }} · {{ integrationStateLabel[project.state] }}
            </option>
          </select>
          <small>
            {{ titlesPublished
              ? "Task titles come from the source; project and session identities stay pseudonymous, and no filesystem path is published."
              : "Pseudonymous identities · titles withheld by this install · no filesystem paths are published" }}
          </small>
        </label>
        <div class="agents-statline">
          <span><strong>{{ stateLabel }}</strong></span>
          <span>
            Observed since
            <strong>{{ board?.observed_since_ms ? formatAge(Date.now() - board.observed_since_ms) : UNKNOWN }}</strong>
          </span>
          <span>
            Last source update
            <strong>{{ board?.source_observed_at_ms ? formatAge(Date.now() - board.source_observed_at_ms) : UNKNOWN }}</strong>
          </span>
          <span>Source lag <strong>{{ board?.lag_ms === null || board?.lag_ms === undefined ? UNKNOWN : `${Math.round(board.lag_ms / 1000)}s` }}</strong></span>
          <span>Tasks <strong>{{ coverage?.observed_tasks ?? 0 }}</strong></span>
          <span>Linked sessions <strong>{{ coverage?.linked_sessions ?? 0 }}</strong></span>
          <span>Usage coverage <strong>{{ usageCoverageLabel }}</strong></span>
          <span v-if="(coverage?.gap_count ?? 0) > 0" class="bounded-pill">
            {{ coverage?.gap_count }} observation gaps
          </span>
        </div>
      </div>

      <p v-if="error" class="stale-ribbon" role="status">
        <AppIcon name="warning" :size="18" />
        <span>Live refresh paused: {{ error }}. Showing the last snapshot.</span>
      </p>
      <p v-if="board?.has_dependency_cycle" class="health-boundary" role="status">
        The observed dependencies contain a cycle. It is shown as recorded; HZR does not
        resolve it and takes no scheduling action.
      </p>
      <p v-if="board?.truncated" class="health-boundary">
        This page is bounded. More tasks exist than are shown.
      </p>

      <nav class="agents-views" aria-label="Agents views">
        <button
          v-for="option in (['board', 'graph', 'timeline'] as const)"
          :key="option"
          type="button"
          :class="{ active: view === option }"
          :aria-current="view === option ? 'page' : undefined"
          @click="setView(option)"
        >
          {{ option === "board" ? "Board" : option === "graph" ? "Dependencies" : "Timeline" }}
        </button>
      </nav>

      <div v-if="view === 'board'" class="agents-board">
        <section v-for="column in columns" :key="column.status" class="agents-column">
          <h3>{{ column.label }} <span>{{ column.tasks.length }}</span></h3>
          <p v-if="!column.tasks.length" class="agents-column-empty">{{ UNKNOWN }}</p>
          <button
            v-for="task in column.tasks"
            :key="task.task_id"
            type="button"
            class="agents-card"
            :class="{ selected: task.task_id === selectedTaskId, tombstoned: task.tombstoned }"
            @click="selectTask(task.task_id)"
          >
            <strong>{{ taskDisplayName(task) }}</strong>
            <span v-if="taskHandle(task)" class="agents-card-handle">
              {{ taskHandle(task) }}<template v-if="task.branch"> · {{ task.branch }}</template>
            </span>
            <span class="agents-card-line">
              Agent {{ task.agent ?? UNKNOWN }} · cycle {{ task.cycle }}
            </span>
            <span class="agents-card-line">
              Board: {{ column.label }}<template v-if="task.unknown_board_status">
                (source said “{{ task.unknown_board_status }}”)</template>
            </span>
            <span class="agents-card-line">
              Runtime: {{ runtimePhaseLabel[task.runtime_phase] }} ·
              {{ freshnessLabel(task.runtime_freshness, task.runtime_observed_at_ms === null ? null : Date.now() - task.runtime_observed_at_ms) }}
            </span>
            <span class="agents-card-line">
              Hook: {{ hookStateLabel[task.hook_state] }} ·
              {{ freshnessLabel(task.hook_freshness, task.hook_observed_at_ms === null ? null : Date.now() - task.hook_observed_at_ms) }}
            </span>
            <span class="agents-card-line">
              Sessions {{ task.linked_session_count }} · cost
              {{ task.usage_covered ? "covered" : "unknown" }}
            </span>
            <span v-if="task.tombstoned" class="agents-card-line">
              No longer present in the source. This is not Done and not Accepted.
            </span>
          </button>
        </section>
      </div>

      <div v-else-if="view === 'graph'" class="agents-graph-layout">
        <div ref="graphElement" class="agents-graph" role="img" aria-label="Task dependency graph"></div>
        <div class="agents-graph-legend">
          <span><i class="legend-dependency"></i> Dependency (A → B: B depends on A)</span>
          <span><i class="legend-unresolved"></i> Unresolved reference</span>
          <span><i class="legend-tombstoned"></i> Absent from source</span>
        </div>
        <table class="agents-graph-table">
          <caption>Accessible equivalent of the dependency graph.</caption>
          <thead>
            <tr><th scope="col">Depends on</th><th scope="col">Task</th><th scope="col">Resolved</th></tr>
          </thead>
          <tbody>
            <tr v-for="edge in board?.edges ?? []" :key="`${edge.from_task_id}-${edge.to_task_id}`">
              <td>{{ nameForTaskId(edge.from_task_id) }}</td>
              <td>
                <button type="button" @click="selectTask(edge.to_task_id)">
                  {{ nameForTaskId(edge.to_task_id) }}
                </button>
              </td>
              <td>{{ edge.resolved ? "yes" : "no" }}</td>
            </tr>
            <tr v-if="!(board?.edges ?? []).length">
              <td colspan="3">{{ UNKNOWN }} no observed dependencies</td>
            </tr>
          </tbody>
        </table>
      </div>

      <ol v-else class="agents-timeline">
        <li v-if="events && events.history_start_ms" class="agents-timeline-note">
          History starts {{ formatAge(Date.now() - events.history_start_ms) }}. Nothing before
          that was observed; earlier durations are unavailable, not zero.
        </li>
        <li v-for="event in events?.events ?? []" :key="event.sequence" :class="event.kind">
          <span class="agents-event-kind">{{ eventKindLabel[event.kind] }}</span>
          <span v-if="event.from_state || event.to_state" class="agents-event-transition">
            {{ event.from_state ?? UNKNOWN }} → {{ event.to_state ?? UNKNOWN }}
          </span>
          <span class="agents-event-time">
            observed {{ formatAge(Date.now() - event.observed_at_ms) }}
            <template v-if="event.source_at_ms">
              · source time {{ formatAge(Date.now() - event.source_at_ms) }}
            </template>
            · {{ event.evidence }}
          </span>
          <button v-if="event.task_id" type="button" @click="selectTask(event.task_id)">
            {{ nameForTaskId(event.task_id) }}
          </button>
        </li>
        <li v-if="!(events?.events ?? []).length" class="agents-timeline-note">
          {{ UNKNOWN }} no observed transitions yet.
        </li>
      </ol>

      <aside v-if="selectedTaskId" class="agents-detail" aria-label="Task detail">
        <div class="agents-detail-head">
          <h3>{{ selectedTask ? taskDisplayName(selectedTask) : "Task" }}</h3>
          <button type="button" @click="selectTask(null)">Close</button>
        </div>
        <p v-if="detailError" role="alert">{{ detailError }}</p>
        <template v-else-if="detail">
          <dl class="agents-detail-grid">
            <dt>Identity</dt>
            <dd>
              {{ detail.task.label }}
              <template v-if="detail.task.branch"> · branch {{ detail.task.branch }}</template>
            </dd>
            <dt>Board</dt>
            <dd>{{ detail.task.board_status }}</dd>
            <dt>Runtime</dt>
            <dd>
              {{ runtimePhaseLabel[detail.task.runtime_phase] }} ·
              {{ freshnessLabel(detail.task.runtime_freshness, detail.task.runtime_observed_at_ms === null ? null : Date.now() - detail.task.runtime_observed_at_ms) }}
            </dd>
            <dt>Hook</dt>
            <dd>
              {{ hookStateLabel[detail.task.hook_state] }} ·
              {{ freshnessLabel(detail.task.hook_freshness, detail.task.hook_observed_at_ms === null ? null : Date.now() - detail.task.hook_observed_at_ms) }}
            </dd>
            <dt>Cycle</dt>
            <dd>{{ detail.task.cycle }}</dd>
            <dt>Agent now</dt>
            <dd>{{ detail.task.agent ?? UNKNOWN }}</dd>
            <dt>Agents observed</dt>
            <dd>{{ detail.previous_agents.join(", ") || UNKNOWN }}</dd>
            <dt>First observed</dt>
            <dd>{{ formatAge(Date.now() - detail.task.first_observed_at_ms) }}</dd>
          </dl>

          <h4>Dependencies</h4>
          <ul class="agents-session-list">
            <li v-for="edge in detail.dependencies" :key="`${edge.from_task_id}-${edge.to_task_id}`">
              {{ nameForTaskId(edge.from_task_id) }} → {{ nameForTaskId(edge.to_task_id) }}
              <em v-if="!edge.resolved">unresolved in the source</em>
            </li>
            <li v-if="!detail.dependencies.length">{{ UNKNOWN }} none observed</li>
          </ul>

          <h4>Linked sessions</h4>
          <ul class="agents-session-list">
            <li v-for="session in detail.sessions" :key="session.session_id">
              <!-- The host and how the link was established are what identify a
                   session to a reader; the digest is the handle, not the name. -->
              <strong>{{ session.host }}</strong>
              · {{ session.provenance === "explicit_user" ? "explicit link" : "agtx hook" }}
              · {{ session.usage_receipt_count }} usage receipts
              <code :title="session.session_id">{{ session.session_id.slice(-12) }}</code>
              <em v-if="session.conflict">
                shared with another task — its spend stays unallocated
              </em>
            </li>
            <li v-if="!detail.sessions.length">{{ UNKNOWN }} no session linked; spending is unknown</li>
          </ul>

          <h4>Economics</h4>
          <p class="health-boundary">
            Reported and estimated amounts are alternatives for the same request, never a sum.
            Neither is a verified provider invoice.
          </p>
          <dl class="agents-detail-grid">
            <dt>Reported tokens</dt>
            <dd>
              in {{ detail.economics.reported_tokens.input_tokens }} ·
              out {{ detail.economics.reported_tokens.output_tokens }}
              (incl. {{ detail.economics.reported_tokens.reasoning_tokens }} reasoning) ·
              {{ detail.economics.reported_receipt_count }} receipts
            </dd>
            <dt>Reported cost</dt>
            <dd>
              <span v-for="subtotal in detail.economics.reported_cost" :key="`r-${subtotal.currency}`">
                {{ formatMicrounits(subtotal) }} ({{ coverageLabel(subtotal) }})
              </span>
              <span v-if="!detail.economics.reported_cost.length">{{ UNKNOWN }}</span>
            </dd>
            <dt>Estimated API cost · current catalog</dt>
            <dd>
              <span v-for="subtotal in detail.economics.estimated_api_cost" :key="`e-${subtotal.currency}`">
                {{ formatMicrounits(subtotal) }} ({{ coverageLabel(subtotal) }})
              </span>
              <span v-if="!detail.economics.estimated_api_cost.length">{{ UNKNOWN }}</span>
            </dd>
            <dt>Shared / unallocated</dt>
            <dd>
              <span v-for="subtotal in detail.economics.shared_unallocated_cost" :key="`s-${subtotal.currency}`">
                {{ formatMicrounits(subtotal) }}
              </span>
              <span v-if="!detail.economics.shared_unallocated_cost.length">{{ UNKNOWN }}</span>
            </dd>
            <dt>HZR operation reduction · lifetime linked sessions</dt>
            <dd>
              baseline {{ detail.economics.hzr_baseline_tokens_estimated }} · delivered
              {{ detail.economics.hzr_delivered_tokens_estimated }} · net
              {{ detail.economics.net_avoided_tokens_estimated }}
              <template v-if="detail.economics.reduction_pct !== null">
                ({{ detail.economics.reduction_pct.toFixed(1) }}%)
              </template>
              <template v-else> ({{ UNKNOWN }} no baseline)</template>
            </dd>
            <dt>Observed time</dt>
            <dd>
              working {{ Math.round(detail.economics.observed_wall_time_ms / 1000) }}s ·
              blocked {{ Math.round(detail.economics.observed_blocked_time_ms / 1000) }}s ·
              idle {{ Math.round(detail.economics.observed_idle_time_ms / 1000) }}s
              (observed intervals only)
            </dd>
            <dt>Cost per accepted task</dt>
            <dd>{{ detail.economics.accepted_task_count === null ? `${UNKNOWN} no acceptance evidence` : detail.economics.accepted_task_count }}</dd>
          </dl>
          <ul v-if="detail.economics.unavailable.length" class="agents-unavailable">
            <li v-for="entry in detail.economics.unavailable" :key="entry.metric">
              <strong>{{ entry.metric }}</strong>: {{ entry.reason }}
            </li>
          </ul>
          <p v-if="detail.economics.unreconciled_existing_receipts > 0" class="health-boundary">
            {{ detail.economics.unreconciled_existing_receipts }} existing provider receipts
            could not be joined to an observed run. They are excluded from the totals above.
          </p>
        </template>
        <p v-else>Loading…</p>
      </aside>
    </template>
  </section>
</template>
