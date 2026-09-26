// 0.11.2: headline selection for the Overview. Host-capped figures lead when the
// daemon supplies all of them; otherwise the producer figures stay the headline,
// exactly as before. Pure, so the choice is unit-tested rather than eyeballed.
import type {
  DashboardLastSession,
  DashboardLastSessionRoute,
  DashboardLocalActivity,
  DashboardRawPublicEstimate,
} from "./types";

export interface TokenFigures {
  baseline: number;
  delivered: number;
  netAvoided: number;
  /** Net as a share of the baseline; null when there is no baseline to divide by. */
  reductionPct: number | null;
}

export interface SavingsHeadline {
  source: "host_capped" | "producer";
  /** The figures the headline shows. */
  primary: TokenFigures;
  /** Raw producer figures, always available as the secondary line. */
  producer: TokenFigures;
  /** Per-operation host ceiling in tokens; null when none applies or the source is producer. */
  ceilingTokens: number | null;
}

function isCount(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

export function reductionPct(netAvoided: number, baseline: number): number | null {
  return baseline > 0 ? (netAvoided * 100) / baseline : null;
}

function figures(baseline: number, delivered: number, netAvoided: number): TokenFigures {
  return { baseline, delivered, netAvoided, reductionPct: reductionPct(netAvoided, baseline) };
}

type HeadlineInput = Pick<
  DashboardLocalActivity,
  | "baseline_tokens_estimated"
  | "delivered_tokens_estimated"
  | "net_avoided_tokens_estimated"
  | "host_visible_baseline_tokens_estimated"
  | "host_visible_delivered_tokens_estimated"
  | "host_visible_net_avoided_tokens_estimated"
  | "host_ceiling_tokens"
>;

/** Host-capped only when all three host figures are present; a partial set never leads. */
export function selectSavingsHeadline(activity: HeadlineInput): SavingsHeadline {
  const producer = figures(
    activity.baseline_tokens_estimated,
    activity.delivered_tokens_estimated,
    activity.net_avoided_tokens_estimated,
  );
  const baseline = activity.host_visible_baseline_tokens_estimated;
  const delivered = activity.host_visible_delivered_tokens_estimated;
  const net = activity.host_visible_net_avoided_tokens_estimated;
  if (isCount(baseline) && isCount(delivered) && isCount(net)) {
    return {
      source: "host_capped",
      primary: figures(baseline, delivered, net),
      producer,
      ceilingTokens: isCount(activity.host_ceiling_tokens) ? activity.host_ceiling_tokens : null,
    };
  }
  return { source: "producer", primary: producer, producer, ceilingTokens: null };
}

export interface LastSessionSummary {
  operations: number;
  optimizedOperations: number | null;
  rawOperations: number | null;
  /** Raw tool output → filtered output, producer side; null when not supplied. */
  producer: TokenFigures | null;
  /** Host-capped net and share; null when not supplied. */
  hostCapped: { netAvoided: number; reductionPct: number | null } | null;
  startedAtMs: number | null;
  endedAtMs: number | null;
  topRoutes: DashboardLastSessionRoute[];
  pricedValue: DashboardRawPublicEstimate | null;
}

/** Ledger timestamps arrive as `YYYY-MM-DD HH:MM:SS` (UTC) or ISO 8601. */
export function parseLedgerTimestamp(value: string | null | undefined): number | null {
  if (!value) return null;
  const iso = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/.test(value) ? `${value.replace(" ", "T")}Z` : value;
  const parsed = Date.parse(iso);
  return Number.isNaN(parsed) ? null : parsed;
}

export function summarizeLastSession(session: DashboardLastSession): LastSessionSummary {
  const hasProducer =
    isCount(session.baseline_tokens_estimated) && isCount(session.delivered_tokens_estimated);
  const producer = hasProducer
    ? figures(
        session.baseline_tokens_estimated as number,
        session.delivered_tokens_estimated as number,
        isCount(session.net_avoided_tokens_estimated)
          ? session.net_avoided_tokens_estimated
          : (session.baseline_tokens_estimated as number) - (session.delivered_tokens_estimated as number),
      )
    : null;
  const hostNet = session.host_visible_net_avoided_tokens_estimated;
  let hostCapped: LastSessionSummary["hostCapped"] = null;
  if (isCount(hostNet)) {
    const pct = isCount(session.host_visible_reduction_pct)
      ? session.host_visible_reduction_pct
      : isCount(session.host_visible_baseline_tokens_estimated)
        ? reductionPct(hostNet, session.host_visible_baseline_tokens_estimated)
        : null;
    hostCapped = { netAvoided: hostNet, reductionPct: pct };
  }
  const topRoutes = [...(session.top_routes ?? [])]
    .filter((route) => route && typeof route.route === "string" && isCount(route.operations))
    .sort((left, right) => right.operations - left.operations)
    .slice(0, 5);
  return {
    operations: isCount(session.operations) ? session.operations : 0,
    optimizedOperations: isCount(session.optimized_operations) ? session.optimized_operations : null,
    rawOperations: isCount(session.raw_operations) ? session.raw_operations : null,
    producer,
    hostCapped,
    startedAtMs: parseLedgerTimestamp(session.first_record_at),
    endedAtMs: parseLedgerTimestamp(session.last_record_at),
    topRoutes,
    pricedValue: session.raw_public_estimate ?? null,
  };
}
