<script setup lang="ts">
import { computed } from "vue";
import type { DashboardState, ProjectState } from "../types";
import { dashboardStateLabel, projectStateLabel } from "../utils";

const props = defineProps<{
  state: DashboardState | ProjectState;
  compact?: boolean;
  /** Overrides the state's default wording where the state alone is ambiguous. */
  label?: string;
}>();

const text = computed(() => {
  if (props.label) {
    return props.label;
  }
  if (props.state === "warming" || props.state === "registered" || props.state === "unavailable") {
    return projectStateLabel[props.state];
  }
  return dashboardStateLabel[props.state];
});
</script>

<template>
  <span class="status-chip" :class="[`status-${state}`, { 'status-chip-compact': compact }]">
    <span class="status-dot" aria-hidden="true"></span>
    {{ text }}
  </span>
</template>
