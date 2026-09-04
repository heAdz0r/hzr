<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { version as uiVersion } from "../package.json";
import CommandCard from "./components/CommandCard.vue";
import AppIcon from "./components/AppIcon.vue";
import MetricCard from "./components/MetricCard.vue";
import EvidenceOverview from "./components/EvidenceOverview.vue";
import MemoryGraph from "./components/MemoryGraph.vue";
import ObservabilityTimeline from "./components/ObservabilityTimeline.vue";
import IndexPipeline from "./components/IndexPipeline.vue";
import LiveActivity from "./components/LiveActivity.vue";
import ProjectCard from "./components/ProjectCard.vue";
import ServiceNode from "./components/ServiceNode.vue";
import SessionRoi from "./components/SessionRoi.vue";
import StatusChip from "./components/StatusChip.vue";
import type { DashboardProjectPage, DashboardResponse, ProjectState } from "./types";
import {
  dashboardStateLabel,
  filterProjects,
  formatBytes,
  formatCost,
  formatCount,
  formatDuration,
  formatPercent,
  formatSignedCount,
  projectStateLabel,
  refreshFailureAnnouncement,
  relativeTime,
} from "./utils";
import { mergeObservability, nextRefreshBackoff } from "./observability";
import {
  LatestProjectRequestCoordinator,
  isCurrentProjectSnapshot,
} from "./detail-request";

const FULL_REFRESH_INTERVAL_MS = 30_000;
const OBSERVABILITY_INTERVAL_MS = 2_000;
const MAX_REFRESH_BACKOFF_MS = 120_000;

const snapshot = ref<DashboardResponse | null>(null);
const error = ref<string | null>(null);
const manualRefreshing = ref(false);
const loadingProjects = ref(false);
const query = ref("");
const projectPageError = ref<string | null>(null);
const section = ref<"overview" | "projects" | "knowledge" | "system">("overview");
const navigation = [
  { id: "overview", label: "Overview" },
  { id: "projects", label: "Projects" },
  { id: "knowledge", label: "Memory & index" },
  { id: "system", label: "System" },
] as const;
const selectedProjectLabel = computed(() =>
  snapshot.value?.projects.find((project) => project.worktree_id === selectedProjectId.value)?.name ??
  (selectedProjectId.value ? "Selected project" : "Choose a workspace"),
);
const projectFilter = ref<ProjectState | "all">("all");
const toast = ref<string | null>(null);
const liveMessage = ref("");
const selectedProjectId = ref<string | null>(null);
let refreshTimer: number | undefined;
let observabilityTimer: number | undefined;
let toastTimer: number | undefined;
let refreshPromise: Promise<void> | null = null;
const refreshRequests = new LatestProjectRequestCoordinator();
const observabilityRequests = new LatestProjectRequestCoordinator();
let mounted = false;
let fullRefreshBackoffMs = FULL_REFRESH_INTERVAL_MS;
let observabilityBackoffMs = OBSERVABILITY_INTERVAL_MS;

const projectFilters: Array<ProjectState | "all"> = [
  "all",
  "ready",
  "warming",
  "registered",
  "degraded",
  "unavailable",
];

const projects = computed(() =>
  filterProjects(snapshot.value?.projects ?? [], query.value, projectFilter.value),
);

// `Standby` covers two different situations. Say which one this is, so an idle daemon is
// never read as a stalled one.
const postureLabel = computed(() =>
  snapshot.value?.overall_state === "standby" && selectedProjectId.value === null
    ? "No project selected"
    : undefined,
);

const projectSnapshotCurrent = computed(
  () =>
    snapshot.value === null ||
    snapshot.value.selected_worktree_id === selectedProjectId.value,
);

const totalProviderTokens = computed(() => {
  const usage = snapshot.value?.provider_receipts;
  return usage ? usage.actual_input_tokens + usage.actual_output_tokens : 0;
});

