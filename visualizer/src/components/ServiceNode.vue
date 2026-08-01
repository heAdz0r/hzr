<script setup lang="ts">
import type { DashboardService } from "../types";
import AppIcon from "./AppIcon.vue";
import StatusChip from "./StatusChip.vue";

defineProps<{
  service: DashboardService;
}>();

defineEmits<{
  copy: [command: string];
}>();

const iconFor = (id: string): "activity" | "cpu" | "database" | "memory" => {
  if (id === "rtk") return "cpu";
  if (id === "icm") return "memory";
  if (id === "grepai") return "database";
  return "activity";
};
</script>

<template>
  <article class="service-node" :class="`service-${service.state}`">
    <div class="service-node-top">
      <span class="service-glyph"><AppIcon :name="iconFor(service.id)" :size="24" /></span>
      <StatusChip :state="service.state" compact />
    </div>
    <div>
      <h3>{{ service.name }}</h3>
      <span class="service-version">{{ service.version ? `v${service.version}` : "Version unknown" }}</span>
    </div>
    <p>{{ service.detail }}</p>
    <button
      v-if="service.command"
      class="ghost-action"
      type="button"
      :aria-label="`Copy diagnostic command for ${service.name}`"
      @click="$emit('copy', service.command)"
    >
      <AppIcon name="terminal" :size="16" />
      Copy diagnostic
    </button>
  </article>
</template>
