import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { type Call, mockApi, renderPage } from "@/components/manage/test-utils";
import { TooltipProvider } from "@/components/ui/tooltip";
import { chatDraft } from "@/lib/chat/chat-draft";

const nav = vi.hoisted(() => ({ path: "/projects/p1/board", push: vi.fn() }));
const store = vi.hoisted(() => ({ newChat: vi.fn(), starter: vi.fn() }));
const toasts = vi.hoisted(() => vi.fn());
vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("next/navigation", () => ({ usePathname: () => nav.path, useRouter: () => ({ push: nav.push, replace: vi.fn() }) }));
vi.mock("sonner", () => ({ toast: toasts }));
vi.mock("@/components/chat/chat-provider", () => ({ useChatStore: () => store, NEW_CHAT_EVENT: "obleth:new-chat", OPEN_SEARCH_EVENT: "obleth:search" }));
// The rest of the shell draws nothing here; only its top bar and shortcuts are tested.
vi.mock("@/components/app-shell/sidebar", () => ({ Sidebar: () => null, projectDot: (c: string) => `bg-project-${c}` }));
vi.mock("@/components/chat/chat-dialogs", () => ({ ChatDialogs: () => null }));
vi.mock("@/components/chat/search-dialog", () => ({ SearchDialog: () => null }));
vi.mock("@/components/chat/status-bar", () => ({ StatusBar: () => null }));
vi.mock("@/components/chat/focus-toggle", () => ({ FocusToggle: () => null }));
vi.mock("@/components/app-shell/feedback-dialog", () => ({ FeedbackDialog: () => null }));

import { AppShell } from "@/components/app-shell/app-shell";
import { OPEN_EVENT as OPEN_SCRATCH_EVENT } from "@/lib/scratch";

const PROJECTS = [
  { id: "p1", name: "Batching paper", color: "blue", mine: true },
  { id: "p2", name: "Seminar site", color: "gray", mine: true },
];
const jotting = (id: string, text: string, projectId: string | null = null) => ({
  id,
  text,
  projectId,
  projectName: PROJECTS.find((p) => p.id === projectId)?.name ?? null,
  projectColor: projectId ? "blue" : null,
  createdAt: new Date(Date.now() - 5 * 60_000).toISOString(),
  updatedAt: new Date().toISOString(),
});

let pad: ReturnType<typeof jotting>[] = [];
function serve({ knowledge = true } = {}) {
  return mockApi(
    (path, options) => {
      const method = options.method || "GET";
      if (path === "/api/projects") return { projects: PROJECTS, groups: [] };
      if (path === "/api/conversations/c1/project") return { project: PROJECTS[1] };
      if (path.startsWith("/api/scratch?")) return { items: pad, more: false };
      if (path === "/api/scratch" && method === "POST") {
        const body = options.body as { text: string; projectId: string | null };
        const made = jotting(`j${pad.length + 10}`, body.text, body.projectId);
        pad = [made, ...pad];
        return { jotting: made };
      }
      if (path.startsWith("/api/scratch/") && method === "DELETE") {
        pad = pad.filter((j) => !path.endsWith(j.id));
        return {};
      }
      if (path === "/api/saved") return { item: { id: "s1" } };
      if (path.endsWith("/cards")) return { card: { id: "k1" } };
      return {};
    },
    { knowledge },
  );
}
const writes = (calls: Call[]) => calls.filter((c) => c.method !== "GET");

function show(path = "/projects/p1/board") {
  nav.path = path;
  return renderPage(
    <TooltipProvider>
      <AppShell>
        <p>The page</p>
      </AppShell>
    </TooltipProvider>,
  );
}
const icon = () => screen.getByRole("button", { name: "Scratch pad" });
const box = () => screen.getByRole("textbox", { name: "Jot something down" });
async function openPad() {
  await userEvent.click(icon());
  await waitFor(() => expect(box()).toHaveFocus());
}

beforeEach(() => {
  pad = [];
  window.localStorage.clear();
  chatDraft.reset();
  nav.push.mockClear();
  store.newChat.mockClear();
  store.starter.mockClear();
  toasts.mockClear();
});

