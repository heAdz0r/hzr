<script setup lang="ts">
import type { DashboardProject } from "../types";
import { formatBytes, relativeTime } from "../utils";
import AppIcon from "./AppIcon.vue";
import StatusChip from "./StatusChip.vue";

defineProps<{
  project: DashboardProject;
  selected: boolean;
}>();

defineEmits<{
  copy: [command: string];
  select: [worktreeId: string];
}>();

function shortIdentity(value: string): string {
  const digest = value.includes(":") ? value.split(":").at(-1) ?? value : value;
  return digest.slice(0, 12);
}
</script>

<template>
  <details class="project-card" :class="{ 'project-selected': selected }">
    <summary>
      <span class="project-mark"><AppIcon name="folder" :size="20" /></span>
      <span class="project-identity">
        <strong>{{ project.name }}</strong>
        <span>Identity {{ shortIdentity(project.root) }}</span>
      </span>
      <span class="project-meta">
        <span>{{ project.git_backed ? "Git" : "Path identity" }}</span>
        <span>{{ formatBytes(project.artifacts.size_bytes) }}</span>
        <span>Seen {{ relativeTime(project.last_seen_at_ms) }}</span>
      </span>
      <StatusChip :state="project.state" compact />
      <span class="project-chevron"><AppIcon name="chevron" :size="18" /></span>
    </summary>
    <div class="project-detail">
      <div class="artifact-grid" aria-label="Index artifact state">
        <div :class="{ 'artifact-ready': project.artifacts.config_present }">
          <span class="artifact-light"></span><span>Config</span>
        </div>
        <div :class="{ 'artifact-ready': project.artifacts.vectors_present }">
          <span class="artifact-light"></span><span>Vectors</span>
        </div>
        <div :class="{ 'artifact-ready': project.artifacts.symbols_present }">
          <span class="artifact-light"></span><span>Symbols</span>
        </div>
        <div :class="{ 'artifact-ready': project.artifacts.repository_graph_present }">
          <span class="artifact-light"></span><span>Graph</span>
        </div>
      </div>
      <dl class="project-ids">
        <div><dt>Repository</dt><dd>{{ shortIdentity(project.repository_id) }}</dd></div>
        <div><dt>Selection key</dt><dd>{{ shortIdentity(project.worktree_id) }}</dd></div>
      </dl>
      <button class="secondary-action" type="button" @click="$emit('copy', project.command)">
        <AppIcon name="copy" :size="16" />
        Copy index command
      </button>
      <button
        class="secondary-action"
        type="button"
        :disabled="selected"
        @click="$emit('select', project.worktree_id)"
      >
        <AppIcon name="activity" :size="16" />
        {{ selected ? "Selected project" : "Open observatory" }}
      </button>
    </div>
  </details>
</template>
