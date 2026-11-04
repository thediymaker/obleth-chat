import { expect, test } from "vitest";
import { chooserModels } from "./model-choices";

const models = [
  { id: "auto", name: "Auto" },
  { id: "glm", name: "GLM", shortList: true },
  { id: "gemma", name: "Gemma", tags: ["vision"] },
  { id: "kimi", name: "Kimi", isDefault: true },
  { id: "qwen", name: "Qwen" },
  { id: "olmo", name: "OLMo" },
];
const ids = (r: ReturnType<typeof chooserModels>) => r.choices.map((m) => m.id);

test("with a short list the chooser starts with it, plus Auto, favorites, the default and the model in use", () => {
  const r = chooserModels(models, { favorites: ["qwen"], current: "olmo" });
  expect(ids(r)).toEqual(["auto", "qwen", "glm", "kimi", "olmo"]);
  expect(r.hidden).toBe(1);
});

test("searching or See all lists every model, and no short list means everything", () => {
  expect(ids(chooserModels(models, { query: "vision" }))).toEqual(["gemma"]);
  expect(chooserModels(models, { all: true }).hidden).toBe(0);
  const none = chooserModels(models.map((m) => ({ ...m, shortList: false })));
  expect(none.choices.length).toBe(6);
  expect(none.hidden).toBe(0);
});
