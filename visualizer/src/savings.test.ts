// 0.11.2: headline and last-session selection.
import { describe, expect, test } from "bun:test";
import { parseLedgerTimestamp, selectSavingsHeadline, summarizeLastSession } from "./savings";

const producerOnly = {
  baseline_tokens_estimated: 1_000,
  delivered_tokens_estimated: 600,
  net_avoided_tokens_estimated: 400,
};

describe("savings headline", () => {
  test("falls back to producer figures when the daemon omits host-capped fields", () => {
    const headline = selectSavingsHeadline(producerOnly);
    expect(headline.source).toBe("producer");
    expect(headline.primary).toEqual(headline.producer);
    expect(headline.primary.netAvoided).toBe(400);
    expect(headline.primary.reductionPct).toBe(40);
    expect(headline.ceilingTokens).toBeNull();
  });

  test("leads with host-capped figures and keeps producer figures secondary", () => {
    const headline = selectSavingsHeadline({
      ...producerOnly,
      host_visible_baseline_tokens_estimated: 500,
      host_visible_delivered_tokens_estimated: 400,
      host_visible_net_avoided_tokens_estimated: 100,
      host_ceiling_tokens: 8_000,
    });
    expect(headline.source).toBe("host_capped");
    expect(headline.primary).toEqual({ baseline: 500, delivered: 400, netAvoided: 100, reductionPct: 20 });
    expect(headline.producer.netAvoided).toBe(400);
    expect(headline.ceilingTokens).toBe(8_000);
  });

  test("a partial host-capped set never becomes the headline", () => {
    const headline = selectSavingsHeadline({ ...producerOnly, host_visible_net_avoided_tokens_estimated: 100 });
    expect(headline.source).toBe("producer");
  });

  test("a null ceiling stays null and zero baseline is unknown, not 0%", () => {
    const headline = selectSavingsHeadline({
      ...producerOnly,
      host_visible_baseline_tokens_estimated: 0,
      host_visible_delivered_tokens_estimated: 0,
      host_visible_net_avoided_tokens_estimated: 0,
      host_ceiling_tokens: null,
    });
    expect(headline.source).toBe("host_capped");
    expect(headline.ceilingTokens).toBeNull();
    expect(headline.primary.reductionPct).toBeNull();
  });
});

describe("last session", () => {
  test("renders only what the payload supplies", () => {
    const summary = summarizeLastSession({ operations: 3 });
    expect(summary.operations).toBe(3);
    expect(summary.producer).toBeNull();
    expect(summary.hostCapped).toBeNull();
    expect(summary.startedAtMs).toBeNull();
    expect(summary.topRoutes).toEqual([]);
    expect(summary.pricedValue).toBeNull();
  });

  test("derives the host-capped share when only the baseline is given", () => {
    const summary = summarizeLastSession({
      operations: 10,
      baseline_tokens_estimated: 2_000,
      delivered_tokens_estimated: 500,
      host_visible_baseline_tokens_estimated: 1_000,
      host_visible_net_avoided_tokens_estimated: 250,
    });
    expect(summary.producer).toEqual({ baseline: 2_000, delivered: 500, netAvoided: 1_500, reductionPct: 75 });
    expect(summary.hostCapped).toEqual({ netAvoided: 250, reductionPct: 25 });
  });

  test("an explicit host share wins and top routes are ordered and bounded", () => {
    const summary = summarizeLastSession({
      operations: 10,
      host_visible_net_avoided_tokens_estimated: 250,
      host_visible_reduction_pct: 12.5,
      top_routes: Array.from({ length: 7 }, (_, index) => ({ route: `r${index}`, operations: index })),
    });
    expect(summary.hostCapped?.reductionPct).toBe(12.5);
    expect(summary.topRoutes.map((route) => route.route)).toEqual(["r6", "r5", "r4", "r3", "r2"]);
  });

  test("ledger timestamps are read as UTC", () => {
    expect(parseLedgerTimestamp("2026-09-26 17:03:28")).toBe(Date.UTC(2026, 8, 26, 17, 3, 28));
    expect(parseLedgerTimestamp("2026-09-26T17:03:28Z")).toBe(Date.UTC(2026, 8, 26, 17, 3, 28));
    expect(parseLedgerTimestamp("not a time")).toBeNull();
    expect(parseLedgerTimestamp(null)).toBeNull();
  });
});
