<script setup lang="ts">
import type { Core, ElementDefinition } from "cytoscape";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import AppIcon from "./AppIcon.vue";
import type {
  DashboardMemoryDetail,
  DashboardMemoryObservatory,
  DashboardMemoryTopicDetails,
  MemoryTopic,
} from "../types";

const props = defineProps<{ observatory: DashboardMemoryObservatory }>();
const GRAPH_MEMORY_LIMIT = 28;
const TOPICS_PER_RING = 12;

const graphElement = ref<HTMLDivElement | null>(null);
const topicButtons = ref<HTMLButtonElement[]>([]);
const query = ref("");
const focusedTopicId = ref<string | null>(null);
const selectedTopicId = ref<string | null>(null);
const expandedTopicId = ref<string | null>(null);
const selectedMemoryId = ref<string | null>(null);
const loadingTopicId = ref<string | null>(null);
const detailError = ref<string | null>(null);
const detailCache = ref(new Map<string, DashboardMemoryTopicDetails>());
let graph: Core | null = null;
let overviewSignature = "";
let detailController: AbortController | null = null;

const normalizedQuery = computed(() => query.value.trim().toLocaleLowerCase());
const filteredTopics = computed(() => {
  if (!normalizedQuery.value) return props.observatory.topics;
  return props.observatory.topics.filter((topic) =>
    topic.label.toLocaleLowerCase().includes(normalizedQuery.value),
  );
});
const selectedTopic = computed(() =>
  props.observatory.topics.find((topic) => topic.id === selectedTopicId.value) ?? null,
);
const expandedDetails = computed(() =>
  expandedTopicId.value
    ? detailCache.value.get(topicDetailCacheKey(expandedTopicId.value)) ?? null
    : null,
);
const selectedMemory = computed(() =>
  expandedDetails.value?.memories.find((memory) => memory.id === selectedMemoryId.value) ?? null,
);

function topicElements(): ElementDefinition[] {
  const orderedTopics = [...props.observatory.topics].sort((left, right) =>
    left.label.localeCompare(right.label) || left.id.localeCompare(right.id),
  );
  const nodes = orderedTopics.map((topic, index) => {
    const ring = Math.floor(index / TOPICS_PER_RING);
    const ringStart = ring * TOPICS_PER_RING;
    const ringSize = Math.min(TOPICS_PER_RING, orderedTopics.length - ringStart);
    const angle = -Math.PI / 2 + (Math.PI * 2 * (index - ringStart)) / Math.max(1, ringSize);
    const radius = 105 + ring * 115;
    return {
      data: {
        id: topic.id,
        kind: "topic",
        label: topic.label,
        count: topic.memory_count,
        weight: topic.average_weight,
        size: Math.min(54, 28 + Math.sqrt(topic.memory_count) * 5),
      },
      position: { x: Math.cos(angle) * radius, y: Math.sin(angle) * radius },
    };
  });
  const edges = props.observatory.edges.map((edge) => ({
    data: {
      id: `topic-edge-${edge.source}-${edge.target}`,
      kind: "topic-edge",
      source: edge.source,
      target: edge.target,
      relationshipCount: edge.relationship_count,
      width: Math.min(5, 1 + Math.log2(edge.relationship_count + 1)),
    },
  }));
  return [...nodes, ...edges];
}