test("the icon opens the pad with the box focused, and Escape closes it and returns focus to the icon", async () => {
  serve();
  show();
  expect(icon()).toHaveAttribute("aria-keyshortcuts", "Meta+Shift+X Control+Shift+X");
  await openPad();
  expect(screen.getByRole("dialog", { name: "Scratch pad" })).toBeInTheDocument();
  expect(box()).toHaveAttribute("placeholder", "Jot something down…");
  expect(await screen.findByText("What you save shows here, on every device you sign in on.")).toBeInTheDocument();
  await userEvent.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Scratch pad" })).not.toBeInTheDocument());
  expect(icon()).toHaveFocus();
});

test("⌘⇧X and Ctrl+Shift+X open and close the pad, and ⌘K's entry opens it", async () => {
  serve();
  show();
  fireEvent.keyDown(window, { key: "X", metaKey: true, shiftKey: true });
  await waitFor(() => expect(box()).toHaveFocus());
  fireEvent.keyDown(window, { key: "x", ctrlKey: true, shiftKey: true });
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Scratch pad" })).not.toBeInTheDocument());
  // Without Shift it is not the shortcut.
  fireEvent.keyDown(window, { key: "x", ctrlKey: true });
  expect(screen.queryByRole("dialog", { name: "Scratch pad" })).not.toBeInTheDocument();
  act(() => void window.dispatchEvent(new Event(OPEN_SCRATCH_EVENT)));
  await waitFor(() => expect(box()).toHaveFocus());
});

test("what is typed is kept when the pad closes without saving", async () => {
  serve();
  show();
  await openPad();
  await userEvent.type(box(), "Half a thought");
  await userEvent.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  await openPad();
  expect(box()).toHaveValue("Half a thought");
  expect(window.localStorage.getItem("obleth.scratch-draft.u1")).toBe("Half a thought");
});

test("the project chip follows the page, and another project or none can be picked", async () => {
  serve();
  const { unmount } = show("/projects/p1/notebook/nb-1");
  await openPad();
  expect(await screen.findByRole("button", { name: /In Batching paper/ })).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: /In Batching paper/ }));
  await userEvent.click(await screen.findByRole("menuitemradio", { name: "Not in a project" }));
  expect(screen.getByRole("button", { name: /Not in a project/ })).toBeInTheDocument();
  unmount();

  // A chat's page: the chat's project.
  show("/chat/c1");
  await openPad();
  expect(await screen.findByRole("button", { name: /In Seminar site/ })).toBeInTheDocument();
});

test("a page outside a project says so", async () => {
  serve();
  show("/library");
  await openPad();
  await waitFor(() => expect(screen.getByRole("button", { name: /Not in a project/ })).toBeInTheDocument());
});

test("Save keeps the text on the pad with the page's project, clears the box and lists it first", async () => {
  pad = [jotting("j1", "An older jotting")];
  const calls = serve();
  show();
  await openPad();
  await screen.findByRole("button", { name: /In Batching paper/ });
  await userEvent.type(box(), "Check the knee{Shift>}{Enter}{/Shift}against memory");
  await userEvent.keyboard("{Control>}{Enter}{/Control}");
  await waitFor(() => expect(box()).toHaveValue(""));
  expect(writes(calls)).toEqual([{ path: "/api/scratch", method: "POST", body: { text: "Check the knee\nagainst memory", projectId: "p1" } }]);
  const items = within(screen.getByRole("region", { name: "Your jottings" })).getAllByRole("listitem");
  expect(items[0]).toHaveTextContent("Check the knee");
  expect(items[0]).toHaveTextContent("Batching paper");
  expect(items[1]).toHaveTextContent("An older jotting");
  expect(window.localStorage.getItem("obleth.scratch-draft.u1")).toBeNull();
});

test("Save as a note goes to the saved items of the chosen project, and is offered only with that feature", async () => {
  const calls = serve();
  show();
  await openPad();
  await screen.findByRole("button", { name: /In Batching paper/ });
  await userEvent.type(box(), "Papers past 256");
  await userEvent.click(screen.getByRole("button", { name: "Save as a note" }));
  await waitFor(() => expect(box()).toHaveValue(""));
  expect(writes(calls)).toEqual([{ path: "/api/saved", method: "POST", body: { kind: "note", body: "Papers past 256", projectId: "p1" } }]);
  expect(toasts).toHaveBeenCalledWith("Saved as a note in Batching paper", expect.objectContaining({ action: expect.objectContaining({ label: "Open" }) }));
});

