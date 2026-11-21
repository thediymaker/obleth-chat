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
      { workspaces: true, previews: true },
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

  it("downloads just the files and folders that are ticked", async () => {
    mockApi(
      (path) =>
        path.endsWith("/workspace/files")
          ? {
              entries: [
                { path: "notes & plan.md", type: "file", size: 10 },
                { path: "src", type: "directory" },
                { path: "src/app.py", type: "file", size: 20 },
                { path: "src/lib", type: "directory" },
                { path: "src/lib/util.py", type: "file", size: 30 },
              ],
            }
          : { workspace: { status: "running", previewPort: null }, diskLimit: 10 * 2 ** 30 },
      { workspaces: true, previews: true },
    );
    renderPage(<WorkspacePanel conversationId="c1" tab="files" />);
    await screen.findByText("app.py");
    // Nothing ticked: only the whole project can be downloaded.
    expect(screen.queryByRole("link", { name: "Download what is picked" })).toBeNull();
    fireEvent.click(screen.getByRole("checkbox", { name: "Select src/app.py" }));
    const one = screen.getByRole("link", { name: "Download what is picked" });
    expect(one).toHaveAttribute("href", "/api/conversations/c1/workspace/download?path=src%2Fapp.py");
    expect(screen.getByText("1 picked")).toBeInTheDocument();
    // A folder brings what is in it, so only the folder is asked for.
    fireEvent.click(screen.getByRole("checkbox", { name: "Select src" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Select notes & plan.md" }));
    expect(screen.getByRole("link", { name: "Download what is picked" })).toHaveAttribute(
      "href",
      "/api/conversations/c1/workspace/download?path=notes%20%26%20plan.md&path=src",
    );
    expect(screen.getByText("2 picked")).toBeInTheDocument();
    const inside = screen.getByRole("checkbox", { name: "Select src/lib/util.py" });
    expect(inside).toBeChecked();
    expect(inside).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    expect(screen.queryByRole("link", { name: "Download what is picked" })).toBeNull();
    // Everything: the top of the tree, which is the whole of it.
    fireEvent.click(screen.getByRole("checkbox", { name: "Select everything" }));
    expect(screen.getByRole("link", { name: "Download what is picked" })).toHaveAttribute(
      "href",
      "/api/conversations/c1/workspace/download?path=notes%20%26%20plan.md&path=src",
    );
    // The whole project is still one click away.
    expect(screen.getByRole("link", { name: "Download project" })).toHaveAttribute("href", "/api/conversations/c1/workspace/download");
  });

  it("offers to wake an asleep workspace", async () => {
    const calls = mockApi((_path, options) => ({ workspace: { status: options.method === "POST" ? "starting" : "stopped" } }), { workspaces: true, previews: true });
    renderPage(<WorkspacePanel conversationId="c1" tab="preview" />);
    expect(await screen.findByText("The workspace is asleep.")).toBeInTheDocument();
    expect(screen.getByText("Asleep")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Wake it up" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
    expect(await screen.findByText("Starting your workspace…")).toBeInTheDocument();
  });

  it("says what went wrong when it could not start", async () => {
    mockApi(() => ({ workspace: { status: "error", errorMessage: "Quota exceeded" } }), { workspaces: true, previews: true });
    renderPage(<WorkspacePanel conversationId="c1" tab="preview" />);
    expect(await screen.findByText("The workspace could not start.")).toBeInTheDocument();
    expect(screen.getByText("Quota exceeded")).toBeInTheDocument();
  });
});

describe("the Build view over the preview", () => {
  const ORIGIN = "https://abc.preview.example.org";
  const checkpoint = (n: number) => ({ id: `checkpoint-${n}`, kind: "checkpoint", sha: `${n}`.repeat(8), summary: `Checkpoint ${n}`, stat: `${n} files changed` });
  const open = async () => {
    const store = fakeStore() as ReturnType<typeof fakeStore> & { draft: string; shared: unknown };
    store.draft = "";
    store.active.messages = [
      { id: "m1", role: "assistant", createdAt: "2026-11-21T10:00:00Z", activity: [checkpoint(1), checkpoint(2)] },
      { id: "m2", role: "assistant", createdAt: "2026-11-21T10:05:00Z", activity: [checkpoint(3)] },
    ];
    vi.stubGlobal("fetch", vi.fn(async () => ({ ok: true, json: async () => ({}) })));
    const calls = mockApi(
      (path, options) =>
        path.endsWith("/workspace/preview")
          ? { url: `${ORIGIN}/`, origin: ORIGIN, enter: `${ORIGIN}/__obleth/enter`, ticket: "t" }
          : path.includes("/checkpoints/") && options.method === "POST"
            ? { before: "f".repeat(8) }
            : { workspace: { status: "running", previewPort: 3000 }, diskLimit: 10 * 2 ** 30 },
      { workspaces: true, previews: true },
    );
    renderPage(<WorkspacePanel conversationId="c1" tab="preview" />);
    const frame = (await screen.findByTitle("Workspace preview")) as HTMLIFrameElement;
    const say = (data: unknown, origin = ORIGIN, source: unknown = frame.contentWindow) => fireEvent(window, new MessageEvent("message", { data, origin, source: source as MessageEventSource }));
    return { store, frame, say, calls };
  };

  it("offers Point once the page's picker is there, and puts the clicked element in the message box", async () => {
    const { store, frame, say } = await open();
    expect(screen.queryByRole("button", { name: /Point/ })).toBeNull();
    // Another page saying it is ready, or the right page from another origin, is not heard.
    say({ source: "obleth-preview", type: "ready" }, "https://elsewhere.example.org");
    say({ source: "obleth-preview", type: "ready" }, ORIGIN, window);
    expect(screen.queryByRole("button", { name: /Point/ })).toBeNull();
    say({ source: "obleth-preview", type: "ready" });
    const post = vi.spyOn(frame.contentWindow!, "postMessage").mockImplementation(() => {});
    fireEvent.click(await screen.findByRole("button", { name: /Point/ }));
    expect(post).toHaveBeenCalledWith({ source: "obleth-chat", type: "pick-start" }, ORIGIN);
    expect(screen.getByRole("button", { name: /Click an element/ })).toHaveAttribute("aria-pressed", "true");
    store.draft = "make it bigger";
    const drawn = store.getVersion();
    say({ source: "obleth-preview", type: "picked", element: { tag: "button", selector: "#hero > button.cta", text: "Sign up", path: "/", width: 120, height: 40 } });
    expect(store.draft).toBe("In the preview, the `button` “Sign up” (`#hero > button.cta`): make it bigger");
    // The message box is told, so it shows what was put in it.
    expect(store.getVersion()).toBeGreaterThan(drawn);
    expect(await screen.findByRole("button", { name: /Point/ })).toHaveAttribute("aria-pressed", "false");
    // Esc in Chat's own page ends a pick that was started.
    fireEvent.click(screen.getByRole("button", { name: /Point/ }));
    fireEvent.keyDown(window, { key: "Escape" });
    expect(post).toHaveBeenLastCalledWith({ source: "obleth-chat", type: "pick-stop" }, ORIGIN);
  });

  it("steps through the chat's versions, marking where the project is", async () => {
    const { store, calls } = await open();
    const bar = document.querySelector("[data-versions]")!;
    expect(bar.querySelector("[data-version-now]")?.textContent).toContain("Checkpoint 3 of 3");
    expect(screen.getByRole("button", { name: "Checkpoint 3 (the project is here)" })).toHaveAttribute("aria-current", "step");
    expect(screen.getByRole("button", { name: "Newer version" })).toBeDisabled();
    // Going back asks first, through the restore dialog the chat page shows.
    fireEvent.click(screen.getByRole("button", { name: "Older version" }));
    expect(store.workspace.restoreDialog()).toMatchObject({ sha: "22222222", label: "Checkpoint 2", step: "confirm" });
    await store.workspace.confirmRestore();
    await waitFor(() => expect(calls.some((c) => c.path.endsWith("/workspace/checkpoints/22222222/restore") && c.method === "POST")).toBe(true));
    expect(await screen.findByRole("button", { name: "Checkpoint 2 (the project is here)" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Newer version" })).not.toBeDisabled();
    // A change to the files makes a new state: the project is no longer at version 2.
    store.workspace.onEvent({ event: "tool", data: { tool: "write_file", status: "completed" } } as never, {} as never, "c1");
    expect(await screen.findByRole("button", { name: "Checkpoint 3 (the project is here)" })).toBeInTheDocument();
  });

  it("shows neither on the Compute page, where there is no message box", async () => {
    const { store, say } = await open();
    store.route = { page: "workspaces", id: "" };
    store.emit();
    say({ source: "obleth-preview", type: "ready" });
    await waitFor(() => expect(document.querySelector("[data-versions]")).toBeNull());
    expect(screen.queryByRole("button", { name: /Point/ })).toBeNull();
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
    }, { workspaces: true, previews: true });
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
    mockApi(() => ({ workspaces: [], diskLimit: 0 }), { workspaces: true, previews: true });
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
