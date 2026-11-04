import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { fakeStore } from "./test-store";
import { WorkspacePanel } from "./workspace-panel";
import { WorkspacesPage } from "./workspaces-page";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("@/components/chat/chat-provider", () => import("@/components/workspace/test-store"));
const push = vi.fn();
vi.mock("next/navigation", () => ({ useRouter: () => ({ push, replace: vi.fn() }), usePathname: () => "/workspaces" }));
vi.mock("sonner", () => ({ toast: vi.fn() }));
vi.mock("@/components/app-shell/chrome", () => ({ usePageChrome: () => {} }));

beforeEach(() => {
  fakeStore();
  push.mockClear();
});

describe("the workspace panel", () => {
  it("lists the files and opens one", async () => {
    mockApi(
      (path) =>
        path.endsWith("/workspace/files")
          ? { entries: [{ path: "src", type: "directory" }, { path: "src/data.csv", type: "file", size: 2048 }] }
          : path.includes("/workspace/file?")
            ? { content: "name,n\nada,1\n" }
            : { workspace: { status: "running", previewPort: null, diskBytes: 2 ** 30 }, diskLimit: 10 * 2 ** 30 },
      { workspaces: true },
    );
    renderPage(<WorkspacePanel conversationId="c1" tab="files" />);
    fireEvent.click(await screen.findByText("data.csv"));
    expect(await screen.findByRole("columnheader", { name: "name" })).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: "ada" })).toBeInTheDocument();
    expect(screen.getByText("1.0 / 10 GB")).toBeInTheDocument();
    expect(screen.getByText("Running")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Download project" })).toHaveAttribute("href", "/api/conversations/c1/workspace/download");
    fireEvent.click(screen.getByRole("button", { name: "Back to files" }));
    expect(await screen.findByText("data.csv")).toBeInTheDocument();
  });

  it("offers to wake an asleep workspace", async () => {
    const calls = mockApi((_path, options) => ({ workspace: { status: options.method === "POST" ? "starting" : "stopped" } }), { workspaces: true });
    renderPage(<WorkspacePanel conversationId="c1" tab="preview" />);
    expect(await screen.findByText("The workspace is asleep.")).toBeInTheDocument();
    expect(screen.getByText("Asleep")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Wake it up" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
    expect(await screen.findByText("Starting your workspace…")).toBeInTheDocument();
  });

  it("says what went wrong when it could not start", async () => {
    mockApi(() => ({ workspace: { status: "error", errorMessage: "Quota exceeded" } }), { workspaces: true });
    renderPage(<WorkspacePanel conversationId="c1" tab="preview" />);
    expect(await screen.findByText("The workspace could not start.")).toBeInTheDocument();
    expect(screen.getByText("Quota exceeded")).toBeInTheDocument();
  });
});

describe("Compute › Workspaces", () => {
  const list = {
    workspaces: [
      { conversationId: "c1", title: "Reading group site", status: "running", previewPort: 5173, lastUsedAt: new Date().toISOString(), diskBytes: 2 * 2 ** 30 },
      { conversationId: "c2", title: "Plot", status: "running", previewPort: null, lastUsedAt: new Date().toISOString(), diskBytes: 2 * 2 ** 30 },
    ],
    diskLimit: 10 * 2 ** 30,
  };
  it("lists each workspace with its container, and deletes one after asking", async () => {
    let items = list;
    const calls = mockApi((path, options) => {
      if (options.method === "DELETE") {
        items = { ...list, workspaces: list.workspaces.slice(1) };
        return {};
      }
      return path === "/api/workspaces" ? items : {};
    }, { workspaces: true });
    renderPage(<WorkspacesPage />);
    expect(await screen.findByText("Reading group site")).toBeInTheDocument();
    expect(screen.getByText("2.0 of 10 GB used")).toBeInTheDocument();
    expect(screen.getByText(/Preview running · Used/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Delete the workspace of Reading group site" }));
    expect(await screen.findByText("Delete this workspace?")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Delete workspace" }));
    await waitFor(() => expect(calls).toContainEqual({ path: "/api/conversations/c1/workspace", method: "DELETE", body: undefined }));
    await waitFor(() => expect(screen.queryByText("Reading group site")).not.toBeInTheDocument());
  });

  it("starts building from the empty page", async () => {
    mockApi(() => ({ workspaces: [], diskLimit: 0 }), { workspaces: true });
    const store = fakeStore();
    renderPage(<WorkspacesPage />);
    fireEvent.click(await screen.findByRole("button", { name: "Start building" }));
    expect(store.starter).toHaveBeenCalledWith("Build me a site for ", true);
    expect(push).toHaveBeenCalledWith("/");
  });

  it("says when workspaces are off", async () => {
    mockApi(() => ({}), {});
    renderPage(<WorkspacesPage />);
    expect(await screen.findByText("Workspaces are not enabled.")).toBeInTheDocument();
  });
});
