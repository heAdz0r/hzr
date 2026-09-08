<script setup lang="ts">
import { computed, ref } from "vue";
import AppIcon from "./AppIcon.vue";
import type { DashboardCommandBreakdown, DashboardLocalOperation } from "../types";
import { formatCount, formatSignedCount } from "../utils";
import { filterActivity, tokenBarWidth, type ActivityRoute } from "../evidence";

const props = defineProps<{
  operations: DashboardLocalOperation[];
  optimizedCount: number;
  rawCount: number;
  nativeCount: number;
  unmeasuredCount: number;
  measurement: string;
  /** Savings by command for the whole project scope. 0.9.1 */
  breakdown?: DashboardCommandBreakdown[];
  baseline?: number;
  delivered?: number;
  netAvoided?: number;
}>();

// 0.9.1: the one sentence a reader needs before any table — what HZR handed the
// model instead of what the tools produced, and how much that was.
const savingsPct = computed(() => {
  const baseline = props.baseline ?? 0;
  return baseline > 0 ? ((props.netAvoided ?? 0) * 100) / baseline : null;
});
const breakdownRows = computed(() => {
  const rows = props.breakdown ?? [];
  const maxBaseline = Math.max(1, ...rows.map((row) => row.baseline_tokens_estimated));
  return rows.map((row) => ({
    ...row,
    pct: row.baseline_tokens_estimated > 0 ? (row.net_avoided_tokens_estimated * 100) / row.baseline_tokens_estimated : null,
    baselineWidth: tokenBarWidth(row.baseline_tokens_estimated, maxBaseline),
    deliveredWidth: tokenBarWidth(row.delivered_tokens_estimated, maxBaseline),
  }));
});
/** What the row says it ran: the recorded summary, else the operation family. */
function commandLabel(operation: DashboardLocalOperation): string {
  return operation.command_summary ?? operation.operation;
}
const selectedKey = ref<string | null>(null);
const routeFilter = ref<ActivityRoute>("all");
const agentFilter = ref("");
const sessionFilter = ref("");
const visibleOperations = computed(() =>
  filterActivity(props.operations, routeFilter.value, agentFilter.value, sessionFilter.value),
);
/**
 * Sessions, described by what they did rather than by their digest.
 *
 * The digest is the only stable identity this endpoint publishes — commands and
 * paths deliberately never leave it — but a truncated hash tells a reader
 * nothing about which session they are filtering to. The agent that ran it, how
 * many operations it covers and when it was last seen are all already in this
 * snapshot, and together they identify a session to a human.
 */
const sessions = computed(() => {
  const groups = new Map<string, { agents: Set<string>; count: number; last: string }>();
  for (const operation of props.operations) {
    const key = operation.session_hash ?? "Unattributed";
    const current = groups.get(key) ?? { agents: new Set<string>(), count: 0, last: operation.timestamp };
    current.agents.add(operation.agent ?? "Unattributed");
    current.count += 1;
    if (Date.parse(operation.timestamp) > Date.parse(current.last)) current.last = operation.timestamp;
    groups.set(key, current);
  }
  return [...groups.entries()]
    .sort((left, right) => Date.parse(right[1].last) - Date.parse(left[1].last))
    .map(([id, value], index) => ({
      id,
      label:
        id === "Unattributed"
          ? "Unattributed operations"
          : `Session ${index + 1} · ${[...value.agents].join(", ")} · ${value.count} ops · ${operationTime(value.last)}`,
    }));
});
function resetFilters(): void {
  routeFilter.value = "all";
  agentFilter.value = "";
  sessionFilter.value = "";
}
const maxTokens = computed(() =>
  Math.max(1, ...props.operations.flatMap((operation) => [operation.baseline_tokens_estimated, operation.delivered_tokens_estimated])),
);
const totalCount = computed(
  () => props.optimizedCount + props.rawCount + props.nativeCount + props.unmeasuredCount,
);
const optimizedShare = computed(() =>
  totalCount.value === 0 ? 0 : (props.optimizedCount * 100) / totalCount.value,
);
const rawShare = computed(() =>
  totalCount.value === 0 ? 0 : (props.rawCount * 100) / totalCount.value,
);
const gapShare = computed(() =>
  totalCount.value === 0
    ? 0
    : ((props.nativeCount + props.unmeasuredCount) * 100) / totalCount.value,
);
const recentAgents = computed(() => {
  const groups = new Map<string, { count: number; last: string }>();
  for (const operation of props.operations) {
    const agent = operation.agent ?? "Unattributed";
    const current = groups.get(agent) ?? { count: 0, last: operation.timestamp };
    current.count += 1;
    if (Date.parse(operation.timestamp) > Date.parse(current.last)) current.last = operation.timestamp;
    groups.set(agent, current);
  }
  return [...groups.entries()]
    .map(([agent, value]) => ({ agent, ...value }))
    .sort((left, right) => right.count - left.count);
});

