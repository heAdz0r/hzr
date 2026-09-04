import type { DashboardLocalActivity, DashboardLocalOperation } from "./types";

export function outputReduction(activity: Pick<DashboardLocalActivity, "baseline_tokens_estimated" | "net_avoided_tokens_estimated">): number | null {
  if (activity.baseline_tokens_estimated <= 0) return null;
  return (activity.net_avoided_tokens_estimated * 100) / activity.baseline_tokens_estimated;
}

export type ActivityRoute = "all" | "regressions" | DashboardLocalOperation["route"];

export function filterActivity(
  operations: DashboardLocalOperation[],
  route: ActivityRoute,
  agent: string,
  session: string,
): DashboardLocalOperation[] {
  return operations.filter((operation) =>
    (route === "all" || (route === "regressions"
      ? operation.net_avoided_tokens_estimated < 0
      : operation.route === route)) &&
    (agent === "" || (operation.agent ?? "Unattributed") === agent) &&
    (session === "" || (operation.session_hash ?? "Unattributed") === session),
  );
}

export function tokenBarWidth(value: number, maximum: number): string {
  if (maximum <= 0 || value <= 0) return "0%";
  return `${Math.min(100, (value * 100) / maximum)}%`;
}
