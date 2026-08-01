<script setup lang="ts">
import { computed } from "vue";
import type { DashboardState, ProjectState } from "../types";
import { dashboardStateLabel, projectStateLabel } from "../utils";

const props = defineProps<{
  state: DashboardState | ProjectState;
  compact?: boolean;
}>();

const label = computed(() => {
  if (props.state === "warming" || props.state === "registered" || props.state === "unavailable") {
    return projectStateLabel[props.state];
  }
  return dashboardStateLabel[props.state];
});
</script>

<template>
  <span class="status-chip" :class="[`status-${state}`, { 'status-chip-compact': compact }]">
    <span class="status-dot" aria-hidden="true"></span>
    {{ label }}
  </span>
</template>
