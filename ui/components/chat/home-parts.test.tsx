// Home's intent tabs and "Running now".

import { fireEvent, screen } from "@testing-library/react";
import { vi } from "vitest";
const apiRequest = vi.hoisted(() => vi.fn());
vi.mock("@/lib/api", async (original) => ({ ...(await original<typeof import("@/lib/api")>()), api: { request: apiRequest } }));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: false, user: { username: "pat", display_name: "Pat Lee" } }), useUser: () => ({ id: "u1", role: "user" }), onBeforeSignOut: () => () => {} }));
vi.mock("next/link", () => ({ default: ({ href, children, ...rest }: { href: string; children: React.ReactNode }) => <a href={href} {...rest}>{children}</a> }));
import { intents } from "@/lib/chat/home";
import { ChatStore } from "@/lib/chat/store";
import { jobsText } from "./status-bar";
import { ChatStoreProvider } from "./chat-provider";
import { IntentTabs, RunningNow } from "./home-parts";
import { renderWithQueries } from "./test-queries";

test("the tabs mark the chosen intent, say what it sets up and report a choice", () => {
  const offered = intents({ workspaces: true, clusters: [{ name: "Sol", confirmActions: true }] });
  const onChange = vi.fn();
  renderWithQueries(<IntentTabs intents={offered} value="build" onChange={onChange} />);
  expect(screen.getAllByRole("radio").map((r) => [r.textContent, r.getAttribute("aria-checked")])).toEqual([
    ["Ask", "false"],
    ["Build", "true"],
    ["Research", "false"],
    ["Run on a cluster", "false"],
  ]);
  expect(screen.getByText("A workspace starts with your first message, with a live preview beside the chat.")).toBeTruthy();
  fireEvent.click(screen.getByRole("radio", { name: "Run on a cluster" }));
  expect(onChange).toHaveBeenCalledWith("cluster");
});

describe("Running now", () => {
  const store = () => new ChatStore({ api: { request: vi.fn().mockRejectedValue(new Error("offline")), send: vi.fn() } as never, toast: vi.fn(), go: vi.fn(), chats: { upsert: vi.fn(), remove: vi.fn(), refresh: vi.fn(), find: vi.fn() } });
  const show = () =>
    renderWithQueries(
      <ChatStoreProvider store={store()}>
        <RunningNow />
      </ChatStoreProvider>,
    );

  test("lists the chat that waits, the chat at work, the jobs and the workspace", async () => {
    apiRequest.mockImplementation(async (path: string) => {
      if (path === "/api/capabilities") return { features: { connections: true, clusters: true, workspaces: true } };
      if (path === "/api/conversations")
        return {
          conversations: [
            { id: "a", title: "Cancel the old run", model: "m", updatedAt: "", working: true, waiting: true, waitingTool: "cancel_job" },
            { id: "b", title: "Decode sweep", model: "m", updatedAt: "", working: true },
            { id: "c", title: "Quiet", model: "m", updatedAt: "" },
          ],
        };
      if (path === "/api/connections") return { connections: [{ id: "c1", kind: "slurm", name: "Sol", active: true, tools: [] }] };
      if (path === "/api/connections/c1/status") return { state: "connected", status: { jobs: [{ id: 1, name: "sweep-32", state: "RUNNING" }, { id: 2, name: "sweep-64", state: "PENDING" }] } };
      if (path === "/api/workspaces") return { workspaces: [{ conversationId: "w", status: "running", title: "Signup site", previewPort: 3000 }] };
      throw new Error(`unexpected ${path}`);
    });
    show();
    expect((await screen.findByText("sweep-32, sweep-64")).closest("a")?.getAttribute("href")).toBe("/clusters");
    await screen.findByText("Signup site");
    const cards = [...document.querySelectorAll("[data-running]")].map((c) => [c.getAttribute("data-running"), c.getAttribute("href")]);
    expect(cards).toEqual([
      ["waiting", "/chat/a"],
      ["working", "/chat/b"],
      ["jobs", "/clusters"],
      ["workspace", "/chat/w"],
    ]);
    expect(screen.getByText("Asks to run cancel_job")).toBeTruthy();
    expect(screen.getByText("1 running · 1 waiting")).toBeTruthy();
    expect(screen.queryByText("Quiet")).toBeNull();
  });

  test("shows nothing when nothing is alive", async () => {
    apiRequest.mockImplementation(async (path: string) => {
      if (path === "/api/capabilities") return { features: {} };
      if (path === "/api/conversations") return { conversations: [{ id: "c", title: "Quiet", model: "m", updatedAt: "" }] };
      throw new Error(`unexpected ${path}`);
    });
    const { container } = show();
    await vi.waitFor(() => expect(apiRequest).toHaveBeenCalledWith("/api/conversations"));
    expect(container.querySelector("[data-running-now]")).toBeNull();
  });
});

test("the status line counts jobs across clusters", () => {
  expect(jobsText([])).toBe("");
  expect(jobsText([{ running: 0, pending: 0 }])).toBe("");
  expect(jobsText([{ running: 2, pending: 0 }, { running: 1, pending: 3 }])).toBe("3 running · 3 waiting");
  expect(jobsText([{ running: 0, pending: 1 }])).toBe("1 waiting");
});