async function createGraph(): Promise<void> {
  if (!graphElement.value || graph) return;
  const { default: cytoscape } = await import("cytoscape");
  if (!graphElement.value || graph) return;
  graph = cytoscape({
    container: graphElement.value,
    elements: topicElements(),
    minZoom: 0.35,
    maxZoom: 2.5,
    boxSelectionEnabled: false,
    autoungrabify: false,
    style: [
      {
        selector: "node[kind = 'topic']",
        style: {
          width: "data(size)",
          height: "data(size)",
          label: "data(label)",
          "font-family": "Inter, SF Pro Display, sans-serif",
          "font-size": 10,
          "font-weight": "bold",
          color: "#d8d5cd",
          "text-wrap": "ellipsis",
          "text-max-width": "118px",
          "text-valign": "bottom",
          "text-margin-y": 9,
          "background-color": "#242831",
          "background-opacity": 0.98,
          "border-width": 1.5,
          "border-color": "#667085",
          "overlay-opacity": 0,
        },
      },
      {
        selector: "node[kind = 'topic']:selected",
        style: {
          "background-color": "#26364d",
          "border-color": "#68a9ff",
          "border-width": 3,
          color: "#fff8e8",
        },
      },
      {
        selector: "node[kind = 'memory']",
        style: {
          width: 16,
          height: 16,
          label: "data(shortLabel)",
          "font-family": "ui-monospace, SFMono-Regular, Menlo, monospace",
          "font-size": 7,
          color: "#9ca3af",
          "text-wrap": "ellipsis",
          "text-max-width": "84px",
          "text-valign": "bottom",
          "text-margin-y": 6,
          "background-color": "#f36a21",
          "border-width": 1,
          "border-color": "#ffb15a",
          "overlay-opacity": 0,
        },
      },
      {
        selector: "node[kind = 'memory']:selected",
        style: {
          width: 22,
          height: 22,
          "background-color": "#3ddc97",
          "border-color": "#baf7dc",
          color: "#fff8e8",
        },
      },
      {
        selector: "edge[kind = 'topic-edge']",
        style: {
          width: "data(width)",
          "line-color": "#566174",
          "curve-style": "bezier",
          opacity: 0.62,
          "overlay-opacity": 0,
        },
      },
      {
        selector: "edge[kind = 'memory-edge']",
        style: {
          width: 1,
          "line-color": "#f36a21",
          "curve-style": "bezier",
          opacity: 0.52,
          "line-style": "dashed",
          "overlay-opacity": 0,
        },
      },
      {
        selector: ".is-filtered",
        style: { opacity: 0.12 },
      },
      {
        selector: ".is-neighbor",
        style: { "border-color": "#ffb15a", opacity: 1 },
      },
      {
        selector: ".is-context-dimmed",
        style: { opacity: 0.16 },
      },
    ],
    layout: {
      name: "preset",
      fit: true,
      padding: 56,
    },
  });
  overviewSignature = topicSignature();
  graph.on("tap", "node[kind = 'topic']", (event) => {
    void selectTopic(event.target.id());
  });
  graph.on("tap", "node[kind = 'memory']", (event) => {
    selectMemory(event.target.data("memoryId") as string);
  });
}

function topicSignature(): string {
  const topics = [...props.observatory.topics]
    .sort((left, right) => left.id.localeCompare(right.id))
    .map((topic) => `${topic.id}:${topic.memory_count}:${topic.average_weight}:${topic.newest_at ?? ""}`);
  const edges = [...props.observatory.edges]
    .sort((left, right) => `${left.source}:${left.target}`.localeCompare(`${right.source}:${right.target}`))
    .map((edge) => `${edge.source}:${edge.target}:${edge.relationship_count}`);
  return [...topics, ...edges].join("|");
}

function topicDetailCacheKey(id: string): string {
  return `${topicSignature()}::${id}`;
}

function syncOverview(): void {
  if (!graph) return;
  const viewport = { zoom: graph.zoom(), pan: graph.pan() };
  const nextSignature = topicSignature();
  const nextIds = new Set(props.observatory.topics.map((topic) => topic.id));
  graph.batch(() => {
    graph?.nodes("[kind = 'topic']").forEach((node) => {
      if (!nextIds.has(node.id())) node.remove();
    });
    graph?.edges("[kind = 'topic-edge']").remove();
    for (const element of topicElements()) {
      const id = element.data.id as string;
      const existing = graph?.getElementById(id);
      if (existing?.length) {
        existing.data(element.data);
        if (element.position && element.data.kind === "topic") existing.position(element.position);
      } else graph?.add(element);
    }
  });
  const refreshExpandedTopic = Boolean(
    overviewSignature && overviewSignature !== nextSignature && expandedTopicId.value,
  );
  graph.zoom(viewport.zoom);
  graph.pan(viewport.pan);
  overviewSignature = nextSignature;
  applySearch();
  applySelection();
  if (refreshExpandedTopic && expandedTopicId.value) {
    void loadTopic(expandedTopicId.value, true);
  }
}

