// The ask_user card: one question at a time, a single choice
// moves on, and the answers go out as the person's next message.

import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: false, user: { username: "pat" } }), useUser: () => ({ id: "u1", role: "user" }), onBeforeSignOut: () => () => {} }));
import { ChatStore } from "@/lib/chat/store";
import type { Message } from "@/lib/chat/types";
import { ChatStoreProvider } from "./chat-provider";
import { answersText, QuestionCard } from "./question-cards";
import { renderWithQueries } from "./test-queries";

const questions = [
  { header: "Stack", question: "Which framework?", options: [{ label: "React", recommended: true }, { label: "Vue" }] },
  { header: "Extras", question: "Anything else?", multiple: true, options: [{ label: "Tests" }, { label: "Docs" }] },
];
const reply: Message = { id: "a1", role: "assistant", content: "", status: "completed", activity: [{ id: "q1", kind: "question", status: "awaiting", questions } as never] };

function card(m = reply) {
  const store = new ChatStore({ api: { request: vi.fn(), send: vi.fn() } as never, toast: vi.fn(), go: vi.fn(), chats: { upsert: vi.fn(), remove: vi.fn(), refresh: vi.fn(), find: vi.fn() } });
  store.active = { id: "c1", title: "Plan", model: "m1", messages: [{ id: "u1", role: "user", content: "Help", status: "completed" }, m] } as never;
  const send = vi.spyOn(store, "send").mockResolvedValue(undefined);
  renderWithQueries(
    <ChatStoreProvider store={store}>
      <QuestionCard m={m} index={1} />
    </ChatStoreProvider>,
  );
  return { store, send };
}

describe("QuestionCard", () => {
  it("walks the questions and sends the answers", () => {
    const { send } = card();
    expect(screen.getByText("Which framework?")).toBeTruthy();
    expect(screen.getByText("Recommended")).toBeTruthy();
    fireEvent.click(screen.getByRole("radio", { name: /React/ }));
    // A single choice moves on to the next question.
    expect(screen.getByText("Anything else?")).toBeTruthy();
    fireEvent.click(screen.getByRole("checkbox", { name: /Tests/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /Docs/ }));
    fireEvent.click(screen.getByRole("button", { name: "Submit answers" }));
    expect(send).toHaveBeenCalledWith("**Which framework?**\nReact\n\n**Anything else?**\nTests, Docs");
  });

  it("can be skipped, leaving the questions as a note", () => {
    const { send } = card();
    fireEvent.click(screen.getByRole("button", { name: "Skip all" }));
    expect(screen.getByText("Skipped. Answer in your own words below.")).toBeTruthy();
    expect(send).not.toHaveBeenCalled();
  });

  it("is closed on an older reply", () => {
    card({ ...reply, status: "streaming" });
    expect(screen.queryByRole("radiogroup")).toBeNull();
    expect(screen.getByText("Which framework?")).toBeTruthy();
  });

  it("writes No preference for an unanswered question", () => {
    expect(answersText(questions as never, { picks: [new Set([1]), new Set()], other: ["", ""] })).toBe("**Which framework?**\nVue\n\n**Anything else?**\nNo preference");
  });
});
