import { describe, expect, it } from "vitest";
import { host, imageSize, kindLabel, parseTags, savedQuery, suggestTags } from "./api";

describe("library helpers", () => {
  it("reads tags the way people type them", () => {
    expect(parseTags("#Research, python  #research #a")).toEqual(["research", "python", "a"]);
    expect(parseTags(Array.from({ length: 20 }, (_, i) => `t${i}`).join(" "))).toHaveLength(12);
    expect(parseTags("")).toEqual([]);
  });

  it("suggests the code's language and the person's tags the text mentions", () => {
    const known = [{ tag: "gaudi" }, { tag: "prefix-cache" }, { tag: "ok" }];
    expect(suggestTags({ title: "Gaudi prefix cache hits", body: "", language: "Python" }, known)).toEqual(["python", "gaudi", "prefix-cache"]);
    expect(suggestTags({ title: "ok gaudi", tags: ["gaudi"] }, known)).toEqual([]);
  });

  it("asks for what the filters pick", () => {
    expect(savedQuery({})).toBe("/api/saved");
    expect(savedQuery({ q: "throughput", kind: "", tag: "gaudi", project: "p1" })).toBe("/api/saved?q=throughput&tag=gaudi&project=p1");
  });

  it("labels and sizes", () => {
    expect(kindLabel({ kind: "code", language: "rust" })).toBe("Function · rust");
    expect(kindLabel({ kind: "excerpt", language: "" })).toBe("Excerpt");
    expect(host("https://docs.vllm.ai/en/latest/")).toBe("docs.vllm.ai");
    expect(host("not a url")).toBe("not a url");
    expect(imageSize(100)).toBe("1 KB");
    expect(imageSize(3 * 1048576)).toBe("3.0 MB");
  });
});