test("without saved items on this server there is no Save as a note", async () => {
  serve({ knowledge: false });
  show();
  await openPad();
  await userEvent.type(box(), "Something");
  expect(screen.getByRole("button", { name: "Make a card" })).toBeEnabled();
  expect(screen.queryByRole("button", { name: "Save as a note" })).not.toBeInTheDocument();
});

test("Make a card adds the first line as the title and the rest as notes to the project's board", async () => {
  const calls = serve();
  show();
  await openPad();
  await screen.findByRole("button", { name: /In Batching paper/ });
  await userEvent.type(box(), "Add a limitations paragraph{Shift>}{Enter}{/Shift}Reviewers will ask.");
  await userEvent.click(screen.getByRole("button", { name: "Make a card" }));
  await waitFor(() => expect(box()).toHaveValue(""));
  expect(writes(calls)).toEqual([{ path: "/api/projects/p1/cards", method: "POST", body: { title: "Add a limitations paragraph", notes: "Reviewers will ask." } }]);
  expect(toasts).toHaveBeenCalledWith("Added to the board of Batching paper", expect.objectContaining({ action: expect.objectContaining({ label: "Open" }) }));
  toasts.mock.calls.at(-1)![1].action.onClick();
  expect(nav.push).toHaveBeenCalledWith("/projects/p1/board");
});

test("Make a card outside a project opens the chip to pick one, then makes it there", async () => {
  const calls = serve();
  show("/");
  await openPad();
  await userEvent.type(box(), "A shorter title?");
  await userEvent.click(screen.getByRole("button", { name: "Make a card" }));
  expect(await screen.findByText("A card goes on a project’s board. Which project?")).toBeInTheDocument();
  expect(writes(calls)).toEqual([]);
  await userEvent.click(screen.getByRole("menuitemradio", { name: "Seminar site" }));
  await waitFor(() => expect(box()).toHaveValue(""));
  expect(writes(calls)).toEqual([{ path: "/api/projects/p2/cards", method: "POST", body: { title: "A shorter title?", notes: "" } }]);
});

test("Ask starts a new chat in the project with the text in its box, unsent", async () => {
  const calls = serve();
  show();
  await openPad();
  await screen.findByRole("button", { name: /In Batching paper/ });
  await userEvent.type(box(), "Why does the knee move?");
  await userEvent.click(screen.getByRole("button", { name: "Ask" }));
  expect(store.newChat).toHaveBeenCalled();
  expect(store.starter).toHaveBeenCalledWith("Why does the knee move?", false);
  expect(chatDraft.get()).toMatchObject({ project: "p1", intent: null });
  expect(nav.push).toHaveBeenCalledWith("/");
  expect(writes(calls)).toEqual([]);
  expect(window.localStorage.getItem("obleth.scratch-draft.u1")).toBeNull();
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Scratch pad" })).not.toBeInTheDocument());
});

test("Research this does the same on Home's Research tab", async () => {
  serve();
  show("/");
  await openPad();
  await userEvent.type(box(), "Prefix caching");
  await userEvent.click(screen.getByRole("button", { name: "Research this" }));
  expect(store.starter).toHaveBeenCalledWith("Prefix caching", false);
  expect(chatDraft.get()).toMatchObject({ project: null, intent: "research" });
  expect(toasts).toHaveBeenCalledWith("Your text is in a new Research chat, ready to send");
});

test("a jotting made a card leaves the pad; asked about, it stays; deleted, only after asking", async () => {
  pad = [jotting("j1", "Check the GPU clocks", "p1"), jotting("j2", "Pick the venue")];
  const calls = serve();
  show("/");
  await openPad();
  const list = await screen.findByRole("region", { name: "Your jottings" });
  await within(list).findByText("Check the GPU clocks");

  await userEvent.click(screen.getByRole("button", { name: "Actions for “Check the GPU clocks”" }));
  await userEvent.click(await screen.findByRole("menuitem", { name: "Make a card" }));
  await waitFor(() => expect(within(list).queryByText("Check the GPU clocks")).not.toBeInTheDocument());
  expect(writes(calls)).toEqual([
    { path: "/api/projects/p1/cards", method: "POST", body: { title: "Check the GPU clocks", notes: "" } },
    { path: "/api/scratch/j1", method: "DELETE", body: undefined },
  ]);

  await userEvent.click(screen.getByRole("button", { name: "Actions for “Pick the venue”" }));
  await userEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));
  const ask = await screen.findByRole("alertdialog", { name: "Delete this jotting?" });
  expect(writes(calls)).toHaveLength(2);
  await userEvent.click(within(ask).getByRole("button", { name: "Delete" }));
  await waitFor(() => expect(writes(calls)).toHaveLength(3));
  expect(writes(calls)[2]).toEqual({ path: "/api/scratch/j2", method: "DELETE", body: undefined });
  expect(toasts).toHaveBeenCalledWith("Jotting deleted");
});

