import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { fakeStore } from "@/components/workspace/test-store";
import { CodePanel, CodeTabLabel } from "./code-panel";
import type { ChatCode } from "./model";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("@/components/chat/chat-provider", () => import("@/components/workspace/test-store"));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn() }), usePathname: () => "/chat/c1" }));
vi.mock("sonner", () => ({ toast: vi.fn() }));

const project = { id: "p1", name: "Lab site" };
const base: ChatCode = {
  project,
  workspace: true,
  running: true,
  main: { sha: "aaaaaaa1", commits: 4 },
  changes: [
    { path: "index.html", status: "modified", added: 3, removed: 1 },
    { path: "new.css", status: "added", added: 10, removed: 0 },
  ],
  behind: 2,
  ahead: 1,
  draft: "Update the home page",
  log: [
    { sha: "bbbbbbb2", author: "Ana", time: Date.now() / 1000 - 60, subject: "Add a header" },
    { sha: "ccccccc3", author: "Bo", time: Date.now() / 1000 - 7200, subject: "Start" },
  ],
};

beforeEach(() => {
  fakeStore();
});

describe("the chat's Git tab", () => {
  it("shows the changes, the drafted message and what Sync will do, and syncs", async () => {
    let state = base;
    const calls = mockApi((path) => {
      if (path.endsWith("/code/sync")) {
        state = { ...base, changes: [], behind: 0, ahead: 0 };
        return { state: "synced", shared: true };
      }
      return state;
    }, { code: true });
    renderPage(
      <>
        <CodeTabLabel conversationId="c1" />
        <CodePanel conversationId="c1" />
      </>,
    );
    expect(await screen.findByText("Changes · 2 files")).toBeInTheDocument();
    expect(screen.getByText("2 new commits in the project since this chat last synced.")).toBeInTheDocument();
    expect(screen.getByLabelText("What changed")).toHaveValue("Update the home page");
    expect(screen.getByText("Sync saves your changes, brings in the project’s 2 new commits, and shares yours with everyone.")).toBeInTheDocument();
    // The latest save is not synced yet, so it can be undone.
    expect(screen.getByText(/not synced/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Undo" })).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("What changed"), { target: { value: "New header" } });
    fireEvent.click(screen.getByRole("button", { name: "Sync" }));
    await waitFor(() => expect(calls.find((c) => c.path.endsWith("/sync"))?.body).toEqual({ message: "New header" }));
    expect(await screen.findByText("Everything here is in the project.")).toBeInTheDocument();
  });

  it("opens a file's diff and undoes it after asking", async () => {
    const calls = mockApi((path) => (path.includes("/code/diff") ? { diff: "@@ -1,1 +1,1 @@\n-old\n+new\n" } : path.endsWith("/code/undo") ? {} : base), { code: true });
    renderPage(<CodePanel conversationId="c1" />);
    fireEvent.click(await screen.findByText("index.html"));
    expect(await screen.findByText("new")).toBeInTheDocument();
    expect(calls.some((c) => c.path === "/api/conversations/c1/code/diff?path=index.html")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Undo this file" }));
    expect(await screen.findByText("Undo the changes to this file?")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Undo changes" }));
    await waitFor(() => expect(calls.find((c) => c.path.endsWith("/code/undo"))?.body).toEqual({ path: "index.html" }));
  });

  it("settles a paused sync: each side of a conflict, keep mine, resolve with the model", async () => {
    const store = fakeStore();
    const merging: ChatCode = { ...base, merging: true, conflicts: ["app.py"], changes: [{ path: "app.py", status: "conflict" }, { path: "b.py", status: "modified" }] };
    const calls = mockApi((path) => (path.includes("/workspace/file") ? { content: "<<<<<<< HEAD\nmine = 1\n=======\ntheirs = 2\n>>>>>>> main\n" } : path.endsWith("/choose") ? {} : merging), { code: true });
    renderPage(
      <>
        <CodeTabLabel conversationId="c1" />
        <CodePanel conversationId="c1" />
      </>,
    );
    expect(await screen.findByText("Sync paused")).toBeInTheDocument();
    expect(await screen.findByText("mine = 1")).toBeInTheDocument();
    expect(screen.getByText("theirs = 2")).toBeInTheDocument();
    expect(screen.getByText("Combined on their own: b.py")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Resolve with the model" }));
    expect(store.send).toHaveBeenCalledWith(expect.stringMatching(/^Sync paused because `app.py` was changed/));
    fireEvent.click(screen.getByRole("button", { name: "Keep mine" }));
    await waitFor(() => expect(calls.find((c) => c.path.endsWith("/choose"))?.body).toEqual({ path: "app.py", side: "ours" }));
  });

  it("asks to move a chat into a project first", async () => {
    mockApi(() => ({ project: null }), { code: true });
    renderPage(<CodePanel conversationId="c1" />);
    expect(await screen.findByText("Git works in project chats")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Move to project" })).toBeInTheDocument();
  });

  it("offers Sync for an asleep workspace", async () => {
    mockApi(() => ({ ...base, running: false }), { code: true });
    renderPage(<CodePanel conversationId="c1" />);
    expect(await screen.findByText("The workspace is asleep")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Sync" })).toBeInTheDocument();
  });

  it("runs git status in the terminal", async () => {
    const store = fakeStore();
    mockApi((path) => (path.endsWith("/workspace") ? { workspace: { status: "running" } } : base), { code: true });
    await store.workspace.load("c1");
    renderPage(<CodePanel conversationId="c1" />);
    fireEvent.click((await screen.findAllByRole("button", { name: "Open in terminal" }))[0]!);
    expect(store.showTab).toHaveBeenCalledWith("c1", "terminal");
    expect(store.workspace.takeCommand("c1")).toBe("git status");
  });
});
