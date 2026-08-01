<script setup lang="ts">
import { computed } from "vue";
import type { DashboardIndexObservatory, DashboardState } from "../types";

const props = defineProps<{ observatory: DashboardIndexObservatory }>();

const stages = computed<Array<{ label: string; detail: string; state: DashboardState }>>(() => [
  {
    label: "Config",
    detail: props.observatory.artifacts.initialized ? "loaded" : "missing",
    state: props.observatory.artifacts.initialized ? "ready" : "rebuilding",
  },
  {
    label: "Vectors",
    detail: props.observatory.artifacts.vectors_present ? "present" : "warming",
    state: props.observatory.artifacts.vectors_present ? "ready" : "rebuilding",
  },
  {
    label: "Symbols",
    detail: props.observatory.artifacts.symbols_present ? "present" : "warming",
    state: props.observatory.artifacts.symbols_present ? "ready" : "rebuilding",
  },
  {
    label: "Watcher",
    detail: props.observatory.watcher.pid ? `PID ${props.observatory.watcher.pid}` : "standby",
    state: props.observatory.watcher.state,
  },
  {
    label: "Semantic canary",
    detail: `${props.observatory.semantic.shown_hits} hits`,
    state: props.observatory.semantic.state,
  },
]);
</script>

<template>
  <div class="index-pipeline" role="list" aria-label="grepai indexing pipeline">
    <div
      v-for="(stage, index) in stages"
      :key="stage.label"
      class="pipeline-stage"
      :class="`pipeline-${stage.state}`"
      role="listitem"
    >
      <span class="pipeline-index">0{{ index + 1 }}</span>
      <span class="pipeline-light" aria-hidden="true"></span>
      <strong>{{ stage.label }}</strong>
      <small>{{ stage.detail }}</small>
    </div>
  </div>
</template>