async function refresh(manual = false): Promise<void> {
  if (refreshPromise) {
    if (manual) {
      await refreshPromise;
      if (mounted) await refresh(true);
    }
    return;
  }
  const requestedProject = selectedProjectId.value;
  let ticket = refreshRequests.begin(requestedProject);
  if (manual) manualRefreshing.value = true;
  refreshPromise = (async () => {
    try {
      const dashboardUrl = requestedProject
        ? `/v1/dashboard?project=${encodeURIComponent(requestedProject)}`
        : "/v1/dashboard";
      let response = await fetch(dashboardUrl, {
        cache: "no-store",
        headers: { Accept: "application/json" },
        signal: ticket.signal,
      });
      if (
        requestedProject &&
        refreshRequests.isCurrent(ticket, selectedProjectId.value) &&
        (response.status === 400 || response.status === 404)
      ) {
        selectedProjectId.value = null;
        refreshRequests.switchProject(null);
        observabilityRequests.switchProject(null);
        window.localStorage.removeItem("hzr.dashboard.project");
        ticket = refreshRequests.begin(null);
        response = await fetch("/v1/dashboard", {
          cache: "no-store",
          headers: { Accept: "application/json" },
          signal: ticket.signal,
        });
        liveMessage.value = "Stored project selection was removed because it is no longer valid";
      }
      if (!refreshRequests.isCurrent(ticket, selectedProjectId.value)) return;
      if (!response.ok) {
        throw new Error(`Dashboard returned HTTP ${response.status}`);
      }
      const next = (await response.json()) as DashboardResponse;
      if (!refreshRequests.isCurrent(ticket, selectedProjectId.value)) return;
      const previousState = snapshot.value?.overall_state;
      snapshot.value = next;
      error.value = null;
      if (manual) liveMessage.value = "Dashboard refreshed";
      if (previousState && previousState !== next.overall_state) {
        liveMessage.value = `System state changed to ${dashboardStateLabel[next.overall_state]}`;
      }
    } catch (cause) {
      if (cause instanceof DOMException && cause.name === "AbortError") return;
      if (!refreshRequests.isCurrent(ticket, selectedProjectId.value)) return;
      error.value = cause instanceof Error ? cause.message : "Dashboard refresh failed";
      liveMessage.value = refreshFailureAnnouncement(snapshot.value !== null);
    } finally {
      manualRefreshing.value = false;
      refreshPromise = null;
      refreshRequests.finish(ticket);
    }
  })();
  await refreshPromise;
}

async function selectProject(worktreeId: string | null): Promise<void> {
  refreshRequests.switchProject(worktreeId);
  observabilityRequests.switchProject(worktreeId);
  selectedProjectId.value = worktreeId;
  error.value = null;
  if (worktreeId) window.localStorage.setItem("hzr.dashboard.project", worktreeId);
  else window.localStorage.removeItem("hzr.dashboard.project");
  liveMessage.value = "Loading selected project observatory";
  await refresh(true);
}

async function loadMoreProjects(): Promise<void> {
  if (snapshot.value?.projects_next_offset === null || !snapshot.value || loadingProjects.value) return;
  loadingProjects.value = true;
  projectPageError.value = null;
  try {
    const response = await fetch(
      `/v1/dashboard/projects?offset=${snapshot.value.projects_next_offset}&limit=100`,
      { cache: "no-store", headers: { Accept: "application/json" } },
    );
    if (!response.ok) throw new Error(`Project page returned HTTP ${response.status}`);
    const page = (await response.json()) as DashboardProjectPage;
    const known = new Set(snapshot.value.projects.map((project) => project.worktree_id));
    snapshot.value.projects.push(...page.projects.filter((project) => !known.has(project.worktree_id)));
    snapshot.value.projects_total = page.total;
    snapshot.value.projects_next_offset = page.next_offset;
    liveMessage.value = `${snapshot.value.projects.length} of ${page.total} projects loaded`;
  } catch (cause) {
    projectPageError.value = cause instanceof Error ? cause.message : "Project page failed";
    liveMessage.value = projectPageError.value;
  } finally {
    loadingProjects.value = false;
  }
}

function scheduleRefresh(): void {
  window.clearTimeout(refreshTimer);
  const jitter = Math.floor(Math.random() * 5_000);
  refreshTimer = window.setTimeout(async () => {
    if (document.visibilityState === "visible") {
      await refresh();
      fullRefreshBackoffMs = nextRefreshBackoff(
        fullRefreshBackoffMs,
        error.value !== null,
        FULL_REFRESH_INTERVAL_MS,
        MAX_REFRESH_BACKOFF_MS,
      );
    }
    scheduleRefresh();
  }, fullRefreshBackoffMs + jitter);
}

