<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import CommandCard from "./components/CommandCard.vue";
import AppIcon from "./components/AppIcon.vue";
import MetricCard from "./components/MetricCard.vue";
import ProjectCard from "./components/ProjectCard.vue";
import ServiceNode from "./components/ServiceNode.vue";
import StatusChip from "./components/StatusChip.vue";
import type { DashboardResponse, ProjectState } from "./types";
import {
  dashboardStateLabel,
  filterProjects,
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
const refreshing = ref(false);
const query = ref("");
const projectFilter = ref<ProjectState | "all">("all");
const toast = ref<string | null>(null);
const liveMessage = ref("");
let refreshTimer: number | undefined;
let toastTimer: number | undefined;

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

const totalObservedTokens = computed(() => {
  const usage = snapshot.value?.observed_usage;
  return usage ? usage.actual_input_tokens + usage.actual_output_tokens : 0;
});

async function refresh(manual = false): Promise<void> {
  if (refreshing.value) return;
  refreshing.value = true;
  try {
    const response = await fetch("/v1/dashboard", {
      cache: "no-store",
      headers: { Accept: "application/json" },
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
    error.value = cause instanceof Error ? cause.message : "Dashboard refresh failed";
    liveMessage.value = refreshFailureAnnouncement(snapshot.value !== null);
  } finally {
    refreshing.value = false;
  }
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
  void refresh();
  scheduleRefresh();
  document.addEventListener("visibilitychange", handleVisibility);
});

onBeforeUnmount(() => {
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
              :class="{ 'is-loading': refreshing }"
              type="button"
              :disabled="refreshing"
              @click="refresh(true)"
            >
              <AppIcon name="refresh" :size="18" />
              <span>{{ refreshing ? "Refreshing" : "Refresh" }}</span>
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
        <button type="button" :disabled="refreshing" @click="refresh(true)">Try again</button>
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
        <button class="primary-action" type="button" :disabled="refreshing" @click="refresh(true)">
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
              One persistent daemon supervises the exact fork-core and managed engines.
              Standby means on-demand, not broken.
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

        <section class="metrics-section" aria-labelledby="metrics-title">
          <div class="section-heading metrics-heading">
            <div>
              <span class="eyebrow">Accounting boundary</span>
              <h2 id="metrics-title">Observed is not estimated</h2>
            </div>
            <p>
              Provider-observed usage stays separate from the deterministic
              <code>{{ snapshot.estimated_efficiency.measurement }}</code> counterfactual.
            </p>
          </div>

          <div class="metric-group observed-group">
            <div class="metric-group-title">
              <span class="metric-group-mark observed-mark"><AppIcon name="activity" :size="18" /></span>
              <div><strong>Observed usage</strong><span>Provider and runtime records</span></div>
              <span class="metric-legend">Actual</span>
            </div>
            <div class="metric-grid">
              <MetricCard
                eyebrow="Observed"
                :value="formatCount(snapshot.observed_usage.tasks)"
                label="Recorded tasks"
                :detail="`${formatCount(snapshot.observed_usage.accepted)} externally accepted`"
                icon="activity"
                tone="observed"
              />
              <MetricCard
                eyebrow="Observed"
                :value="formatCount(totalObservedTokens)"
                label="Actual tokens"
                :detail="`${formatCount(snapshot.observed_usage.actual_input_tokens)} input · ${formatCount(snapshot.observed_usage.actual_output_tokens)} output`"
                icon="memory"
                tone="observed"
              />
              <MetricCard
                eyebrow="Observed"
                :value="formatCost(snapshot.observed_usage.cost_microusd)"
                label="Provider cost"
                detail="Only provider-attributed records are included"
                icon="database"
                tone="observed"
              />
            </div>
          </div>

          <div class="metric-group estimate-group">
            <div class="metric-group-title">
              <span class="metric-group-mark estimate-mark"><AppIcon name="cpu" :size="18" /></span>
              <div><strong>Direct efficiency estimate</strong><span>Deterministic before/after output sizing</span></div>
              <span class="metric-legend">Estimate</span>
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
