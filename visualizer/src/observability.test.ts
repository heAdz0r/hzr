import { describe, expect, test } from "bun:test";
import { groupTraceSpans, mergeObservability, nextRefreshBackoff } from "./observability";
import type { DashboardTraceSpan } from "./types";

function span(overrides: Partial<DashboardTraceSpan>): DashboardTraceSpan {
  return {
    sequence: 1,
    trace_hash: "hmac-sha256:trace-a",
    linked_trace_hash: null,
    span_id: 1,
    parent_span_id: null,
    stage: "request",
    state: "completed",
    engine: "hzrd",
    observed_at_ms: 10,
    duration_ms: 4,
    project_hash: "hmac-sha256:project",
    session_hash: "hmac-sha256:session",
    route: null,
    error_code: null,
    producer_version: "hzr-daemon/test",
    policy_version: "privacy_typed_v1",
    generation: null,
    ...overrides,
  };
}

describe("privacy-safe observability timeline", () => {
  test("groups correlated spans, preserves stage order, and surfaces failures", () => {
    const groups = groupTraceSpans([
      span({ span_id: 9, stage: "engine", state: "failed", error_code: "search_failed" }),
      span({ span_id: 7, stage: "request" }),
      span({ trace_hash: "hmac-sha256:trace-b", span_id: 10, observed_at_ms: 20 }),
    ]);

    expect(groups.map((group) => group.hash)).toEqual([
      "hmac-sha256:trace-b",
      "hmac-sha256:trace-a",
    ]);
    expect(groups[1]?.spans.map((item) => item.span_id)).toEqual([7, 9]);
    expect(groups[1]?.failed).toBe(true);
  });

  test("serialized component input has no content-bearing fields", () => {
    const encoded = JSON.stringify(groupTraceSpans([span({ route: "search" })]));
    for (const forbidden of [
      "command",
      "working_directory",
      "query",
      "stdin",
      "stdout",
      "provider_trace_id",
      "/Users/private/project",
    ]) {
      expect(encoded).not.toContain(forbidden);
    }
  });

  test("surfaces approval continuations as a visible causal link", () => {
    const parent = "hmac-sha256:parent-trace";
    const groups = groupTraceSpans([
      span({
        trace_hash: "hmac-sha256:approval-trace",
        linked_trace_hash: parent,
        state: "completed",
      }),
    ]);
    expect(groups[0]?.linkedFrom).toBe(parent);
  });

  test("merges cursor deltas with hard bounds and preserves truncation evidence", () => {
    const current = {
      trace_spans: [span({ sequence: 1 })],
      lifecycle_events: [],
      next_cursor: 1,
      truncated: true,
    };
    const delta = {
      trace_spans: [span({ sequence: 2 }), span({ sequence: 3 })],
      lifecycle_events: [],
      next_cursor: 3,
      truncated: false,
    };
    const merged = mergeObservability(current, delta, 2);
    expect(merged.trace_spans.map((item) => item.sequence)).toEqual([2, 3]);
    expect(merged.next_cursor).toBe(3);
    expect(merged.truncated).toBe(true);
  });

  test("refresh backoff resets on success and doubles to a cap", () => {
    expect(nextRefreshBackoff(2_000, true, 2_000, 8_000)).toBe(4_000);
    expect(nextRefreshBackoff(8_000, true, 2_000, 8_000)).toBe(8_000);
    expect(nextRefreshBackoff(8_000, false, 2_000, 8_000)).toBe(2_000);
  });
});