async function syncObservability(): Promise<void> {
  if (!snapshot.value || !selectedProjectId.value) return;
  const projectId = selectedProjectId.value;
  const capturedSnapshot = snapshot.value;
  const ticket = observabilityRequests.begin(projectId);
  const after = capturedSnapshot.observability.next_cursor;
  const parameters = new URLSearchParams({
    project: projectId,
    limit: "100",
  });
  if (after !== null) parameters.set("after", String(after));
  try {
    const response = await fetch(`/v1/dashboard/observability?${parameters}`, {
      cache: "no-store",
      headers: { Accept: "application/json" },
      signal: ticket.signal,
    });
    if (!response.ok) throw new Error(`Observability delta returned HTTP ${response.status}`);
    const delta = (await response.json()) as DashboardResponse["observability"];
    if (
      !snapshot.value ||
      !isCurrentProjectSnapshot(
        observabilityRequests,
        ticket,
        selectedProjectId.value,
        capturedSnapshot,
        snapshot.value,
      )
    ) {
      return;
    }
    if (delta.trace_spans.length || delta.lifecycle_events.length) {
      snapshot.value.observability = mergeObservability(snapshot.value.observability, delta);
      liveMessage.value = "New control-plane observability events received";
    }
    observabilityBackoffMs = OBSERVABILITY_INTERVAL_MS;
  } catch (cause) {
    if (cause instanceof DOMException && cause.name === "AbortError") return;
    if (
      !snapshot.value ||
      !isCurrentProjectSnapshot(
        observabilityRequests,
        ticket,
        selectedProjectId.value,
        capturedSnapshot,
        snapshot.value,
      )
    ) {
      return;
    }
    observabilityBackoffMs = nextRefreshBackoff(
      observabilityBackoffMs,
      true,
      OBSERVABILITY_INTERVAL_MS,
      MAX_REFRESH_BACKOFF_MS,
    );
  } finally {
    observabilityRequests.finish(ticket);
  }
}

function scheduleObservability(): void {
  window.clearTimeout(observabilityTimer);
  const jitter = Math.floor(Math.random() * 500);
  observabilityTimer = window.setTimeout(async () => {
    if (document.visibilityState === "visible") await syncObservability();
    scheduleObservability();
  }, observabilityBackoffMs + jitter);
}

function handleVisibility(): void {
  if (document.visibilityState === "visible") {
    void refresh();
    void syncObservability();
  }
}

async function copyCommand(command: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(command);
  } catch {
    const textarea = document.createElement("textarea");
    textarea.value = command;
    textarea.setAttribute("readonly", "");
    textarea.className = "clipboard-fallback";
    document.body.appendChild(textarea);
    textarea.select();
    let copied = false;
    try {
      copied = document.execCommand("copy");
    } catch {
      copied = false;
    } finally {
      textarea.remove();
    }
    if (!copied) {
      toast.value = "Copy failed. Clipboard access is unavailable.";
      liveMessage.value = toast.value;
      window.clearTimeout(toastTimer);
      toastTimer = window.setTimeout(() => { toast.value = null; }, 5_000);
      return;
    }
  }
  window.clearTimeout(toastTimer);
  toast.value = "Command copied";
  liveMessage.value = "Command copied to clipboard";
  toastTimer = window.setTimeout(() => {
    toast.value = null;
  }, 2_500);
}

function filterLabel(filter: ProjectState | "all"): string {
  return filter === "all" ? "All" : projectStateLabel[filter];
}

function filterCount(filter: ProjectState | "all"): number {
  const all = snapshot.value?.projects ?? [];
  return filter === "all" ? all.length : all.filter((project) => project.state === filter).length;
}

onMounted(() => {
  mounted = true;
  selectedProjectId.value = window.localStorage.getItem("hzr.dashboard.project");
  refreshRequests.switchProject(selectedProjectId.value);
  observabilityRequests.switchProject(selectedProjectId.value);
  void refresh();
  scheduleRefresh();
  scheduleObservability();
  document.addEventListener("visibilitychange", handleVisibility);
});

