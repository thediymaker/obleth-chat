import { describe, expect, it } from "vitest";
import { fileMention, marked, matchItems, menuTrigger, replaceTrigger, type MenuItem } from "./composer-menu";

describe("menuTrigger", () => {
  it("opens the commands for a slash that starts the message", () => {
    expect(menuTrigger("/", 1)).toEqual({ kind: "command", query: "", from: 0, to: 1 });
    expect(menuTrigger("/model son", 10)).toEqual({ kind: "command", query: "model son", from: 0, to: 10 });
  });
  it("leaves a path, a later slash and a second line alone", () => {
    expect(menuTrigger("/usr/bin", 8)).toBeNull();
    expect(menuTrigger("see /new", 8)).toBeNull();
    expect(menuTrigger("/new\nmore", 4)).toBeNull();
    expect(menuTrigger("/new", 2)).toBeNull();
  });
  it("opens what a message can point at after an @ that starts a word", () => {
    expect(menuTrigger("Compare results.csv with @sw", 28)).toEqual({ kind: "mention", query: "sw", from: 25, to: 28 });
    expect(menuTrigger("@", 1)).toEqual({ kind: "mention", query: "", from: 0, to: 1 });
    expect(menuTrigger("(@logs/sweep-1", 14)).toEqual({ kind: "mention", query: "logs/sweep-1", from: 1, to: 14 });
    expect(menuTrigger("@sweep and more", 6)).toEqual({ kind: "mention", query: "sweep", from: 0, to: 6 });
  });
  it("does not take an address or a finished word for one", () => {
    expect(menuTrigger("mail sam@example.org", 20)).toBeNull();
    expect(menuTrigger("@sweep ", 7)).toBeNull();
  });
});

describe("matchItems", () => {
  const items: MenuItem[] = [
    { id: "new", group: "Chat", slash: "new", label: "New chat" },
    { id: "rename", group: "Chat", slash: "rename", label: "Rename this chat", keywords: "title name" },
    { id: "m1", group: "Model", slash: "model", label: "Use Sonnet 5.5" },
    { id: "m2", group: "Model", slash: "model", label: "Use Opus 5.5" },
    { id: "f1", group: "Files", label: "logs/sweep-128.out", mono: true },
    { id: "f2", group: "Files", label: "sweep.sbatch", mono: true },
  ];
  const ids = (query: string) => matchItems(items, query).map((i) => i.id);
  it("lists everything, in order, before a query", () => {
    expect(ids("")).toEqual(["new", "rename", "m1", "m2", "f1", "f2"]);
  });
  it("finds by the slash word, the label or a keyword, the closest first", () => {
    expect(ids("n")).toEqual(["new", "rename"]);
    expect(ids("ne")).toEqual(["new"]);
    expect(ids("name")).toEqual(["rename"]);
    expect(ids("model")).toEqual(["m1", "m2"]);
    expect(ids("model son")).toEqual(["m1"]);
    expect(ids("opus")).toEqual(["m2"]);
  });
  it("finds a file by any part of its path, its name first", () => {
    expect(ids("sw")).toEqual(["f2", "f1"]);
    expect(ids("128")).toEqual(["f1"]);
    expect(ids("logs/sw")).toEqual(["f1"]);
  });
  it("shows a few of each group before a query", () => {
    const many: MenuItem[] = [...Array.from({ length: 9 }, (_, i) => ({ id: `f${i}`, group: "Files", label: `file-${i}` })), { id: "t", group: "Tools", label: "Sol" }];
    expect(matchItems(many, "").map((i) => i.id)).toEqual(["f0", "f1", "f2", "f3", "f4", "f5", "t"]);
    expect(matchItems(many, "file").length).toBe(9);
  });
  it("keeps a group together, placed by its best item", () => {
    const mixed: MenuItem[] = [
      { id: "f1", group: "Files", label: "logs/sweep-64.out", mono: true },
      { id: "j1", group: "Jobs", label: "sweep-64" },
      { id: "f2", group: "Files", label: "sweep.sbatch", mono: true },
    ];
    expect(matchItems(mixed, "sw").map((i) => i.id)).toEqual(["j1", "f2", "f1"]);
  });
  it("finds nothing for words no item has", () => {
    expect(ids("zebra")).toEqual([]);
  });
});

describe("marked", () => {
  it("cuts a label around the query", () => {
    expect(marked("logs/sweep-128.out", "sw")).toEqual(["logs/", "sw", "eep-128.out"]);
    expect(marked("New chat", "")).toEqual(["New chat", "", ""]);
    expect(marked("New chat", "model")).toEqual(["New chat", "", ""]);
  });
});

describe("replaceTrigger", () => {
  it("puts the pick where the @ and its query were, with a space after", () => {
    expect(replaceTrigger("Compare a with @sw", { from: 15, to: 18 }, fileMention("sweep.sbatch"))).toEqual({ text: "Compare a with `sweep.sbatch` ", caret: 30 });
    expect(replaceTrigger("@sw and more", { from: 0, to: 3 }, "Handbook")).toEqual({ text: "Handbook and more", caret: 8 });
  });
  it("clears a command", () => {
    expect(replaceTrigger("/new", { from: 0, to: 4 }, "")).toEqual({ text: "", caret: 0 });
  });
});
