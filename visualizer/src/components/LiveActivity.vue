<script setup lang="ts">
import { computed } from "vue";
import type { DashboardLocalOperation } from "../types";
import { formatCount, formatSignedCount } from "../utils";

const props = defineProps<{
  operations: DashboardLocalOperation[];
  optimizedCount: number;
  rawCount: number;
}>();
const maxTokens = computed(() =>
  Math.max(1, ...props.operations.map((operation) => operation.baseline_tokens_estimated)),
);
const totalCount = computed(() => props.optimizedCount + props.rawCount);
const rawShare = computed(() =>
  totalCount.value === 0 ? 0 : (props.rawCount * 100) / totalCount.value,
);

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
</script>

<template>
  <div class="live-activity">
    <div class="live-activity-head">
      <div><span class="live-beacon" aria-hidden="true"></span><strong>Live routed output operations</strong></div>
      <span>RAW baseline → delivered estimate</span>
    </div>
    <div class="route-summary">
      <div class="route-summary-bar" aria-label="Optimized and raw operation share">
        <span class="route-summary-optimized" :style="{ width: `${100 - rawShare}%` }"></span>
        <span class="route-summary-raw" :style="{ width: `${rawShare}%` }"></span>
      </div>
      <span><strong>{{ formatCount(optimizedCount) }}</strong> optimized</span>
      <span><strong>{{ formatCount(rawCount) }}</strong> raw · {{ rawShare.toFixed(1) }}%</span>
      <span class="raw-credit">RAW savings credit: 0</span>
    </div>
    <div v-if="operations.length" class="activity-stream">
      <div v-for="(operation, index) in operations" :key="`${operation.timestamp}-${index}`" class="activity-row">
        <time :datetime="operation.timestamp">{{ operationTime(operation.timestamp) }}</time>
        <span class="route-badge" :class="`route-${operation.route}`">{{ operation.route }}</span>
        <strong>{{ operation.operation }}</strong>
        <div class="output-bars" :aria-label="`${operation.baseline_tokens_estimated} raw baseline tokens and ${operation.delivered_tokens_estimated} delivered tokens`">
          <span class="baseline-bar" :style="{ width: width(operation.baseline_tokens_estimated) }"></span>
          <span class="delivered-bar" :style="{ width: width(operation.delivered_tokens_estimated) }"></span>
        </div>
        <span class="activity-volume">{{ formatCount(operation.baseline_tokens_estimated) }} → {{ formatCount(operation.delivered_tokens_estimated) }}</span>
        <span class="activity-saving" :class="{ negative: operation.net_avoided_tokens_estimated < 0 }">
          {{ formatSignedCount(operation.net_avoided_tokens_estimated) }}
        </span>
        <span class="activity-latency">{{ operation.execution_ms }}ms</span>
      </div>
    </div>
    <div v-else class="activity-empty">No routed operations have been recorded for this project yet.</div>
    <p class="activity-footnote">
      Coverage: output-bearing fork-core rows for this project and its subdirectories. Codex host prompts/responses, memory recalls, and context selection are not captured here. Raw routes are explicit and receive zero savings credit.
    </p>
  </div>
</template>
