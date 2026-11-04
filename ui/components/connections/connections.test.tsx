import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { ToolsButton, applyToolsDraft, resetToolsDraft, startAutoDraft } from "./chat-tools";
import { addedMessage, connectionBody, type Connection } from "./connections";
import { McpServersPage } from "./mcp-page";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("next/navigation", () => ({ usePathname: () => "/settings/mcp", useRouter: () => ({ push: vi.fn() }) }));

const server = (more: Partial<Connection> = {}): Connection => ({
  id: "c1",
  kind: "mcp",
  name: "Research tools",
  url: "https://example.com/mcp",
  shared: false,
  enabled: true,
  active: true,
  canManage: true,
  confirmActions: true,
  tools: [
    { name: "search", readOnly: true },
    { name: "write", readOnly: false, description: "Writes a file" },
  ],
  ...more,
});
const form = (values: Record<string, string>) => {
  const f = new FormData();
  for (const [k, v] of Object.entries(values)) f.set(k, v);
  return f;
};

test("a new shared server can ask each person for their own token", () => {
  const body = connectionBody(form({ "connection-name": " Docs ", "connection-url": "https://d/mcp", "connection-token": "t", "connection-confirm": "on", "connection-shared": "on", "connection-personal-tokens": "on" }), null, "mcp", { admin: true, jobOutput: false });
  expect(body).toEqual({ name: "Docs", url: "https://d/mcp", confirmActions: true, kind: "mcp", token: "t", shared: true, options: { personalTokens: true } });
  expect(addedMessage(server({ personalTokens: true, shared: true }))).toEqual({ message: "Research tools shared. Add your own token to use it.", askToken: true });
  expect(addedMessage(server()).message).toBe("Research tools connected with 2 tools");
});

test("a shared cluster takes no token, and a personal one sends its user only when it changed", () => {
  const shared = connectionBody(form({ "connection-name": "HPC", "connection-url": "https://s", "connection-token": "x", "connection-shared": "on", "connection-api-version": "" }), null, "slurm", { admin: true, jobOutput: true });
  expect(shared).toEqual({ name: "HPC", url: "https://s", confirmActions: false, kind: "slurm", shared: true, options: { apiVersion: "", returnOutput: false } });
  const c = server({ kind: "slurm", user: "jdoe", apiVersion: "" });
  expect(connectionBody(form({ "connection-name": "HPC", "connection-url": "https://s", "connection-user": "jdoe", "connection-api-version": "" }), c, "slurm", { admin: false, jobOutput: false })).toEqual({
    name: "HPC",
    url: "https://s",
    confirmActions: false,
  });
  expect(connectionBody(form({ "connection-name": "HPC", "connection-url": "https://s", "connection-user": "ana", "connection-api-version": "" }), c, "slurm", { admin: false, jobOutput: false }).options).toEqual({ user: "ana", apiVersion: "" });
});

test("MCP servers lists yours and shared ones, and turns one off", async () => {
  const calls = mockApi(
    (path, options) => {
      if (path === "/api/connections") return { connections: [server(), server({ id: "c2", name: "Campus wiki", shared: true, canManage: false }), server({ id: "k1", kind: "kubernetes", name: "Lab" })] };
      if (options.method === "PATCH") return server({ active: false });
      return {};
    },
    { connections: true },
  );
  renderPage(<McpServersPage />, { user: { role: "user" } });
  expect(await screen.findByText("Research tools")).toBeInTheDocument();
  expect(screen.getByText("Campus wiki")).toBeInTheDocument();
  expect(screen.queryByText("Lab")).toBeNull();
  expect(screen.getAllByText(/1 can make changes/).length).toBe(2);
  await userEvent.click(screen.getByRole("button", { name: "Stop using Research tools in my chats" }));
  await waitFor(() => expect(calls.find((c) => c.method === "PATCH")).toEqual({ path: "/api/connections/c1", method: "PATCH", body: { enabled: false } }));
  expect(await screen.findByRole("button", { name: "Use Research tools in my chats" })).toBeInTheDocument();
  await userEvent.click(screen.getAllByRole("button", { name: "2 tools" })[0]!);
  expect(screen.getByText("Asks first")).toBeInTheDocument();
});

test("removing a server is confirmed and says what is deleted", async () => {
  const calls = mockApi(() => ({ connections: [server({ personalTokens: true })] }), { connections: true });
  renderPage(<McpServersPage />);
  await userEvent.click(await screen.findByRole("button", { name: "Remove" }));
  expect(screen.getByText(/Everyone’s saved tokens for it are deleted/)).toBeInTheDocument();
  const dialog = screen.getByRole("alertdialog");
  await userEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
  await waitFor(() => expect(calls.some((c) => c.method === "DELETE" && c.path === "/api/connections/c1")).toBe(true));
});

test("a new chat keeps its tool choices until it exists, then stores them", async () => {
  resetToolsDraft();
  const calls = mockApi(() => ({ connections: [server(), server({ id: "c2", name: "Wiki", shared: true })] }), { connections: true, autoMode: true });
  renderPage(<ToolsButton chatId={null} />);
  await userEvent.click(await screen.findByRole("button", { name: "Tools: 2 of 2 on" }));
  await userEvent.click(screen.getByRole("switch", { name: "Use Wiki in this chat" }));
  await userEvent.click(screen.getByRole("switch", { name: "Auto mode" }));
  expect(screen.getByRole("button", { name: "Tools: 1 of 2 on, Auto mode", hidden: true })).toBeInTheDocument();
  expect(calls.some((c) => c.method === "PATCH")).toBe(false);
  await applyToolsDraft("chat9");
  expect(calls.filter((c) => c.method === "PATCH")).toEqual([
    { path: "/api/conversations/chat9/connections", method: "PATCH", body: { auto: true } },
    { path: "/api/conversations/chat9/connections", method: "PATCH", body: { connectionId: "c2", on: false } },
  ]);
  startAutoDraft();
  resetToolsDraft();
});
