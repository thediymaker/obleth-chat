// The suggest_project card: it shows the suggested project, opens New project filled
// in with it, and once the chat has moved there says so as the person's next message.

import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: false, user: { username: "pat" } }), useUser: () => ({ id: "u1", role: "user" }), onBeforeSignOut: () => () => {} }));
vi.mock("@/components/projects/chat-project", () => ({ useChatProject: () => null, useChatProjectId: () => "" }));
// The dialog stands in for New project: it shows what it was filled in with, and its
// button moves the chat.
vi.mock("@/components/projects/project-dialog", () => ({
  NewProjectDialog: ({ open, initial, fromChat, onMoved }: { open: boolean; initial: { name: string; instructions: string }; fromChat: string; onMoved: (p: unknown) => void }) =>
    open ? (
      <div role="dialog">
        <p>New project {initial.name}</p>
        <p>From {fromChat}</p>
        <button type="button" onClick={() => onMoved({ id: "p9", name: initial.name })}>
          Create
        </button>
      </div>
    ) : null,
}));
import { ChatStore } from "@/lib/chat/store";
import type { Message } from "@/lib/chat/types";
import { ChatStoreProvider } from "./chat-provider";
import { ProjectOffer } from "./question-cards";
import { renderWithQueries } from "./test-queries";

const suggestion = { id: "s1", kind: "project", status: "asked", name: "Coral thesis", description: "My PhD thesis on coral bleaching.", instructions: "Cite in APA." };
const reply: Message = { id: "a1", role: "assistant", content: "", status: "completed", activity: [suggestion as never] };

function card(m = reply) {
  const store = new ChatStore({ api: { request: vi.fn(), send: vi.fn() } as never, toast: vi.fn(), go: vi.fn(), chats: { upsert: vi.fn(), remove: vi.fn(), refresh: vi.fn(), find: vi.fn() } });
  store.active = { id: "c1", title: "Thesis", model: "m1", messages: [{ id: "u1", role: "user", content: "Help", status: "completed" }, m] } as never;
  const send = vi.spyOn(store, "send").mockResolvedValue(undefined);
  renderWithQueries(
    <ChatStoreProvider store={store}>
      <ProjectOffer m={m} index={1} />
    </ChatStoreProvider>,
  );
  return { send };
}

describe("ProjectOffer", () => {
  it("opens New project filled in, and carries on once the chat has moved", () => {
    const { send } = card();
    expect(screen.getByText("Coral thesis")).toBeTruthy();
    expect(screen.getByText("Cite in APA.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Start project" }));
    expect(screen.getByText("New project Coral thesis")).toBeTruthy();
    expect(screen.getByText("From c1")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(send).toHaveBeenCalledWith("I started the project “Coral thesis” for this chat. Go ahead.");
    expect(screen.getByText(/Started\./)).toBeTruthy();
  });

  it("can be turned down", () => {
    const { send } = card();
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(screen.getByText(/Not started\./)).toBeTruthy();
    expect(send).not.toHaveBeenCalled();
  });

  it("is closed on an older reply", () => {
    card({ ...reply, status: "streaming" });
    expect(screen.queryByRole("button", { name: "Start project" })).toBeNull();
    expect(screen.getByText("Coral thesis")).toBeTruthy();
  });
});
