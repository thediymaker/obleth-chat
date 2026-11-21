import { describe, expect, it } from "vitest";
import { better, firstPair, versionStats, type ReplyVersion } from "./compare";

const version = (id: string, selected = false): ReplyVersion => ({ id, status: "completed", createdAt: "", model: "m", content: "", selected });

describe("firstPair", () => {
  it("shows the version in the chat beside the newest other one, in the order they were written", () => {
    expect(firstPair([version("a"), version("b", true), version("c")])).toEqual(["b", "c"]);
    expect(firstPair([version("a"), version("b"), version("c", true)])).toEqual(["b", "c"]);
    expect(firstPair([version("a", true), version("b")])).toEqual(["a", "b"]);
  });
  it("needs two versions", () => {
    expect(firstPair([version("a", true)])).toBeNull();
  });
});

describe("versionStats", () => {
  it("says what was measured", () => {
    expect(versionStats({ usage: { completion_tokens: 1840, generation_ms: 9200, first_token_ms: 410, steps: 3 } })).toEqual([
      ["Output", "1,840 tokens"],
      ["Speed", "200 tokens/s"],
      ["First token", "0.41 s"],
      ["Time", "9.2 s"],
      ["Steps", "3"],
    ]);
    expect(versionStats({ usage: { completion_tokens: 12 } })).toEqual([["Output", "12 tokens"]]);
    expect(versionStats({ usage: null })).toEqual([]);
  });
});

describe("better", () => {
  it("marks the faster side, and no side for a tie or a size", () => {
    expect(better("Speed", "200 tokens/s", "1,240 tokens/s")).toBe("right");
    expect(better("First token", "0.41 s", "1.20 s")).toBe("left");
    expect(better("Time", "9.2 s", "9.2 s")).toBeNull();
    expect(better("Output", "10 tokens", "20 tokens")).toBeNull();
    expect(better("Speed", "200 tokens/s", undefined)).toBeNull();
  });
});
