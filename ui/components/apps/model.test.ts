import { beforeEach, describe, expect, it, vi } from "vitest";
import { blank, branches, freshId, ids, insertStep, isInside, listAt, moveStep, moveStepTo, pathOf, removeStep, replaceStep, staysPut, stepAt, summary, traceName, type Definition } from "./model";
import { curlExample, fromDraft, fromInputDrafts, newIfPath, readTryInputs, toDraft } from "./step-draft";
import { layout, tookPath, traceByStep } from "./layout";

vi.mock("@/lib/api", () => ({ api: { request: vi.fn() } }));
import { api } from "@/lib/api";
import { createAppFromSuggestion } from "./api";

const definition: Definition = {
  inputs: [{ name: "question", type: "text", required: true }],
  steps: [
    {
      type: "decide",
      id: "route",
      title: "",
      model: "glm",
      question: "What is it about?",
      input: "{{inputs.question}}",
      paths: [{ name: "Samples", description: "a sample", steps: [{ type: "search", id: "search", title: "", query: "{{inputs.question}}", limit: 8 }] }],
      otherwise: [{ type: "response", id: "nothing", title: "", value: "Ask about <samples>" }],
    },
    { type: "if", id: "check", title: "", paths: [{ when: [{ value: "{{steps.search}}", test: "is_empty" }, { code: "inputs.question.length > 3" }], steps: [] }], otherwise: [] },
    { type: "agent", id: "agent", title: "Answer", model: "glm", instructions: "", prompt: "{{inputs.question}}", json: false },
  ],
};

describe("the step tree", () => {
  it("finds steps by their path, and new ids never repeat", () => {
    expect(listAt(definition, "steps.0.paths.0.steps")[0].id).toBe("search");
    expect(listAt(definition, "steps.0.otherwise")[0].id).toBe("nothing");
    expect(freshId(definition, "search")).toBe("search_2");
    expect(freshId(definition, "code")).toBe("code");
    expect(blank("code", "code")).toEqual({ type: "code", id: "code", title: "", code: "return {};" });
    expect(branches(definition.steps[0]!, "steps.0").map(([label]) => label)).toEqual(["When: Samples", "Otherwise"]);
    expect(branches(definition.steps[1]!, "steps.1")[0]![0]).toMatch(/If \{\{steps.search\}\} is empty and code: inputs.question.length > 3/);
    expect(summary(definition.steps[2]!)).toMatch(/glm · \{\{inputs.question\}\}/);
    expect(ids(definition.steps)).toEqual(["route", "search", "nothing", "check", "agent"]);
  });

  it("each type has a one-line summary", () => {
    expect(summary(blank("search", "s"))).toBe("The project’s knowledge · {{inputs.question}}");
    expect(summary({ ...blank("search", "s"), collections: ["a", "b"] })).toBe("2 collections · {{inputs.question}}");
    expect(summary({ ...blank("code", "c"), code: "\n  const a = 1;\nreturn a;" })).toBe("  const a = 1;");
    expect(summary(blank("decide", "d"))).toBe("No model picks a path for {{inputs.question}}");
    expect(summary(blank("for_each", "f"))).toBe("Each of {{inputs.items}}, 4 at a time");
    expect(summary(blank("http", "h"))).toBe("GET https://");
    expect(summary(blank("response", "r"))).toBe('{"answer":"{{steps.agent}}"}');
    expect(summary({ ...blank("agent", "a", "glm"), json: true })).toBe("glm · JSON · {{inputs.question}}");
  });

  it("blank steps do not share their defaults", () => {
    const a = blank("decide", "a");
    a.paths!.push({ name: "x", steps: [] });
    expect(blank("decide", "b").paths).toHaveLength(1);
  });

  it("edits copies: insert, replace, remove and move", () => {
    const added = insertStep(definition, "steps.1.otherwise", 0, blank("code", "shape"));
    expect(listAt(added, "steps.1.otherwise").map((s: { id: string }) => s.id)).toEqual(["shape"]);
    expect(definition.steps[1]!.otherwise).toEqual([]);
    const replaced = replaceStep(added, "steps.1.otherwise.0", { ...blank("code", "shape"), title: "Shape" });
    expect(stepAt(replaced, "steps.1.otherwise.0")!.title).toBe("Shape");
    const [removed, gone] = removeStep(replaced, "steps.0.paths.0.steps.0");
    expect(gone.id).toBe("search");
    expect(listAt(removed, "steps.0.paths.0.steps")).toEqual([]);
    const moved = moveStep(definition, "steps.2", -1);
    expect(moved.steps.map((s) => s.id)).toEqual(["route", "agent", "check"]);
    expect(moveStep(definition, "steps.0", -1).steps.map((s) => s.id)).toEqual(["route", "check", "agent"]);
  });

  it("names the path a try took the way the trace does", () => {
    expect(traceName(definition.steps[0]!, "steps.0.paths.0.steps")).toBe("Samples");
    expect(traceName(definition.steps[0]!, "steps.0.otherwise")).toBe("Otherwise");
    expect(traceName(definition.steps[1]!, "steps.1.paths.0.steps")).toBe("Path 1");
  });
});

