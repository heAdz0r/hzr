<script setup lang="ts">
/**
 * The HZR logomark: a single flame on an ink tile.
 *
 * 0.9.1: the first mark tried to quote the whole hero bird and turned to noise
 * at 16 px. One flame with a hollow core reads at every size, keeps the ember
 * gradient of the illustration, and its off-centre lean is the same forward
 * sweep the bird has. The favicon is this exact geometry.
 */
defineProps<{ size?: number }>();
</script>

<template>
  <svg
    class="brand-mark"
    :width="size ?? 42"
    :height="size ?? 42"
    viewBox="0 0 64 64"
    role="img"
    aria-label="HZR"
  >
    <defs>
      <linearGradient id="hzr-ember" x1="20" y1="56" x2="46" y2="10" gradientUnits="userSpaceOnUse">
        <stop offset="0" stop-color="#cf3d18" />
        <stop offset="0.55" stop-color="#f36a21" />
        <stop offset="1" stop-color="#ffc46a" />
      </linearGradient>
      <linearGradient id="hzr-ink" x1="0" y1="0" x2="64" y2="64" gradientUnits="userSpaceOnUse">
        <stop offset="0" stop-color="#26272d" />
        <stop offset="1" stop-color="#0c0d0f" />
      </linearGradient>
      <radialGradient id="hzr-glow" cx="34" cy="40" r="26" gradientUnits="userSpaceOnUse">
        <stop offset="0" stop-color="#f36a21" stop-opacity="0.35" />
        <stop offset="1" stop-color="#f36a21" stop-opacity="0" />
      </radialGradient>
    </defs>
    <rect x="2" y="2" width="60" height="60" rx="16" fill="url(#hzr-ink)" />
    <rect x="2" y="2" width="60" height="60" rx="16" fill="url(#hzr-glow)" />
    <rect x="2.5" y="2.5" width="59" height="59" rx="15.5" fill="none" stroke="rgba(255,255,255,0.08)" />
    <!-- Flame: leans right like the bird, hollow so the core reads as light. -->
    <path
      d="M35 9c1 8-3 12-8 17-4 4-7 8-7 14a12 12 0 0 0 24 0c0-5-2-9-5-13 .5 4-.5 7-3 9 1-7-2-13-8-18 3-2 5-5 7-9Z"
      fill="url(#hzr-ember)"
    />
    <path
      d="M32 32c-3 4-5 7-5 10a5 5 0 0 0 10 0c0-2-1-4-2-6 0 2-1 3-2 4 .5-3 0-5-1-8Z"
      fill="#0c0d0f"
      opacity="0.92"
    />
  </svg>
</template>