function applySearch(): void {
  if (!graph) return;
  graph.nodes("[kind = 'topic']").forEach((node) => {
    const label = String(node.data("label")).toLocaleLowerCase();
    node.toggleClass("is-filtered", Boolean(normalizedQuery.value) && !label.includes(normalizedQuery.value));
  });
}

function applySelection(): void {
  if (!graph) return;
  graph.elements().removeClass("is-neighbor is-context-dimmed");
  graph.nodes().unselect();
  if (!selectedTopicId.value) return;
  const topic = graph.getElementById(selectedTopicId.value);
  topic.select();
  const neighborhood = topic.union(topic.neighborhood());
  neighborhood.addClass("is-neighbor");
  if (expandedTopicId.value) {
    graph.nodes("[kind = 'topic']").difference(neighborhood).addClass("is-context-dimmed");
  }
}

async function selectTopic(id: string): Promise<void> {
  selectedTopicId.value = id;
  selectedMemoryId.value = null;
  applySelection();
  const topicNode = graph?.getElementById(id);
  if (topicNode?.length) {
    graph?.center(topicNode);
    if ((graph?.zoom() ?? 1) < 0.8) graph?.zoom(0.8);
  }
  await loadTopic(id);
}

async function loadTopic(id: string, force = false): Promise<void> {
  const cacheKey = topicDetailCacheKey(id);
  const cached = detailCache.value.get(cacheKey);
  if (cached && !force) {
    expandedTopicId.value = id;
    await renderTopicDetails(cached);
    return;
  }
  detailController?.abort();
  detailController = new AbortController();
  loadingTopicId.value = id;
  detailError.value = null;
  try {
    const response = await fetch(`/v1/dashboard/memory/topics/${encodeURIComponent(id)}`, {
      cache: "no-store",
      headers: { Accept: "application/json" },
      signal: detailController.signal,
    });
    if (!response.ok) throw new Error(`Memory topic returned HTTP ${response.status}`);
    const details = (await response.json()) as DashboardMemoryTopicDetails;
    const generationPrefix = `${topicSignature()}::`;
    const currentGeneration = new Map(
      [...detailCache.value].filter(([key]) => key.startsWith(generationPrefix)),
    );
    detailCache.value = currentGeneration.set(topicDetailCacheKey(id), details);
    expandedTopicId.value = id;
    await renderTopicDetails(details);
  } catch (cause) {
    if (cause instanceof DOMException && cause.name === "AbortError") return;
    detailError.value = cause instanceof Error ? cause.message : "Memory topic could not be loaded";
  } finally {
    if (loadingTopicId.value === id) loadingTopicId.value = null;
  }
}

