import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, expect, test, vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { topicNames, type MemoryState } from "./api";
import { ago } from "@/lib/format";
import { MemoryCards } from "./memory-cards";
import { MemoryPage } from "./memory-page";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn() }), useSearchParams: () => new URLSearchParams(), usePathname: () => "/memory" }));
vi.mock("sonner", () => ({ toast: vi.fn() }));

beforeAll(() => {
  // jsdom has no layout; the graph only needs to be told a size.
  globalThis.ResizeObserver ||= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

const now = new Date().toISOString();
const state: MemoryState = {
  available: true,
  useMemory: true,
  learn: true,
  max: 500,
  topics: [
    { id: "t1", name: "Flood forecasting", kind: "project", count: 2 },
    { id: "t2", name: "PyTorch", kind: "tool", count: 1 },
  ],
  memories: [
    { id: "m1", kind: "about", content: "You are a PhD student in hydrology", origin: "learned", pinned: true, uses: 4, usedAt: now, createdAt: now, updatedAt: now, source: { conversationId: "c1", title: "Flood data" }, topics: ["t1"], projectId: null, projectName: null, projectColor: null, projectUnavailable: false },
    { id: "m2", kind: "work", content: "You train flood models with PyTorch", origin: "added", pinned: false, uses: 0, usedAt: null, createdAt: now, updatedAt: now, source: null, topics: ["t1", "t2"], projectId: null, projectName: null, projectColor: null, projectUnavailable: false },
  ],
};

function page(overrides: Partial<MemoryState> = {}) {
  const calls = mockApi((path, options) => {
    if (path === "/api/memory" && (options.method || "GET") === "GET") return { ...state, ...overrides };
    return {};
  });
  renderPage(<MemoryPage />);
  return calls;
}

test("the page draws memories and topics around you, and opens one to change it", async () => {
  const calls = page();
  expect(await screen.findByRole("img", { name: "Memory graph: 2 memories about 2 topics" })).toBeInTheDocument();
  expect(screen.getByText("2 memories · 2 topics")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Your work: You train flood models with PyTorch" }));
  const panel = screen.getByRole("heading", { name: "Your work" }).closest("section")!;
  expect(within(panel).getByText("Added by you just now")).toBeInTheDocument();
  expect(within(panel).getByText("Not used in a reply yet")).toBeInTheDocument();
  await userEvent.click(within(panel).getByRole("button", { name: "Pin" }));
  await waitFor(() => expect(calls).toContainEqual({ path: "/api/memory/m2", method: "PATCH", body: { pinned: true } }));
});

test("the switches save the person's choices", async () => {
  const calls = page();
  await userEvent.click(await screen.findByRole("switch", { name: "Learn from my chats" }));
  await waitFor(() => expect(calls).toContainEqual({ path: "/api/memory/settings", method: "PUT", body: { useMemory: true, learn: false } }));
});

test("the list groups memories under their first topic and finds by words", async () => {
  page();
  await userEvent.click(await screen.findByRole("button", { name: "List" }));
  expect(within(screen.getByRole("region", { name: "Memories by topic" })).getByRole("button", { name: /Flood forecasting/ })).toBeInTheDocument();
  await userEvent.type(screen.getByLabelText("Find a memory or topic"), "hydrology");
  const list = screen.getByRole("region", { name: "Memories by topic" });
  expect(within(list).getByText("You are a PhD student in hydrology")).toBeInTheDocument();
  expect(within(list).queryByText("You train flood models with PyTorch")).toBeNull();
});

test("a server with memory off and nothing remembered shows only the off state", async () => {
  page({ available: false, memories: [], topics: [] });
  expect(await screen.findByText("Memory is turned off on this server")).toBeInTheDocument();
  expect(screen.getByText("Ask an administrator to turn it on.")).toBeInTheDocument();
  expect(screen.getAllByRole("heading", { level: 2 })).toHaveLength(1);
  expect(screen.queryByText("Nothing remembered yet")).toBeNull();
  expect(screen.queryByLabelText("Find a memory or topic")).toBeNull();
  expect(screen.queryByRole("button", { name: "Add memory" })).toBeNull();
});

test("a reply shows what it remembered and what it learned, with Undo", async () => {
  const calls = mockApi(() => ({}));
  renderPage(
    <MemoryCards
      message={{
        activity: [
          { kind: "recalled", items: [{ id: "m1", text: "You are a PhD student in hydrology", pinned: true }] },
          { kind: "learned", items: [{ id: "m3", text: "You teach CHM 501", change: "added" }] },
        ],
      }}
    />,
  );
  await userEvent.click(screen.getByRole("button", { name: /Remembered 1 thing about you/ }));
  expect(screen.getByRole("link", { name: "You are a PhD student in hydrology" })).toHaveAttribute("href", "/memory?m=m1");
  await userEvent.click(screen.getByRole("button", { name: "Undo" }));
  await waitFor(() => expect(calls).toContainEqual({ path: "/api/memory/m3", method: "DELETE", body: undefined }));
  expect(await screen.findByText("Forgot “You teach CHM 501”")).toBeInTheDocument();
});

test("helpers", () => {
  expect(topicNames(" CHM 501, teaching ,chm 501, a, b")).toEqual(["CHM 501", "teaching", "a"]);
  expect(ago(new Date(Date.now() - 5 * 60_000).toISOString())).toBe("5 min ago");
  expect(ago(null)).toBe("");
});

const LONG = "Quick Verifiable RAG: maximize hallucination detection F1 and FactScore with minimal latency";
const projects = [
  { id: "p1", name: LONG, color: "green", mine: true },
  { id: "p2", name: "Flood forecasting", color: "blue", mine: true },
];
const scoped: MemoryState = {
  ...state,
  memories: [
    ...state.memories,
    { ...state.memories[1]!, id: "m3", kind: "work", content: "You evaluate on the 2024 held-out split", topics: [], projectId: "p1", projectName: LONG, projectColor: "green" },
    { ...state.memories[1]!, id: "m4", kind: "work", content: "You log runs to the lab tracker", topics: [], projectUnavailable: true },
  ],
};

function scopedPage() {
  const calls = mockApi((path, options) => {
    if (path === "/api/memory" && (options.method || "GET") === "GET") return scoped;
    if (path === "/api/projects") return { projects };
    return {};
  });
  renderPage(<MemoryPage />);
  return calls;
}

test("the list shows where each memory is used, and the filter shows one project's", async () => {
  scopedPage();
  await userEvent.click(await screen.findByRole("button", { name: "List" }));
  const list = screen.getByRole("region", { name: "Memories by topic" });
  const row = within(list).getByRole("button", { name: /You evaluate on the 2024 held-out split/ });
  // Beside the memory, or under it on a phone; truncated either way.
  for (const label of within(row).getAllByText(LONG)) expect(label).toHaveClass("truncate");
  expect(within(within(list).getByRole("button", { name: /You train flood models with PyTorch/ })).getAllByText("Everywhere").length).toBeGreaterThan(0);
  expect(within(within(list).getByRole("button", { name: /You log runs to the lab tracker/ })).getAllByText("A project you can no longer use").length).toBeGreaterThan(0);
  await userEvent.click(screen.getByRole("button", { name: "Show memories used in: All memories" }));
  expect(screen.getByRole("menuitemradio", { name: /^Everywhere\s*2$/ })).toBeInTheDocument();
  expect(screen.getByRole("menuitemradio", { name: /^A project you can no longer use\s*1$/ })).toBeInTheDocument();
  // Only projects that have memories are offered.
  expect(screen.queryByRole("menuitemradio", { name: /Flood forecasting/ })).toBeNull();
  await userEvent.click(screen.getByRole("menuitemradio", { name: new RegExp(`^${LONG}\\s*1$`) }));
  expect(screen.getByText("1 of 4 memories · 0 topics")).toBeInTheDocument();
  const filtered = screen.getByRole("region", { name: "Memories by topic" });
  expect(within(filtered).getByText("You evaluate on the 2024 held-out split")).toBeInTheDocument();
  expect(within(filtered).queryByText("You train flood models with PyTorch")).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: `Show memories used in: ${LONG}` }));
  await userEvent.click(screen.getByRole("menuitemradio", { name: /^Everywhere\s*2$/ }));
  const general = screen.getByRole("region", { name: "Memories by topic" });
  expect(within(general).getByText("You train flood models with PyTorch")).toBeInTheDocument();
  expect(within(general).queryByText("You evaluate on the 2024 held-out split")).toBeNull();
  expect(within(general).queryByText("You log runs to the lab tracker")).toBeNull();
});

