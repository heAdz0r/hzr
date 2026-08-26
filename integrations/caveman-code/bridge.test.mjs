import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  formatPrefetchedContext,
  isDirectExecution,
  persistUsageOutbox,
  prepareManagedRuntime,
  replayUsageOutbox,
  stripManagedHzrContract,
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

const BRIDGE_MANIFEST = JSON.parse(
  await readFile(new URL("./package.json", import.meta.url), "utf8"),
);

test("bridge import is side-effect free", () => {
  assert.equal(isDirectExecution(), false);
});

test("production preparation owns tools and disables duplicate subsystems in order", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "hzr-caveman-bridge-"));
  const workspace = join(root, "workspace");
  const agentDir = join(root, "agent");
  await mkdir(workspace);
  await mkdir(agentDir);
  await writeFile(
    join(workspace, "AGENTS.md"),
    "# Project rules\nPreserve exact errors.\n\n<!-- hzr:begin managed agent contract — do not edit inside -->\n# HZR tool contract (managed)\nDuplicated harness contract.\n<!-- hzr:end managed agent contract -->\n",
  );
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
        hzr_version: BRIDGE_MANIFEST.version,
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
  assert.doesNotMatch(
    prepared.resourceLoader.getAppendSystemPrompt()[0],
    /HZR tool contract \(managed\)|Duplicated harness contract/,
  );
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

test("managed HZR contract parser rejects stray, nested, and unterminated markers", () => {
  assert.throws(
    () => stripManagedHzrContract(`rules\n${"<!-- hzr:end managed agent contract -->"}`, "AGENTS.md"),
    /unexpected HZR contract end/,
  );
  assert.throws(
    () => stripManagedHzrContract(
      `${"<!-- hzr:begin managed agent contract — do not edit inside -->"}\n${"<!-- hzr:begin managed agent contract — do not edit inside -->"}\n${"<!-- hzr:end managed agent contract -->"}`,
      "AGENTS.md",
    ),
    /nested HZR contract/,
  );
  assert.throws(
    () => stripManagedHzrContract(
      `${"<!-- hzr:begin managed agent contract — do not edit inside -->"}\nrules`,
      "AGENTS.md",
    ),
    /unterminated HZR contract/,
  );
});

test("context formatter enforces its byte budget for multibyte UTF-8 delivery", () => {
  const formatted = formatPrefetchedContext(JSON.stringify({
    pack: {
      hard_limit: 1_024,
      selected: [
        { source: "context", content_ref: "first", tokens: 900 },
        { source: "context", content_ref: "second", tokens: 900 },
      ],
    },
    contents: {
      first: "😀".repeat(950),
      second: "界".repeat(2_000),
    },
    warnings: [],
  }));

  assert.ok(Buffer.byteLength(formatted) <= 4 * 1024);
  assert.match(formatted, /selected candidates? omitted by managed delivery budget/);
  assert.doesNotMatch(formatted, /�/u);
});

test("context formatter preserves provenance and truncates only at candidate boundaries", () => {
  const first = `${"a".repeat(20_000)}END_FIRST`;
  const second = `BEGIN_SECOND${"b".repeat(20_000)}`;
  const formatted = formatPrefetchedContext(JSON.stringify({
    pack: {
      hard_limit: 6_000,
      selected: [
        {
          source: "memory",
          content_ref: "sha256:first",
          relevance: 0.8,
          tokens: 5_000,
          freshness: "2026-08-25T00:00:00Z",
          trust: "icm:user",
          provenance: {
            source: "icm",
            generation: "memory-7",
            canonical_ref: "project:architecture:ssot",
          },
        },
        {
          source: "context",
          content_ref: "sha256:second",
          relevance: 0.7,
          tokens: 5_000,
        },
      ],
    },
    contents: {
      "sha256:first": first,
      "sha256:second": second,
    },
    warnings: [],
  }));

  assert.match(formatted, /freshness="2026-08-25T00:00:00Z"/);
  assert.match(formatted, /trust="icm:user"/);
  assert.match(formatted, /provenance_source="icm"/);
  assert.match(formatted, /generation="memory-7"/);
  assert.match(formatted, /canonical_ref="project:architecture:ssot"/);
  assert.match(formatted, /END_FIRST/);
  assert.doesNotMatch(formatted, /BEGIN_SECOND/);
  assert.match(formatted, /1 selected candidate omitted by managed delivery budget/);
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

// The bridge manifest restates the product version, and `prepareManagedRuntime` refuses to
// start when it drifts. Catch that here, in a one-second test, instead of three minutes into
// a release bundle build.
test("bridge manifest version tracks the workspace version", async () => {
  const [manifest, workspaceManifest] = await Promise.all([
    readFile(new URL("./package.json", import.meta.url), "utf8").then(JSON.parse),
    readFile(new URL("../../Cargo.toml", import.meta.url), "utf8"),
  ]);
  const workspaceVersion = workspaceManifest.match(
    /^\[workspace\.package\][\s\S]*?^version = "([^"]+)"/m,
  )?.[1];
  assert.ok(workspaceVersion, "workspace version is declared in Cargo.toml");
  assert.equal(
    manifest.version,
    workspaceVersion,
    "bump integrations/caveman-code/package.json (and its lock) with the workspace version",
  );
});
