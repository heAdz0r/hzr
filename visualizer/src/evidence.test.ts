import { describe, expect, test } from "bun:test";
import { filterActivity, outputReduction, tokenBarWidth } from "./evidence";
import type { DashboardLocalOperation } from "./types";

const operation = (overrides: Partial<DashboardLocalOperation>): DashboardLocalOperation => ({
  ledger_id: 1, timestamp: "2026-09-04T12:00:00Z", operation: "read", route: "optimized",
  command_hash: "private", project_hash: "project", agent: "codex", session_hash: "session-1",
  producer_version: "0.7.1", policy_version: "v1", baseline_tokens_estimated: 100,
  delivered_tokens_estimated: 80, net_avoided_tokens_estimated: 20, execution_ms: 1,
  replacement: null, rationale: null, ...overrides,
});

describe("bounded activity exploration", () => {
  const rows = [operation({ ledger_id: 1 }), operation({ ledger_id: 2, route: "raw", agent: "claude", session_hash: "session-2", net_avoided_tokens_estimated: 0 }), operation({ ledger_id: 3, net_avoided_tokens_estimated: -30 }), operation({ ledger_id: 4, agent: null, session_hash: null, route: "native_unaccounted" })];
  test("combines route, agent and session without changing the source snapshot", () => {
    expect(filterActivity(rows, "optimized", "codex", "session-1").map(row => row.ledger_id)).toEqual([1, 3]);
    expect(filterActivity(rows, "raw", "codex", "")).toEqual([]);
    expect(rows).toHaveLength(4);
  });
  test("makes regressions and missing attribution discoverable", () => {
    expect(filterActivity(rows, "regressions", "", "").map(row => row.ledger_id)).toEqual([3]);
    expect(filterActivity(rows, "all", "Unattributed", "Unattributed").map(row => row.ledger_id)).toEqual([4]);
    expect(filterActivity(rows, "all", "", "")).toHaveLength(4);
  });
});

test("zero baseline is unknown, while output growth stays negative", () => {
  const activity = { baseline_tokens_estimated: 0, net_avoided_tokens_estimated: 0 };
  expect(outputReduction(activity)).toBeNull();
  expect(outputReduction({ ...activity, baseline_tokens_estimated: 100, net_avoided_tokens_estimated: -50 })).toBe(-50);
});

test("comparison bars never invent nonzero output or overflow", () => {
  expect(tokenBarWidth(0, 100)).toBe("0%");
  expect(tokenBarWidth(150, 150)).toBe("100%");
  expect(tokenBarWidth(150, 100)).toBe("100%");
  expect(tokenBarWidth(30, 100)).toBe("30%");
  expect(tokenBarWidth(1, 0)).toBe("0%");
});
