import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { WorkspacesView } from "./workspaces-tab";
import { bytes, cores, hours, pointTip, since, until, type WorkspacesOverview } from "./workspaces";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));

const now = Date.now();
const data: WorkspacesOverview = {
  enabled: true,
  days: 7,
  start: new Date(now - 6 * 864e5).toISOString(),
  slots: 32,
  everyone: true,
  browsers: true,
  metrics: true,
  idleMinutes: { workspace: 60, browser: 15 },
  limits: { cpu: "2", memory: "4Gi", disk: "10Gi", browserCpu: "1", browserMemory: "2Gi" },
  now: { inUse: 27, workspaces: { running: 20, starting: 1, error: 1 }, browsers: { running: 6, starting: 0, error: 0 }, previews: 3, cpuMillis: 2500, memoryBytes: 3 * 2 ** 30 },
  open: [
    {
      kind: "workspace",
      ownerId: "u1",
      username: "ana",
      displayName: "Ana <b>",
      status: "running",
      startedAt: new Date(now - 30 * 60000).toISOString(),
      lastUsedAt: new Date(now - 5 * 60000).toISOString(),
      stopsAt: new Date(now + 55 * 60000).toISOString(),
      chats: 2,
      previews: 1,
      cpuMillis: 250,
      memoryBytes: 64 * 2 ** 20,
      diskBytes: 2 ** 30,
    },
    { kind: "browser", ownerId: "u2", username: "bo", displayName: "Bo", status: "error", errorMessage: "All browser and workspace slots are in use right now.", lastUsedAt: new Date(now).toISOString(), chats: 1, previews: 0, cpuMillis: null, memoryBytes: null, diskBytes: null },
  ],
  history: {
    series: [
      { at: "2026-11-01T00:00:00Z", workspaceHours: 3, browserHours: 1, peak: 30, starts: 9, failed: 1, refused: 2 },
      { at: "2026-11-02T00:00:00Z", workspaceHours: 0.5, browserHours: 0, peak: 2, starts: 1, failed: 0, refused: 0 },
    ],
    totals: { at: "", workspaceHours: 3.5, browserHours: 1, peak: 30, peakAt: "2026-11-01T14:00:00Z", starts: 10, failed: 1, refused: 2, people: 7, medianStartSeconds: 14 },
  },
  recordedSince: new Date(now - 864e5).toISOString(),
};
const view = (d: WorkspacesOverview = data, onStop = vi.fn()) => render(<WorkspacesView data={d} days={7} metric="hours" setMetric={() => {}} onStop={onStop} />);

test("workspaces show pods now, who has them open and the period", async () => {
  const onStop = vi.fn();
  const { container } = view(data, onStop);
  const t = container.textContent!;
  expect(t).toContain("27 of 32");
  expect(container.querySelector('[data-level="near"]')).not.toBeNull();
  expect(t).toContain("1 starting · 1 failed");
  expect(screen.getByText("Ana <b>")).toBeInTheDocument();
  expect(t).toContain("2 chats · 1 preview");
  expect(t).toContain("in 55 min");
  expect(t).toContain("250m");
  expect(t).toContain("64 MiB");
  expect(t).toContain("All browser and workspace slots are in use");
  expect(t).toMatch(/Turned away2/);
  expect(t).toContain("Ready in 14 s, typically");
  expect(t).toContain("Chat started keeping this history");
  expect(t).toContain("Workspaces stop after 60 minutes without use, browsers after 15");
  expect(t).not.toContain("need permission to read pod metrics");
  await userEvent.click(screen.getByRole("button", { name: "Stop Bo’s browser" }));
  expect(onStop).toHaveBeenCalledWith(expect.objectContaining({ kind: "browser", ownerId: "u2" }));
});

test("workspaces explain when they are off or metrics cannot be read", () => {
  expect(view({ enabled: false, off: "No Kubernetes access" }).container.textContent).toContain("No Kubernetes access");
  const blind = view({ ...data, metrics: false }).container.textContent;
  expect(blind).toContain("Chat can’t read pod metrics");
  expect(blind).toContain("metrics.k8s.io, pods, get and list");
  expect(view({ ...data, open: [] }).container.textContent).toContain("No workspace or browser is open right now");
});

test("pod numbers read plainly, and each bar says what happened", () => {
  expect(pointTip(data.history!.series![0]!, 7)).toMatch(/3\.0 h of workspaces, 1\.0 h of browsers, at most 30 pods at once, 9 starts, 1 failed, 2 turned away/);
  expect(cores(2500)).toBe("2.5 cores");
  expect(bytes(1536 * 2 ** 20)).toBe("1.5 GiB");
  expect(hours(0.25)).toBe("15 min");
  expect(until(new Date(now + 3 * 3600e3).toISOString(), now)).toBe("in 3 hours");
  expect(since(new Date(now - 90 * 60000).toISOString(), now)).toBe("2 h ago");
});
