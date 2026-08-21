<script setup lang="ts">
import { computed, ref } from "vue";
import AppIcon from "./AppIcon.vue";
import type { DashboardLocalOperation } from "../types";
import { formatCount, formatSignedCount } from "../utils";

const props = defineProps<{
  operations: DashboardLocalOperation[];
  optimizedCount: number;
  rawCount: number;
  nativeCount: number;
  unmeasuredCount: number;
  measurement: string;
}>();
const selectedKey = ref<string | null>(null);
const maxTokens = computed(() =>
  Math.max(1, ...props.operations.map((operation) => operation.baseline_tokens_estimated)),
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
  return `${Math.max(2, (value / maxTokens.value) * 100)}%`;
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
        <p>Commands, arguments, queries, paths, environment values, SQL, and heredocs are never returned by this endpoint.</p>
      </section>
    </div>

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

    <div v-if="operations.length" class="activity-stream">
      <article v-for="operation in operations" :key="operationKey(operation)" class="activity-entry">
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
          <strong>{{ operation.operation }}</strong>
          <div class="output-bars" :aria-label="`${operation.baseline_tokens_estimated} raw baseline tokens and ${operation.delivered_tokens_estimated} delivered tokens`">
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
            <div class="wide"><dt>Command digest</dt><dd><code>{{ shortHash(operation.command_hash) }}</code></dd></div>
            <div class="wide"><dt>Project digest</dt><dd><code>{{ shortHash(operation.project_hash) }}</code></dd></div>
            <div><dt>Agent</dt><dd>{{ operation.agent ?? "Unattributed" }}</dd></div>
            <div><dt>Session digest</dt><dd><code>{{ shortHash(operation.session_hash) }}</code></dd></div>
            <div><dt>Producer</dt><dd><code>{{ operation.producer_version ?? "legacy" }}</code></dd></div>
            <div><dt>Policy</dt><dd><code>{{ operation.policy_version ?? "legacy" }}</code></dd></div>
            <div><dt>Route</dt><dd>{{ routeDetail(operation) }}</dd></div>
            <div><dt>Latency</dt><dd>{{ operation.execution_ms }}ms</dd></div>
            <div><dt>Baseline estimate</dt><dd>{{ formatCount(operation.baseline_tokens_estimated) }} tokens</dd></div>
            <div><dt>Delivered estimate</dt><dd>{{ formatCount(operation.delivered_tokens_estimated) }} tokens</dd></div>
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
    <div v-else class="activity-empty">No routed operations have been recorded for this project yet.</div>
    <p class="activity-footnote">
      Coverage: current privacy-typed rows for this project and its subdirectories. Commands, arguments, queries, paths, environment values, SQL, heredocs, prompts, responses, stdin, and output bodies are not exposed here.
    </p>
  </div>
</template>
