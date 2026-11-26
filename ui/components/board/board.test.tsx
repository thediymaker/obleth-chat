import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { fakeStore } from "@/components/workspace/test-store";
import { applyChatDraft, chatDraft } from "@/lib/chat/chat-draft";
import { api } from "@/lib/api";
import type { Card } from "./api";
import { ProjectBoard } from "./board-page";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("@/components/chat/chat-provider", () => import("@/components/workspace/test-store"));
const push = vi.fn();
vi.mock("next/navigation", () => ({ useRouter: () => ({ push, replace: vi.fn() }), usePathname: () => "/projects/p1/board" }));
vi.mock("sonner", () => ({ toast: vi.fn() }));
vi.mock("@/components/projects/project-frame", () => ({ useProjectFrame: () => ({ detail: { project: { id: "p1", name: "Lantern Keep", mine: true } } }) }));

const card = (id: string, title: string, rest: Partial<Card> = {}): Card => ({
  id,
  projectId: "p1",
  title,
  notes: "",
  kind: "",
  status: "idea",
  position: 0,
  madeBy: "person",
  maker: { id: "u1", name: "Ana" },
  worker: null,
  workerUser: null,
  chat: null,
  sourceChat: null,
  steps: [],
  progress: "",
  question: "",
  options: [],
  answer: "",
  answeredAt: null,
  links: [],
  createdAt: "2026-11-20T10:00:00Z",
  updatedAt: "2026-11-20T10:00:00Z",
  startedAt: null,
  doneAt: null,
  ...rest,
});

const board = {
  doneCount: 7,
  cards: [
    card("c-level", "Design level 1", {
      status: "doing",
      kind: "Level",
      worker: "assistant",
      workerUser: { id: "u1", name: "Ana" },
      chat: { id: "chat-1", mine: true, title: "Levels", working: true, waiting: false },
      steps: [
        { text: "Gate", state: "done" },
        { text: "Armory", state: "now" },
        { text: "Tower", state: "todo" },
      ],
      progress: "Laying out the armory",
    }),
    card("c-menu", "Update the menu's look", { status: "doing", kind: "Look and feel", worker: "person", workerUser: { id: "u1", name: "Ana" }, startedAt: new Date(Date.now() - 20 * 60_000).toISOString() }),
    card("c-suggest", "Add a checkpoint after the bridge", { status: "suggested", madeBy: "assistant", maker: { id: "u1", name: "Ana" } }),
    card("c-map", "A map screen", { kind: "Feature" }),
    card("c-play", "Play level 1 and list what breaks", { status: "next", position: 0, madeBy: "assistant" }),
    card("c-items", "Write the item descriptions", { status: "next", position: 1 }),
    card("c-save", "Save and load", { status: "next", position: 2 }),
    card("c-gold", "Quest 3 pays out", { status: "waiting", question: "Gold or a key?", options: ["Gold", "A key"] }),
    ...[1, 2, 3, 4, 5].map((n) => card(`c-done-${n}`, `Done card ${n}`, { status: "done", doneAt: `2026-11-${10 + n}T10:00:00Z` })),
  ],
};

let store: ReturnType<typeof fakeStore>;
beforeEach(() => {
  store = fakeStore();
  push.mockClear();
  chatDraft.reset();
});

const column = (name: string) => screen.getByRole("region", { name });
const menu = async (title: string) => {
  await userEvent.click(await screen.findByRole("button", { name: `Actions for ${title}` }));
  return screen.findByRole("menu");
};

