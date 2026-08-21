import assert from "node:assert/strict";
import { mkdtemp, mkdir, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  formatPrefetchedContext,
  isDirectExecution,
  persistUsageOutbox,
  prepareManagedRuntime,
  replayUsageOutbox,
} from "./bridge.mjs";

const MANAGED_TOOLS = [
  "hzr_context",
  "hzr_search",
  "hzr_read",
  "hzr_edit",
  "hzr_write",
  "hzr_memory_recall",
  "hzr_memory_store",
  "hzr_memory_forget",
  "hzr_memory_update",
  "hzr_memory_prune",
  "hzr_exec",
];

test("bridge import is side-effect free", () => {
  assert.equal(isDirectExecution(), false);
});

test("production preparation owns tools and disables duplicate subsystems in order", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "hzr-caveman-bridge-"));
  const workspace = join(root, "workspace");
  const agentDir = join(root, "agent");
  await mkdir(workspace);
  await mkdir(agentDir);
  await writeFile(join(workspace, "AGENTS.md"), "# Project rules\nPreserve exact errors.\n");
  await writeFile(join(workspace, "CLAUDE.md"), "# Repository map\nUse HZR tools.\n");

  const environmentKeys = ["CAVE_OMIT_CLAUDE_MD", "CAVE_MEMORY_AUTO_RECORD", "CAVE_CHAT_MODE"];
  const originalEnvironment = new Map(
    environmentKeys.map((key) => [key, process.env[key]]),
  );
  t.after(async () => {
    for (const [key, value] of originalEnvironment) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    await rm(root, { recursive: true, force: true });
  });

  const order = [];
  const delegated = [];
  let sessionOptions;
  const callHzr = async (route, body, _signal, method = "POST") => {
    order.push(route);
    if (route === "/v1/health") {
      assert.equal(method, "GET");
      return JSON.stringify({
        protocol_version: 1,
        hzr_version: "0.4.5",
        engines: [
          { name: "rtk", state: "ready" },
          { name: "grepai", state: "stopped" },
          { name: "icm", state: "ready" },
        ],
      });
    }
    assert.equal(route, "/v1/context/plan");
    assert.deepEqual(body, {
      workspace,
      intent: "inspect ownership",
      search_limit: 10,
      memory_limit: 5,
    });
    return JSON.stringify({
      pack: {
        selected: [
          {
            source: "context",
            content_ref: "sha256:owned-by-hzr",
            path: "src/lib.rs",
            line_start: 10,
            line_end: 20,
            relevance: 0.75,
            tokens: 40,
          },
        ],
        used: 40,
        hard_limit: 16_000,
        coverage: 1,
        confidence: 0.8,
      },
      contents: { "sha256:owned-by-hzr": "fn owned_by_hzr() {}" },
      warnings: [],
    });
  };

  const createSession = async (options) => {
    order.push("create_session");
    sessionOptions = options;
    const appended = options.resourceLoader.getAppendSystemPrompt();
    const responseContract = appended.at(-1);
    const session = {
      agent: {
        async beforeToolCall(context) {
          delegated.push(context.toolCall.name);
          return context;
        },
      },
      memoryEnabled: true,
      systemPrompt: responseContract,
      setRepomapEnabled(value) {
        this._repomapEnabled = value;
      },
      setMemoryEnabled(value) {
        this.memoryEnabled = value;
      },
      setAutoSnapshotEnabled(value) {
        this._checkpointAutoSnapshotEnabled = value;
      },
      getActiveToolNames() {
        return options.customTools.map((tool) => tool.name);
      },
      getSessionStats() {
        return { tokens: {}, assistantMessages: 0 };
      },
      abort() {},
    };
    return { session, modelFallbackMessage: null };
  };

  const prepared = await prepareManagedRuntime({
    request: {
      prompt: "inspect ownership",
      response_format: "text",
      max_turns: 3,
    },
    environment: { agentDir },
    callHzr,
    workspace,
    createSession,
  });

  assert.deepEqual(order, ["/v1/health", "/v1/context/plan", "create_session"]);
  assert.deepEqual(sessionOptions.tools, []);
  assert.deepEqual(
    sessionOptions.customTools.map((tool) => tool.name),
    MANAGED_TOOLS,
  );
  assert.equal(prepared.settings.getRtkEnabled(), false);
  assert.equal(prepared.settings.getCaveModeEnabled(), false);
  assert.equal(prepared.settings.getCaveModeToolCompression(), false);
  assert.equal(prepared.settings.getCaveModeMLCompression(), false);
  assert.equal(prepared.settings.getTelemetryEnabled(), false);
  assert.equal(prepared.settings.getDisableAllHooks(), true);
  assert.equal(prepared.session._repomapEnabled, false);
  assert.equal(prepared.session.memoryEnabled, false);
  assert.equal(prepared.session._checkpointAutoSnapshotEnabled, false);
  assert.deepEqual(prepared.resourceLoader.getExtensions().extensions, []);
  assert.deepEqual(prepared.resourceLoader.getSkills().skills, []);
  assert.deepEqual(prepared.resourceLoader.getPrompts().prompts, []);
  assert.deepEqual(prepared.resourceLoader.getAgentsFiles().agentsFiles, []);
  assert.equal(prepared.resourceLoader.getAppendSystemPrompt().length, 2);
  assert.match(prepared.resourceLoader.getAppendSystemPrompt()[0], /Project rules/);
  assert.match(prepared.resourceLoader.getAppendSystemPrompt()[0], /Repository map/);
  assert.match(prepared.prefetchedContext, /path="src\/lib\.rs"/);
  assert.match(prepared.prefetchedContext, /lines=10-20/);
  assert.match(prepared.prefetchedContext, /ref="sha256:owned-by-hzr"/);
  assert.match(prepared.prefetchedContext, /fn owned_by_hzr\(\) \{\}/);
  assert.doesNotMatch(prepared.prefetchedContext, /\"pack\"/);
  assert.equal(process.env.CAVE_OMIT_CLAUDE_MD, "1");
  assert.equal(process.env.CAVE_MEMORY_AUTO_RECORD, "0");
  assert.equal(process.env.CAVE_CHAT_MODE, "auto");

  await assert.rejects(
    prepared.session.agent.beforeToolCall({ toolCall: { name: "bash" } }),
    /Caveman native tool execution blocked/,
  );
  assert.deepEqual(delegated, []);
  await prepared.session.agent.beforeToolCall({ toolCall: { name: "hzr_read" } });
  assert.deepEqual(delegated, ["hzr_read"]);
});

test("context formatter rejects malformed planner responses", () => {
  assert.throws(
    () => formatPrefetchedContext("{\"pack\":{}}"),
    /selected candidates/,
  );
});

test("usage outbox survives a failed send and replays exactly once", async (t) => {
  const agentDir = await mkdtemp(join(tmpdir(), "hzr-usage-outbox-"));
  t.after(() => rm(agentDir, { recursive: true, force: true }));
  const payload = { trace_id: "request-7", provider: "test", model: "model", turns: 1 };

  await persistUsageOutbox(agentDir, payload);
  const outbox = join(agentDir, "usage-outbox");
  const queued = await readdir(outbox);
  assert.equal(queued.length, 1);
  assert.equal((await stat(join(outbox, queued[0]))).mode & 0o777, 0o600);
  const seen = [];
  const warnings = await replayUsageOutbox(agentDir, async (route, body) => {
    seen.push({ route, body });
    return JSON.stringify({ recorded: true });
  });

  assert.deepEqual(warnings, []);
  assert.deepEqual(seen, [{ route: "/v1/usage", body: payload }]);
  assert.deepEqual(await readdir(outbox), []);
  assert.equal((await stat(outbox)).mode & 0o777, 0o700);
});
