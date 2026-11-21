import { describe, expect, it } from "vitest";
import { autoRead, autoWhyShort, autoWhyText } from "./auto-why";

describe("how Auto read the request", () => {
  it("says how hard, what about and what it needs", () => {
    expect(autoRead({ difficulty: 3, topics: ["coding"], needs: ["tools"] })).toBe("a hard coding request that needs tools");
    expect(autoRead({ difficulty: 1 })).toBe("a simple request");
    expect(autoRead({ topics: ["data_analysis", "math"], needs: ["vision", "schema"] })).toBe("a data analysis and math request that needs images and structured output");
    expect(autoRead({ needs: ["tools"] })).toBe("a request that needs tools");
    expect(autoRead({ reason: "kept" })).toBe("");
  });
});

describe("autoWhyText", () => {
  it("joins what happened to the chat's model with how the request was read", () => {
    expect(autoWhyText({ reason: "fresh", difficulty: 2, topics: ["coding"], needs: ["tools"], from: "heuristic", candidates: 4 })).toBe("Read as a moderate coding request that needs tools, picked from 4 models.");
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
    expect(autoWhyShort({ reason: "fresh", difficulty: 3, topics: ["coding", "math", "physics"], needs: ["tools"] })).toBe("hard coding, math · needs tools");
    expect(autoWhyShort({ reason: "kept", difficulty: 2 })).toBe("kept for this chat");
    expect(autoWhyShort({ reason: "escalated:complaint" })).toBe("moved up");
    expect(autoWhyShort({ reason: "fresh" })).toBe("");
    expect(autoWhyShort(null)).toBe("");
  });
});