async function renderTopicDetails(details: DashboardMemoryTopicDetails): Promise<void> {
  await nextTick();
  if (!graph) return;
  const topic = graph.getElementById(details.id);
  if (!topic.length) return;
  const origin = topic.position();
  const visible = details.memories.slice(0, GRAPH_MEMORY_LIMIT);
  const visibleIds = new Set(visible.map((memory) => memory.id));
  graph.batch(() => {
    graph?.elements("[kind = 'memory'], [kind = 'memory-edge']").remove();
    visible.forEach((memory, index) => {
      const angle = (Math.PI * 2 * index) / Math.max(visible.length, 1);
      const ring = 92 + (index % 3) * 34;
      graph?.add({
        group: "nodes",
        data: {
          id: `memory-${memory.id}`,
          kind: "memory",
          memoryId: memory.id,
          shortLabel: memory.summary.replace(/\s+/g, " ").slice(0, 34),
        },
        position: {
          x: origin.x + Math.cos(angle) * ring,
          y: origin.y + Math.sin(angle) * ring,
        },
      });
      graph?.add({
        group: "edges",
        data: {
          id: `memory-topic-${memory.id}`,
          kind: "memory-edge",
          source: details.id,
          target: `memory-${memory.id}`,
        },
      });
    });
    for (const memory of visible) {
      for (const relatedId of memory.related_ids) {
        if (!visibleIds.has(relatedId) || memory.id >= relatedId) continue;
        graph?.add({
          group: "edges",
          data: {
            id: `memory-link-${memory.id}-${relatedId}`,
            kind: "memory-edge",
            source: `memory-${memory.id}`,
            target: `memory-${relatedId}`,
          },
        });
      }
    }
  });
  const neighborhood = topic.union(topic.neighborhood()).union(graph.nodes("[kind = 'memory']"));
  graph.fit(neighborhood, 72);
  applySelection();
}

function selectMemory(id: string): void {
  selectedMemoryId.value = id;
  if (!graph) return;
  graph.nodes().unselect();
  const node = graph.getElementById(`memory-${id}`);
  if (node.length) node.select();
}

async function moveTopicFocus(event: KeyboardEvent, currentId: string): Promise<void> {
  const topics = filteredTopics.value;
  if (!topics.length) return;
  const currentIndex = Math.max(0, topics.findIndex((topic) => topic.id === currentId));
  let nextIndex: number | null = null;
  if (event.key === "ArrowDown" || event.key === "ArrowRight") {
    nextIndex = (currentIndex + 1) % topics.length;
  } else if (event.key === "ArrowUp" || event.key === "ArrowLeft") {
    nextIndex = (currentIndex - 1 + topics.length) % topics.length;
  } else if (event.key === "Home") {
    nextIndex = 0;
  } else if (event.key === "End") {
    nextIndex = topics.length - 1;
  }
  if (nextIndex === null) return;
  event.preventDefault();
  focusedTopicId.value = topics[nextIndex].id;
  await nextTick();
  topicButtons.value[nextIndex]?.focus();
}

async function closeInspector(): Promise<void> {
  const returnTopicId = expandedTopicId.value ?? selectedTopicId.value;
  selectedMemoryId.value = null;
  expandedTopicId.value = null;
  selectedTopicId.value = null;
  graph?.elements("[kind = 'memory'], [kind = 'memory-edge']").remove();
  applySelection();
  if (!returnTopicId) return;
  focusedTopicId.value = returnTopicId;
  await nextTick();
  topicButtons.value.find((button) => button.dataset.topicId === returnTopicId)?.focus();
}

function zoomBy(multiplier: number): void {
  if (!graph) return;
  graph.zoom({ level: graph.zoom() * multiplier, renderedPosition: { x: graph.width() / 2, y: graph.height() / 2 } });
}

function fitGraph(): void {
  graph?.fit(graph.elements(":visible"), 56);
}

function resetView(): void {
  selectedTopicId.value = null;
  expandedTopicId.value = null;
  selectedMemoryId.value = null;
  query.value = "";
  detailError.value = null;
  graph?.elements("[kind = 'memory'], [kind = 'memory-edge']").remove();
  graph?.elements().removeClass("is-neighbor is-context-dimmed is-filtered").unselect();
  graph?.fit(graph.elements(), 56);
}

function topicAge(topic: MemoryTopic): string {
  if (!topic.newest_at) return "No timestamp";
  const parsed = Date.parse(topic.newest_at);
  if (Number.isNaN(parsed)) return topic.newest_at;
  const days = Math.max(0, Math.floor((Date.now() - parsed) / 86_400_000));
  return days === 0 ? "Updated today" : `${days}d ago`;
}

