<script setup lang="ts">
import type { DashboardProject } from "../types";
import { formatBytes, relativeTime } from "../utils";
import AppIcon from "./AppIcon.vue";
import StatusChip from "./StatusChip.vue";

defineProps<{
  project: DashboardProject;
}>();

defineEmits<{
  copy: [command: string];
}>();
</script>

<template>
  <details class="project-card">
    <summary>
      <span class="project-mark"><AppIcon name="folder" :size="20" /></span>
      <span class="project-identity">
        <strong>{{ project.name }}</strong>
        <span>{{ project.root }}</span>
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
        <div><dt>Repository</dt><dd>{{ project.repository_id }}</dd></div>
        <div><dt>Worktree</dt><dd>{{ project.worktree_id }}</dd></div>
      </dl>
      <button class="secondary-action" type="button" @click="$emit('copy', project.command)">
        <AppIcon name="copy" :size="16" />
        Copy index command
      </button>
    </div>
  </details>
</template>
