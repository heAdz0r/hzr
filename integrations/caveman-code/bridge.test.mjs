import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { isDirectExecution, prepareManagedRuntime } from "./bridge.mjs";

const MANAGED_TOOLS = [
  "hzr_context",
  "hzr_search",
  "hzr_read",
  "hzr_edit",
  "hzr_write",
  "hzr_memory_recall",
  "hzr_memory_store",
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
        hzr_version: "0.3.2",
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
    return "{\"pack\":{\"selected\":[]}}";
  };

  const createSession = async (options) => {
    order.push("create_session");
    sessionOptions = options;
    const responseContract = options.resourceLoader.getAppendSystemPrompt()[0];
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
