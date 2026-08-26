import type { DashboardObservability, DashboardTraceSpan } from "./types";

export interface TraceGroup {
  hash: string;
  linkedFrom: string | null;
  spans: DashboardTraceSpan[];
  observedAt: number;
  duration: number;
  failed: boolean;
}

export function groupTraceSpans(spans: DashboardTraceSpan[], limit = 12): TraceGroup[] {
  const grouped = new Map<string, DashboardTraceSpan[]>();
  for (const span of spans) {
    const trace = grouped.get(span.trace_hash) ?? [];
    trace.push(span);
    grouped.set(span.trace_hash, trace);
  }
  return [...grouped.entries()]
    .map(([hash, trace]) => ({
      hash,
      linkedFrom: trace.find((span) => span.linked_trace_hash !== null)?.linked_trace_hash ?? null,
      spans: [...trace].sort((left, right) => left.span_id - right.span_id),
      observedAt: Math.max(...trace.map((span) => span.observed_at_ms)),
      duration: Math.max(...trace.map((span) => span.duration_ms)),
      failed: trace.some((span) => span.state === "failed" || span.state === "denied"),
    }))
    .sort((left, right) => right.observedAt - left.observedAt)
    .slice(0, limit);
}

export function mergeObservability(
  current: DashboardObservability,
  delta: DashboardObservability,
  limit = 100,
): DashboardObservability {
  return {
    trace_spans: [...current.trace_spans, ...delta.trace_spans].slice(-limit),
    lifecycle_events: [...current.lifecycle_events, ...delta.lifecycle_events].slice(-limit),
    next_cursor: delta.next_cursor ?? current.next_cursor,
    truncated: current.truncated || delta.truncated,
  };
}

export function nextRefreshBackoff(current: number, failed: boolean, base: number, cap: number): number {
  return failed ? Math.min(Math.max(current, base) * 2, cap) : base;
}
