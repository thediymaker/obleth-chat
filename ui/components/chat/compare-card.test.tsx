// The card under a reply that Chat also put to other models.

import { screen } from "@testing-library/react";
import { vi } from "vitest";
import { ChatStore } from "@/lib/chat/store";
import type { Message } from "@/lib/chat/types";
import { ChatStoreProvider } from "./chat-provider";
import { CompareCard } from "./compare-card";
import { polyfillLayout, renderWithQueries } from "./test-queries";

polyfillLayout();

const version = (id: string) => ({ id, status: "completed", createdAt: new Date().toISOString() });
const reply = (over: Partial<Message>): Message => ({ id: "v1", role: "assistant", content: "Answer", status: "completed", versions: [version("v1")], ...over }) as Message;

function show(m: Message, shared = false) {
  const store = new ChatStore({ api: { request: vi.fn().mockRejectedValue(new Error("offline")), send: vi.fn() } as never, toast: vi.fn(), go: vi.fn(), chats: { upsert: vi.fn(), remove: vi.fn(), refresh: vi.fn(), find: vi.fn() } });
  store.models = [
    { id: "glm", name: "GLM" },
    { id: "qwen", name: "Qwen" },
  ] as never;
  store.active = { id: "c1", title: "Chat", messages: [m] } as never;
  if (shared) store.shared = { id: "s" } as never;
  renderWithQueries(
    <ChatStoreProvider store={store}>
      <CompareCard m={m} />
    </ChatStoreProvider>,
  );
  return document.querySelector("[data-compare-card]");
}

const note = (ago = 0) => ({ models: ["glm", "qwen"], why: "close", at: new Date(Date.now() - ago).toISOString() });

test("a reply nobody compared has no card", () => {
  expect(show(reply({}))).toBeNull();
  expect(show(reply({ versions: [version("v1"), version("v2")] }))).toBeNull();
});

test("while the other models answer, it says who and why, lit, with nothing to open yet", () => {
  const card = show(reply({ settings: { compare: note() } }))!;
  expect(card.getAttribute("data-compare-card")).toBe("waiting");
  expect(card.textContent).toContain("Auto's pick was a close call, so GLM and Qwen are answering this too.");
  expect(card.querySelector(".signal-pulse")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Compare the answers" })).toBeNull();
});

test("once answers are in, it offers them side by side", () => {
  const card = show(reply({ settings: { compare: note(300_000) }, versions: [version("v1"), version("v2"), version("v3")] }))!;
  expect(card.getAttribute("data-compare-card")).toBe("ready");
  expect(card.textContent).toContain("Auto's pick was a close call, so Chat also asked GLM and Qwen.");
  expect(card.querySelector(".signal-pulse")).toBeNull();
  expect(screen.getByRole("button", { name: "Compare the answers" })).toBeTruthy();
});

test("nobody answered in time: the card goes", () => {
  expect(show(reply({ settings: { compare: note(300_000) } }))).toBeNull();
});

test("a kept second opinion still leads back to the others", () => {
  const card = show(reply({ id: "v2", settings: { compareOf: "v1", model: "glm" }, versions: [version("v1"), version("v2"), version("v3")] }))!;
  expect(card.textContent).toContain("One of 3 answers to this message.");
  expect(screen.getByRole("button", { name: "Compare the answers" })).toBeTruthy();
});

test("a shared chat shows no card", () => {
  expect(show(reply({ settings: { compare: note(300_000) }, versions: [version("v1"), version("v2")] }), true)).toBeNull();
});
