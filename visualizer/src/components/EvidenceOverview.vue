<script setup lang="ts">
import { computed } from "vue";
import type { DashboardLocalActivity, DashboardProviderReceipts } from "../types";
import { formatCount, formatSignedCount } from "../utils";
import { tokenBarWidth } from "../evidence";
import { selectSavingsHeadline } from "../savings"; // 0.11.2
import AppIcon from "./AppIcon.vue";

const props = defineProps<{
  activity: DashboardLocalActivity;
  receipts: DashboardProviderReceipts;
  selected: boolean;
}>();
// 0.11.2: host-capped figures lead when the daemon supplies them; producer figures otherwise.
const headline = computed(() => selectSavingsHeadline(props.activity));
const hostCapped = computed(() => headline.value.source === "host_capped");
const primary = computed(() => headline.value.primary);
const gapCount = computed(() => props.activity.native_unaccounted_operations + props.activity.unmeasured_bypass_operations);
const maximum = computed(() => Math.max(primary.value.baseline, primary.value.delivered, 1));
const pct = (value: number | null) => (value === null ? null : `${value.toFixed(1)}%`); // 0.11.2
</script>

<template>
  <div class="evidence-overview">
    <div v-if="!selected" class="scope-notice">
      <AppIcon name="folder" :size="20" />
      <div><strong>Select a project to inspect its activity</strong><p>The workspace selector above sets the boundary for every project metric.</p></div>
    </div>
    <div class="evidence-cards">
      <!-- 0.11.2: plain-language headline; the method and its limits live in the disclosure below. -->
      <article class="evidence-card evidence-headline">
        <span class="evidence-label">{{ hostCapped ? "Tokens the model did not have to read" : "Estimated reduction in tool output" }}</span>
        <strong class="evidence-number" :class="{ 'value-negative': primary.netAvoided < 0 }">
          {{ activity.operations > 0 ? formatSignedCount(primary.netAvoided) : "—" }}
          <small v-if="activity.operations > 0">tokens</small>
        </strong>
        <span v-if="primary.reductionPct === null">No baseline recorded yet</span>
        <span v-else-if="hostCapped">{{ pct(primary.reductionPct) }} of the {{ formatCount(primary.baseline) }} tokens the host would have shown</span>
        <span v-else>{{ pct(primary.reductionPct) }} of the {{ formatCount(primary.baseline) }} tokens the tools produced</span>
        <p v-if="hostCapped" class="evidence-secondary">
          Before the host cap: {{ formatSignedCount(headline.producer.netAvoided) }} tokens<template v-if="headline.producer.reductionPct !== null"> ({{ pct(headline.producer.reductionPct) }})</template> cut from raw tool output.
        </p>
        <p v-else class="evidence-secondary">Measured at the tool. How much of it the model would actually have seen is not capped here.</p>
      </article>
      <article class="evidence-card">
        <span class="evidence-label">Commands recorded</span>
        <strong class="evidence-number">{{ selected ? formatCount(activity.operations) : "—" }}</strong>
        <span>{{ formatCount(activity.optimized_operations) }} through HZR · {{ formatCount(activity.raw_operations) }} passed through raw</span>
        <p>Routing through HZR does not by itself prove a saving.</p>
      </article>
      <article class="evidence-card" :class="{ 'evidence-attention': gapCount > 0 }">
        <span class="evidence-label">Not measured</span>
        <strong class="evidence-number">{{ selected ? formatCount(gapCount) : "—" }}</strong>
        <span>Commands seen outside the measurement</span>
        <p>{{ gapCount > 0 ? "Listed under Recent activity below. Calls HZR never saw are not counted." : "Zero known gaps does not prove every call was seen." }}</p>
      </article>
      <article class="evidence-card evidence-whole-task">
        <span class="evidence-label">Whole-task savings</span>
        <strong class="evidence-verdict">Not established</strong>
        <span>{{ receipts.state === "available" ? `${formatCount(receipts.records)} provider receipt records available` : "No provider receipts connected" }}</span>
        <p>Retries, prompt overhead and answer quality are not in these estimates.</p>
      </article>
    </div>
    <div v-if="activity.operations > 0" class="output-comparison" aria-label="Estimated token comparison">
      <div class="comparison-label">
        <strong>{{ hostCapped ? "What the host would have shown vs. what it did" : "Tool output before and after HZR" }}</strong>
        <span>Estimated tokens · this project</span>
      </div>
      <div class="comparison-row"><span>Before</span><div><i :style="{ width: tokenBarWidth(primary.baseline, maximum) }"></i></div><strong>{{ formatCount(primary.baseline) }}</strong></div>
      <div class="comparison-row delivered"><span>After</span><div><i :style="{ width: tokenBarWidth(primary.delivered, maximum) }"></i></div><strong>{{ formatCount(primary.delivered) }}</strong></div>
    </div>
    <!-- 0.11.2: every honesty disclosure kept, moved behind one expandable. -->
    <details class="measurement-detail evidence-disclosure">
      <summary>How these numbers are measured, and what they do not prove</summary>
      <ul>
        <li v-if="hostCapped">
          <strong>Host cap.</strong> Each command's output is capped at what the host would actually show the model<template v-if="headline.ceilingTokens !== null"> ({{ formatCount(headline.ceilingTokens) }} tokens per command)</template>, so savings on output the model would never have seen are not counted.
        </li>
        <li>
          <strong>Raw tool output (producer estimate).</strong> {{ formatCount(headline.producer.baseline) }} → {{ formatCount(headline.producer.delivered) }} tokens, net {{ formatSignedCount(headline.producer.netAvoided) }}. It already subtracts {{ formatCount(activity.regression_tokens_estimated) }} tokens where HZR's output grew.
        </li>
        <li>
          <strong>Explicit adapter delivery:</strong> {{ activity.explicit_delivery?.tokens_estimated == null ? "unknown" : formatCount(activity.explicit_delivery.tokens_estimated) + " estimated tokens" }}. A separate payload measurement — do not add it to the figures above. Complete delivery to the host remains unproven.
        </li>
        <li>
          <strong>Method.</strong> <code>{{ activity.measurement }}</code> · policy <code>{{ activity.accounting_policy_version }}</code> · {{ formatCount(activity.excluded_legacy_operations) }} legacy operations excluded. This sizes output only; it does not measure the model's full context, tool-call overhead, answer quality or the provider invoice.
        </li>
      </ul>
    </details>
  </div>
</template>
