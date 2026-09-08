<script setup lang="ts">
import { computed } from "vue";
import type { DashboardProject } from "../types";
import { formatBytes, projectStateLabel, relativeTime } from "../utils";
import AppIcon from "./AppIcon.vue";
import StatusChip from "./StatusChip.vue";

const props = defineProps<{
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

/**
 * The four index artifacts, each with a state a reader can act on.
 *
 * A grey dot beside a word says only "not green". Naming what the artifact is
 * for, and whether it is present, is what lets the diagnosis below refer to it.
 */
const artifacts = computed(() => [
  { key: "config", label: "Config", hint: "index placement", present: props.project.artifacts.config_present },
  { key: "vectors", label: "Vectors", hint: "semantic search", present: props.project.artifacts.vectors_present },
  { key: "symbols", label: "Symbols", hint: "code structure", present: props.project.artifacts.symbols_present },
  { key: "graph", label: "Graph", hint: "repository map", present: props.project.artifacts.repository_graph_present },
]);

const ready = computed(() => props.project.state === "ready");
const presentCount = computed(() => artifacts.value.filter((artifact) => artifact.present).length);
/** The command a reader should copy: the remedy when there is one, else the status check. */
const actionCommand = computed(() => props.project.remedy ?? props.project.command);
</script>

<template>
  <details class="project-card" :class="[`project-state-${project.state}`, { 'project-selected': selected }]">
    <summary>
      <span class="project-mark"><AppIcon name="folder" :size="20" /></span>
      <span class="project-identity">
        <strong>{{ project.name }}</strong>
        <!-- The path is what separates two workspaces with the same basename;
             the identity digest stays because it is what a support conversation
             can quote. -->
        <span>{{ project.display_path ?? `Identity ${shortIdentity(project.root)}` }}</span>
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
      <!-- Diagnosis leads. A coloured chip on its own sends the reader hunting;
           this says what is wrong and hands over a command with the real path. -->
      <section v-if="!ready && project.state_reason" class="project-diagnosis" :aria-label="`${projectStateLabel[project.state]}: what is wrong and how to fix it`">
        <div class="project-diagnosis-head">
          <span class="project-diagnosis-glyph"><AppIcon name="warning" :size="16" /></span>
          <div>
            <span class="eyebrow">{{ projectStateLabel[project.state] }} · what is wrong</span>
            <p>{{ project.state_reason }}</p>
          </div>
        </div>
        <div v-if="project.remedy" class="project-remedy">
          <span class="project-remedy-label">Run this to fix it</span>
          <code>{{ project.remedy }}</code>
          <button class="primary-action project-remedy-copy" type="button" @click="$emit('copy', project.remedy)">
            <AppIcon name="copy" :size="15" /> Copy
          </button>
        </div>
        <p v-else class="project-remedy-none">No command can fix this from here — see the note above.</p>
      </section>
      <section v-else class="project-diagnosis project-diagnosis-ready" aria-label="Index complete">
        <div class="project-diagnosis-head">
          <span class="project-diagnosis-glyph"><AppIcon name="check" :size="16" /></span>
          <div>
            <span class="eyebrow">Ready</span>
            <p>All index artifacts are present. Semantic search and symbol lookups are served from this workspace's own index.</p>
          </div>
        </div>
      </section>

      <div class="project-facts">
        <div class="artifact-grid" :aria-label="`${presentCount} of ${artifacts.length} index artifacts present`">
          <div
            v-for="artifact in artifacts"
            :key="artifact.key"
            :class="{ 'artifact-ready': artifact.present }"
            :title="`${artifact.label} — ${artifact.hint}: ${artifact.present ? 'present' : 'absent'}`"
          >
            <span class="artifact-light"></span>
            <span class="artifact-name">{{ artifact.label }}</span>
            <span class="artifact-hint">{{ artifact.present ? artifact.hint : "absent" }}</span>
          </div>
        </div>
        <dl class="project-ids">
          <div><dt>Repository</dt><dd :title="project.repository_id">{{ shortIdentity(project.repository_id) }}</dd></div>
          <div><dt>Selection key</dt><dd :title="project.worktree_id">{{ shortIdentity(project.worktree_id) }}</dd></div>
        </dl>
      </div>

      <div class="project-actions">
        <button class="secondary-action" type="button" @click="$emit('copy', actionCommand)">
          <AppIcon name="copy" :size="16" />
          {{ project.remedy ? "Copy fix command" : "Copy status command" }}
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
    </div>
  </details>
</template>
