<script setup lang="ts">
import AppIcon from "./AppIcon.vue";

defineProps<{
  eyebrow: string;
  value: string;
  label: string;
  detail: string;
  icon: "activity" | "clock" | "database" | "memory";
  tone?: "observed" | "estimated" | "neutral";
}>();
</script>

<template>
  <article class="metric-card" :class="`metric-${tone ?? 'neutral'}`">
    <div class="metric-head">
      <span class="metric-eyebrow">{{ eyebrow }}</span>
      <span class="metric-icon"><AppIcon :name="icon" :size="18" /></span>
    </div>
    <strong class="metric-value">{{ value }}</strong>
    <span class="metric-label">{{ label }}</span>
    <p>{{ detail }}</p>
  </article>
</template>
