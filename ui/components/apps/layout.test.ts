import { describe, expect, it } from "vitest";
import { layout } from "./layout";
import { insertStep, moveStepTo, removeStep, type Definition, type Step } from "./model";

// Every list in the tree, with how many steps it holds.
function lists(steps: Step[], at = "steps", out: [string, number][] = []): [string, number][] {
  out.push([at, steps.length]);
  steps.forEach((s, i) => {
    s.paths?.forEach((p, j) => lists(p.steps, `${at}.${i}.paths.${j}.steps`, out));
    if (s.otherwise) lists(s.otherwise, `${at}.${i}.otherwise`, out);
    if (s.type === "for_each") lists(s.steps || [], `${at}.${i}.steps`, out);
  });
  return out;
}

// A + for every place a step can go (each list, index 0..length), each once, each placed.
function expectEveryPlus(d: Definition) {
  const want = lists(d.steps).flatMap(([list, n]) => Array.from({ length: n + 1 }, (_, i) => `${list}@${i}`)).sort();
  const edges = layout(d).edges.filter((e) => e.insert);
  expect(edges.map((e) => `${e.insert!.list}@${e.insert!.index}`).sort()).toEqual(want);
  for (const e of edges) expect(e.plus && Number.isFinite(e.plus.x) && Number.isFinite(e.plus.y)).toBe(true);
  const spots = new Set(edges.map((e) => `${Math.round(e.plus!.x)},${Math.round(e.plus!.y)}`));
  expect(spots.size).toBe(edges.length);
}

const start: Definition = {
  inputs: [],
  steps: [
    { type: "code", id: "a", title: "", code: "" },
    { type: "if", id: "b", title: "", paths: [{ when: [], steps: [{ type: "code", id: "inner", title: "", code: "" }] }], otherwise: [] },
    { type: "decide", id: "c", title: "", model: "m", question: "", input: "", paths: [{ name: "x", description: "", steps: [] }], otherwise: [] },
    { type: "for_each", id: "d", title: "", items: "", steps: [] },
    { type: "response", id: "e", title: "", value: 1 },
  ],
};

describe("a + at every place a step can go", () => {
  it("at the start, with empty paths, Otherwise and For each", () => expectEveryPlus(start));
  it("after dragging a step out of a path (the path is left empty)", () => expectEveryPlus(moveStepTo(start, "steps.1.paths.0.steps.0", "steps", 0)));
  it("after dragging into For each, a Decide path and Otherwise", () => {
    let d = moveStepTo(start, "steps.0", "steps.3.steps", 0);
    expectEveryPlus(d);
    d = moveStepTo(d, "steps.3", "steps.1.paths.0.steps", 0);
    expectEveryPlus(d);
    d = moveStepTo(d, "steps.1.paths.0.steps.0", "steps.0.otherwise", 0);
    expectEveryPlus(d);
  });
  it("after deleting steps, down to none", () => {
    let d = start;
    expectEveryPlus((d = removeStep(d, "steps.1.paths.0.steps.0")[0]));
    while (d.steps.length) expectEveryPlus((d = removeStep(d, "steps.0")[0]));
    expect(layout(d).edges.filter((e) => e.insert).map((e) => `${e.insert!.list}@${e.insert!.index}`)).toEqual(["steps@0"]);
  });
  it("after adding into an empty list and nesting deep", () => {
    let d = insertStep(start, "steps.3.steps", 0, { type: "if", id: "deep", title: "", paths: [{ when: [], steps: [] }], otherwise: [] });
    d = insertStep(d, "steps.3.steps.0.otherwise", 0, { type: "code", id: "z", title: "", code: "" });
    expectEveryPlus(d);
  });
});
