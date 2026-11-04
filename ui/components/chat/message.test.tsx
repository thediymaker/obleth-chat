// One message of a chat: the answer as Markdown, its
// actions, versions, and a read-only shared chat.

import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: false, user: { username: "pat", display_name: "Pat Lee" } }), useUser: () => ({ id: "u1", role: "user" }), onBeforeSignOut: () => () => {} }));
import { ChatStore } from "@/lib/chat/store";
import type { Message } from "@/lib/chat/types";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ChatStoreProvider } from "./chat-provider";
import { MessageView, messageSignature } from "./message";
import { polyfillLayout, renderWithQueries } from "./test-queries";

polyfillLayout();

function view(messages: Message[], index: number, setup?: (store: ChatStore) => void) {
  const store = new ChatStore({ api: { request: vi.fn().mockRejectedValue(new Error("offline")), send: vi.fn() } as never, toast: vi.fn(), go: vi.fn(), chats: { upsert: vi.fn(), remove: vi.fn(), refresh: vi.fn(), find: vi.fn() } });
  store.active = { id: "c1", title: "Plan", model: "m1", messages } as never;
  store.route = { page: "chat", id: "c1" };
  setup?.(store);
  const m = messages[index]!;
  renderWithQueries(
    <TooltipProvider>
      <ChatStoreProvider store={store}>
        <MessageView m={m} index={index} signature={messageSignature(store, m, index, messages.length)} version={0} />
      </ChatStoreProvider>
    </TooltipProvider>,
  );
  return store;
}
const user: Message = { id: "u1", role: "user", content: "Hello", status: "completed" };
const answer: Message = { id: "a1", role: "assistant", content: "Use **bold** text.", status: "completed", settings: { model: "m1" } as never };

describe("MessageView", () => {
  it("renders the answer and regenerates it", () => {
    const store = view([user, answer], 1);
    expect(screen.getByText("bold").tagName).toBe("STRONG");
    const regenerate = vi.spyOn(store, "forkAndSend").mockResolvedValue(undefined);
    fireEvent.click(screen.getByRole("button", { name: "Regenerate response" }));
    expect(regenerate).toHaveBeenCalledWith("a1", "regenerate");
    expect(screen.getByRole("button", { name: "Good response" })).toBeTruthy();
  });

  it("offers edit and resend on the person's message", () => {
    const store = view([user, answer], 0);
    expect(screen.getByText("You")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Edit and resend message" }));
    expect(store.dialog).toMatchObject({ kind: "edit", messageId: "u1", content: "Hello" });
  });

  it("steps between versions of a reply", () => {
    const store = view([user, { ...answer, versionIndex: 2, versions: [{ id: "a0" }, { id: "a1" }] as never }], 1);
    expect(screen.getByLabelText("Version 2 of 2")).toBeTruthy();
    const select = vi.spyOn(store, "selectMessageVersion").mockResolvedValue(undefined);
    fireEvent.click(screen.getByRole("button", { name: "Previous version" }));
    expect(select).toHaveBeenCalledWith("a0");
    expect((screen.getByRole("button", { name: "Next version" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("is read-only in someone else's shared chat", () => {
    view([user, answer], 1, (store) => {
      store.shared = { scope: "link", ownerName: "Ana" };
    });
    expect(screen.queryByRole("button", { name: "Regenerate response" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Good response" })).toBeNull();
    expect(screen.getByRole("button", { name: "Copy response" })).toBeTruthy();
  });
});
