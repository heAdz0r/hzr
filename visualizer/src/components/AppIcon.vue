<script setup lang="ts">
defineProps<{
  name:
    | "activity"
    | "box"
    | "check"
    | "chevron"
    | "clock"
    | "copy"
    | "cpu"
    | "database"
    | "folder"
    | "git"
    | "help"
    | "memory"
    | "refresh"
    | "search"
    | "terminal"
    | "warning";
  size?: number;
}>();
</script>

<template>
  <svg
    class="app-icon"
    :width="size ?? 20"
    :height="size ?? 20"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.75"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <template v-if="name === 'activity'">
      <path d="M3 12h4l2.5-7 5 14 2.5-7h4" />
    </template>
    <template v-else-if="name === 'box'">
      <path d="m12 3 8 4.5v9L12 21l-8-4.5v-9L12 3Z" />
      <path d="m4.5 7.8 7.5 4.3 7.5-4.3M12 12v8.6" />
    </template>
    <template v-else-if="name === 'check'">
      <path d="m5 12.5 4.2 4.2L19 7" />
    </template>
    <template v-else-if="name === 'chevron'">
      <path d="m8 10 4 4 4-4" />
    </template>
    <template v-else-if="name === 'clock'">
      <circle cx="12" cy="12" r="8.5" />
      <path d="M12 7v5l3 2" />
    </template>
    <template v-else-if="name === 'copy'">
      <rect x="8" y="8" width="11" height="11" rx="2" />
      <path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" />
    </template>
    <template v-else-if="name === 'cpu'">
      <rect x="7" y="7" width="10" height="10" rx="2" />
      <path d="M9 2v3m6-3v3M9 19v3m6-3v3M2 9h3m-3 6h3m14-6h3m-3 6h3M10 10h4v4h-4z" />
    </template>
    <template v-else-if="name === 'database'">
      <ellipse cx="12" cy="5.5" rx="7.5" ry="3" />
      <path d="M4.5 5.5v6c0 1.7 3.4 3 7.5 3s7.5-1.3 7.5-3v-6M4.5 11.5v6c0 1.7 3.4 3 7.5 3s7.5-1.3 7.5-3v-6" />
    </template>
    <template v-else-if="name === 'folder'">
      <path d="M3.5 7.5a2 2 0 0 1 2-2h4l2 2h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z" />
    </template>
    <template v-else-if="name === 'git'">
      <circle cx="7" cy="5" r="2" /><circle cx="17" cy="7" r="2" /><circle cx="7" cy="19" r="2" />
      <path d="M7 7v10M9 9c5 0 4-2 6-2" />
    </template>
    <template v-else-if="name === 'help'">
      <circle cx="12" cy="12" r="9" />
      <path d="M9.6 9a2.6 2.6 0 1 1 3.6 2.4c-.8.4-1.2 1-1.2 1.8M12 17h.01" />
    </template>
    <template v-else-if="name === 'memory'">
      <path d="M9 4a3 3 0 0 0-3 3v1a3 3 0 0 0-1 5.2V15a3 3 0 0 0 3 3h1m6-14a3 3 0 0 1 3 3v1a3 3 0 0 1 1 5.2V15a3 3 0 0 1-3 3h-1M9 4v16m6-16v16M9 9h3m-3 6h3m0-6h3m-3 6h3" />
    </template>
    <template v-else-if="name === 'refresh'">
      <path d="M20 7v5h-5M4 17v-5h5" />
      <path d="M18.3 9A7 7 0 0 0 6.5 6.5L4 9m2 6a7 7 0 0 0 11.5 2.5L20 15" />
    </template>
    <template v-else-if="name === 'search'">
      <circle cx="11" cy="11" r="6.5" /><path d="m16 16 4 4" />
    </template>
    <template v-else-if="name === 'terminal'">
      <rect x="3" y="4" width="18" height="16" rx="3" />
      <path d="m7 9 3 3-3 3m6 0h4" />
    </template>
    <template v-else-if="name === 'warning'">
      <path d="M10.3 4.2 2.8 18a2 2 0 0 0 1.8 3h14.8a2 2 0 0 0 1.8-3L13.7 4.2a2 2 0 0 0-3.4 0Z" />
      <path d="M12 9v4m0 4h.01" />
    </template>
  </svg>
</template>