function width(value: number): string {
  return tokenBarWidth(value, maxTokens.value);
}

function operationTime(timestamp: string): string {
  const parsed = Date.parse(timestamp);
  return Number.isNaN(parsed)
    ? timestamp
    : new Intl.DateTimeFormat("en-US", {
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
        hour12: false,
      }).format(parsed);
}

function operationKey(operation: DashboardLocalOperation): string {
  return String(operation.ledger_id);
}

function shortHash(hash: string | null): string {
  if (!hash) return "Not recorded";
  return hash.length > 24 ? `${hash.slice(0, 15)}…${hash.slice(-6)}` : hash;
}

function creditedSaving(operation: DashboardLocalOperation): number {
  return operation.route === "optimized" ? operation.net_avoided_tokens_estimated : 0;
}

function routeDetail(operation: DashboardLocalOperation): string {
  if (operation.route === "raw") return "RAW · zero savings credit";
  if (operation.route === "native_unaccounted") return "Native observed · outside ratio";
  return "Optimized";
}

</script>

<template>
  <div class="live-activity">
    <div class="live-activity-head">
      <div><span class="live-beacon" aria-hidden="true"></span><strong>Recent HZR ledger activity</strong></div>
      <span>Latest successful snapshot · not process liveness</span>
    </div>

    <div class="activity-context-grid">
      <section>
        <header><span>Recently observed agents</span><b>{{ recentAgents.length }}</b></header>
        <div v-if="recentAgents.length" class="context-chip-list">
          <span v-for="agent in recentAgents" :key="agent.agent">
            <i :class="{ unattributed: agent.agent === 'Unattributed' }"></i>
            <strong>{{ agent.agent }}</strong>
            <small>{{ agent.count }} ops</small>
          </span>
        </div>
        <p v-else>No agent-attributed operations in this project snapshot.</p>
      </section>
      <section>
        <header><span>Privacy boundary</span><b>ON</b></header>
        <p>Each row names its program, subcommand and flags. Operands — paths, queries, environment values, SQL, heredocs — are never recorded or returned.</p>
      </section>
    </div>

    <!-- 0.9.1: efficiency, stated plainly, then broken down by the commands that produced it. -->
    <section v-if="(baseline ?? 0) > 0" class="savings-brief" aria-label="What HZR saved in this project">
      <div class="savings-brief-head">
        <div>
          <span class="eyebrow">What HZR saved here</span>
          <p class="savings-sentence">
            Tools produced <strong>{{ formatCount(baseline ?? 0) }}</strong> tokens; HZR handed the model
            <strong>{{ formatCount(delivered ?? 0) }}</strong>
            <span v-if="savingsPct !== null" class="savings-pct" :class="{ negative: (netAvoided ?? 0) < 0 }">{{ savingsPct >= 0 ? "−" : "+" }}{{ Math.abs(savingsPct).toFixed(1) }}%</span>
          </p>
        </div>
        <div class="savings-brief-total" :class="{ negative: (netAvoided ?? 0) < 0 }">
          <strong>{{ formatSignedCount(netAvoided ?? 0) }}</strong>
          <span>estimated tokens the model never had to read</span>
        </div>
      </div>
      <div class="savings-scale" aria-hidden="true">
        <i class="savings-scale-baseline"></i>
        <i class="savings-scale-delivered" :style="{ width: tokenBarWidth(delivered ?? 0, baseline ?? 1) }"></i>
      </div>
      <div v-if="breakdownRows.length" class="command-breakdown" role="table" aria-label="Savings by command">
        <div class="command-breakdown-row command-breakdown-head" role="row">
          <span role="columnheader">Command</span>
          <span role="columnheader">Runs</span>
          <span role="columnheader">Produced → delivered</span>
          <span role="columnheader">Saved</span>
        </div>
        <div v-for="row in breakdownRows" :key="row.command" class="command-breakdown-row" role="row">
          <code role="cell" :title="row.command">{{ row.command }}</code>
          <span role="cell" class="command-breakdown-runs">{{ formatCount(row.executions) }}<small v-if="row.optimized_executions < row.executions"> · {{ formatCount(row.executions - row.optimized_executions) }} raw</small></span>
          <span role="cell" class="command-breakdown-bars">
            <i class="baseline-bar" :style="{ width: row.baselineWidth }"></i>
            <i class="delivered-bar" :style="{ width: row.deliveredWidth }"></i>
            <small>{{ formatCount(row.baseline_tokens_estimated) }} → {{ formatCount(row.delivered_tokens_estimated) }}</small>
          </span>
          <span role="cell" class="command-breakdown-saved" :class="{ negative: row.net_avoided_tokens_estimated < 0 }">
            <strong>{{ formatSignedCount(row.net_avoided_tokens_estimated) }}</strong>
            <small v-if="row.pct !== null">{{ row.pct.toFixed(0) }}%</small>
          </span>
        </div>
      </div>
      <p v-else class="command-breakdown-empty">Command summaries are recorded from 0.9.1 on; rows written before that show only their family.</p>
    </section>

    <div class="route-summary">
      <div class="route-summary-bar" aria-label="Measured and uncovered operation share">
        <span class="route-summary-optimized" :style="{ width: `${optimizedShare}%` }"></span>
        <span class="route-summary-raw" :style="{ width: `${rawShare}%` }"></span>
        <span class="route-summary-gap" :style="{ width: `${gapShare}%` }"></span>
      </div>
      <span><strong>{{ formatCount(optimizedCount) }}</strong> optimized</span>
      <span><strong>{{ formatCount(rawCount) }}</strong> RAW · {{ rawShare.toFixed(1) }}%</span>
      <span><strong>{{ formatCount(nativeCount + unmeasuredCount) }}</strong> outside ratio · {{ gapShare.toFixed(1) }}%</span>
      <span class="raw-credit">RAW savings credit: 0</span>
    </div>

    <div class="activity-filters">
      <label>Route<select v-model="routeFilter"><option value="all">All routes</option><option value="optimized">Managed</option><option value="raw">Raw</option><option value="native_unaccounted">Native outside ratio</option><option value="regressions">Output growth</option></select></label>
      <label>Agent<select v-model="agentFilter"><option value="">All agents</option><option v-for="agent in recentAgents" :key="agent.agent" :value="agent.agent">{{ agent.agent }}</option></select></label>
      <label>Session<select v-model="sessionFilter"><option value="">All recent sessions</option><option v-for="session in sessions" :key="session.id" :value="session.id" :title="session.id">{{ session.label }}</option></select></label>
      <button class="ghost-action" type="button" :disabled="routeFilter === 'all' && !agentFilter && !sessionFilter" @click="resetFilters">Reset</button>
    </div>
    <p class="activity-filter-count" role="status">{{ visibleOperations.length }} of {{ operations.length }} recent operations · filters apply to this bounded snapshot</p>
    <div v-if="visibleOperations.length" class="activity-stream">
      <article v-for="operation in visibleOperations" :key="operationKey(operation)" class="activity-entry">
        <button
          class="activity-row"
          type="button"
          :aria-expanded="selectedKey === operationKey(operation)"
          @click="selectedKey = selectedKey === operationKey(operation) ? null : operationKey(operation)"
        >
          <time :datetime="operation.timestamp">{{ operationTime(operation.timestamp) }}</time>
          <span class="route-badge" :class="`route-${operation.route}`">{{ operation.route }}</span>
          <span class="activity-agent"><i></i>{{ operation.agent ?? "Unattributed" }}</span>
          <span class="activity-directory" title="Project identity is hashed">
            <AppIcon name="folder" :size="13" />private scope
          </span>
          <strong class="activity-command" :title="commandLabel(operation)">{{ commandLabel(operation) }}</strong>
          <div class="output-bars" :aria-label="`${operation.baseline_tokens_estimated} producer baseline tokens and ${operation.delivered_tokens_estimated} produced tokens`">
            <span class="baseline-bar" :style="{ width: width(operation.baseline_tokens_estimated) }"></span>
            <span class="delivered-bar" :style="{ width: width(operation.delivered_tokens_estimated) }"></span>
          </div>
          <span class="activity-volume">{{ formatCount(operation.baseline_tokens_estimated) }} → {{ formatCount(operation.delivered_tokens_estimated) }}</span>
          <span class="activity-saving" :class="{ negative: creditedSaving(operation) < 0 }">
            {{ formatSignedCount(creditedSaving(operation)) }}
          </span>
          <span class="activity-latency">{{ operation.execution_ms }}ms</span>
          <AppIcon class="activity-chevron" name="chevron" :size="14" />
        </button>

        <section v-if="selectedKey === operationKey(operation)" class="activity-detail">
          <div class="activity-detail-head">
            <div><span>Request evidence</span><strong>{{ operation.agent ?? "Unattributed agent" }}</strong></div>
            <span class="evidence-state"><AppIcon name="check" :size="14" /> Recorded by HZR</span>
          </div>
          <dl>
            <div class="wide"><dt>Command</dt><dd><code>{{ commandLabel(operation) }}</code><small v-if="!operation.command_summary"> · family only; summaries start at 0.9.1</small></dd></div>
            <div class="wide"><dt>Command digest</dt><dd><code>{{ shortHash(operation.command_hash) }}</code></dd></div>
            <div class="wide"><dt>Project digest</dt><dd><code>{{ shortHash(operation.project_hash) }}</code></dd></div>
            <div><dt>Agent</dt><dd>{{ operation.agent ?? "Unattributed" }}</dd></div>
            <div><dt>Session digest</dt><dd><code>{{ shortHash(operation.session_hash) }}</code></dd></div>
            <div><dt>Producer</dt><dd><code>{{ operation.producer_version ?? "legacy" }}</code></dd></div>
            <div><dt>Policy</dt><dd><code>{{ operation.policy_version ?? "legacy" }}</code></dd></div>
            <div><dt>Route</dt><dd>{{ routeDetail(operation) }}</dd></div>
            <div><dt>Latency</dt><dd>{{ operation.execution_ms }}ms</dd></div>
            <div><dt>Baseline estimate</dt><dd>{{ formatCount(operation.baseline_tokens_estimated) }} tokens</dd></div>
            <div><dt>Produced estimate</dt><dd>{{ formatCount(operation.delivered_tokens_estimated) }} tokens</dd></div>
            <div><dt>Credited delta</dt><dd>{{ formatSignedCount(creditedSaving(operation)) }} tokens</dd></div>
            <div class="wide"><dt>Measurement</dt><dd><code>{{ measurement }}</code></dd></div>
          </dl>
          <div v-if="operation.route === 'raw' && operation.replacement" class="activity-advice">
            <span>First-class route</span>
            <code>{{ operation.replacement }}</code>
            <small>{{ operation.rationale }}</small>
          </div>
        </section>
      </article>
    </div>
    <div v-else class="activity-empty">{{ operations.length ? "No recent operations match these filters. Reset filters to see the available snapshot." : "No operations in this project snapshot. Choose a project with recorded activity or refresh after an agent runs a command." }}</div>
    <p class="activity-footnote">
      Coverage: current privacy-typed rows for this project and its subdirectories. Command summaries carry program, subcommand and flags only. Arguments, queries, paths, environment values, SQL, heredocs, prompts, responses, stdin, and output bodies are not exposed here.
    </p>
  </div>
</template>
