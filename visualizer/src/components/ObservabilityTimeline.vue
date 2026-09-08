<script setup lang="ts">
import { computed } from "vue";
import AppIcon from "./AppIcon.vue";
import type { DashboardObservability } from "../types";
import { formatDuration, relativeTime } from "../utils";
import { groupTraceSpans } from "../observability";

const props = defineProps<{ observability: DashboardObservability }>();

const traces = computed(() => groupTraceSpans(props.observability.trace_spans));

function shortHash(value: string): string {
  return value.length > 24 ? `${value.slice(0, 15)}…${value.slice(-6)}` : value;
}

function label(value: string): string {
  return value.replaceAll("_", " ");
}
</script>

<template>
  <article class="observability-timeline" aria-labelledby="observability-title">
    <div class="observability-title-row">
      <div>
        <span class="eyebrow">Privacy-safe tracing</span>
        <h3 id="observability-title">Control-plane spans and lifecycle</h3>
      </div>
      <span class="bounded-pill">Bounded · cursor {{ observability.next_cursor ?? "empty" }}</span>
    </div>

    <div class="trace-lifecycle-grid">
      <section aria-label="Recent distributed traces">
        <header><strong>Recent traces</strong><span>{{ traces.length }}</span></header>
        <div v-if="traces.length" class="trace-list">
          <details v-for="trace in traces" :key="trace.hash">
            <!-- 0.9.1: one grid, label first and left-aligned, so a list of traces scans as a list. -->
            <summary class="trace-summary">
              <span class="trace-state" :class="{ failed: trace.failed }"></span>
              <span class="trace-title">
                <strong class="trace-label">{{ trace.label }}</strong>
                <small>{{ trace.spans.length }} span{{ trace.spans.length === 1 ? "" : "s" }}<template v-if="trace.linkedFrom"> · continues {{ shortHash(trace.linkedFrom) }}</template></small>
              </span>
              <code class="trace-hash" :title="trace.hash">{{ shortHash(trace.hash) }}</code>
              <time class="trace-time">{{ relativeTime(trace.observedAt) }}</time>
              <strong class="trace-duration">{{ formatDuration(trace.duration) }}</strong>
            </summary>
            <ol>
              <li v-if="trace.linkedFrom" class="trace-link-row">
                <span>↳</span>
                <strong>approval continuation</strong>
                <code>{{ shortHash(trace.linkedFrom) }}</code>
              </li>
              <li v-for="span in trace.spans" :key="`${trace.hash}-${span.span_id}`">
                <span>{{ span.span_id }}</span>
                <strong>{{ label(span.stage) }}</strong>
                <code>{{ span.engine }}</code>
                <em :class="`trace-${span.state}`">{{ label(span.state) }}</em>
                <small>{{ formatDuration(span.duration_ms) }}</small>
                <small v-if="span.route">{{ span.route }}</small>
                <small v-if="span.error_code" class="trace-error">{{ span.error_code }}</small>
              </li>
            </ol>
          </details>
        </div>
        <p v-else class="trace-empty">No traced requests since this daemon started. Spans live in memory and reset on restart; the next HZR-routed command in this workspace appears here.</p>
      </section>

      <section aria-label="Recent lifecycle events">
        <header><strong>Lifecycle history</strong><span>{{ observability.lifecycle_events.length }}</span></header>
        <ol v-if="observability.lifecycle_events.length" class="lifecycle-list">
          <li v-for="event in [...observability.lifecycle_events].reverse().slice(0, 16)" :key="event.sequence">
            <span class="lifecycle-icon"><AppIcon name="activity" :size="14" /></span>
            <div><strong>{{ event.engine }}</strong><span>{{ label(event.kind) }}</span></div>
            <code>{{ event.detail_code }}</code>
            <time>{{ relativeTime(event.observed_at_ms) }}</time>
          </li>
        </ol>
        <p v-else>No lifecycle transitions recorded since this daemon started.</p>
      </section>
    </div>

    <p class="observability-boundary">
      Trace and lifecycle rows contain closed states, keyed digests, timings, versions, and sanitized error codes only. Commands, paths, queries, payloads, output bodies, and raw provider trace IDs are never exposed.
    </p>
  </article>
</template>