test("a jotting taken into a chat stays on the pad", async () => {
  pad = [jotting("j2", "Pick the venue", "p2")];
  const calls = serve();
  show("/");
  await openPad();
  await userEvent.click(await screen.findByRole("button", { name: "Actions for “Pick the venue”" }));
  await userEvent.click(await screen.findByRole("menuitem", { name: "Ask" }));
  expect(store.starter).toHaveBeenCalledWith("Pick the venue", false);
  expect(chatDraft.get()).toMatchObject({ project: "p2" });
  expect(writes(calls)).toEqual([]);
});

test("a jotting is edited in place, its text and its project", async () => {
  pad = [jotting("j2", "Pick the venue")];
  const calls = serve();
  show("/");
  await openPad();
  await userEvent.click(await screen.findByRole("button", { name: "Actions for “Pick the venue”" }));
  await userEvent.click(await screen.findByRole("menuitem", { name: "Edit" }));
  const edit = screen.getByRole("textbox", { name: "Edit the jotting" });
  await userEvent.clear(edit);
  await userEvent.type(edit, "Pick the venue by Friday");
  const item = edit.closest("li")!;
  await userEvent.click(within(item).getByRole("button", { name: /Not in a project/ }));
  await userEvent.click(await screen.findByRole("menuitemradio", { name: "Batching paper" }));
  await userEvent.click(within(item).getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(writes(calls)).toEqual([{ path: "/api/scratch/j2", method: "PATCH", body: { text: "Pick the venue by Friday", projectId: "p1" } }]));
});

test("Escape while editing a jotting ends the edit and leaves the pad open", async () => {
  pad = [jotting("j2", "Pick the venue")];
  const calls = serve();
  show("/");
  await openPad();
  await userEvent.click(await screen.findByRole("button", { name: "Actions for “Pick the venue”" }));
  await userEvent.click(await screen.findByRole("menuitem", { name: "Edit" }));
  await userEvent.type(screen.getByRole("textbox", { name: "Edit the jotting" }), " soon");
  await userEvent.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("textbox", { name: "Edit the jotting" })).not.toBeInTheDocument());
  expect(screen.getByRole("dialog", { name: "Scratch pad" })).toBeInTheDocument();
  expect(screen.getByText("Pick the venue")).toBeInTheDocument();
  expect(writes(calls)).toEqual([]);
});

test("on a phone the pad is a sheet, and an action that leaves it forgets the draft", async () => {
  const wide = window.matchMedia;
  window.matchMedia = ((query: string) => ({ ...wide(query), matches: query === "(max-width: 760px)" })) as typeof window.matchMedia;
  try {
    serve();
    show("/");
    await openPad();
    expect(screen.getByRole("dialog", { name: "Scratch pad" })).toHaveAttribute("data-slot", "sheet-content");
    await userEvent.type(box(), "Prefix caching");
    expect(window.localStorage.getItem("obleth.scratch-draft.u1")).toBe("Prefix caching");
    await userEvent.click(screen.getByRole("button", { name: "Ask" }));
    expect(store.starter).toHaveBeenCalledWith("Prefix caching", false);
    expect(window.localStorage.getItem("obleth.scratch-draft.u1")).toBeNull();
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  } finally {
    window.matchMedia = wide;
  }
});

test("a refusal from the server shows in the pad and keeps the text", async () => {
  mockApi((path, options) => {
    if (path === "/api/projects") return { projects: PROJECTS, groups: [] };
    if (path.startsWith("/api/scratch?")) return { items: [], more: false };
    if (path === "/api/scratch" && options.method === "POST") return new Error("Your scratch pad holds 500 jottings, the most it can.");
    return {};
  });
  show("/");
  await openPad();
  await userEvent.type(box(), "One too many");
  await userEvent.click(screen.getByRole("button", { name: /^Save/ }));
  expect(await screen.findByRole("alert")).toHaveTextContent("500 jottings");
  expect(box()).toHaveValue("One too many");
});
