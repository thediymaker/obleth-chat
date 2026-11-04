import { describe, expect, it } from "vitest";
import { chartOf, checkText, diffLineKind, facts, fmt, minutes, placementText, type ProjectResearchData } from "./model";

const data = (direction = "min"): ProjectResearchData => ({
  research: { goal: "Faster decode", metric: "ms", direction, runCommand: "python bench.py" },
  experiments: [
    { id: "base", parentId: null, title: "Baseline", frozen: true },
    { id: "a", parentId: "base", title: "Cache", frozen: true },
    { id: "b", parentId: "base", title: "Batch" },
    { id: "a1", parentId: "a", title: "Bigger cache" },
  ],
  runs: [
    { id: "r1", experimentId: "base", number: 1, status: "completed", metric: 100, startedAt: "2026-11-03T10:00:00Z" },
    { id: "r2", experimentId: "a", number: 2, status: "completed", metric: 80, startedAt: "2026-11-03T10:05:00Z" },
    { id: "r3", experimentId: "b", number: 3, status: "failed", startedAt: "2026-11-03T10:06:00Z" },
    { id: "r4", experimentId: "a1", number: 4, status: "running", startedAt: "2026-11-03T10:07:00Z" },
  ],
});

describe("research facts", () => {
  it("finds the best run in the direction the goal wants", () => {
    expect(facts(data()).bestRun()?.id).toBe("r2");
    expect(facts(data("max")).bestRun()?.id).toBe("r1");
    expect(facts(data()).gain().gain).toBeCloseTo(-20);
  });
  it("opens on the best node, and picks a node's running, best or last run", () => {
    const f = facts(data());
    expect(f.defaultNode()).toBe("a");
    expect(f.pickRun("a1", null)?.id).toBe("r4");
    expect(f.pickRun("b", null)?.id).toBe("r3");
    expect(f.pickRun("a", "r2")?.id).toBe("r2");
    expect(f.pickRun(null, null)).toBeNull();
  });
  it("labels each node in the tree, depth first", () => {
    const f = facts(data());
    expect(f.tree().map(({ e, depth }) => [e.id, depth])).toEqual([
      ["base", 0],
      ["a", 1],
      ["a1", 2],
      ["b", 1],
    ]);
    expect(f.tree().map(({ e }) => f.chip(e).text)).toEqual(["baseline", "★ promoted", "● running", "failed · open"]);
  });
  it("draws a mark per run and the best so far", () => {
    const c = chartOf(data())!;
    expect(c.marks.map((m) => [m.run.number, m.measured, m.kept])).toEqual([
      [1, true, true],
      [2, true, true],
      [3, false, false],
      [4, false, false],
    ]);
    expect(c.line.split(" ")).toHaveLength(4);
    expect(chartOf({ ...data(), runs: [] })).toBeNull();
  });
});

describe("research words", () => {
  it("formats metrics and times", () => {
    expect(fmt(null)).toBe("—");
    expect(fmt(1234.567)).toBe((1234.6).toLocaleString());
    expect(fmt(0.123456)).toBe("0.1235");
    expect(minutes("2026-11-03T10:00:00Z", "2026-11-03T10:00:20Z")).toBe("20 s");
    expect(minutes("2026-11-03T10:00:00Z", "2026-11-03T10:05:00Z")).toBe("5 min");
  });
  it("says what the checks found", () => {
    expect(checkText({ skipped: "no model" })).toEqual({ text: "Skipped: no model", doubt: false });
    expect(checkText({ tested: 0.8, model: "m" })).toEqual({ text: "Tested its idea · 80% that it did · m", doubt: false });
    expect(checkText({ tested: 0.2 })).toEqual({ text: "20% that it did", doubt: true });
    expect(placementText({ fits: 0.9 }).text).toBe("Fits where it is · 90% that it fits");
    expect(placementText({ fits: 0.3 }).text).toBe("Unsure it fits here · 30% that it fits");
    expect(placementText({ fits: 0.3, confirmed: true }).doubt).toBe(true);
  });
  it("colors a run's change", () => {
    expect(["+a", "+++ b", "-c", "--- d", "@@ -1 +1 @@", " e"].map(diffLineKind)).toEqual(["add", "", "del", "", "hunk", ""]);
  });
});
