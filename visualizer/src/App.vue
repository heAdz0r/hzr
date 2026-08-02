<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import CommandCard from "./components/CommandCard.vue";
import AppIcon from "./components/AppIcon.vue";
import MetricCard from "./components/MetricCard.vue";
import MemoryGraph from "./components/MemoryGraph.vue";
import IndexPipeline from "./components/IndexPipeline.vue";
import LiveActivity from "./components/LiveActivity.vue";
import ProjectCard from "./components/ProjectCard.vue";
import ServiceNode from "./components/ServiceNode.vue";
import StatusChip from "./components/StatusChip.vue";
import type { DashboardResponse, ProjectState } from "./types";
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

const REFRESH_INTERVAL_MS = 5_000;

const snapshot = ref<DashboardResponse | null>(null);
const error = ref<string | null>(null);
const manualRefreshing = ref(false);
const query = ref("");
const projectFilter = ref<ProjectState | "all">("all");
const toast = ref<string | null>(null);
const liveMessage = ref("");
let refreshTimer: number | undefined;
let toastTimer: number | undefined;
let refreshPromise: Promise<void> | null = null;
let refreshController: AbortController | null = null;
let mounted = false;

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

const readyProjectCount = computed(
  () => snapshot.value?.projects.filter((project) => project.state === "ready").length ?? 0,
);

const totalProviderTokens = computed(() => {
  const usage = snapshot.value?.provider_receipts;
  return usage ? usage.actual_input_tokens + usage.actual_output_tokens : 0;
});

const localReduction = computed(() => {
  const activity = snapshot.value?.local_activity;
  if (!activity || activity.baseline_tokens_estimated === 0) return 0;
  return (activity.net_avoided_tokens_estimated * 100) / activity.baseline_tokens_estimated;
});

async function refresh(manual = false): Promise<void> {
  if (refreshPromise) {
    if (manual) {
      await refreshPromise;
      if (mounted) await refresh(true);
    }
    return;
  }
  refreshController = new AbortController();
  if (manual) manualRefreshing.value = true;
  refreshPromise = (async () => {
    try {
      const response = await fetch("/v1/dashboard", {
        cache: "no-store",
        headers: { Accept: "application/json" },
        signal: refreshController?.signal,
      });
      if (!response.ok) {
        throw new Error(`Dashboard returned HTTP ${response.status}`);
      }
      const next = (await response.json()) as DashboardResponse;
      const previousState = snapshot.value?.overall_state;
      snapshot.value = next;
      error.value = null;
      if (manual) liveMessage.value = "Dashboard refreshed";
      if (previousState && previousState !== next.overall_state) {
        liveMessage.value = `System state changed to ${dashboardStateLabel[next.overall_state]}`;
      }
    } catch (cause) {
      if (cause instanceof DOMException && cause.name === "AbortError") return;
      error.value = cause instanceof Error ? cause.message : "Dashboard refresh failed";
      liveMessage.value = refreshFailureAnnouncement(snapshot.value !== null);
    } finally {
      manualRefreshing.value = false;
      refreshPromise = null;
    }
  })();
  await refreshPromise;
}

function scheduleRefresh(): void {
  window.clearInterval(refreshTimer);
  refreshTimer = window.setInterval(() => {
    if (document.visibilityState === "visible") void refresh();
  }, REFRESH_INTERVAL_MS);
}

function handleVisibility(): void {
  if (document.visibilityState === "visible") void refresh();
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
    document.execCommand("copy");
    textarea.remove();
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
  void refresh();
  scheduleRefresh();
  document.addEventListener("visibilitychange", handleVisibility);
});

onBeforeUnmount(() => {
  mounted = false;
  refreshController?.abort();
  window.clearInterval(refreshTimer);
  window.clearTimeout(toastTimer);
  document.removeEventListener("visibilitychange", handleVisibility);
});
</script>