describe("the step editor", () => {
  it("a code step reads back from its fields", () => {
    const old = blank("code", "code");
    const d = { ...toDraft(old), id: " shape ", title: "Shape rows", code: "return steps.search.length;" };
    expect(fromDraft(d, old)).toEqual({ type: "code", id: "shape", title: "Shape rows", code: "return steps.search.length;" });
  });

  it("decide paths keep their steps while their names stay", () => {
    const old = definition.steps[0]!;
    const d = toDraft(old);
    expect(d.paths).toBe("Samples: a sample");
    const s = fromDraft({ ...d, paths: "Samples: changed\nProtocols" }, old);
    expect(s.paths).toEqual([
      { name: "Samples", description: "changed", steps: old.paths![0]!.steps },
      { name: "Protocols", description: "", steps: [] },
    ]);
  });

  it("if rules: empty values drop, only comparing tests keep 'to', code comes last, paths add and go", () => {
    const old = definition.steps[1]!;
    const d = toDraft(old);
    expect(d.ifPaths[0]).toEqual({ from: 0, rules: [{ value: "{{steps.search}}", test: "is_empty", to: "" }], code: "inputs.question.length > 3" });
    const s = fromDraft(
      {
        ...d,
        ifPaths: [
          { from: 0, rules: [{ value: "{{steps.search}}", test: "equals", to: "x" }, { value: " ", test: "is_empty", to: "" }, { value: "{{inputs.question}}", test: "not_empty", to: "ignored" }], code: "" },
          newIfPath(),
        ],
      },
      old,
    );
    expect(s.paths).toEqual([
      { when: [{ value: "{{steps.search}}", test: "equals", to: "x" }, { value: "{{inputs.question}}", test: "not_empty" }], steps: [] },
      { when: [{ value: "{{inputs.question}}", test: "is_empty" }], steps: [] },
    ]);
  });

  it("http headers and body, and response values, read back", () => {
    const old = blank("http", "h");
    const s = fromDraft({ ...toDraft(old), method: "POST", url: " https://x.example/a ", headers: "Accept: application/json\nbad line\nX-Time: 10:30", body: '{"a":1}' }, old);
    expect(s).toMatchObject({ method: "POST", url: "https://x.example/a", headers: { Accept: "application/json", "X-Time": "10:30" }, body: { a: 1 } });
    expect(fromDraft({ ...toDraft(old), body: "plain" }, old).body).toBe("plain");
    expect(fromDraft({ ...toDraft(old), body: " " }, old).body).toBeNull();
    const r = blank("response", "r");
    expect(toDraft(r).value).toBe('{\n  "answer": "{{steps.agent}}"\n}');
    expect(fromDraft({ ...toDraft(r), value: "{{steps.agent}}" }, r).value).toBe("{{steps.agent}}");
  });

  it("numbers fall back to the defaults", () => {
    const f = blank("for_each", "f");
    expect(fromDraft({ ...toDraft(f), concurrency: "" }, f).concurrency).toBe(4);
    const s = blank("search", "s");
    expect(fromDraft({ ...toDraft(s), limit: "0" }, s).limit).toBe(8);
  });

  it("inputs without a name are removed", () => {
    expect(fromInputDrafts([{ name: " q ", type: "text", required: true }, { name: "", type: "json", required: false }])).toEqual([{ name: "q", type: "text", required: true, description: "" }]);
  });

  it("a try reads each input by its type", () => {
    const inputs = [
      { name: "q", type: "text" as const },
      { name: "n", type: "number" as const },
      { name: "b", type: "boolean" as const },
      { name: "j", type: "json" as const },
      { name: "empty", type: "text" as const },
    ];
    expect(readTryInputs(inputs, { q: "hi", n: "2", b: false, j: '["a"]', empty: " " })).toEqual({ q: "hi", n: 2, b: false, j: ["a"] });
    expect(() => readTryInputs(inputs, { j: "[" })).toThrow("j isn’t valid JSON");
  });

  it("the API card's example sends one value per input", () => {
    const curl = curlExample("https://chat.example.edu", "/api/apps/k7f3q2xm/run", definition.inputs);
    expect(curl).toMatch(/^curl https:\/\/chat.example.edu\/api\/apps\/k7f3q2xm\/run/);
    expect(curl).toContain(`-d '{"question":"…"}'`);
  });
});

