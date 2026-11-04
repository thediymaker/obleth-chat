import { expect, test } from "vitest";
import { ApiClient } from "@/lib/api";
import { chatMenuItems, leftTemporary, temporaryNoteText } from "./temporary";

test("the temporary note says the chat is not kept", () => {
  expect(temporaryNoteText()).toMatch(/deleted when you leave it/);
  expect(temporaryNoteText(false)).toMatch(/won’t be in your chats/);
});

test("a temporary chat can only be renamed or deleted from its menu", () => {
  const actions = (c: any) => chatMenuItems(c).map(([, , action]) => action);
  expect(actions({ temporary: true })).toEqual(["rename-chat", "delete-chat"]);
  expect(actions({ temporary: false, pinned: true })).toEqual(["rename-chat", "pin-chat", "archive-chat", "share-chat", "export-chat", "delete-chat"]);
  expect(chatMenuItems({ pinned: true })[1]![1]).toBe("Unpin");
  expect(chatMenuItems({ archived: true })[2]![1]).toBe("Restore");
});

test("opening anything other than the temporary chat leaves it", () => {
  expect(leftTemporary(null, { page: "history", id: "" })).toBe(null);
  expect(leftTemporary("t1", { page: "chat", id: "t1" })).toBe(null);
  expect(leftTemporary("t1", { page: "chat", id: "other" })).toBe("t1");
  expect(leftTemporary("t1", { page: "chat", id: "" })).toBe("t1");
  expect(leftTemporary("t1", { page: "settings", id: "" })).toBe("t1");
});

test("leaving sends a request that outlives the page and never throws", async () => {
  let captured: { path: string; options: RequestInit } | undefined;
  const client = new ApiClient((async (path: string, options: RequestInit) => {
    captured = { path, options };
    throw new TypeError("offline");
  }) as unknown as typeof fetch);
  client.csrfToken = "csrf-test";
  await client.send("/api/conversations/t1/leave");
  expect(captured!.path).toBe("/api/conversations/t1/leave");
  expect(captured!.options.method).toBe("POST");
  expect(captured!.options.keepalive).toBe(true);
  expect(captured!.options.credentials).toBe("same-origin");
  expect((captured!.options.headers as Record<string, string>)["X-CSRF-Token"]).toBe("csrf-test");
  expect(() => client.send("https://elsewhere.test/api/x")).toThrow(/this origin/);
});
