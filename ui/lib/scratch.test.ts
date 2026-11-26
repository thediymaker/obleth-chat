import { cardOf, isShortcut, pageOf, preview, readDraft, writeDraft } from "./scratch";

test("a page's project comes from its address, and a chat's is looked up", () => {
  expect(pageOf("/projects/p%201/board")).toEqual({ project: "p 1", chat: null });
  expect(pageOf("/projects/p1")).toEqual({ project: "p1", chat: null });
  expect(pageOf("/chat/c1")).toEqual({ project: null, chat: "c1" });
  expect(pageOf("/projects")).toEqual({ project: null, chat: null });
  expect(pageOf("/")).toEqual({ project: null, chat: null });
  expect(pageOf(null)).toEqual({ project: null, chat: null });
});

test("a card's title is the first line and its notes the rest", () => {
  expect(cardOf("  Check the knee \n\nMaya thinks it is memory-bound.\nLook tomorrow. ")).toEqual({ title: "Check the knee", notes: "Maya thinks it is memory-bound.\nLook tomorrow." });
  expect(cardOf("Just a title")).toEqual({ title: "Just a title", notes: "" });
  const long = "x".repeat(250);
  const card = cardOf(`${long}\nmore`);
  expect([...card.title]).toHaveLength(200);
  expect(card.title.endsWith("…")).toBe(true);
  expect(card.notes).toBe(`${long}\nmore`);
});

test("the shortcut is ⌘ or Ctrl with Shift and X, nothing else", () => {
  const key = (o: Partial<KeyboardEvent>) => ({ metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, key: "x", ...o });
  expect(isShortcut(key({ metaKey: true, shiftKey: true, key: "X" }))).toBe(true);
  expect(isShortcut(key({ ctrlKey: true, shiftKey: true }))).toBe(true);
  expect(isShortcut(key({ ctrlKey: true }))).toBe(false);
  expect(isShortcut(key({ ctrlKey: true, shiftKey: true, altKey: true }))).toBe(false);
  expect(isShortcut(key({ metaKey: true, shiftKey: true, key: "o" }))).toBe(false);
});

test("a draft is kept per person, and an empty one is forgotten", () => {
  writeDraft("u1", "half a thought");
  expect(readDraft("u1")).toBe("half a thought");
  expect(readDraft("u2")).toBe("");
  writeDraft("u1", "");
  expect(window.localStorage.getItem("obleth.scratch-draft.u1")).toBeNull();
});

test("a preview is the first line, cut short", () => {
  expect(preview("Ask Maya\nabout logs")).toBe("Ask Maya");
  expect(preview("y".repeat(60))).toHaveLength(40);
});