test("Move takes a memory to a project or to everywhere", async () => {
  const calls = scopedPage();
  await userEvent.click(await screen.findByRole("button", { name: "List" }));
  await userEvent.click(within(screen.getByRole("region", { name: "Memories by topic" })).getByRole("button", { name: /You train flood models with PyTorch/ }));
  const panel = screen.getByRole("heading", { name: "Your work" }).closest("section")!;
  expect(within(panel).getByText("Every chat of yours can get it.")).toBeInTheDocument();
  await userEvent.click(within(panel).getByRole("button", { name: "Move to" }));
  expect(screen.getByRole("menuitemradio", { name: /Everywhere/ })).toHaveAttribute("aria-checked", "true");
  await userEvent.click(screen.getByRole("menuitemradio", { name: LONG }));
  await waitFor(() => expect(calls).toContainEqual({ path: "/api/memory/m2", method: "PATCH", body: { projectId: "p1" } }));
  // One in a project the person left can be moved out, to everywhere.
  await userEvent.click(within(screen.getByRole("region", { name: "Memories by topic" })).getByRole("button", { name: /You log runs to the lab tracker/ }));
  const left = screen.getByRole("heading", { name: "Your work" }).closest("section")!;
  expect(within(left).getByText("No chat gets it until you move it.")).toBeInTheDocument();
  await userEvent.click(within(left).getByRole("button", { name: "Move to" }));
  await userEvent.click(screen.getByRole("menuitemradio", { name: /Everywhere/ }));
  await waitFor(() => expect(calls).toContainEqual({ path: "/api/memory/m4", method: "PATCH", body: { projectId: null } }));
});

