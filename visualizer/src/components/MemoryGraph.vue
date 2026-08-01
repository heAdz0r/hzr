<script setup lang="ts">
import { computed, ref } from "vue";
import type { DashboardMemoryObservatory } from "../types";
import { layoutMemoryTopics } from "../utils";

const props = defineProps<{ observatory: DashboardMemoryObservatory }>();
const width = 800;
const height = 460;
const nodes = computed(() => layoutMemoryTopics(props.observatory.topics, width, height));
const positions = computed(() => new Map(nodes.value.map((node) => [node.id, node])));
const query = ref("");
const pinnedTopic = ref<string | null>(null);
const normalizedQuery = computed(() => query.value.trim().toLocaleLowerCase());
const relatedTopics = computed(() => {
  const related = new Set<string>();
  if (!pinnedTopic.value) return related;
  related.add(pinnedTopic.value);
  for (const edge of props.observatory.edges) {
    if (edge.source === pinnedTopic.value) related.add(edge.target);
    if (edge.target === pinnedTopic.value) related.add(edge.source);
  }
  return related;
});
const visibleEdges = computed(() =>
  props.observatory.edges.flatMap((edge) => {
    const source = positions.value.get(edge.source);
    const target = positions.value.get(edge.target);
    return source && target ? [{ ...edge, source, target }] : [];
  }),
);

function nodeState(id: string, label: string): Record<string, boolean> {
  const matchesQuery = normalizedQuery.value.length === 0 || label.toLocaleLowerCase().includes(normalizedQuery.value);
  const matchesPin = pinnedTopic.value === null || relatedTopics.value.has(id);
  return {
    "is-pinned": pinnedTopic.value === id,
    "is-dimmed": !matchesQuery || !matchesPin,
  };
}

function toggleTopic(id: string): void {
  pinnedTopic.value = pinnedTopic.value === id ? null : id;
}

function resetView(): void {
  query.value = "";
  pinnedTopic.value = null;
}
</script>

<template>
  <div class="memory-graph-shell">
    <div v-if="nodes.length" class="memory-graph-controls">
      <label>
        <span class="sr-only">Find a memory topic</span>
        <input v-model="query" type="search" placeholder="Find topic" />
      </label>
      <span>{{ pinnedTopic ? "Pinned neighborhood" : "Click a node to inspect its neighborhood" }}</span>
      <button type="button" :disabled="!query && !pinnedTopic" @click="resetView">Reset</button>
    </div>
    <svg
      v-if="nodes.length"
      class="memory-graph"
      :viewBox="`0 0 ${width} ${height}`"
      role="group"
      :aria-label="`${observatory.memory_count} memories across ${nodes.length} project topics`"
    >
      <defs>
        <radialGradient id="memory-node-fill" cx="32%" cy="26%">
          <stop offset="0" stop-color="#ffb15a" stop-opacity="0.94" />
          <stop offset="0.42" stop-color="#f36a21" stop-opacity="0.78" />
          <stop offset="1" stop-color="#4a241a" stop-opacity="0.96" />
        </radialGradient>
      </defs>
      <circle class="memory-orbit orbit-one" cx="400" cy="230" r="176" />
      <circle class="memory-orbit orbit-two" cx="400" cy="230" r="112" />
      <line
        v-for="edge in visibleEdges"
        :key="`${edge.source.id}-${edge.target.id}`"
        class="memory-edge"
        :x1="edge.source.x"
        :y1="edge.source.y"
        :x2="edge.target.x"
        :y2="edge.target.y"
        :style="{ '--edge-weight': Math.min(1, 0.25 + edge.relationship_count * 0.12) }"
      >
        <title>{{ edge.relationship_count }} relationship{{ edge.relationship_count === 1 ? "" : "s" }}</title>
      </line>
      <g
        v-for="node in nodes"
        :key="node.id"
        class="memory-node"
        :class="nodeState(node.id, node.label)"
        :transform="`translate(${node.x} ${node.y})`"
        tabindex="0"
        role="button"
        :aria-pressed="pinnedTopic === node.id"
        @click="toggleTopic(node.id)"
        @keydown.enter.prevent="toggleTopic(node.id)"
        @keydown.space.prevent="toggleTopic(node.id)"
      >
        <title>{{ node.label }}: {{ node.memory_count }} memories, average weight {{ node.average_weight.toFixed(2) }}</title>
        <circle class="memory-node-halo" :r="36 + Math.min(18, node.memory_count * 2)" />
        <circle class="memory-node-core" :r="22 + Math.min(11, node.memory_count)" />
        <text class="memory-node-count" y="4">{{ node.memory_count }}</text>
        <text class="memory-node-label" y="58">{{ node.label }}</text>
      </g>
      <g class="memory-center" transform="translate(400 230)">
        <circle r="58" />
        <text y="-3">{{ observatory.memory_count }}</text>
        <text class="memory-center-caption" y="18">project memories</text>
      </g>
    </svg>
    <div v-else class="graph-empty">
      <span>0</span>
      <strong>No project memories yet</strong>
      <p>ICM is reachable. Store the first durable project fact to create this graph.</p>
    </div>
  </div>
</template>
