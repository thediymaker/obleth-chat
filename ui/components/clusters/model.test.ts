import { describe, expect, it } from "vitest";
import { blocked, gib, number, quotaMeters, quotaName, tone } from "./model";

describe("cluster states", () => {
  it("colors job, pod and node states", () => {
    expect(["RUNNING", "Running", "COMPLETED", "IDLE", "MIXED", "true"].map(tone)).toEqual(Array(6).fill("ok"));
    expect(["PENDING", "ContainerCreating", "COMPLETING"].map(tone)).toEqual(Array(3).fill("amber"));
    expect(["FAILED", "CrashLoopBackOff", "DOWN", "DRAIN", "TIMEOUT", "OUT_OF_MEMORY", "false"].map(tone)).toEqual(Array(7).fill("bad"));
    expect(tone("Unknown")).toBe("");
    expect(tone(undefined)).toBe("");
  });
  it("shows memory and counts", () => {
    expect(gib(512)).toBe("512 MiB");
    expect(gib(2048)).toBe("2.0 GiB");
    expect(gib(20480)).toBe("20 GiB");
    expect(number(null)).toBe("–");
  });
  it("orders quotas and tells a section it may not see", () => {
    const quota = [
      { resource: "pods", ratio: 0.1 },
      { resource: "custom", ratio: 0.5 },
      { resource: "requests.cpu", ratio: 0.2 },
      { resource: "limits.memory", ratio: null },
      { resource: "requests.memory", ratio: 0.3 },
      { resource: "requests.storage", ratio: 0.3 },
    ];
    expect(quotaMeters(quota).map((q) => q.resource)).toEqual(["requests.cpu", "requests.memory", "pods", "requests.storage"]);
    expect(quotaName("requests.cpu")).toBe("CPU requested");
    expect(quotaName("custom")).toBe("custom");
    expect(blocked({ error: "Forbidden" })).toBe(true);
    expect(blocked([])).toBe(false);
  });
});