<template>
  <a class="skip-link" href="#main-content">Skip to dashboard</a>
  <div class="app-shell">
    <header class="hero-header">
      <div class="hero-art" aria-hidden="true"></div>
      <div class="ember-line ember-line-one" aria-hidden="true"></div>
      <div class="ember-line ember-line-two" aria-hidden="true"></div>

      <nav class="topbar" aria-label="Product">
        <a class="wordmark" href="#main-content" aria-label="HZR dashboard home">
          <span class="wordmark-glyph" aria-hidden="true">H</span>
          <span class="wordmark-copy">
            <strong>HZR</strong>
            <small>Local control plane</small>
          </span>
        </a>
        <div class="topbar-meta" v-if="snapshot">
          <span class="version-pill">v{{ snapshot.hzr_version }}</span>
          <span class="endpoint-pill"><span></span>{{ snapshot.daemon_endpoint }}</span>
        </div>
      </nav>

      <div class="hero-content">
        <div class="hero-copy">
          <span class="eyebrow">Zero redundancy · full signal</span>
          <h1>One owner.<br /><span>Everything visible.</span></h1>
          <p>
            Projects, managed engines, health, and accounting — one local surface,
            served by the same HZR daemon.
          </p>
        </div>

        <div class="hero-status" v-if="snapshot">
          <div class="hero-status-head">
            <span>System posture</span>
            <StatusChip :state="snapshot.overall_state" />
          </div>
          <strong>{{ readyProjectCount }}/{{ snapshot.projects.length }}</strong>
          <span>projects index-ready</span>
          <div class="hero-status-foot">
            <span><AppIcon name="clock" :size="16" /> Uptime {{ formatDuration(snapshot.uptime_ms) }}</span>
            <button
              class="refresh-action"
              :class="{ 'is-loading': manualRefreshing }"
              type="button"
              :disabled="manualRefreshing"
              @click="refresh(true)"
            >
              <AppIcon name="refresh" :size="18" />
              <span>{{ manualRefreshing ? "Refreshing" : "Refresh" }}</span>
            </button>
          </div>
        </div>
      </div>
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
        <section class="section-block system-section" aria-labelledby="system-title">
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
          <div class="service-flow">
            <div class="flow-rail" aria-hidden="true"><span></span></div>
            <ServiceNode
              v-for="service in snapshot.services"
              :key="service.id"
              :service="service"
              @copy="copyCommand"
            />
          </div>
        </section>

        <section id="memory-observatory" class="observatory-grid" aria-label="Project memory and index observatories">
          <article class="observatory-panel memory-panel">
            <div class="observatory-head">
              <div>
                <span class="eyebrow">ICM memory observatory</span>
                <h2>Project knowledge, alive.</h2>
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
            <MemoryGraph :observatory="snapshot.memory_observatory" />
            <div class="evidence-strip">
              <span><AppIcon name="check" :size="15" /> Positive repository filter</span>
              <span><AppIcon name="check" :size="15" /> Read-only snapshot</span>
              <span><AppIcon name="check" :size="15" /> Details load on explicit topic selection</span>
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
              <div v-if="snapshot.index_observatory.search_activity.command">
                <span>Latest routed HZR search · ledger #{{ snapshot.index_observatory.search_activity.ledger_id }}</span>
                <strong :title="snapshot.index_observatory.search_activity.command">
                  {{ snapshot.index_observatory.search_activity.command }}
                </strong>
                <small>
                  {{ snapshot.index_observatory.search_activity.agent ?? "Unattributed agent" }} ·
                  {{ snapshot.index_observatory.search_activity.working_directory ?? "Unknown directory" }} ·
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

        <section id="activity-observatory" class="metrics-section" aria-labelledby="metrics-title">
          <div class="section-heading metrics-heading">
            <div>
              <span class="eyebrow">Verifiable accounting</span>
              <h2 id="metrics-title">This project. This ledger.</h2>
            </div>
            <p>
              Exact canonical-path records for <strong>{{ snapshot.local_activity.project ?? "the selected project" }}</strong>.
              Estimates remain named; provider receipts remain separate.
            </p>
          </div>

          <div class="metric-group local-group">
            <div class="metric-group-title">
              <span class="metric-group-mark local-mark"><AppIcon name="activity" :size="18" /></span>
              <div><strong>Verified local activity</strong><span>{{ snapshot.local_activity.measurement }}</span></div>
              <span class="metric-legend">Project-scoped</span>
            </div>
            <div class="metric-grid metric-grid-four">
              <MetricCard
                eyebrow="Ledger rows"
                :value="formatCount(snapshot.local_activity.operations)"
                label="Recorded operations"
                :detail="`${formatDuration(snapshot.local_activity.total_execution_ms)} measured execution`"
                icon="activity"
                tone="estimated"
              />
              <MetricCard
                eyebrow="Estimate"
                :value="formatSignedCount(snapshot.local_activity.net_avoided_tokens_estimated)"
                label="Net avoided tokens"
                :detail="`${formatCount(snapshot.local_activity.regression_tokens_estimated)} regression tokens accounted`"
                icon="memory"
                tone="estimated"
              />
              <MetricCard
                eyebrow="Estimate"
                :value="formatPercent(localReduction)"
                label="Project reduction"
                detail="Deterministic output sizing, not billing"
                icon="database"
                tone="estimated"
              />
              <MetricCard
                eyebrow="Delivered"
                :value="formatCount(snapshot.local_activity.delivered_tokens_estimated)"
                label="Delivered token estimate"
                :detail="`${formatCount(snapshot.local_activity.baseline_tokens_estimated)} baseline`"
                icon="clock"
                tone="estimated"
              />
            </div>
            <LiveActivity
              :operations="snapshot.local_activity.recent_operations"
              :optimized-count="snapshot.local_activity.optimized_operations"
              :raw-count="snapshot.local_activity.raw_operations"
              :measurement="snapshot.local_activity.measurement"
            />
          </div>

          <div class="metric-group estimate-group">
            <div class="metric-group-title">
              <span class="metric-group-mark estimate-mark"><AppIcon name="cpu" :size="18" /></span>
              <div><strong>All-workspace efficiency ledger</strong><span>Global operational context, separate from selected-project proof</span></div>
              <span class="metric-legend">Global estimate</span>
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
          </div>

          <div class="metric-group provider-group">
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
          </div>
        </section>

        <section class="section-block projects-section" aria-labelledby="projects-title">
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
                placeholder="Search name or exact path"
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

          <div v-if="snapshot.registry_warnings > 0" class="registry-warning" role="status">
            <AppIcon name="warning" :size="18" />
            {{ snapshot.registry_warnings }} invalid or unsafe registry entr{{ snapshot.registry_warnings === 1 ? "y was" : "ies were" }} ignored.
          </div>

          <div v-if="projects.length" class="project-list">
            <ProjectCard
              v-for="project in projects"
              :key="project.worktree_id"
              :project="project"
              @copy="copyCommand"
            />
          </div>
          <div v-else-if="snapshot.projects.length === 0" class="empty-state">
            <span class="empty-icon"><AppIcon name="folder" :size="24" /></span>
            <span class="eyebrow">No registered workspaces</span>
            <h3>Initialize HZR inside a project.</h3>
            <p>The project will appear here on the next five-second refresh.</p>
            <button class="secondary-action" type="button" @click="copyCommand('hzr init --if-needed')">
              <AppIcon name="copy" :size="16" /> Copy <code>hzr init --if-needed</code>
            </button>
          </div>
          <div v-else class="empty-state">
            <span class="empty-icon"><AppIcon name="search" :size="24" /></span>
            <span class="eyebrow">No match</span>
            <h3>No project matches these filters.</h3>
            <p>Clear the query or choose another project state.</p>
            <button class="secondary-action" type="button" @click="query = ''; projectFilter = 'all'">
              Reset filters
            </button>
          </div>
        </section>

        <section class="help-section" aria-labelledby="help-title">
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
        <span>UI v{{ snapshot.visualizer_version }}</span>
        <span>Protocol {{ snapshot.protocol_version }}</span>
        <span>Snapshot {{ relativeTime(snapshot.generated_at_ms) }}</span>
      </div>
    </footer>

    <Transition name="toast">
      <div v-if="toast" class="copy-toast" role="status">
        <span><AppIcon name="check" :size="18" /></span>{{ toast }}
      </div>
    </Transition>
    <p class="sr-only" aria-live="polite">{{ liveMessage }}</p>
  </div>
</template>
