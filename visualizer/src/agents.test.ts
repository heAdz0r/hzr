import { describe, expect, test } from "bun:test";
import {
  AGENT_BOARD_COLUMNS,
  AGENT_ONBOARDING_COMMANDS,
  UNKNOWN,
  coverageLabel,
  formatAge,
  formatMicrounits,
  freshnessLabel,
  graphElements,
  graphSignature,
  groupByColumn,
  taskDisplayName,
  taskHandle,
  type AgentEdgeView,
  type AgentTaskSummary,
} from "./agents";

function task(overrides: Partial<AgentTaskSummary> = {}): AgentTaskSummary {
  return {
    task_id: "a".repeat(64),
    label: "Task 7ac3",
    title: "Wire the retry budget",
    branch: "feat/retry-budget",
    board_status: "running",
    unknown_board_status: null,
    runtime_phase: "working",
    runtime_freshness: "fresh",
    runtime_observed_at_ms: 1_788_258_000_000,
    hook_state: "working",
    hook_freshness: "fresh",
    hook_observed_at_ms: 1_788_258_000_000,
    agent: "claude",
    cycle: 1,
    first_observed_at_ms: 1_788_257_000_000,
    last_observed_at_ms: 1_788_258_000_000,
    source_updated_at_ms: 1_788_257_900_000,
    source_age_ms: 100_000,
    linked_session_count: 1,
    usage_covered: true,
    tombstoned: false,
    ...overrides,
  };
}

describe("board grouping", () => {
  test("keeps the five upstream columns and a separate unknown group", () => {
    expect(AGENT_BOARD_COLUMNS).toEqual([
      "backlog",
      "planning",
      "running",
      "review",
      "done",
      "unknown",
    ]);
    const columns = groupByColumn([
      task({ task_id: "1", board_status: "running" }),
      task({ task_id: "2", board_status: "unknown", unknown_board_status: "shipping" }),
    ]);
    expect(columns.map((column) => column.tasks.length)).toEqual([0, 0, 1, 0, 0, 1]);
    expect(columns[5].label).toBe("Unrecognised state");
  });
});

describe("freshness", () => {
  test("says no evidence rather than showing a zero age", () => {
    expect(freshnessLabel("unavailable", null)).toContain(UNKNOWN);
    expect(freshnessLabel("unavailable", null)).not.toContain("0s");
  });

  test("labels stale evidence as stale however recently it was read", () => {
    expect(freshnessLabel("stale", 600_000)).toBe("Stale · 10m ago");
    expect(freshnessLabel("fresh", 4_000)).toBe("Fresh · 4s ago");
  });

  test("a negative or non-finite age is unknown", () => {
    expect(formatAge(-1)).toBe(UNKNOWN);
    expect(formatAge(Number.NaN)).toBe(UNKNOWN);
  });
});

describe("money", () => {
  test("microunits render exactly, with their own currency", () => {
    expect(
      formatMicrounits({
        currency: "USD",
        microunits: 5_400,
        covered_receipts: 2,
        total_receipts: 2,
      }),
    ).toBe("USD 0.005400");
    expect(
      formatMicrounits({
        currency: "EUR",
        microunits: 1_800,
        covered_receipts: 1,
        total_receipts: 1,
      }),
    ).toBe("EUR 0.001800");
  });

  test("coverage always carries both halves", () => {
    expect(
      coverageLabel({
        currency: "USD",
        microunits: 4_000,
        covered_receipts: 1,
        total_receipts: 2,
      }),
    ).toBe("1/2 receipts");
  });
});

describe("dependency graph", () => {
  const edges: AgentEdgeView[] = [
    { from_task_id: "1", to_task_id: "2", kind: "depends_on", resolved: true },
    { from_task_id: "ghost", to_task_id: "2", kind: "depends_on", resolved: false },
  ];

  test("an unresolved reference stays visible as its own node", () => {
    const elements = graphElements(
      [task({ task_id: "1" }), task({ task_id: "2" })],
      edges,
    );
    const ghost = elements.find((element) => element.data.id === "ghost");
    expect(ghost?.classes).toContain("unresolved");
    expect(
      elements.filter((element) => element.classes?.startsWith("dependency")).length,
    ).toBe(2);
  });

  test("a cycle produces elements without looping", () => {
    const cyclic: AgentEdgeView[] = [
      { from_task_id: "1", to_task_id: "2", kind: "depends_on", resolved: true },
      { from_task_id: "2", to_task_id: "1", kind: "depends_on", resolved: true },
    ];
    const elements = graphElements([task({ task_id: "1" }), task({ task_id: "2" })], cyclic);
    expect(elements.length).toBe(4);
  });

  test("the signature is stable across an unchanged poll and moves on a real change", () => {
    const tasks = [task({ task_id: "1" }), task({ task_id: "2" })];
    const first = graphSignature(tasks, edges);
    expect(graphSignature([...tasks].reverse(), [...edges].reverse())).toBe(first);
    expect(
      graphSignature([task({ task_id: "1", board_status: "review" }), tasks[1]], edges),
    ).not.toBe(first);
  });
});

describe("naming", () => {
  test("a title leads and the pseudonym stays as the handle", () => {
    const named = task();
    expect(taskDisplayName(named)).toBe("Wire the retry budget");
    expect(taskHandle(named)).toBe("Task 7ac3");
  });

  test("with no title the pseudonym stands alone rather than being doubled", () => {
    const anonymous = task({ title: null });
    expect(taskDisplayName(anonymous)).toBe("Task 7ac3");
    expect(taskHandle(anonymous)).toBe("");
  });

  test("a blank title is not a name", () => {
    expect(taskDisplayName(task({ title: "   " }))).toBe("Task 7ac3");
  });

  test("graph nodes are labelled by what a human can recognise", () => {
    const elements = graphElements([task({ task_id: "1" })], []);
    expect(elements[0].data.label).toBe("Wire the retry budget");
  });
});

describe("onboarding", () => {
  test("names installation and enrollment as two separate steps", () => {
    expect(AGENT_ONBOARDING_COMMANDS).toHaveLength(2);
    expect(AGENT_ONBOARDING_COMMANDS[0]).toContain("component install");
    expect(AGENT_ONBOARDING_COMMANDS[1]).toContain("--agtx-data-dir");
  });
});
