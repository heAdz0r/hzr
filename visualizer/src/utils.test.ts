import { describe, expect, test } from "bun:test";
import type { DashboardProject } from "./types";
import {
  dashboardStateLabel,
  filterProjects,
  formatBytes,
  formatCost,
  formatDuration,
  formatSignedCount,
  refreshFailureAnnouncement,
  relativeTime,
  layoutMemoryTopics,
} from "./utils";

const project = (name: string, root: string, state: DashboardProject["state"]): DashboardProject => ({
  name,
  root,
  repository_id: "a".repeat(64),
  worktree_id: "b".repeat(64),
  git_backed: true,
  linked_worktree: false,
  state,
  registered_at_ms: 1,
  last_seen_at_ms: 2,
  artifacts: {
    config_present: true,
    vectors_present: true,
    symbols_present: true,
    repository_graph_present: false,
    size_bytes: 0,
    modified_at_ms: null,
  },
  command: "hzr index status",
});

describe("dashboard formatters", () => {
  test("keeps missing provider cost distinct from a derived estimate", () => {
    expect(formatCost(0)).toBe("Not reported");
    expect(formatCost(1_250_000)).toBe("$1.25");
  });

  test("formats exact units and signs", () => {
    expect(formatBytes(1024)).toBe("1.0 KiB");
    expect(formatSignedCount(-420)).toBe("-420");
    expect(formatDuration(3_660_000)).toBe("1h 1m");
  });

  test("formats relative time without future negative values", () => {
    expect(relativeTime(9_000, 10_000)).toBe("just now");
    expect(relativeTime(0, 120_000)).toBe("2m ago");
    expect(relativeTime(20_000, 10_000)).toBe("just now");
  });

  test("defines a text label for every service state", () => {
    expect(Object.keys(dashboardStateLabel).sort()).toEqual([
      "degraded",
      "ready",
      "rebuilding",
      "standby",
      "stopped",
      "unknown",
    ]);
  });

  test("retains a successful snapshot when a later refresh fails", () => {
    expect(refreshFailureAnnouncement(true)).toBe(
      "Dashboard refresh failed; showing the last successful snapshot",
    );
    expect(refreshFailureAnnouncement(false)).toBe("Dashboard is unavailable");
  });
});

describe("project filtering", () => {
  const projects = [
    project("hzr", "/work/hzr", "ready"),
    project("caveman", "/work/caveman", "warming"),
  ];

  test("filters by visible name or exact visible path text", () => {
    expect(filterProjects(projects, "CAVE", "all")).toHaveLength(1);
    expect(filterProjects(projects, "/work/hzr", "all")[0]?.name).toBe("hzr");
  });

  test("combines query and state filters", () => {
    expect(filterProjects(projects, "work", "ready").map((item) => item.name)).toEqual(["hzr"]);
    expect(filterProjects(projects, "hzr", "warming")).toHaveLength(0);
  });
});

describe("memory graph layout", () => {
  test("is deterministic and keeps every topic inside the view box", () => {
    const topics = [
      { id: "one", label: "decisions", memory_count: 3, average_weight: 0.8, newest_at: null },
      { id: "two", label: "architecture", memory_count: 2, average_weight: 0.6, newest_at: null },
      { id: "three", label: "errors", memory_count: 1, average_weight: 0.4, newest_at: null },
    ];

    const first = layoutMemoryTopics(topics, 800, 460);
    const second = layoutMemoryTopics(topics, 800, 460);

    expect(first).toEqual(second);
    expect(first).toHaveLength(3);
    expect(first.every((node) => node.x >= 60 && node.x <= 740)).toBe(true);
    expect(first.every((node) => node.y >= 60 && node.y <= 400)).toBe(true);
  });
});
