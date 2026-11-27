import { describe, expect, it } from "vitest";
import { best, better, compareGround, compareState, firstPair, firstShown, versionStats, type ReplyVersion } from "./compare";

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

describe("firstShown", () => {
  it("shows the version in the chat and the newest others, in the order they were written", () => {
    const four = [version("a"), version("b", true), version("c"), version("d")];
    expect(firstShown(four, 3)).toEqual(["b", "c", "d"]);
    expect(firstShown([version("a", true), version("b"), version("c")], 3)).toEqual(["a", "b", "c"]);
    expect(firstShown([version("a"), version("b")], 3)).toEqual(["a", "b"]);
    expect(firstShown([version("a", true)], 3)).toEqual(["a"]);
    expect(firstShown([], 3)).toEqual([]);
  });
});

describe("best", () => {
  it("marks the one that beat every other that measured it", () => {
    expect(best("Speed", "300 tokens/s", ["200 tokens/s", "120 tokens/s"])).toBe(true);
    expect(best("Speed", "200 tokens/s", ["300 tokens/s", "120 tokens/s"])).toBe(false);
    expect(best("First token", "0.41 s", ["1.20 s", undefined])).toBe(true);
    expect(best("First token", "0.41 s", [undefined])).toBe(false);
    expect(best("Output", "10 tokens", ["20 tokens"])).toBe(false);
  });
});

describe("compareState", () => {
  const at = "2026-11-21T10:00:00Z",
    then = Date.parse(at);
  const note = { models: ["b", "c"], why: "close", at };
  it("waits for the other answers, then says how many came", () => {
    expect(compareState(note, 1, then + 5_000)).toEqual({ arrived: 0, pending: true });
    expect(compareState(note, 2, then + 5_000)).toEqual({ arrived: 1, pending: true });
    expect(compareState(note, 3, then + 5_000)).toEqual({ arrived: 2, pending: false });
    expect(compareState(note, 2, then + 300_000)).toEqual({ arrived: 1, pending: false });
  });
  it("says nothing when nobody answered or nothing was asked", () => {
    expect(compareState(note, 1, then + 300_000)).toBeNull();
    expect(compareState(null, 3, then)).toBeNull();
    expect(compareState({ models: [] }, 3, then)).toBeNull();
    expect(compareState({ models: ["b"] }, 1, then)).toBeNull();
  });
  it("gives the ground in words", () => {
    expect(compareGround("close")).toBe("Auto's pick was a close call");
    expect(compareGround("other")).toBe("Chat checked Auto's pick");
  });
});
