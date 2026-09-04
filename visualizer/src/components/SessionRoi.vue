<script setup lang="ts">
import type { DashboardSessionRoi } from "../types";
import { formatCount, formatEconomicAmount, formatSignedCount } from "../utils";

defineProps<{ roi: DashboardSessionRoi }>();
</script>

<template>
  <article class="session-roi" aria-labelledby="session-roi-title">
    <div class="session-roi-head">
      <div>
        <span class="eyebrow">Latest attributed session</span>
        <h3 id="session-roi-title">{{ roi.session_hash ? "Session output evidence" : "No attributed session" }}</h3>
        <p>{{ roi.detail }}</p>
      </div>
      <code v-if="roi.session_hash" :title="roi.session_hash">{{ roi.session_hash.slice(0, 20) }}…</code>
    </div>
    <div v-if="roi.operations" class="session-token-line">
      <div><span>Recorded operations</span><strong>{{ formatCount(roi.operations) }}</strong></div>
      <div><span>Producer output estimates</span><strong>{{ formatCount(roi.baseline_tokens_estimated) }} → {{ formatCount(roi.delivered_tokens_estimated) }}</strong></div>
      <div><span>Estimated net reduction</span><strong :class="{ 'value-negative': roi.net_avoided_tokens_estimated < 0 }">{{ formatSignedCount(roi.net_avoided_tokens_estimated) }}</strong></div>
    </div>
    <p>Explicit adapter delivery: <strong>{{ roi.explicit_delivery?.tokens_estimated == null ? "Unknown" : formatCount(roi.explicit_delivery.tokens_estimated) + " estimated tokens" }}</strong>. Separate from producer output; complete host receipt and causal linkage are unproven.</p>
    <div class="session-command-list">
      <strong>Top command families · output estimates</strong>
      <ol v-if="roi.top_commands.length">
        <li v-for="command in roi.top_commands" :key="command.command_family">
          <code>{{ command.command_family }}</code>
          <span>{{ formatCount(command.executions) }} runs · {{ formatSignedCount(command.net_avoided_tokens_estimated) }} tokens</span>
        </li>
      </ol>
      <span v-else>No attributed commands in the selected session.</span>
    </div>
    <details class="detail-disclosure">
      <summary>Pricing assumptions & imported claims <span>Not proof of billed savings</span></summary>
      <div v-if="roi.raw_public_estimate" class="session-roi-value">
        <span>Potential public-list savings · preliminary</span>
        <strong>{{ formatEconomicAmount(roi.raw_public_estimate.currency, roi.raw_public_estimate.savings_microunits) }}</strong>
        <small>{{ roi.raw_public_estimate.disclaimer }}</small>
      </div>
      <div v-else class="session-roi-unavailable">
        <strong>No public-list estimate</strong>
        <span>{{ roi.raw_public_estimate_unavailable_reason ?? "No claim-ready pricing evidence." }}</span>
      </div>
      <dl class="session-roi-evidence">
        <div><dt>Selected provider / model</dt><dd>{{ roi.selected_provider || "Not selected" }} · {{ roi.selected_model || "No model" }}</dd></div>
        <div><dt>Harness / method</dt><dd>{{ roi.selected_harness || "Not selected" }} · {{ roi.selected_method || "No method" }}</dd></div>
        <div><dt>Request input / pricing basis</dt><dd>{{ roi.selected_request_input_tokens ?? "Not supplied" }} tokens · {{ roi.selected_pricing_basis }}</dd></div>
        <div><dt>Catalog</dt><dd><code>{{ roi.raw_public_estimate?.catalog_identity ?? roi.catalog_identity ?? "Unavailable" }}</code></dd></div>
        <div><dt>Imported claims</dt><dd>{{ formatCount(roi.imported_claim_records) }} · {{ roi.receipt_provenance ?? "No provenance supplied" }}</dd></div>
        <div><dt>External verification</dt><dd>{{ roi.receipt_externally_verified ? "Reported as verified" : "Not verified" }}</dd></div>
      </dl>
      <div v-if="roi.reported_actual" class="session-roi-claim">
        <strong>User-supplied reported amount</strong>
        <span>{{ formatEconomicAmount(roi.reported_actual.currency, roi.reported_actual.savings_microunits) }} reported savings ({{ formatEconomicAmount(roi.reported_actual.currency, roi.reported_actual.baseline_microunits) }} → {{ formatEconomicAmount(roi.reported_actual.currency, roi.reported_actual.delivered_microunits) }}). This dashboard does not independently verify the claim.</span>
      </div>
    </details>
  </article>
</template>
