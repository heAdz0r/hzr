<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";

type DelegatedRun = {
  id: string; provider: string; model: string; status: string;
  started_at_ms: number; updated_at_ms: number; turns: number; tool_calls: number;
  current_tool: string | null; actual_input_tokens: number | null;
  actual_output_tokens: number | null; actual_cache_read_tokens: number | null;
  execution: string; parent_acceptance: string;
};
const runs = ref<DelegatedRun[]>([]);
const error = ref<string | null>(null);
const loaded = ref(false);
let timer: ReturnType<typeof setTimeout> | undefined;
let request: AbortController | undefined;
let active = false;
const providerLabel = (provider: string) =>
  ({ "opencode-go": "OpenCode GO", openrouter: "OpenRouter", deepseek: "DeepSeek" })[provider] ?? provider;
const count = (value: number | null) => value === null ? "—" : value.toLocaleString();
const statusLabel = (status: string) => ({
  starting: "Starting", running: "Working", completed: "Worker finished",
  failed: "Failed", interrupted: "No heartbeat", timed_out: "Timed out", cancelled: "Cancelled",
})[status] ?? status;
async function refresh() {
  request = new AbortController();
  const timeout = setTimeout(() => request?.abort(), 8000);
  try {
    const response = await fetch("/v1/dashboard/delegations", { signal: request.signal, cache: "no-store" });
    if (!response.ok) throw new Error("Delegation status is unavailable.");
    const payload: unknown = await response.json();
    if (!Array.isArray(payload)) throw new Error("Invalid delegation status.");
    if (active) { runs.value = payload; loaded.value = true; error.value = null; }
  } catch {
    if (active) error.value = "Connection interrupted. Retaining the last observed status.";
  } finally {
    clearTimeout(timeout);
    if (active) timer = setTimeout(refresh, 2000);
  }
}
onMounted(() => { active = true; void refresh(); });
onBeforeUnmount(() => { active = false; clearTimeout(timer); request?.abort(); });
</script>

<template>
  <section class="delegation-panel" aria-labelledby="delegation-title">
    <header>
      <p class="eyebrow">LIVE DELEGATION</p>
      <h1 id="delegation-title">Your model plans. The worker executes.</h1>
      <p>Codex or Claude retains planning, review and final acceptance. These are HZR-managed workers.</p>
    </header>
    <div class="delegation-flow" aria-label="Delegation architecture">
      <div><span>01 · ORCHESTRATOR</span><strong>Your selected model</strong><small>Plan · scope · accept</small></div>
      <b aria-hidden="true">→</b>
      <div><span>02 · CONTROL PLANE</span><strong>HZR</strong><small>Tools · limits · usage</small></div>
      <b aria-hidden="true">→</b>
      <div><span>03 · EXECUTOR</span><strong>Configured worker</strong><small>Exact provider and model below</small></div>
    </div>
    <p v-if="error" role="status">{{ error }}</p>
    <p v-if="!loaded && !error" role="status">Loading delegation activity…</p>
    <div v-if="loaded && !runs.length" class="delegation-empty">
      <h3>No delegated tasks observed yet</h3>
      <p>Choose your worker with <code>hzr settings</code>, then start a bounded task with <code>hzr delegate --file task.md</code>.</p>
    </div>
    <ol v-if="runs.length" class="delegation-runs">
      <li v-for="run in runs" :key="run.id" :class="['delegation-run', { working: run.status === 'running' }]">
        <div class="run-heading">
          <div><span class="eyebrow">{{ providerLabel(run.provider) }} · Task {{ run.id.slice(7, 15) }}</span><h3>{{ run.model }}</h3></div>
          <span class="run-status">{{ statusLabel(run.status) }}</span>
        </div>
        <p class="run-activity">{{ run.current_tool ?? (run.status === 'running' ? 'Model is working' : 'No active tool') }}</p>
        <dl>
          <div><dt>Turns</dt><dd>{{ count(run.turns) }}</dd></div>
          <div><dt>Tool calls</dt><dd>{{ count(run.tool_calls) }}</dd></div>
          <div><dt>Uncached input</dt><dd>{{ count(run.actual_input_tokens) }}</dd></div>
          <div><dt>Output tokens</dt><dd>{{ count(run.actual_output_tokens) }}</dd></div>
          <div><dt>Cache read</dt><dd>{{ count(run.actual_cache_read_tokens) }}</dd></div>
        </dl>
        <footer>
          <time :datetime="new Date(run.started_at_ms).toISOString()">{{ new Date(run.started_at_ms).toLocaleString() }}</time>
          <span>{{ Math.max(0, Math.round((run.updated_at_ms - run.started_at_ms) / 1000)) }} s · Parent acceptance not recorded</span>
        </footer>
      </li>
    </ol>
    <p class="delegation-note">Provider-reported tokens. No inferred cost savings. Worker completion still requires the parent's review. All workspaces on this machine. Last 50 observed runs within 30 days.</p>
  </section>
</template>

<style scoped>
.delegation-panel { display: grid; gap: 1.5rem; }
.delegation-panel h1 { font-size: clamp(1.6rem, 3vw, 2.6rem); margin: .4rem 0 .8rem; letter-spacing: -.04em; }
.delegation-panel h3 { margin: .35rem 0; overflow-wrap: anywhere; }
.delegation-panel p { line-height: 1.6; }
.eyebrow { font-size: .7rem; font-weight: 750; letter-spacing: .14em; color: #e9a06b; }
.delegation-flow { display: grid; grid-template-columns: 1fr auto 1fr auto 1fr; align-items: center; gap: 1.2rem; padding: 1.5rem; border: 1px solid #ffffff20; border-radius: 1rem; }
.delegation-flow div { display: grid; gap: .6rem; }
.delegation-flow span, .delegation-flow small { font-size: .75rem; opacity: .75; }
.delegation-flow strong { font-size: 1.1rem; }
.delegation-flow b { color: #e9a06b; }
.delegation-runs { display: grid; gap: 1rem; list-style: none; padding: 0; margin: 0; }
.delegation-run, .delegation-empty { padding: 1.5rem; border: 1px solid #ffffff20; border-radius: 1rem; background: #ffffff04; }
.delegation-run.working { border-color: #e9a06b80; }
.run-heading, .delegation-run footer { display: flex; justify-content: space-between; gap: 1rem; flex-wrap: wrap; align-items: center; }
.run-status { font-size: .8rem; border: 1px solid #ffffff30; border-radius: 2rem; padding: .4rem .8rem; }
.run-activity { font-family: monospace; color: #e9a06b; }
.delegation-run dl { display: grid; grid-template-columns: repeat(5, 1fr); gap: 1rem; margin: 1.5rem 0; }
.delegation-run dt { font-size: .75rem; opacity: .7; }
.delegation-run dd { margin: .4rem 0 0; font-size: 1.35rem; font-variant-numeric: tabular-nums; }
.delegation-run footer, .delegation-note { font-size: .75rem; opacity: .7; }
@media (max-width: 640px) {
  .delegation-flow { grid-template-columns: 1fr; }
  .delegation-flow b { transform: rotate(90deg); justify-self: center; }
  .delegation-run dl { grid-template-columns: repeat(2, 1fr); }
}
</style>
