<script setup lang="ts">
import type { DashboardHelpCommand } from "../types";
import AppIcon from "./AppIcon.vue";

defineProps<{
  item: DashboardHelpCommand;
}>();

defineEmits<{
  copy: [command: string];
}>();
</script>

<template>
  <article class="command-card">
    <div class="command-card-head">
      <span class="command-icon"><AppIcon name="terminal" :size="18" /></span>
      <h3>{{ item.label }}</h3>
    </div>
    <p>{{ item.description }}</p>
    <button type="button" @click="$emit('copy', item.command)">
      <code>{{ item.command }}</code>
      <AppIcon name="copy" :size="16" />
    </button>
  </article>
</template>
