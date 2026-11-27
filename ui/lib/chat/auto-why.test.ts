import { describe, expect, it } from "vitest";
import { autoAble, autoRead, autoWhyShort, autoWhyText } from "./auto-why";

describe("how Auto read the request", () => {
  it("says how hard and what about", () => {
    expect(autoRead({ difficulty: 3, topics: ["coding"], needs: ["tools"] })).toBe("a hard coding request");
    expect(autoRead({ difficulty: 1 })).toBe("a simple request");
    expect(autoRead({ topics: ["data_analysis", "math"], needs: ["vision", "schema"] })).toBe("a data analysis and math request");
    expect(autoRead({ needs: ["tools"] })).toBe("");
    expect(autoRead({ reason: "kept" })).toBe("");
  });
  it("says what the model had to be able to do, which is not what the message needed", () => {
    expect(autoAble({ needs: ["tools"] })).toBe("use tools");
    expect(autoAble({ needs: ["tools", "vision", "schema"] })).toBe("use tools, read images and return structured output");
    expect(autoAble({ needs: ["gpu"] })).toBe("");
    expect(autoAble({})).toBe("");
  });
});

describe("autoWhyText", () => {
  it("joins what happened to the chat's model with how the request was read", () => {
    expect(autoWhyText({ reason: "fresh", difficulty: 2, topics: ["coding"], needs: ["tools"], from: "heuristic", candidates: 4 })).toBe("Read as a moderate coding request, picked from 4 models that can use tools.");
    // A greeting in a chat with tools on: simple, and the tools only limited the models.
    expect(autoWhyText({ reason: "fresh", difficulty: 1, needs: ["tools"], from: "heuristic", candidates: 9 })).toBe("Read as a simple request, picked from 9 models that can use tools.");
    expect(autoWhyText({ reason: "fresh", difficulty: 1, needs: ["vision"], candidates: 1 })).toBe("Read as a simple request. It had to be a model that can read images.");
    expect(autoWhyText({ reason: "fresh", needs: ["tools"], candidates: 3 })).toBe("Picked from 3 models that can use tools.");
    expect(autoWhyText({ reason: "kept", difficulty: 1, needs: ["tools"], candidates: 1 })).toBe("Kept the model this chat was already on. Read as a simple request.");
    expect(autoWhyText({ reason: "escalated:tool_loop", difficulty: 3, candidates: 2 })).toBe("The last model was repeating itself, so Auto moved up. Read as a hard request, picked from 2 models.");
    expect(autoWhyText({ reason: "kept", difficulty: 1, candidates: 1 })).toBe("Kept the model this chat was already on. Read as a simple request.");
    expect(autoWhyText({ difficulty: 3, from: "header", candidates: 1 })).toBe("Read as a hard request (from your effort setting).");
    expect(autoWhyText({ candidates: 5 })).toBe("Picked from 5 models.");
  });
  it("says nothing when the gateway said nothing", () => {
    expect(autoWhyText(undefined)).toBe("");
    expect(autoWhyText({})).toBe("");
    expect(autoWhyText({ reason: "fresh" })).toBe("");
  });
});

describe("autoWhyShort", () => {
  it("is a few words for the line under the reply", () => {
    expect(autoWhyShort({ reason: "fresh", difficulty: 3, topics: ["coding", "math", "physics"], needs: ["tools"] })).toBe("hard coding, math · tools on");
    expect(autoWhyShort({ reason: "fresh", difficulty: 1, needs: ["tools", "vision"] })).toBe("simple · tools on, image");
    expect(autoWhyShort({ reason: "kept", difficulty: 2 })).toBe("kept for this chat");
    expect(autoWhyShort({ reason: "escalated:complaint" })).toBe("moved up");
    expect(autoWhyShort({ reason: "fresh" })).toBe("");
    expect(autoWhyShort(null)).toBe("");
  });
});

describe("autoWhyText and the models ranked next", () => {
  it("names them, by their display names when given", () => {
    const why = { reason: "fresh", difficulty: 2, candidates: 4, next: [{ model: "glm", gap: 0.01 }, { model: "qwen" }] };
    expect(autoWhyText(why)).toBe("Read as a moderate request, picked from 4 models. Next in line: glm and qwen.");
    expect(autoWhyText(why, (id) => id.toUpperCase())).toContain("Next in line: GLM and QWEN.");
  });
});