describe("the Board tab", () => {
  it("shows Now with the live card in the accent, then Ideas, Up next in order, Needs you and Done", async () => {
    mockApi((path) => (path === "/api/projects/p1/cards" ? board : {}));
    renderPage(<ProjectBoard />, { user: { role: "user" } });
    const now = await screen.findByRole("region", { name: /Now/ });
    expect(within(now).getByText("The assistant is on 1 card and you are on 1 card.")).toBeInTheDocument();
    const live = within(now).getByRole("article", { name: "Design level 1" });
    expect(live).toHaveAttribute("data-alive", "true");
    expect(live).toHaveClass("border-signal");
    expect(within(live).getByText("Laying out the armory")).toHaveClass("text-signal");
    expect(within(live).getByText("Armory")).toBeInTheDocument();
    // The person's card is neutral.
    const mine = within(now).getByRole("article", { name: "Update the menu's look" });
    expect(mine).not.toHaveAttribute("data-alive");
    expect(mine.className).not.toMatch(/signal/);
    expect(within(mine).getByText("You started this 20 min ago")).toBeInTheDocument();

    const ideas = column("Ideas");
    expect(within(ideas).getByText("Suggested by the assistant")).toBeInTheDocument();
    expect(within(ideas).getByRole("article", { name: "Add a checkpoint after the bridge" })).toHaveClass("border-dashed");
    const next = column("Up next");
    expect(within(next).getAllByRole("article").map((a) => a.getAttribute("aria-label"))).toEqual(["Play level 1 and list what breaks", "Write the item descriptions", "Save and load"]);
    expect(within(column("Needs you")).getByText("Gold or a key?")).toBeInTheDocument();
    expect(within(column("Done")).getAllByRole("article")).toHaveLength(5);
    expect(within(column("Done")).getByRole("button", { name: "2 more" })).toBeInTheDocument();
  });

  it("adds an idea on Enter, answers a question, keeps and dismisses a suggestion", async () => {
    const calls = mockApi((path) => (path === "/api/projects/p1/cards" ? board : { card: board.cards[0] }));
    renderPage(<ProjectBoard />, { user: { role: "user" } });
    const input = await screen.findByRole("textbox", { name: "Add an idea" });
    await userEvent.type(input, "Footsteps that change with the floor{Enter}");
    await waitFor(() => expect(calls).toContainEqual({ path: "/api/projects/p1/cards", method: "POST", body: { title: "Footsteps that change with the floor", status: "idea" } }));
    await userEvent.click(within(column("Needs you")).getByRole("button", { name: "A key" }));
    await waitFor(() => expect(calls).toContainEqual({ path: "/api/projects/p1/cards/c-gold", method: "PATCH", body: { answer: "A key" } }));
    await userEvent.click(within(column("Ideas")).getByRole("button", { name: "Keep" }));
    await waitFor(() => expect(calls).toContainEqual({ path: "/api/projects/p1/cards/c-suggest", method: "PATCH", body: { status: "idea" } }));
    await userEvent.click(within(column("Ideas")).getByRole("button", { name: "Dismiss" }));
    await waitFor(() => expect(calls).toContainEqual({ path: "/api/projects/p1/cards/c-suggest", method: "PATCH", body: { status: "dismissed" } }));
  });

  it("reorders Up next and moves cards from a card's menu", async () => {
    const calls = mockApi((path) => (path === "/api/projects/p1/cards" ? board : { card: board.cards[0] }));
    renderPage(<ProjectBoard />, { user: { role: "user" } });
    await screen.findByRole("region", { name: "Up next" });
    let open = await menu("Write the item descriptions");
    await userEvent.click(within(open).getByRole("menuitem", { name: "Move up" }));
    await waitFor(() => expect(calls).toContainEqual({ path: "/api/projects/p1/cards/c-items", method: "PATCH", body: { status: "next", position: 0 } }));
    // The first card can only move down.
    open = await menu("Play level 1 and list what breaks");
    expect(within(open).queryByRole("menuitem", { name: "Move up" })).not.toBeInTheDocument();
    await userEvent.click(within(open).getByRole("menuitem", { name: "Done" }));
    await waitFor(() => expect(calls).toContainEqual({ path: "/api/projects/p1/cards/c-play", method: "PATCH", body: { status: "done" } }));
    open = await menu("A map screen");
    await userEvent.click(within(open).getByRole("menuitem", { name: /I’m on it/ }));
    await waitFor(() => expect(calls).toContainEqual({ path: "/api/projects/p1/cards/c-map", method: "PATCH", body: { worker: "me" } }));
  });

  it("moves a card by dragging it onto another column or above another card", async () => {
    const calls = mockApi((path) => (path === "/api/projects/p1/cards" ? board : { card: board.cards[0] }));
    renderPage(<ProjectBoard />, { user: { role: "user" } });
    const idea = await screen.findByRole("article", { name: "A map screen" });
    const dataTransfer = { setData: vi.fn(), effectAllowed: "" };
    fireEvent.dragStart(idea, { dataTransfer });
    const target = within(column("Up next")).getByRole("article", { name: "Write the item descriptions" });
    fireEvent.dragOver(target, { dataTransfer });
    fireEvent.drop(target, { dataTransfer });
    await waitFor(() => expect(calls).toContainEqual({ path: "/api/projects/p1/cards/c-map", method: "PATCH", body: { status: "next", position: 1 } }));
    fireEvent.dragStart(screen.getByRole("article", { name: "Save and load" }), { dataTransfer });
    fireEvent.drop(column("Needs you"), { dataTransfer });
    await waitFor(() => expect(calls).toContainEqual({ path: "/api/projects/p1/cards/c-save", method: "PATCH", body: { status: "waiting" } }));
  });

  it("starts a card with the assistant in a new chat in the project", async () => {
    mockApi((path) => (path === "/api/projects/p1/cards" ? board : {}));
    renderPage(<ProjectBoard />, { user: { role: "user" } });
    const open = await menu("A map screen");
    await userEvent.click(within(open).getByRole("menuitem", { name: /Start with the assistant/ }));
    expect(store.newChat).toHaveBeenCalled();
    expect(store.starter).toHaveBeenCalledWith("Let’s work on the card “A map screen”.", false);
    expect(chatDraft.get()).toMatchObject({ project: "p1", card: { id: "c-map", title: "A map screen" } });
    expect(push).toHaveBeenCalledWith("/");
  });

  it("asks before deleting a card", async () => {
    const calls = mockApi((path, options) => (path === "/api/projects/p1/cards" ? board : options.method === "DELETE" ? {} : {}));
    renderPage(<ProjectBoard />, { user: { role: "user" } });
    const open = await menu("A map screen");
    await userEvent.click(within(open).getByRole("menuitem", { name: /Delete/ }));
    const dialog = await screen.findByRole("alertdialog");
    expect(calls.some((c) => c.method === "DELETE")).toBe(false);
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete card" }));
    await waitFor(() => expect(calls).toContainEqual({ path: "/api/projects/p1/cards/c-map", method: "DELETE", body: undefined }));
  });

  it("opens a card with its steps and history, and saves a step's change at once", async () => {
    const level = board.cards[0]!;
    const history = [
      { action: "made", actor: "assistant", actorId: "u1", actorName: "Ana", from: null, to: "idea", detail: "", at: "2026-11-20T10:00:00Z" },
      { action: "moved", actor: "person", actorId: "u2", actorName: "Ben", from: "idea", to: "next", detail: "", at: "2026-11-20T11:00:00Z" },
      { action: "picked", actor: "assistant", actorId: "u1", actorName: "Ana", from: "next", to: "doing", detail: "", at: "2026-11-21T09:00:00Z" },
    ];
    const calls = mockApi((path) => (path === "/api/projects/p1/cards" ? board : path === "/api/projects/p1/cards/c-level" ? { card: level, history } : { card: level }));
    renderPage(<ProjectBoard />, { user: { role: "user" } });
    const now = await screen.findByRole("region", { name: /Now/ });
    await userEvent.click(within(within(now).getByRole("article", { name: "Design level 1" })).getByRole("button", { name: "Open" }));
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByRole("heading", { name: "Design level 1" })).toBeInTheDocument();
    expect(within(dialog).getByText("In progress")).toBeInTheDocument();
    expect(within(dialog).getByText("The assistant made it in Ideas.")).toBeInTheDocument();
    expect(within(dialog).getByText("Ben moved it from Ideas to Up next.")).toBeInTheDocument();
    expect(within(dialog).getByText("The assistant picked it up.")).toBeInTheDocument();
    expect(within(dialog).getByRole("link", { name: "Levels" })).toHaveAttribute("href", "/chat/chat-1");
    await userEvent.click(within(dialog).getByRole("button", { name: "Step 3, to do: mark it being worked on" }));
    await waitFor(() =>
      expect(calls).toContainEqual({
        path: "/api/projects/p1/cards/c-level",
        method: "PATCH",
        body: {
          steps: [
            { text: "Gate", state: "done" },
            { text: "Armory", state: "now" },
            { text: "Tower", state: "now" },
          ],
        },
      }),
    );
    // Editing the title shows Save; saving sends title, kind and notes together.
    const title = within(dialog).getByRole("textbox", { name: "Title" });
    await userEvent.clear(title);
    await userEvent.type(title, "Design level one");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(calls).toContainEqual({ path: "/api/projects/p1/cards/c-level", method: "PATCH", body: { title: "Design level one", kind: "Level", notes: "" } }));
  });

  it("explains the board when it is empty", async () => {
    mockApi((path) => (path === "/api/projects/p1/cards" ? { cards: [], doneCount: 0 } : {}));
    renderPage(<ProjectBoard />, { user: { role: "user" } });
    expect(await screen.findByRole("heading", { name: "Nothing on the board yet" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Add an idea" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /New card/ }));
    expect(await screen.findByRole("dialog", { name: "New card" })).toBeInTheDocument();
  });
});

describe("a chat started for a card", () => {
  it("goes into the project first, then is linked to the card", async () => {
    const order: string[] = [];
    vi.spyOn(api, "request").mockImplementation((async (path: string, options: { method?: string; body?: unknown } = {}) => {
      order.push(`${options.method} ${path} ${JSON.stringify(options.body)}`);
      return {};
    }) as typeof api.request);
    chatDraft.setProject("p1");
    chatDraft.setCard({ id: "c-map", title: "A map screen" });
    expect(await applyChatDraft("chat-9")).toEqual([]);
    expect(order).toEqual(['PUT /api/conversations/chat-9/project {"projectId":"p1"}', 'PATCH /api/projects/p1/cards/c-map {"chatId":"chat-9"}']);
    expect(chatDraft.get().card).toBeNull();
    // Another project drops the card.
    chatDraft.setProject("p1");
    chatDraft.setCard({ id: "c-map", title: "A map screen" });
    chatDraft.setProject("p2");
    expect(chatDraft.get().card).toBeNull();
  });
});
