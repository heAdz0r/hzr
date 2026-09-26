import { chmodSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";

// A bounded status projection in the existing private session directory.
// Never persist prompts, tool arguments/results, reasoning, credentials or costs.
export function createDelegationProgress(directory, selection, workspace, runId) {
  if (!selection) return null;
  if (!/^[a-z0-9-]{1,40}$/.test(selection.provider)
      || !/^[a-zA-Z0-9_./:-]{1,160}$/.test(selection.model)) {
    throw new Error("Invalid delegation progress identity");
  }
  const file = join(directory, "delegation.json");
  const temporary = join(directory, ".delegation.json.tmp");
  const state = {
    schema_version: 1, run_id: runId, execution: "hzr-managed",
    provider: selection.provider, model: selection.model, workspace,
    status: "starting", started_at_ms: Date.now(), updated_at_ms: Date.now(),
    tool_calls: 0, turns: 0, current_tool: null, history: [],
    actual_input_tokens: 0, actual_output_tokens: 0,
    actual_cache_read_tokens: 0, usage_observed: false,
    parent_acceptance: "not_recorded",
  };
  function save() {
    state.updated_at_ms = Date.now();
    writeFileSync(temporary, JSON.stringify(state), { mode: 0o600 });
    chmodSync(temporary, 0o600);
    renameSync(temporary, file);
  }
  save();
  let closed = false;
  let warned = false;
  function saveObservedStatus() {
    try { save(); } catch {
      if (!warned) {
        process.stderr.write("HZR delegation progress could not be saved.\n");
        warned = true;
      }
    }
  }
  const heartbeat = setInterval(saveObservedStatus, 5000);
  heartbeat.unref();
  return {
    observe(kind, event) {
      if (closed) return;
      if (kind === "ready") state.status = "running";
      if (kind === "agent_event" && event?.type === "tool_execution_start"
          && /^hzr_[a-z_]{1,40}$/.test(event.toolName ?? "")) {
        state.current_tool = event.toolName;
        state.tool_calls += 1;
        state.history.push({ tool: event.toolName, at_ms: Date.now() });
        state.history = state.history.slice(-30);
      } else if (kind === "agent_event" && event?.type === "tool_execution_end") {
        state.current_tool = null;
      } else if (kind === "agent_event" && event?.type === "message_end"
          && event.message?.role === "assistant") {
        state.turns += 1;
        const usage = event.message.usage;
        if (usage && usage.input + usage.output + usage.cacheRead > 0
            && ["input", "output", "cacheRead"].every(key =>
          Number.isSafeInteger(usage[key]) && usage[key] >= 0)) {
          state.usage_observed = true;
          state.actual_input_tokens += usage.input;
          state.actual_output_tokens += usage.output;
          state.actual_cache_read_tokens += usage.cacheRead;
        }
      } else if (kind === "result" || kind === "error") {
        // 0.10.1: an exhausted budget is "incomplete", and a failure keeps its reason
        state.status = kind === "result" ? (event?.status === "incomplete" ? "incomplete" : "completed") : "failed";
        if (kind === "error" && typeof event?.message === "string") {
          state.error = event.message.slice(0, 300);
        }
        state.current_tool = null;
        closed = true;
        clearInterval(heartbeat);
      } else {
        return;
      }
      saveObservedStatus();
    },
    close() { closed = true; clearInterval(heartbeat); },
  };
}
