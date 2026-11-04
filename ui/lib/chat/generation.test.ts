import { describe, expect, test } from "vitest";
import { generationValues } from "./generation";

const model = {
  id: "m",
  defaults: { systemPrompt: "Workspace instructions", temperature: 0.4, topP: 0.8, maxTokens: 4096, reasoningEffort: "high" },
  allowedOverrides: ["systemPrompt", "temperature", "topP", "maxTokens", "reasoningEffort"],
};
describe("generationValues", () => {
  test("new accounts and appearance-only preferences inherit model defaults", () => {
    expect(generationValues(model)).toEqual(model.defaults);
    expect(generationValues(model, { density: "compact", context: false })).toEqual(model.defaults);
  });
  test("explicit personal values override defaults and conversation values take precedence", () => {
    expect(generationValues(model, { systemPrompt: "", temperature: 0 }, { maxTokens: 512 })).toEqual({ ...model.defaults, systemPrompt: "", temperature: 0, maxTokens: 512 });
  });
  test("model locks take precedence over both user and conversation overrides", () => {
    expect(generationValues({ ...model, lockedParameters: ["temperature"] }, { temperature: 1 }, { temperature: 2 }).temperature).toBe(0.4);
    expect(generationValues({ ...model, allowedOverrides: ["temperature"] }, { maxTokens: 512 }, { maxTokens: 1024 }).maxTokens).toBe(4096);
  });
});
