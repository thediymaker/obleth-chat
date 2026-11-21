import { describe, expect, it } from "vitest";
import { pickedElement, pickedText, versionAt, versionsOf, withPicked } from "./build-view";
import type { Message } from "./types";

const reply = (id: string, activity: Record<string, unknown>[]): Message => ({ id, role: "assistant", content: "", createdAt: `2026-11-21T10:0${id}:00Z`, activity } as Message);

describe("versionsOf", () => {
  const messages = [
    { id: "u", role: "user", content: "build it" } as Message,
    reply("1", [{ id: "s1", kind: "tool", tool: "write_file" }, { id: "checkpoint-aaaaaaa1", kind: "checkpoint", sha: "aaaaaaa1", summary: "Checkpoint 1", stat: "3 files changed" }]),
    reply("2", [{ id: "checkpoint-bbbbbbb2", kind: "checkpoint", sha: "bbbbbbb2", summary: "Checkpoint 2" }, { id: "checkpoint-aaaaaaa1", kind: "checkpoint", sha: "aaaaaaa1", summary: "Checkpoint 1" }, { id: "x", kind: "checkpoint", sha: "not a sha" }]),
  ];
  it("lists every checkpoint of the chat once, oldest first", () => {
    expect(versionsOf(messages)).toEqual([
      { sha: "aaaaaaa1", label: "Checkpoint 1", stat: "3 files changed", at: "2026-11-21T10:01:00Z" },
      { sha: "bbbbbbb2", label: "Checkpoint 2", stat: "", at: "2026-11-21T10:02:00Z" },
    ]);
    expect(versionsOf(undefined)).toEqual([]);
  });
  it("is at the newest version unless one was restored", () => {
    const versions = versionsOf(messages);
    expect(versionAt(versions, "")).toBe(1);
    expect(versionAt(versions, "aaaaaaa1")).toBe(0);
    expect(versionAt(versions, "gone")).toBe(1);
    expect(versionAt([], "")).toBe(-1);
  });
});

describe("point and say", () => {
  it("keeps a picked element as clipped text, and refuses what is not one", () => {
    expect(pickedElement({ element: { tag: "BUTTON", selector: " #hero > button.cta ", text: "Sign\n up `now`", path: "/pricing?x=1", width: 120.4, height: 40, extra: "<script>" } })).toEqual({
      tag: "button",
      selector: "#hero > button.cta",
      text: "Sign up now",
      path: "/pricing?x=1",
      width: 120,
      height: 40,
    });
    expect(pickedElement({ element: { tag: "a b", selector: "x" } })).toBeNull();
    expect(pickedElement({ element: { tag: "div", selector: "" } })).toBeNull();
    expect(pickedElement({ element: "div" })).toBeNull();
    expect(pickedElement(null)).toBeNull();
    expect(pickedElement({ element: { tag: "p", selector: "p", text: "x".repeat(500) } })?.text).toHaveLength(140);
  });
  it("puts the element into the message box ahead of what was typed", () => {
    const el = { tag: "button", selector: "#hero > button.cta", text: "Sign up", path: "/pricing", width: 120, height: 40 };
    expect(pickedText(el)).toBe("In the preview on `/pricing`, the `button` “Sign up” (`#hero > button.cta`): ");
    expect(pickedText({ ...el, text: "", path: "/" })).toBe("In the preview, the `button` (`#hero > button.cta`): ");
    expect(withPicked("  make it bigger ", el)).toBe("In the preview on `/pricing`, the `button` “Sign up” (`#hero > button.cta`): make it bigger");
    expect(withPicked("", el)).toBe(pickedText(el));
  });
});
