// Streaming paints re-render the transcript, not the rest of the chat page.

import { act, render, screen } from "@testing-library/react";
import { vi } from "vitest";
import { ChatStore } from "@/lib/chat/store";
import { ChatStoreProvider, useChat, useChatContent, useChatSelect } from "./chat-provider";

const renders = { page: 0, transcript: 0, focus: 0 };
function Page() {
  useChat();
  renders.page++;
  return null;
}
function Transcript() {
  const store = useChatContent();
  renders.transcript++;
  return <output>{store.contentVersion}</output>;
}
function Focus() {
  const focus = useChatSelect((s) => s.focus);
  renders.focus++;
  return <span>{focus ? "focus" : "normal"}</span>;
}

test("fifty streaming paints re-render the transcript alone", () => {
  const store = new ChatStore({ api: { request: vi.fn(), send: vi.fn() } as never, toast: vi.fn(), go: vi.fn(), chats: { upsert: vi.fn(), remove: vi.fn(), refresh: vi.fn(), find: vi.fn() } });
  render(
    <ChatStoreProvider store={store}>
      <Page />
      <Transcript />
      <Focus />
    </ChatStoreProvider>,
  );
  const before = { ...renders };
  act(() => {
    for (let i = 0; i < 50; i++) store.paint();
  });
  expect(renders.page).toBe(before.page);
  expect(renders.focus).toBe(before.focus);
  expect(renders.transcript).toBeGreaterThan(before.transcript);
  expect(screen.getByRole("status")).toHaveTextContent(String(store.contentVersion));
  // A state change still reaches everyone, and the selector follows its field.
  act(() => {
    store.focus = true;
    store.emit();
  });
  expect(renders.page).toBe(before.page + 1);
  expect(screen.getByText("focus")).toBeInTheDocument();
});