onBeforeUnmount(() => {
  mounted = false;
  refreshRequests.abort();
  observabilityRequests.abort();
  window.clearTimeout(refreshTimer);
  window.clearTimeout(observabilityTimer);
  window.clearTimeout(toastTimer);
  document.removeEventListener("visibilitychange", handleVisibility);
});
</script>

<template>
  <a class="skip-link" href="#main-content">Skip to dashboard</a>
  <div class="app-shell">
    <header class="dashboard-header">
      <div class="dashboard-topbar">
        <a class="wordmark" href="#main-content" aria-label="HZR dashboard home" @click="section = 'overview'">
          <span class="wordmark-glyph" aria-hidden="true">H</span>
          <span class="wordmark-copy"><strong>HZR</strong><small>Agent control plane</small></span>
        </a>
        <div v-if="snapshot" class="dashboard-status">
          <span class="version-pill">Daemon v{{ snapshot.hzr_version }}</span>
          <StatusChip v-if="projectSnapshotCurrent" :state="snapshot.overall_state" :label="postureLabel" />
          <span v-else class="loading-scope">Switching project…</span>
          <button class="refresh-action" type="button" :disabled="manualRefreshing" :aria-busy="manualRefreshing" @click="refresh(true)">
            <AppIcon name="refresh" :size="16" /><span>{{ manualRefreshing ? "Refreshing…" : "Refresh" }}</span>
          </button>
        </div>
      </div>
      <div class="workspace-toolbar">
        <div><span class="eyebrow">Workspace intelligence</span><h1>{{ selectedProjectLabel }}</h1><p>Useful output. Visible gaps. Evidence before savings claims.</p></div>
        <label v-if="snapshot" class="workspace-select">
          <span>Project scope</span>
          <select :value="selectedProjectId ?? ''" @change="selectProject(($event.target as HTMLSelectElement).value || null)">
            <option value="">Select a workspace</option>
            <option v-for="project in snapshot.projects" :key="project.worktree_id" :value="project.worktree_id">{{ project.name }} · {{ projectStateLabel[project.state] }}</option>
          </select>
          <small>{{ snapshot.projects.length }} of {{ snapshot.projects_total }} loaded · private identities</small>
        </label>
      </div>
      <nav class="dashboard-navigation" aria-label="Dashboard sections">
        <button v-for="item in navigation" :key="item.id" type="button" :class="{ active: section === item.id }" :aria-current="section === item.id ? 'page' : undefined" @click="section = item.id">
          {{ item.label }}<span v-if="item.id === 'projects' && snapshot">{{ snapshot.projects_total }}</span>
        </button>
        <span v-if="snapshot" class="snapshot-freshness">{{ error ? "Last snapshot" : "Snapshot" }} {{ relativeTime(snapshot.generated_at_ms) }}</span>
      </nav>
    </header>

    <main id="main-content">
      <div v-if="error && snapshot" class="stale-ribbon" role="status">
        <AppIcon name="warning" :size="18" />
        <div>
          <strong>Live refresh paused</strong>
          <span>{{ error }}. Showing the last successful snapshot from {{ relativeTime(snapshot.generated_at_ms) }}.</span>
        </div>
        <button type="button" :disabled="manualRefreshing" @click="refresh(true)">Try again</button>
      </div>

      <section v-if="!snapshot && !error" class="loading-layout" aria-label="Loading dashboard">
        <div class="skeleton skeleton-wide"></div>
        <div class="skeleton-grid">
          <div v-for="item in 4" :key="item" class="skeleton skeleton-card"></div>
        </div>
        <div class="skeleton skeleton-panel"></div>
      </section>

      <section v-else-if="!snapshot && error" class="empty-state error-state" role="alert">
        <span class="empty-icon"><AppIcon name="warning" :size="24" /></span>
        <span class="eyebrow">Visualizer unavailable</span>
        <h2>HZR did not return a dashboard snapshot.</h2>
        <p>{{ error }}</p>
        <button class="primary-action" type="button" :disabled="manualRefreshing" @click="refresh(true)">
          <AppIcon name="refresh" :size="18" /> Try again
        </button>
      </section>

      <template v-else-if="snapshot">
        <section v-if="projectSnapshotCurrent && section === 'system'" class="section-block system-section" aria-labelledby="system-title">
          <div class="section-heading">
            <div>
              <span class="eyebrow">Live topology</span>
              <h2 id="system-title">The control plane pulse</h2>
            </div>
            <p>
              One persistent daemon supervises fork-core, ICM, and each project watcher.
              Readiness comes from live protocol, managed watcher, and artifact evidence.
            </p>
          </div>
          <p class="health-boundary">Component readiness and accounting coverage are independent. A ready engine does not establish complete interception or savings.</p>
          <div class="service-flow">
            <ServiceNode
              v-for="service in snapshot.services"
              :key="service.id"
              :service="service"
              @copy="copyCommand"
            />
          </div>
        </section>

        <section v-if="projectSnapshotCurrent && section === 'knowledge'" id="memory-observatory" class="observatory-grid" aria-label="Project memory and index observatories">
          <article class="observatory-panel memory-panel">
            <div class="observatory-head">
              <div>
                <span class="eyebrow">ICM memory observatory</span>
                <h2>Anonymous memory topology.</h2>
                <p>{{ snapshot.memory_observatory.detail }}</p>
              </div>
              <StatusChip :state="snapshot.memory_observatory.state" />
            </div>
            <div class="observatory-statline">
              <span><strong>{{ formatCount(snapshot.memory_observatory.memory_count) }}</strong> memories</span>
              <span><strong>{{ snapshot.memory_observatory.topics.length }}</strong> topics</span>
              <span><strong>{{ snapshot.memory_observatory.edges.length }}</strong> links</span>
              <span v-if="snapshot.memory_observatory.truncated" class="bounded-pill">
                Graph bounded<span v-if="snapshot.memory_observatory.hidden_memory_count"> · {{ snapshot.memory_observatory.hidden_memory_count }} hidden</span>
              </span>
              <span class="capability-pill">{{ snapshot.memory_observatory.retrieval.toUpperCase() }}</span>
            </div>
            <MemoryGraph
              :observatory="snapshot.memory_observatory"
              :project-id="snapshot.selected_worktree_id"
            />
            <div class="evidence-strip">
              <span><AppIcon name="check" :size="15" /> Positive repository filter</span>
              <span><AppIcon name="check" :size="15" /> Read-only snapshot</span>
              <span><AppIcon name="check" :size="15" /> Public details stay content-redacted</span>
              <span><AppIcon name="check" :size="15" /> Authenticated project tools expose bounded content</span>
              <span><AppIcon name="clock" :size="15" /> {{ snapshot.memory_observatory.latency_ms }}ms · {{ relativeTime(snapshot.memory_observatory.observed_at_ms) }}</span>
            </div>
          </article>

          <article class="observatory-panel index-panel">
            <div class="observatory-head">
              <div>
                <span class="eyebrow">grepai index observatory</span>
                <h2>Index health, without synthetic traffic.</h2>
                <p>{{ snapshot.index_observatory.watcher.detail }}. Routed activity is ledger-backed only.</p>
              </div>
              <StatusChip :state="snapshot.index_observatory.state" />
            </div>
            <IndexPipeline :observatory="snapshot.index_observatory" />
            <div class="search-activity-card">
              <div class="search-activity-orb" :class="`search-activity-${snapshot.index_observatory.search_activity.state}`">
                <AppIcon name="search" :size="24" />
              </div>
              <div v-if="snapshot.index_observatory.search_activity.operation">
                <span>Latest routed HZR search · ledger #{{ snapshot.index_observatory.search_activity.ledger_id }}</span>
                <strong>
                  {{ snapshot.index_observatory.search_activity.operation }} · private command
                </strong>
                <small>
                  {{ snapshot.index_observatory.search_activity.agent ?? "Unattributed agent" }} ·
                  project digest recorded ·
                  {{ snapshot.index_observatory.search_activity.execution_ms ?? 0 }}ms
                </small>
              </div>
              <div v-else>
                <span>No routed HZR search observed</span>
                <strong>The dashboard does not generate probe queries.</strong>
                <small>Waiting for a real search in this project's recent accounting ledger.</small>
              </div>
            </div>
            <dl class="index-evidence">
              <div><dt>Index status</dt><dd>{{ snapshot.index_observatory.artifacts.initialized ? "Initialized" : "Warming" }}</dd></div>
              <div><dt>Watcher</dt><dd>{{ snapshot.index_observatory.watcher.detail }}</dd></div>
              <div>
                <dt>Latest routed search</dt>
                <dd>{{ snapshot.index_observatory.search_activity.observed_at ? relativeTime(Date.parse(snapshot.index_observatory.search_activity.observed_at)) : "None observed" }}</dd>
              </div>
              <div><dt>Artifacts</dt><dd>{{ formatBytes(snapshot.index_observatory.artifacts.size_bytes) }} · {{ snapshot.index_observatory.artifacts.modified_at_ms ? relativeTime(snapshot.index_observatory.artifacts.modified_at_ms) : "No files" }}</dd></div>
              <div><dt>Watcher age</dt><dd>{{ snapshot.index_observatory.watcher.uptime_ms !== null ? formatDuration(snapshot.index_observatory.watcher.uptime_ms) : "Standby" }}</dd></div>
              <div><dt>Ownership</dt><dd>{{ snapshot.index_observatory.watcher.owned_by_hzr ? "HZR managed" : "Not attached" }}</dd></div>
            </dl>
          </article>
        </section>

        <section v-if="!projectSnapshotCurrent" class="loading-layout project-transition" aria-live="polite" aria-busy="true">
          <div class="loading-panel">
            <span class="spinner"></span>
            <strong>Loading the selected project boundary</strong>
            <p>Previous project memory, index, accounting, and traces are hidden until the scoped snapshot is verified.</p>
          </div>
        </section>

        <section v-if="projectSnapshotCurrent && section === 'overview'" id="activity-observatory" class="metrics-section" aria-labelledby="metrics-title">
          <div class="section-heading metrics-heading">
            <div>
              <span class="eyebrow">Verifiable accounting</span>
              <h2 id="metrics-title">What the evidence shows</h2>
            </div>
            <p>
              Output observations for the selected workspace.
              Provider receipts and global estimates have separate sections below.
            </p>
          </div>

          <div class="component-strip" aria-label="Component states">
            <button v-for="service in snapshot.services" :key="service.id" type="button" @click="section = 'system'">
              <span>{{ service.name }}</span><StatusChip :state="service.state" compact />
            </button>
          </div>
          <div class="metric-group local-group">
            <div class="metric-group-title">
              <span class="metric-group-mark local-mark"><AppIcon name="activity" :size="18" /></span>
              <div><strong>Project output ledger</strong><span>Estimated output sizes · {{ snapshot.local_activity.accounting_policy_version }}</span></div>
              <span class="metric-legend">Project-scoped · {{ formatCount(snapshot.local_activity.excluded_legacy_operations) }} legacy excluded</span>
            </div>
            <EvidenceOverview :activity="snapshot.local_activity" :receipts="snapshot.provider_receipts" :selected="selectedProjectId !== null" />
            <LiveActivity
              :key="snapshot.selected_worktree_id ?? 'unselected'"
              :operations="snapshot.local_activity.recent_operations"
              :optimized-count="snapshot.local_activity.optimized_operations"
              :raw-count="snapshot.local_activity.raw_operations"
              :native-count="snapshot.local_activity.native_unaccounted_operations"
              :unmeasured-count="snapshot.local_activity.unmeasured_bypass_operations"
              :measurement="snapshot.local_activity.measurement"
            />
            <SessionRoi :roi="snapshot.session_roi" />
            <details class="detail-disclosure"><summary>Request traces & lifecycle events</summary><ObservabilityTimeline :observability="snapshot.observability" /></details>
          </div>

          <details class="metric-group estimate-group detail-disclosure"><summary>All-workspace output estimates <span>Global scope</span></summary>
            <div class="metric-group-title">
              <span class="metric-group-mark estimate-mark"><AppIcon name="cpu" :size="18" /></span>
              <div><strong>All-workspace efficiency ledger</strong><span>Global operational context · {{ snapshot.estimated_efficiency.accounting_policy_version }}</span></div>
              <span class="metric-legend">Global estimate · {{ formatCount(snapshot.estimated_efficiency.excluded_legacy_operations) }} legacy excluded</span>
            </div>
            <div class="metric-grid metric-grid-four">
              <MetricCard
                eyebrow="Estimate"
                :value="formatCount(snapshot.estimated_efficiency.operations)"
                label="Filtered operations"
                :detail="`${formatDuration(snapshot.estimated_efficiency.total_execution_ms)} measured execution`"
                icon="activity"
                tone="estimated"
              />
              <MetricCard
                eyebrow="Estimate"
                :value="formatSignedCount(snapshot.estimated_efficiency.net_avoided_tokens_estimated)"
                label="Net avoided tokens"
                :detail="`${formatCount(snapshot.estimated_efficiency.regression_tokens_estimated)} regression tokens removed`"
                icon="memory"
                tone="estimated"
              />
              <MetricCard
                eyebrow="Estimate"
                :value="formatPercent(snapshot.estimated_efficiency.reduction_pct)"
                label="Direct reduction"
                detail="Not a provider billing claim"
                icon="database"
                tone="estimated"
              />
              <MetricCard
                eyebrow="Estimate"
                :value="formatCount(snapshot.estimated_efficiency.delivered_tokens_estimated)"
                label="Delivered tokens"
                :detail="`${formatCount(snapshot.estimated_efficiency.baseline_tokens_estimated)} baseline estimate`"
                icon="clock"
                tone="estimated"
              />
            </div>
          </details>

          <details class="metric-group provider-group detail-disclosure"><summary>Provider receipts <span>Separately attributed usage</span></summary>
            <div class="metric-group-title">
              <span class="metric-group-mark observed-mark"><AppIcon name="database" :size="18" /></span>
              <div><strong>Provider receipt coverage</strong><span>Externally attributed usage only; not project-scoped unless the provider supplies attribution</span></div>
              <span class="metric-legend">Observed</span>
            </div>
            <div v-if="snapshot.provider_receipts.state === 'available'" class="metric-grid">
              <MetricCard
                eyebrow="Receipts"
                :value="formatCount(snapshot.provider_receipts.records)"
                label="Provider records"
                :detail="`${formatCount(snapshot.provider_receipts.accepted)} accepted`"
                icon="activity"
                tone="observed"
              />
              <MetricCard
                eyebrow="Actual"
                :value="formatCount(totalProviderTokens)"
                label="Provider tokens"
                :detail="`${formatCount(snapshot.provider_receipts.actual_input_tokens)} input · ${formatCount(snapshot.provider_receipts.actual_output_tokens)} output`"
                icon="memory"
                tone="observed"
              />
              <MetricCard
                eyebrow="Actual"
                :value="formatCost(snapshot.provider_receipts.cost_microusd)"
                label="Provider cost"
                detail="Attributed receipts only"
                icon="database"
                tone="observed"
              />
            </div>
            <div v-else class="receipt-empty">
              <span class="receipt-icon"><AppIcon name="database" :size="22" /></span>
              <div>
                <strong>No provider receipts connected</strong>
                <p>{{ snapshot.provider_receipts.detail }}</p>
              </div>
              <span class="receipt-state">No data ≠ zero usage</span>
            </div>
          </details>
        </section>

        <section v-if="section === 'projects'" class="section-block projects-section" aria-labelledby="projects-title">
          <div class="section-heading projects-heading">
            <div>
              <span class="eyebrow">Workspace registry</span>
              <h2 id="projects-title">HZR projects</h2>
            </div>
            <div class="project-search">
              <AppIcon name="search" :size="18" />
              <label class="sr-only" for="project-search">Search projects</label>
              <input
                id="project-search"
                v-model="query"
                type="search"
                placeholder="Search project or stable ID"
                autocomplete="off"
              />
              <span
                class="project-result-count"
                :aria-label="`${projects.length} visible project${projects.length === 1 ? '' : 's'}`"
              >{{ projects.length }}</span>
            </div>
          </div>

          <div class="filter-row" role="group" aria-label="Filter projects by status">
            <button
              v-for="filter in projectFilters"
              :key="filter"
              type="button"
              :class="{ active: projectFilter === filter }"
              :aria-pressed="projectFilter === filter"
              @click="projectFilter = filter"
            >
              {{ filterLabel(filter) }} <span>{{ filterCount(filter) }}</span>
            </button>
          </div>

          <p class="loaded-scope">{{ projects.length }} matching of {{ snapshot.projects.length }} loaded projects · {{ snapshot.projects_total }} registered. Filters apply to loaded projects.</p>
          <p v-if="projectPageError" class="inline-error" role="alert">{{ projectPageError }}. Use Load more to retry.</p>
          <div v-if="snapshot.registry_warnings > 0" class="registry-warning" role="status">
            <AppIcon name="warning" :size="18" />
            {{ snapshot.registry_warnings }} invalid or unsafe registry entr{{ snapshot.registry_warnings === 1 ? "y was" : "ies were" }} ignored.
          </div>

          <div v-if="projects.length" class="project-list">
            <ProjectCard
              v-for="project in projects"
              :key="project.worktree_id"
              :project="project"
              :selected="snapshot.selected_worktree_id === project.worktree_id"
              @copy="copyCommand"
              @select="(worktreeId) => { section = 'overview'; selectProject(worktreeId); }"
            />
          </div>
          <button
            v-if="snapshot.projects_next_offset !== null"
            class="secondary-action project-load-more"
            type="button"
            :disabled="loadingProjects"
            @click="loadMoreProjects"
          >
            <AppIcon name="refresh" :size="16" />
            {{ loadingProjects ? "Loading projects" : `Load more · ${snapshot.projects.length}/${snapshot.projects_total}` }}
          </button>
          <div v-else-if="snapshot.projects.length === 0" class="empty-state">
            <span class="empty-icon"><AppIcon name="folder" :size="24" /></span>
            <span class="eyebrow">No registered workspaces</span>
            <h3>Initialize HZR inside a project.</h3>
          <p>The project will appear here on the next automatic refresh.</p>
            <button class="secondary-action" type="button" @click="copyCommand('hzr init --if-needed')">
              <AppIcon name="copy" :size="16" /> Copy <code>hzr init --if-needed</code>
            </button>
          </div>
          <div v-else-if="!projects.length" class="empty-state">
            <span class="empty-icon"><AppIcon name="search" :size="24" /></span>
            <span class="eyebrow">No match</span>
            <h3>No project matches these filters.</h3>
            <p>Clear the query or choose another project state.</p>
            <button class="secondary-action" type="button" @click="query = ''; projectFilter = 'all'">
              Reset filters
            </button>
          </div>
        </section>

        <section v-if="section === 'system'" class="help-section" aria-labelledby="help-title">
          <div class="section-heading">
            <div>
              <span class="eyebrow">Operator deck</span>
              <h2 id="help-title">Exact next commands</h2>
            </div>
            <p>Read-only UI, exact CLI. Copy a bounded diagnostic command when you need depth.</p>
          </div>
          <div class="command-grid">
            <CommandCard
              v-for="item in snapshot.help"
              :key="item.command"
              :item="item"
              @copy="copyCommand"
            />
          </div>
        </section>
      </template>
    </main>

    <footer v-if="snapshot" class="site-footer">
      <div class="footer-brand">
        <span class="wordmark-glyph small" aria-hidden="true">H</span>
        <div><strong>HZR Visualizer</strong><span>Private · local · loopback only</span></div>
      </div>
      <div class="footer-meta">
        <span>UI v{{ uiVersion }}</span>
        <span>Protocol {{ snapshot.protocol_version }}</span>
        <span>Snapshot {{ relativeTime(snapshot.generated_at_ms) }}</span>
      </div>
    </footer>

    <Transition name="toast">
      <div v-if="toast" class="copy-toast" role="status">
        <span><AppIcon :name="toast.startsWith('Copy failed') ? 'warning' : 'check'" :size="18" /></span>{{ toast }}
      </div>
    </Transition>
    <p class="sr-only" aria-live="polite">{{ liveMessage }}</p>
  </div>
</template>
