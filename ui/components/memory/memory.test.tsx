import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, expect, test, vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { ago, topicNames, type MemoryState } from "./api";
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
    { id: "m1", kind: "about", content: "You are a PhD student in hydrology", origin: "learned", pinned: true, uses: 4, usedAt: now, createdAt: now, updatedAt: now, source: { conversationId: "c1", title: "Flood data" }, topics: ["t1"] },
    { id: "m2", kind: "work", content: "You train flood models with PyTorch", origin: "added", pinned: false, uses: 0, usedAt: null, createdAt: now, updatedAt: now, source: null, topics: ["t1", "t2"] },
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

test("a server with memory off says so", async () => {
  page({ available: false, memories: [], topics: [] });
  expect(await screen.findByText("Memory is not turned on for this server.")).toBeInTheDocument();
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
