// The composer's bottom row: the tool buttons never wrap, so on a phone the model
// button shrinks instead of pushing Workspace and Browser onto a second line.

import { fireEvent, screen } from "@testing-library/react";
import { vi } from "vitest";
const apiRequest = vi.hoisted(() => vi.fn());
vi.mock("@/lib/api", async (original) => ({ ...(await original<typeof import("@/lib/api")>()), api: { request: apiRequest } }));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: false, user: { username: "pat", display_name: "Pat Lee" } }), useUser: () => ({ id: "u1", role: "user" }), onBeforeSignOut: () => () => {} }));
import { ChatStore } from "@/lib/chat/store";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ChatStoreProvider } from "./chat-provider";
import { Composer } from "./composer";
import { polyfillLayout, renderWithQueries } from "./test-queries";

polyfillLayout();

test("the tool buttons stay on one line", () => {
  const store = new ChatStore({ api: { request: vi.fn().mockRejectedValue(new Error("offline")), send: vi.fn() } as never, toast: vi.fn(), go: vi.fn(), chats: { upsert: vi.fn(), remove: vi.fn(), refresh: vi.fn(), find: vi.fn() } });
  store.route = { page: "home" } as never;
  renderWithQueries(
    <TooltipProvider>
      <ChatStoreProvider store={store}>
        <Composer home />
      </ChatStoreProvider>
    </TooltipProvider>,
  );
  const tools = screen.getByRole("group", { name: "What the model can use" });
  expect(tools.className).toContain("flex-nowrap");
  expect(tools.className).toContain("shrink-0");
});

describe("the lists the box opens", () => {
  function setup(request = vi.fn().mockRejectedValue(new Error("offline"))) {
    const go = vi.fn();
    const store = new ChatStore({ api: { request, send: vi.fn() } as never, toast: vi.fn(), go, chats: { upsert: vi.fn(), remove: vi.fn(), refresh: vi.fn(), find: vi.fn() } });
    store.route = { page: "chat", id: "" };
    store.models = [
      { id: "small", name: "Small" },
      { id: "large", name: "Large" },
    ] as never;
    store.model = "small";
    renderWithQueries(
      <TooltipProvider>
        <ChatStoreProvider store={store}>
          <Composer home />
        </ChatStoreProvider>
      </TooltipProvider>,
    );
    const box = screen.getByRole("combobox", { name: "Message" }) as HTMLTextAreaElement;
    const type = (text: string) => {
      fireEvent.focus(box);
      fireEvent.change(box, { target: { value: text, selectionStart: text.length } });
    };
    return { store, go, box, type };
  }

  test("a slash lists what can be done, and Enter does the marked one", () => {
    const { store, box, type } = setup();
    expect(screen.queryByRole("listbox")).toBeNull();
    type("/");
    const list = screen.getByRole("listbox", { name: "Actions" });
    expect(box.getAttribute("aria-expanded")).toBe("true");
    expect(list.textContent).toContain("/temporary");
    expect(list.textContent).toContain("/new");
    type("/temp");
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["/temporaryMake this a temporary chat"]);
    fireEvent.keyDown(box, { key: "Enter" });
    expect(store.temporaryDraft).toBe(true);
    expect(box.value).toBe("");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  test("the arrows move the mark, and a model is one of the things to pick", () => {
    const { store, box, type } = setup();
    const select = vi.spyOn(store, "selectModel").mockResolvedValue(undefined as never);
    type("/model");
    expect(screen.getAllByRole("option").map((o) => o.getAttribute("data-menu-item"))).toEqual(["model-large"]);
    type("/");
    const first = box.getAttribute("aria-activedescendant");
    fireEvent.keyDown(box, { key: "ArrowDown" });
    expect(box.getAttribute("aria-activedescendant")).not.toBe(first);
    fireEvent.keyDown(box, { key: "ArrowUp" });
    expect(box.getAttribute("aria-activedescendant")).toBe(first);
    type("/model la");
    fireEvent.keyDown(box, { key: "Tab" });
    expect(select).toHaveBeenCalledWith("large");
  });

  test("Escape closes the list and leaves the text, and Enter then sends it", () => {
    const { store, box, type } = setup();
    const send = vi.spyOn(store, "send").mockResolvedValue(undefined as never);
    type("/new");
    fireEvent.keyDown(box, { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(box.value).toBe("/new");
    fireEvent.keyDown(box, { key: "Enter" });
    expect(send).toHaveBeenCalledWith("/new");
  });

  test("an @ lists the tools of the chat, and a pick names it in the message", async () => {
    const request = vi.fn(async (path: string) => {
      if (path === "/api/capabilities") return { features: { connections: true, clusters: true } };
      if (path === "/api/connections") return { connections: [{ id: "c1", kind: "slurm", name: "Sol", active: true, tools: [{ name: "submit_job" }] }] };
      if (path === "/api/connections/c1/status") return { state: "connected", status: { jobs: [{ id: 4821970, name: "sweep-256", state: "PENDING", partition: "gpu", reason: "Priority" }] } };
      throw new Error("offline");
    });
    apiRequest.mockImplementation(request);
    const { box, type } = setup(request);
    type("Why is @sw");
    const option = await screen.findByRole("option", { name: /sweep-256/ });
    expect(option.textContent).toContain("pending · Sol");
    expect(screen.getByRole("listbox", { name: "Point at something" })).toBeTruthy();
    fireEvent.click(option);
    expect(box.value).toBe("Why is job 4821970 (sweep-256) on Sol ");
    type("Ask @sol");
    fireEvent.keyDown(box, { key: "Enter" });
    expect(box.value).toBe("Ask the Sol cluster ");
  });
});