test("a memory added by hand can be for one project", async () => {
  const calls = scopedPage();
  await userEvent.click(await screen.findByRole("button", { name: "Add memory" }));
  await userEvent.type(screen.getByLabelText("Memory"), "You run the eval on four GPUs");
  await userEvent.click(screen.getByRole("button", { name: "Used in: Everywhere" }));
  await userEvent.click(screen.getByRole("menuitemradio", { name: "Flood forecasting" }));
  expect(screen.getByText("Only your chats in Flood forecasting get it.")).toBeInTheDocument();
  await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Add memory" }));
  await waitFor(() => expect(calls).toContainEqual(expect.objectContaining({ path: "/api/memory", method: "POST", body: expect.objectContaining({ content: "You run the eval on four GPUs", projectId: "p2" }) })));
});

test("a memory learned in a project chat is for this project or everywhere, in one click", async () => {
  const memory = { ...state.memories[1]!, id: "m5", content: "You evaluate on the 2024 held-out split", projectId: "p1" as string | null };
  const calls = mockApi((path, options) => {
    if (path === "/api/memory" && (options.method || "GET") === "GET") return { ...state, memories: [{ ...memory }] };
    if (path === "/api/memory/m5" && options.method === "PATCH") memory.projectId = (options.body as { projectId: string | null }).projectId;
    if (path === "/api/projects") return { projects };
    return {};
  });
  renderPage(
    <MemoryCards
      message={{
        activity: [
          {
            kind: "learned",
            items: [
              { id: "m5", text: "You evaluate on the 2024 held-out split", change: "added", projectId: "p1", chatProjectId: "p1" },
              { id: "m6", text: "You teach CHM 501", change: "updated" },
            ],
          },
        ],
      }}
    />,
  );
  const choices = await screen.findByRole("radiogroup", { name: "Where Chat uses this memory" });
  expect(screen.getAllByRole("radiogroup")).toHaveLength(1);
  expect(within(choices).getByRole("radio", { name: "For this project" })).toHaveAttribute("aria-checked", "true");
  expect(within(choices).getByRole("radio", { name: "Everywhere" })).toHaveAttribute("aria-checked", "false");
  expect(within(choices).getByText(`Only your chats in ${LONG} get it.`)).toHaveClass("truncate");
  await userEvent.click(within(choices).getByRole("radio", { name: "Everywhere" }));
  await waitFor(() => expect(calls).toContainEqual({ path: "/api/memory/m5", method: "PATCH", body: { projectId: null } }));
  await waitFor(() => expect(within(choices).getByRole("radio", { name: "Everywhere" })).toHaveAttribute("aria-checked", "true"));
  expect(within(choices).getByText("Every chat of yours can get it.")).toBeInTheDocument();
  await userEvent.click(within(choices).getByRole("radio", { name: "For this project" }));
  await waitFor(() => expect(calls).toContainEqual({ path: "/api/memory/m5", method: "PATCH", body: { projectId: "p1" } }));
});

test("outside a project a learned memory offers no choice", async () => {
  mockApi((path) => (path === "/api/projects" ? { projects } : {}));
  renderPage(<MemoryCards message={{ activity: [{ kind: "learned", items: [{ id: "m7", text: "You prefer tables", change: "added" }] }] }} />);
  expect(await screen.findByRole("button", { name: "Undo" })).toBeInTheDocument();
  expect(screen.queryByRole("radiogroup")).toBeNull();
});