function shortId(id: string): string {
  return id.slice(0, 10);
}

function sourceLabel(memory: DashboardMemoryDetail): string {
  return memory.source_type ?? "not recorded";
}

watch(normalizedQuery, applySearch);
watch(filteredTopics, (topics) => {
  if (!topics.some((topic) => topic.id === focusedTopicId.value)) {
    focusedTopicId.value = topics[0]?.id ?? null;
  }
});
watch(
  () => props.observatory,
  () => syncOverview(),
  { deep: false },
);

onMounted(() => {
  void createGraph();
});
onBeforeUnmount(() => {
  detailController?.abort();
  graph?.destroy();
  graph = null;
});
</script>

<template>
  <div v-if="observatory.topics.length" class="memory-explorer">
    <div class="memory-explorer-toolbar">
      <label class="memory-search">
        <AppIcon name="search" :size="16" />
        <span class="sr-only">Find a memory topic</span>
        <input v-model="query" type="search" placeholder="Find topic or decision" />
      </label>
      <div class="graph-actions" aria-label="Memory graph view controls">
        <button type="button" aria-label="Zoom out" title="Zoom out" @click="zoomBy(0.82)">−</button>
        <button type="button" aria-label="Zoom in" title="Zoom in" @click="zoomBy(1.22)">+</button>
        <button type="button" @click="fitGraph"><AppIcon name="focus" :size="15" /> Fit</button>
        <button type="button" :disabled="!selectedTopicId && !query" @click="resetView">Reset</button>
      </div>
      <span class="graph-hint">Drag to pan · scroll to zoom · select to inspect</span>
    </div>

    <div class="memory-explorer-grid">
      <aside class="topic-rail" aria-label="Memory topics">
        <div class="rail-title"><strong>Topics</strong><span>{{ filteredTopics.length }}</span></div>
        <div class="topic-list">
          <button
            v-for="(topic, index) in filteredTopics"
            :key="topic.id"
            ref="topicButtons"
            type="button"
            :data-topic-id="topic.id"
            :class="{ active: selectedTopicId === topic.id }"
            :aria-current="selectedTopicId === topic.id ? 'true' : undefined"
            :tabindex="focusedTopicId ? (focusedTopicId === topic.id ? 0 : -1) : (index === 0 ? 0 : -1)"
            @focus="focusedTopicId = topic.id"
            @keydown="moveTopicFocus($event, topic.id)"
            @click="selectTopic(topic.id)"
          >
            <span class="topic-dot" aria-hidden="true"></span>
            <span><strong>{{ topic.label }}</strong><small>{{ topicAge(topic) }}</small></span>
            <b>{{ topic.memory_count }}</b>
          </button>
        </div>
      </aside>

      <div class="graph-stage">
        <div
          ref="graphElement"
          class="memory-canvas"
          role="img"
          :aria-label="`${observatory.memory_count} memories across ${observatory.topics.length} topics. Use the synchronized topic list to inspect.`"
        ></div>
        <div class="graph-legend" aria-hidden="true">
          <span><i class="legend-topic"></i> Topic</span>
          <span><i class="legend-memory"></i> Memory</span>
          <span><i class="legend-link"></i> Observed link</span>
        </div>
      </div>

      <aside class="memory-inspector" aria-live="polite">
        <template v-if="detailError">
          <span class="inspector-kicker error-text">Detail unavailable</span>
          <h3>Topic could not be loaded.</h3>
          <p>{{ detailError }}</p>
          <button v-if="selectedTopicId" type="button" class="inspector-action" @click="loadTopic(selectedTopicId, true)">Try again</button>
        </template>
        <template v-else-if="loadingTopicId">
          <span class="inspector-kicker">Reading canonical memory</span>
          <h3>{{ selectedTopic?.label ?? "Topic" }}</h3>
          <div class="inspector-skeleton"></div>
          <div class="inspector-skeleton short"></div>
        </template>
        <template v-else-if="selectedMemory">
          <button class="inspector-back" type="button" @click="selectedMemoryId = null">← {{ expandedDetails?.label }}</button>
          <button class="inspector-close" type="button" aria-label="Close memory inspector" @click="closeInspector">×</button>
          <span class="inspector-kicker">Memory {{ shortId(selectedMemory.id) }}</span>
          <h3>{{ selectedMemory.summary }}</h3>
          <dl class="memory-facts">
            <div><dt>Importance</dt><dd>{{ selectedMemory.importance }}</dd></div>
            <div><dt>Weight</dt><dd>{{ selectedMemory.weight.toFixed(2) }}</dd></div>
            <div><dt>Accesses</dt><dd>{{ selectedMemory.access_count }}</dd></div>
            <div><dt>Source</dt><dd>{{ sourceLabel(selectedMemory) }}</dd></div>
            <div class="wide"><dt>Updated</dt><dd>{{ selectedMemory.updated_at }}</dd></div>
            <div class="wide"><dt>Opaque ID</dt><dd><code>{{ selectedMemory.id }}</code></dd></div>
          </dl>
          <div v-if="selectedMemory.keywords.length" class="memory-keywords">
            <span v-for="keyword in selectedMemory.keywords" :key="keyword">{{ keyword }}</span>
          </div>
          <details v-if="selectedMemory.raw_excerpt" class="memory-excerpt">
            <summary>Reveal bounded raw excerpt</summary>
            <pre>{{ selectedMemory.raw_excerpt }}</pre>
          </details>
          <details v-if="selectedMemory.source_data" class="memory-excerpt">
            <summary>Source evidence</summary>
            <pre>{{ selectedMemory.source_data }}</pre>
          </details>
        </template>
        <template v-else-if="expandedDetails">
          <span class="inspector-kicker">Topic detail</span>
          <h3>{{ expandedDetails.label }}</h3>
          <p>{{ expandedDetails.visible_memory_count }} of {{ expandedDetails.memory_count }} records loaded from the repository-scoped ICM snapshot.</p>
          <div class="memory-record-list">
            <button
              v-for="memory in expandedDetails.memories"
              :key="memory.id"
              type="button"
              @click="selectMemory(memory.id)"
            >
              <span><strong>{{ memory.summary }}</strong><small>{{ memory.importance }} · weight {{ memory.weight.toFixed(2) }}</small></span>
              <AppIcon name="chevron" :size="14" />
            </button>
          </div>
          <small v-if="expandedDetails.memories.length > GRAPH_MEMORY_LIMIT" class="canvas-limit">
            {{ GRAPH_MEMORY_LIMIT }} records shown on canvas; all {{ expandedDetails.memories.length }} remain inspectable here.
          </small>
          <div class="inspector-actions">
            <button type="button" class="inspector-action" @click="loadTopic(expandedDetails.id, true)">Load latest</button>
            <button type="button" class="inspector-action secondary" @click="closeInspector">Close topic</button>
          </div>
        </template>
        <template v-else>
          <span class="inspector-kicker">Memory explorer</span>
          <h3>Select a topic to open its records.</h3>
          <p>The overview contains aggregate evidence only. Drill-down is fetched on demand from a bounded, read-only repository filter.</p>
          <ul class="inspector-principles">
            <li>Opaque topic and memory IDs</li>
            <li>No database path exposed</li>
            <li>Maximum 100 records per topic</li>
          </ul>
        </template>
      </aside>
    </div>
  </div>
  <div v-else class="graph-empty">
    <span>0</span>
    <strong>No project memories yet</strong>
    <p>ICM is reachable. Store the first durable project fact to create this graph.</p>
  </div>
</template>
