<script setup lang="ts">
import { computed } from "vue";
import type { DashboardLocalActivity, DashboardProviderReceipts } from "../types";
import { formatCount, formatSignedCount } from "../utils";
import { outputReduction, tokenBarWidth } from "../evidence";
import AppIcon from "./AppIcon.vue";

const props = defineProps<{
  activity: DashboardLocalActivity;
  receipts: DashboardProviderReceipts;
  selected: boolean;
}>();
const reduction = computed(() => outputReduction(props.activity));
const gapCount = computed(() => props.activity.native_unaccounted_operations + props.activity.unmeasured_bypass_operations);
const maximum = computed(() => Math.max(props.activity.baseline_tokens_estimated, props.activity.delivered_tokens_estimated, 1));
</script>

<template>
  <div class="evidence-overview">
    <div v-if="!selected" class="scope-notice">
      <AppIcon name="folder" :size="20" />
      <div><strong>Select a project to inspect its activity</strong><p>The workspace selector above sets the boundary for every project metric.</p></div>
    </div>
    <div class="evidence-cards">
      <article class="evidence-card">
        <span class="evidence-label">Estimated net output reduction</span>
        <strong class="evidence-number" :class="{ 'value-negative': activity.net_avoided_tokens_estimated < 0 }">
          {{ activity.operations > 0 ? formatSignedCount(activity.net_avoided_tokens_estimated) : "—" }}
          <small v-if="activity.operations > 0">tokens</small>
        </strong>
        <span>{{ reduction === null ? "No baseline recorded" : `${reduction.toFixed(1)}% of the recorded output baseline` }}</span>
        <p>Includes {{ formatCount(activity.regression_tokens_estimated) }} estimated tokens of output growth.</p>
      </article>
      <article class="evidence-card">
        <span class="evidence-label">Recorded operations</span>
        <strong class="evidence-number">{{ selected ? formatCount(activity.operations) : "—" }}</strong>
        <span>{{ formatCount(activity.optimized_operations) }} managed · {{ formatCount(activity.raw_operations) }} raw</span>
        <p>A managed route does not by itself prove useful savings.</p>
      </article>
      <article class="evidence-card" :class="{ 'evidence-attention': gapCount > 0 }">
        <span class="evidence-label">Known coverage gaps</span>
        <strong class="evidence-number">{{ selected ? formatCount(gapCount) : "—" }}</strong>
        <span>Observed outside the measured ratio</span>
        <p>{{ gapCount > 0 ? "Inspect native and unmeasured bypasses below. Unseen calls are not counted." : "Zero known gaps does not prove all host calls were observed." }}</p>
      </article>
      <article class="evidence-card">
        <span class="evidence-label">Whole-task savings</span>
        <strong class="evidence-verdict">Not established</strong>
        <span>{{ receipts.state === "available" ? `${formatCount(receipts.records)} provider receipt records available` : "No provider receipts connected" }}</span>
        <p>Output estimates exclude model retries, input overhead and answer quality. Receipts alone are not a comparison.</p>
      </article>
    </div>
    <div v-if="activity.operations > 0" class="output-comparison" aria-label="Estimated output token comparison">
      <div class="comparison-label"><strong>Output token estimates</strong><span>Baseline compared with delivered output</span></div>
      <div class="comparison-row"><span>Baseline</span><div><i :style="{ width: tokenBarWidth(activity.baseline_tokens_estimated, maximum) }"></i></div><strong>{{ formatCount(activity.baseline_tokens_estimated) }}</strong></div>
      <div class="comparison-row delivered"><span>Delivered</span><div><i :style="{ width: tokenBarWidth(activity.delivered_tokens_estimated, maximum) }"></i></div><strong>{{ formatCount(activity.delivered_tokens_estimated) }}</strong></div>
      <p>Recorded output only · project scope · {{ formatCount(activity.excluded_legacy_operations) }} legacy operations excluded</p>
      <details class="measurement-detail"><summary>How these numbers are estimated</summary><p>{{ activity.measurement }}. This is the ledger’s output-sizing method. It does not measure the model’s full context, tool-call overhead, quality or provider invoice.</p></details>
    </div>
  </div>
</template>
