import { expect, test } from "vitest";
import { createLoad, loadLabel } from "./load";

const answer = {
  available: true,
  models: {
    glm: { state: "full", slots: 40, inFlight: 40, waiting: 12, you: { inFlight: 1, limit: 4 } },
    qwen: { state: "free", slots: 16, inFlight: 3, waiting: 0 },
    gemma: { state: "busy", slots: 10, inFlight: 9, waiting: 0 },
    cold: { state: "offline", slots: 0, inFlight: 0, waiting: 0 },
  },
};
function setup(reply: unknown = answer, preferences = { favorites: ["glm", "qwen"], defaultModel: "gemma" }) {
  let calls = 0;
  const state = { preferences, models: [{ id: "auto" }, { id: "glm" }, { id: "qwen" }, { id: "gemma", metadata: { supportsVision: true } }, { id: "cold" }] };
  const load = createLoad({ api: { request: async () => { calls++; return reply as any; } }, state: () => state, modelName: (id) => `${id}-name` });
  return { load, calls: () => calls };
}
const waiting = (model: string, ms = 4000) => [{ role: "assistant", content: "", status: "streaming", settings: { model } }, { waitingSince: Date.now() - ms }] as const;

test("only a busy, full or serverless model has its icon tinted and a label", async () => {
  const { load } = setup();
  await load.refresh();
  for (const id of ["qwen", "unknown"]) {
    expect(load.tint(id)).toBe("");
    expect(load.label(id)).toBe("");
  }
  expect(load.tint("gemma")).toBe("busy");
  expect(load.label("gemma")).toBe("Busy: 9 running on 10 slots");
  expect(load.tint("glm")).toBe("full");
  expect(load.label("glm")).toBe("Full: 40 running on 40 slots, 12 waiting");
  expect(load.tint("cold")).toBe("offline");
  expect(load.label("cold")).toBe("No servers running right now");
  expect(loadLabel(undefined)).toBe("");
  expect(loadLabel({ state: "full", slots: 32, inFlight: 37, waiting: 0 })).toBe("Full: 37 running on 32 slots");
});

test("load is fetched at most once per window, and a gateway without it is left alone", async () => {
  const live = setup();
  await live.load.refresh();
  await live.load.refresh(5000);
  expect(live.calls()).toBe(1);
  await live.load.refresh(0);
  expect(live.calls()).toBe(2);
  const none = setup({ available: false });
  await none.load.refresh();
  await none.load.refresh(0);
  expect(none.calls()).toBe(1);
  expect(none.load.offered()).toBe(false);
  expect(none.load.notice(...waiting("glm"))).toBe(null);
});

test("a waiting reply says what holds it and offers a model that is not full", async () => {
  const { load } = setup();
  await load.refresh();
  expect(load.notice(...waiting("glm", 1000))).toBe(null);
  const notice = load.notice(...waiting("glm"))!;
  expect(notice.text).toBe("glm-name is full right now: 40 running on 40 slots, 12 waiting. Your message is in line.");
  // The first favorite is the full model itself, so the next one is offered.
  expect([notice.next, notice.label]).toEqual(["qwen", "Switch to qwen-name"]);
  expect(notice.waiting).toBe(true);
  expect(load.notice(...waiting("cold"))!.text).toMatch(/cold-name has no servers running right now/);
  expect(load.notice(...waiting("qwen"))).toBe(null);
  expect(load.notice(...waiting("auto"))).toBe(null);
  // An image needs a vision model: the default, here.
  expect(load.notice(waiting("glm")[0], { waitingSince: Date.now() - 4000, needsVision: true })!.next).toBe("gemma");
});

test("the person’s own limit is named, and a reply the gateway gave up on can switch", async () => {
  const own = setup({ ...answer, models: { ...answer.models, glm: { ...answer.models.glm, you: { inFlight: 4, limit: 4 } } } });
  await own.load.refresh();
  const held = own.load.notice(...waiting("glm"))!;
  expect(held.text).toBe("You have 4 of 4 requests running on glm-name, so this one starts when one of them finishes.");
  expect(held.next).toBe("");
  const { load } = setup(answer, { favorites: [], defaultModel: "glm" });
  await load.refresh();
  const failed = { role: "assistant", content: "", status: "error", errorMessage: "The model stayed full, so the gateway stopped waiting. Retry, or pick another model.", settings: { model: "glm" } };
  const notice = load.notice(failed)!;
  expect(notice.text).toBe("glm-name stayed full.");
  expect([notice.next, notice.label]).toEqual(["auto", "Let Auto choose"]);
  expect(notice.waiting).toBe(false);
});
