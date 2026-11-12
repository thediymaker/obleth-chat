// The better-suited model card: switching uses the model for this chat only and
// records the outcome; keeping and "don't suggest again" record theirs.

import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: false, user: { username: "pat" } }), useUser: () => ({ id: "u1", role: "user" }), onBeforeSignOut: () => () => {} }));
vi.mock("@/components/projects/chat-project", () => ({ useChatProject: () => null, useChatProjectId: () => "" }));
import { ChatStore } from "@/lib/chat/store";
import type { Message } from "@/lib/chat/types";
import { ChatStoreProvider } from "./chat-provider";
import { ModelOffer } from "./question-cards";
import { renderWithQueries } from "./test-queries";

const offer = { id: "model-suggestion", kind: "model", status: "asked", model: "small-7b", suggested: "glm-5-3", name: "GLM 5.3", score: 98.5 };
const reply: Message = { id: "a1", role: "assistant", content: "", status: "completed", activity: [offer as never] };

function card() {
  const request = vi.fn().mockResolvedValue(undefined);
  const store = new ChatStore({ api: { request, send: vi.fn() } as never, toast: vi.fn(), go: vi.fn(), chats: { upsert: vi.fn(), remove: vi.fn(), refresh: vi.fn(), find: vi.fn() } });
  store.active = { id: "c1", title: "Build", model: "small-7b", messages: [{ id: "u1", role: "user", content: "Build it", status: "completed" }, reply] } as never;
  store.model = "small-7b";
  const save = vi.spyOn(store, "savePreference").mockResolvedValue(undefined as never);
  renderWithQueries(
    <ChatStoreProvider store={store}>
      <ModelOffer m={reply} index={1} />
    </ChatStoreProvider>,
  );
  return { store, request, save };
}

describe("the better-suited model card", () => {
  it("switches the model for this chat only and says so", () => {
    const { store, request, save } = card();
    expect(screen.getByText(/GLM 5.3 did better in Chat’s certification \(98.5%\)/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Switch to GLM 5.3" }));
    expect(store.model).toBe("glm-5-3");
    expect(save).not.toHaveBeenCalled();
    expect(request).toHaveBeenCalledWith("/api/conversations/c1/suggestion", { method: "POST", body: { outcome: "switched" } });
    expect(screen.getByText("Switched to GLM 5.3 for this chat.")).toBeInTheDocument();
  });

  it("keeps the model when dismissed, and can stop suggestions", () => {
    const { store, request } = card();
    fireEvent.click(screen.getByRole("button", { name: "Don’t suggest models again" }));
    expect(store.model).toBe("small-7b");
    expect(request).toHaveBeenCalledWith("/api/conversations/c1/suggestion", { method: "POST", body: { outcome: "never" } });
    expect(screen.getByText(/No more suggestions/)).toBeInTheDocument();
  });
});
