<script setup lang="ts">
import type { DashboardSessionRoi } from "../types";
import { formatCount, formatEconomicAmount, formatPercent, formatSignedCount } from "../utils";

defineProps<{ roi: DashboardSessionRoi }>();
</script>

<template>
  <article class="session-roi" aria-labelledby="session-roi-title">
    <div class="session-roi-head">
      <div>
        <span class="eyebrow">Latest attributed session</span>
        <h3 id="session-roi-title">Session ROI</h3>
        <p>{{ roi.detail }}</p>
      </div>
      <code v-if="roi.session_hash">{{ roi.session_hash.slice(0, 20) }}…</code>
    </div>

    <div v-if="roi.raw_public_estimate" class="session-roi-value">
      <span>Potential public-list savings</span>
      <strong>{{ formatEconomicAmount(roi.raw_public_estimate.currency, roi.raw_public_estimate.savings_microunits) }}</strong>
      <small>
        Preliminary from {{ formatCount(roi.raw_public_estimate.avoided_input_tokens_estimated) }} estimated avoided input tokens · not an invoice
      </small>
    </div>
    <div v-else class="session-roi-unavailable">
      <strong>Potential public-list savings unavailable</strong>
      <span>{{ roi.raw_public_estimate_unavailable_reason ?? "No claim-ready pricing evidence." }}</span>
    </div>

    <dl class="session-roi-evidence">
      <div><dt>Measured commands</dt><dd>{{ formatCount(roi.operations) }} measured command{{ roi.operations === 1 ? "" : "s" }}</dd></div>
      <div><dt>Estimated token flow</dt><dd>{{ formatCount(roi.baseline_tokens_estimated) }} → {{ formatCount(roi.delivered_tokens_estimated) }}</dd></div>
      <div><dt>Session net tokens</dt><dd>{{ formatSignedCount(roi.net_avoided_tokens_estimated) }} estimated · {{ roi.baseline_tokens_estimated > 0 ? formatPercent((roi.net_avoided_tokens_estimated * 100) / roi.baseline_tokens_estimated) : "No baseline" }}</dd></div>
      <div><dt>Selection</dt><dd>{{ roi.selected_provider || "Not selected" }} · {{ roi.selected_model || "No model" }}</dd></div>
      <div><dt>Harness / method</dt><dd>{{ roi.selected_harness || "Not selected" }} · {{ roi.selected_method || "No method" }}</dd></div>
      <div><dt>Priced request input / basis</dt><dd>{{ roi.selected_request_input_tokens ?? "Not supplied" }} tokens · {{ roi.selected_pricing_basis }}</dd></div>
      <div><dt>Catalog</dt><dd><code>{{ roi.raw_public_estimate?.catalog_identity ?? roi.catalog_identity ?? "Unavailable" }}</code></dd></div>
      <div><dt>Imported claims</dt><dd>{{ formatCount(roi.imported_claim_records) }} · {{ roi.receipt_provenance ?? "none" }} · externally verified={{ roi.receipt_externally_verified }}</dd></div>
    </dl>

    <div v-if="roi.reported_actual" class="session-roi-claim">
      <strong>User-supplied reported amount — unverified</strong>
      <span>
        Reported savings {{ formatEconomicAmount(roi.reported_actual.currency, roi.reported_actual.savings_microunits) }}
        ({{ formatEconomicAmount(roi.reported_actual.currency, roi.reported_actual.baseline_microunits) }} →
        {{ formatEconomicAmount(roi.reported_actual.currency, roi.reported_actual.delivered_microunits) }})
      </span>
    </div>

    <div class="session-command-list">
      <strong>Top command families</strong>
      <ol v-if="roi.top_commands.length">
        <li v-for="command in roi.top_commands" :key="command.command_family">
          <code>{{ command.command_family }}</code>
          <span>{{ formatCount(command.executions) }} runs · {{ formatSignedCount(command.net_avoided_tokens_estimated) }} tokens</span>
        </li>
      </ol>
      <span v-else>No attributed commands in the selected session.</span>
    </div>
  </article>
</template>