describe("the flow", () => {
  it("lays out inputs, steps, paths side by side and the end, with a + on every connector", () => {
    const flow = layout(definition);
    const byId = new Map(flow.nodes.map((n) => [n.id, n]));
    expect(flow.nodes[0]!.kind).toBe("inputs");
    expect(byId.get("end")!.y).toBeGreaterThan(byId.get("step:agent")!.y);
    // Decide's two paths sit side by side, under the step, and rejoin.
    const samples = byId.get("label:steps.0.paths.0.steps")!;
    const otherwise = byId.get("label:steps.0.otherwise")!;
    expect(samples.y).toBe(otherwise.y);
    expect(samples.x).toBeLessThan(otherwise.x);
    expect(samples.label).toBe("When: Samples");
    expect(byId.get("join:route")!.y).toBeGreaterThan(byId.get("step:search")!.y);
    const inserts = flow.edges.filter((e) => e.insert).map((e) => `${e.insert!.list}@${e.insert!.index}`);
    expect(inserts).toEqual(
      expect.arrayContaining(["steps@0", "steps@1", "steps@2", "steps@3", "steps.0.paths.0.steps@0", "steps.0.paths.0.steps@1", "steps.0.otherwise@1", "steps.1.paths.0.steps@0", "steps.1.otherwise@0"]),
    );
    // Steps never overlap.
    const cards = flow.nodes.filter((n) => n.kind === "step");
    for (const a of cards)
      for (const b of cards) if (a !== b) expect(a.x + a.width <= b.x || b.x + b.width <= a.x || a.y + a.height <= b.y || b.y + b.height <= a.y).toBe(true);
  });

  it("wraps For each's steps in a box", () => {
    const d: Definition = { inputs: [], steps: [{ ...blank("for_each", "each"), steps: [blank("code", "code")] }, blank("response", "response")] };
    const flow = layout(d);
    const box = flow.nodes.find((n) => n.id === "loop:each")!;
    const inner = flow.nodes.find((n) => n.id === "step:code")!;
    expect(inner.x).toBeGreaterThan(box.x);
    expect(inner.y + inner.height).toBeLessThan(box.y + box.height);
    expect(flow.edges.find((e) => e.source === "loopEnd:each")!.insert).toEqual({ list: "steps", index: 1 });
    expect(flow.edges.find((e) => e.source === "step:each")!.insert).toEqual({ list: "steps.0.steps", index: 0 });
  });

  it("an empty app is its inputs and the end", () => {
    const flow = layout({ inputs: [], steps: [] });
    expect(flow.nodes.map((n) => n.kind)).toEqual(["inputs", "end"]);
    expect(flow.edges).toEqual([expect.objectContaining({ source: "inputs", target: "end", insert: { list: "steps", index: 0 } })]);
  });

  it("a try's trace sums each step's runs and marks the path taken", () => {
    const runs = traceByStep([
      { id: "route", ms: 5, ok: true, path: "Samples" },
      { id: "search", ms: 3, ok: true },
      { id: "search", ms: 4, ok: false, error: "no gateway" },
    ]);
    expect(runs.get("search")).toMatchObject({ ok: false, ms: 7, runs: 2, failures: 1, error: "no gateway" });
    const flow = layout(definition);
    expect(tookPath(flow.nodes.find((n) => n.id === "label:steps.0.paths.0.steps")!, runs)).toBe(true);
    expect(tookPath(flow.nodes.find((n) => n.id === "label:steps.0.otherwise")!, runs)).toBe(false);
  });
});

