import { describe, expect, it } from "vitest";
import { meterTone, sandboxMeters, sandboxNote, sandboxShort, sandboxView, type SandboxUsage } from "./sandbox";

const GB = 2 ** 30;
const limits = { cpuMillis: 2000, memoryBytes: 2 * GB, diskBytes: 10 * GB };
const now = Date.parse("2026-11-21T12:00:00Z");
const running: SandboxUsage = { status: "running", cpuMillis: 250, memoryBytes: 412 * 2 ** 20, diskBytes: 1.5 * GB, limits, idleMinutes: 60, stopsAt: "2026-11-21T12:42:00Z" };

describe("sandboxMeters", () => {
  it("holds each number against what the container may use", () => {
    expect(sandboxMeters(running)).toEqual([
      { key: "cpu", label: "CPU", text: "0.25 of 2 cores", short: "0.25 cpu", share: 0.125 },
      { key: "memory", label: "Memory", text: "412.0 MB of 2.0 GB", short: "412.0 MB", share: 412 / 2048 },
      { key: "disk", label: "Disk", text: "1.5 GB of 10.0 GB", short: "1.5 GB", share: 0.15 },
    ]);
  });
  it("leaves out what the cluster did not say, and never reads past full", () => {
    expect(sandboxMeters({ status: "stopped", diskBytes: GB, limits }).map((m) => m.key)).toEqual(["disk"]);
    expect(sandboxMeters({ status: "running", cpuMillis: 1500, limits: { cpuMillis: 1000 } })[0]).toMatchObject({ text: "1.5 of 1 core", share: 1 });
    expect(sandboxMeters({ status: "running", cpuMillis: 0, memoryBytes: 5 })[0]).toMatchObject({ text: "0 cores", share: null });
  });
});

describe("sandboxNote", () => {
  it("says when it sleeps, from the time it was last used", () => {
    expect(sandboxNote(running, now)).toBe("Sleeps in 42 minutes if left alone. Files are kept.");
    expect(sandboxNote({ ...running, stopsAt: "2026-11-21T12:00:20Z" }, now)).toBe("Sleeps within a minute if left alone. Files are kept.");
    expect(sandboxNote({ ...running, stopsAt: null }, now)).toBe("Sleeps after 1 hour unused. Files are kept.");
  });
  it("says what waking takes when it is not running", () => {
    expect(sandboxNote({ status: "stopped" })).toContain("wakes when you open a project");
    expect(sandboxNote({ status: "none", idleMinutes: 90 })).toBe("Starts with your first workspace, and sleeps after 1 hour 30 minutes unused.");
  });
});

it("is alive while it runs or wakes, and short for the status line", () => {
  expect(sandboxView(running, now)).toMatchObject({ label: "Awake", alive: true });
  expect(sandboxView({ status: "stopped" })).toMatchObject({ label: "Asleep", alive: false, meters: [] });
  expect(sandboxShort(running)).toBe("0.25 cpu · 412.0 MB");
  expect(sandboxShort({ status: "stopped", diskBytes: 1 })).toBe("");
  expect(sandboxShort(undefined)).toBe("");
});

it("turns a meter amber when close and red when full", () => {
  expect([null, 0.5, 0.8, 0.95].map(meterTone)).toEqual(["calm", "calm", "close", "full"]);
});
