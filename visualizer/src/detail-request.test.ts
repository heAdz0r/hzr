import { describe, expect, test } from "bun:test";
import {
  DetailRequestCoordinator,
  LatestProjectRequestCoordinator,
  isCurrentProjectSnapshot,
} from "./detail-request";

describe("DetailRequestCoordinator", () => {
  test("rejects an A response after the selected project switches to B", async () => {
    const requests = new DetailRequestCoordinator();
    requests.switchProject("project-a");
    const alpha = requests.begin("project-a");

    requests.switchProject("project-b");
    await Promise.resolve();

    expect(alpha.signal.aborted).toBe(true);
    expect(requests.isCurrent(alpha, "project-b")).toBe(false);
  });

  test("cached topic A invalidates an older in-flight topic B in the same project", async () => {
    const requests = new DetailRequestCoordinator();
    requests.switchProject("project-a");
    const networkTopicB = requests.begin("project-a");

    const cachedTopicA = requests.begin("project-a");
    await Promise.resolve();

    expect(networkTopicB.signal.aborted).toBe(true);
    expect(requests.isCurrent(networkTopicB, "project-a")).toBe(false);
    expect(requests.isCurrent(cachedTopicA, "project-a")).toBe(true);
  });

  test("late project B refresh cannot clear or replace selected project C", async () => {
    const requests = new LatestProjectRequestCoordinator();
    requests.switchProject("project-b");
    const projectB = requests.begin("project-b");
    const lateResponse = Promise.resolve(404);

    requests.switchProject("project-c");
    await lateResponse;

    expect(projectB.signal.aborted).toBe(true);
    expect(requests.isCurrent(projectB, "project-c")).toBe(false);
  });

  test("late A observability delta cannot merge into the replacement B snapshot", async () => {
    const requests = new LatestProjectRequestCoordinator();
    const snapshotA = { project: "project-a" };
    const snapshotB = { project: "project-b" };
    requests.switchProject("project-a");
    const deltaA = requests.begin("project-a");
    const lateDelta = Promise.resolve({ trace: "alpha" });

    requests.switchProject("project-b");
    await lateDelta;

    expect(
      isCurrentProjectSnapshot(requests, deltaA, "project-b", snapshotA, snapshotB),
    ).toBe(false);
  });
});