describe("an app from a suggestion", () => {
  const request = api.request as unknown as ReturnType<typeof vi.fn>;
  let calls: [string, string, unknown][];
  let collections: { id: string }[];
  beforeEach(() => {
    calls = [];
    collections = [];
    request.mockReset();
    request.mockImplementation(async (path: string, options: { method?: string; body?: { name?: string } } = {}) => {
      calls.push([options.method || "GET", path, options.body]);
      if (path === "/api/projects/p1/app") return { app: { id: "k7f3q2xm" } };
      if (path === "/api/projects/p1") return { collections };
      if (path === "/api/knowledge/collections") return { collection: { id: "c1", name: options.body!.name } };
      return {};
    });
  });

  it("starts with its steps, and a collection only when the project has none", async () => {
    const entry = { name: "Assay Lab", description: "Answers", documents: true, definition };
    expect(await createAppFromSuggestion("p1", entry, true)).toBe("Assay Lab documents");
    expect(calls[0]).toEqual(["POST", "/api/projects/p1/app", { description: "Answers", definition }]);
    expect(calls.at(-1)).toEqual(["PUT", "/api/projects/p1/collections", { collectionIds: ["c1"] }]);
    calls.length = 0;
    collections.push({ id: "old" });
    expect(await createAppFromSuggestion("p1", entry, true)).toBe("");
    expect(calls).toHaveLength(2);
    calls.length = 0;
    expect(await createAppFromSuggestion("p1", { ...entry, documents: false }, true)).toBe("");
    expect(calls).toHaveLength(1);
    calls.length = 0;
    expect(await createAppFromSuggestion("p1", entry, false)).toBe("");
    expect(calls).toHaveLength(1);
  });
});

describe("moving a step to a +", () => {
  const d: Definition = {
    inputs: [],
    steps: [
      { type: "code", id: "a", title: "", code: "" },
      { type: "if", id: "b", title: "", paths: [{ when: [], steps: [{ type: "code", id: "inner", title: "", code: "" }] }], otherwise: [] },
      { type: "for_each", id: "c", title: "", items: "", steps: [] },
      { type: "response", id: "d", title: "", value: 1 },
    ],
  };
  const tree = (x: Definition) => JSON.stringify(x.steps.map((s) => [s.id, ...(s.paths || []).map((p) => p.steps.map((t) => t.id)), (s.otherwise || []).map((t) => t.id), (s.steps || []).map((t) => t.id)]));
  it("out of a path, keeping the path empty", () => {
    const m = moveStepTo(d, "steps.1.paths.0.steps.0", "steps", 3);
    expect(m.steps.map((s) => s.id)).toEqual(["a", "b", "c", "inner", "d"]);
    expect(m.steps[1]!.paths![0]!.steps).toEqual([]);
  });
  it("into For each and into Otherwise, even from earlier in the same list", () => {
    expect(moveStepTo(d, "steps.0", "steps.2.steps", 0).steps.map((s) => s.id)).toEqual(["b", "c", "d"]);
    expect(moveStepTo(d, "steps.0", "steps.2.steps", 0).steps[1]!.steps!.map((s) => s.id)).toEqual(["a"]);
    expect(pathOf(moveStepTo(d, "steps.3", "steps.1.otherwise", 0), "d")).toBe("steps.1.otherwise.0");
  });
  it("down the same list, by the index as it is now", () => {
    expect(moveStepTo(d, "steps.0", "steps", 3).steps.map((s) => s.id)).toEqual(["b", "c", "a", "d"]);
    expect(moveStepTo(d, "steps.3", "steps", 0).steps.map((s) => s.id)).toEqual(["d", "a", "b", "c"]);
  });
  it("never into itself, and knows when a drop changes nothing", () => {
    expect(tree(moveStepTo(d, "steps.1", "steps.1.otherwise", 0))).toBe(tree(d));
    expect(isInside("steps.1.paths.0.steps", "steps.1")).toBe(true);
    expect(isInside("steps.10.steps", "steps.1")).toBe(false);
    expect(staysPut("steps.1", "steps", 1)).toBe(true);
    expect(staysPut("steps.1", "steps", 2)).toBe(true);
    expect(staysPut("steps.1", "steps", 3)).toBe(false);
  });
  it("finds a step's path by its id", () => {
    expect(pathOf(d, "inner")).toBe("steps.1.paths.0.steps.0");
    expect(pathOf(d, "nope")).toBeNull();
  });
});
