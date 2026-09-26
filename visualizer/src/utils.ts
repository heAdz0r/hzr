import type { DashboardProject, DashboardState, ProjectState } from "./types";

const compactNumber = new Intl.NumberFormat("en-US", {
  notation: "compact",
  maximumFractionDigits: 1,
});
const exactNumber = new Intl.NumberFormat("en-US");

export const dashboardStateLabel: Record<DashboardState, string> = {
  ready: "Ready",
  degraded: "Degraded",
  rebuilding: "Rebuilding",
  standby: "Standby",
  stopped: "Stopped",
  unknown: "Unknown",
};

export const projectStateLabel: Record<ProjectState, string> = {
  ready: "Ready",
  warming: "Warming",
  registered: "Registered",
  unavailable: "Unavailable",
  degraded: "Degraded",
};

export function formatCount(value: number): string {
  return Math.abs(value) >= 10_000 ? compactNumber.format(value) : exactNumber.format(value);
}

export function formatSignedCount(value: number): string {
  const sign = value > 0 ? "+" : "";
  return `${sign}${formatCount(value)}`;
}

export function formatPercent(value: number): string {
  const sign = value > 0 ? "+" : "";
  return `${sign}${value.toFixed(1)}%`;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let value = bytes / 1024;
  let unit = units[0];
  for (let index = 1; index < units.length && value >= 1024; index += 1) {
    value /= 1024;
    unit = units[index];
  }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${unit}`;
}

export function formatCost(microusd: number): string {
  if (microusd === 0) return "Not reported";
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: 4,
  }).format(microusd / 1_000_000);
}

export function formatEconomicAmount(currency: string, microunits: number): string {
  const sign = microunits < 0 ? "-" : "";
  const absolute = Math.abs(Math.trunc(microunits));
  const units = Math.floor(absolute / 1_000_000);
  const fraction = String(absolute % 1_000_000).padStart(6, "0");
  return `${currency} ${sign}${units}.${fraction}`;
}

export function formatDuration(milliseconds: number): string {
  const seconds = Math.floor(milliseconds / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${minutes % 60}m`;
  const days = Math.floor(hours / 24);
  return `${days}d ${hours % 24}h`;
}

export function relativeTime(timestampMs: number, nowMs = Date.now()): string {
  const delta = Math.max(0, nowMs - timestampMs);
  const seconds = Math.floor(delta / 1000);
  if (seconds < 10) return "just now";
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

export function refreshFailureAnnouncement(hasSnapshot: boolean): string {
  return hasSnapshot
    ? "Dashboard refresh failed; showing the last successful snapshot"
    : "Dashboard is unavailable";
}

export function filterProjects(
  projects: DashboardProject[],
  query: string,
  state: ProjectState | "all",
): DashboardProject[] {
  const normalized = query.trim().toLocaleLowerCase();
  return projects.filter((project) => {
    const matchesState = state === "all" || project.state === state;
    const matchesQuery =
      normalized.length === 0 ||
      project.name.toLocaleLowerCase().includes(normalized) ||
      // Searching by the path people actually type is the point of showing it.
      (project.display_path ?? "").toLocaleLowerCase().includes(normalized) ||
      project.root.toLocaleLowerCase().includes(normalized) ||
      project.repository_id.toLocaleLowerCase().includes(normalized) ||
      project.worktree_id.toLocaleLowerCase().includes(normalized);
    return matchesState && matchesQuery;
  });
}

// 0.11.2: say what failed and what to run, instead of echoing a bare fetch error.
export interface LoadFailure {
  title: string;
  detail: string;
  command: string;
}

export function describeLoadFailure(message: string): LoadFailure {
  const status = /HTTP (\d{3})/.exec(message)?.[1];
  if (status === "404") {
    return {
      title: "This daemon does not serve the dashboard API.",
      detail: `The request returned HTTP 404. The daemon may be older than this UI; restart it after upgrading HZR.`,
      command: "hzr daemon service status",
    };
  }
  if (status && status.startsWith("5")) {
    return {
      title: "The HZR daemon answered with an error.",
      detail: `The dashboard request returned HTTP ${status}. The daemon may be restarting or not running behind this address.`,
      command: "hzr doctor --workspace .",
    };
  }
  if (status) {
    return {
      title: "The dashboard request was rejected.",
      detail: `The request returned HTTP ${status}.`,
      command: "hzr doctor --workspace .",
    };
  }
  if (/fetch|network|load failed/i.test(message)) {
    return {
      title: "The HZR daemon is not reachable.",
      detail: "No response from the local daemon. It may be stopped, or this page was opened from a different address.",
      command: "hzr daemon service status",
    };
  }
  if (/json|unexpected token/i.test(message)) {
    return {
      title: "The daemon sent a response this UI cannot read.",
      detail: "The snapshot was not valid JSON. The daemon and UI versions may not match.",
      command: "hzr doctor --workspace .",
    };
  }
  return {
    title: "HZR did not return a dashboard snapshot.",
    detail: message || "The request failed without a reason.",
    command: "hzr doctor --workspace .",
  };
}

// 0.11.2: money for headline cards — two decimals, currency symbol when the code is known.
export function formatMoney(currency: string, microunits: number): string {
  const value = microunits / 1_000_000;
  try {
    return new Intl.NumberFormat("en-US", {
      style: "currency",
      currency,
      minimumFractionDigits: 2,
      maximumFractionDigits: Math.abs(value) > 0 && Math.abs(value) < 0.01 ? 4 : 2,
    }).format(value);
  } catch {
    return formatEconomicAmount(currency, microunits);
  }
}
