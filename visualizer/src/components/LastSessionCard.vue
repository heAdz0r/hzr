<script setup lang="ts">
// 0.11.2: the last session in the selected project, stated first and plainly.
// Renders only the fields the daemon supplied; nothing is derived from other scopes.
import { computed } from "vue";
import type { DashboardLastSession } from "../types";
import { summarizeLastSession } from "../savings";
import { formatCount, formatDuration, formatMoney, formatSignedCount, relativeTime } from "../utils";

const props = defineProps<{ session: DashboardLastSession }>();
const summary = computed(() => summarizeLastSession(props.session));

const clock = new Intl.DateTimeFormat("en-US", { hour: "2-digit", minute: "2-digit", hour12: false });
const timeRange = computed(() => {
  const { startedAtMs, endedAtMs } = summary.value;
  if (startedAtMs === null && endedAtMs === null) return null;
  if (startedAtMs === null || endedAtMs === null) return clock.format((startedAtMs ?? endedAtMs) as number);
  const span = formatDuration(Math.max(0, endedAtMs - startedAtMs));
  return `${clock.format(startedAtMs)}–${clock.format(endedAtMs)} · ${span}`;
});
const title = computed(() => {
  const agent = props.session.agent;
  return `${agent ? `${agent} · ` : ""}${formatCount(summary.value.operations)} command${summary.value.operations === 1 ? "" : "s"}`;
});
</script>

<template>
  <article class="last-session" aria-labelledby="last-session-title">
    <header class="last-session-head">
      <div>
        <span class="eyebrow">Last session</span>
        <h3 id="last-session-title">{{ title }}</h3>
      </div>
      <p v-if="timeRange" class="last-session-time">
        <time v-if="summary.endedAtMs !== null" :datetime="new Date(summary.endedAtMs).toISOString()">{{ timeRange }}</time>
        <span v-else>{{ timeRange }}</span>
        <small v-if="summary.endedAtMs !== null"> · ended {{ relativeTime(summary.endedAtMs) }}</small>
      </p>
    </header>
    <div class="last-session-stats">
      <div v-if="summary.hostCapped" class="last-session-lead">
        <span>Tokens the model did not have to read</span>
        <strong :class="{ 'value-negative': summary.hostCapped.netAvoided < 0 }">{{ formatSignedCount(summary.hostCapped.netAvoided) }}</strong>
        <small v-if="summary.hostCapped.reductionPct !== null">{{ summary.hostCapped.reductionPct.toFixed(1) }}% less, after the host cap</small>
      </div>
      <div v-if="summary.producer">
        <span>Raw tool output → after HZR</span>
        <strong>{{ formatCount(summary.producer.baseline) }} → {{ formatCount(summary.producer.delivered) }}</strong>
        <small>Estimated tokens, before any host cap</small>
      </div>
      <div>
        <span>Commands</span>
        <strong>{{ formatCount(summary.operations) }}</strong>
        <small v-if="summary.optimizedOperations !== null || summary.rawOperations !== null">
          <template v-if="summary.optimizedOperations !== null">{{ formatCount(summary.optimizedOperations) }} through HZR</template>
          <template v-if="summary.optimizedOperations !== null && summary.rawOperations !== null"> · </template>
          <template v-if="summary.rawOperations !== null">{{ formatCount(summary.rawOperations) }} raw</template>
        </small>
      </div>
      <div v-if="summary.pricedValue">
        <span>Potential value at list price</span>
        <strong>{{ formatMoney(summary.pricedValue.currency, summary.pricedValue.savings_microunits) }}</strong>
        <small :title="summary.pricedValue.disclaimer">Preliminary · not a billed amount</small>
      </div>
    </div>
    <ol v-if="summary.topRoutes.length" class="last-session-routes" aria-label="Top routes in the last session">
      <li v-for="route in summary.topRoutes" :key="route.route">
        <code>{{ route.route }}</code>
        <span>{{ formatCount(route.operations) }}×<template v-if="typeof route.net_avoided_tokens_estimated === 'number'"> · {{ formatSignedCount(route.net_avoided_tokens_estimated) }}</template></span>
      </li>
    </ol>
  </article>
</template>
