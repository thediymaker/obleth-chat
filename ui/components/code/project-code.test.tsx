import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { fakeStore } from "@/components/workspace/test-store";
import { ApiError } from "@/lib/api";
import { ProjectCode } from "./project-code";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("@/components/chat/chat-provider", () => import("@/components/workspace/test-store"));
const push = vi.fn();
let path = "/projects/p1/code";
vi.mock("next/navigation", () => ({ useRouter: () => ({ push, replace: vi.fn() }), usePathname: () => path }));
vi.mock("sonner", () => ({ toast: vi.fn() }));
const newChat = vi.fn();
vi.mock("@/components/projects/project-frame", () => ({ useProjectFrame: () => ({ detail: { project: { id: "p1", name: "Lab site", mine: true } } }), useNewChatInProject: () => newChat }));

const now = Date.now() / 1000;
const commits = [
  { sha: "bbbbbbbbbb", parents: ["aaaaaaaaaa"], author: "Ana", time: now - 60, subject: "Add a header", chat: { id: "c1", title: "Header chat", readable: true } },
  { sha: "aaaaaaaaaa", parents: [], author: "Bo", time: now - 7200, subject: "Start" },
];
const overview = { main: { sha: "bbbbbbbbbb", commits: 2, bytes: 2048, updatedBy: "Ana", updatedAt: new Date().toISOString() }, chats: [{ chat: { id: "c1", title: "Header chat", ownerName: "Ana", readable: true, mine: true }, syncedAt: new Date().toISOString(), behind: 1 }], exports: [], limits: { bytes: 50 * 1048576 } };

beforeEach(() => {
  fakeStore();
  push.mockClear();
  path = "/projects/p1/code";
});

describe("the project's Code tab", () => {
  it("shows the latest commits and the chats working on the code", async () => {
    mockApi((p) => (p.includes("/log") ? { commits } : overview), { code: true, workspaces: true });
    renderPage(<ProjectCode projectId="p1" />);
    expect(await screen.findByText(/2 commits/)).toBeInTheDocument();
    expect(screen.getAllByText("Add a header").length).toBeGreaterThan(0);
    expect(screen.getByText("1 commit behind")).toBeInTheDocument();
    expect(screen.getByText(/2.0 KB of 50 MB/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Export" }));
    expect(await screen.findByText("Take the code with you")).toBeInTheDocument();
    expect(screen.getByLabelText("Repository")).toHaveValue("lab-site");
  });

  it("offers a chat or a repository to an empty project", async () => {
    const calls = mockApi((p) => (p.endsWith("/import") ? { history: true, commits: 12 } : p.includes("/log") ? { commits: [] } : { ...overview, main: null }), { code: true });
    renderPage(<ProjectCode projectId="p1" />);
    expect(await screen.findByText("No code yet")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "New chat in project" }));
    expect(newChat).toHaveBeenCalledWith("p1");
    fireEvent.click(screen.getByRole("button", { name: "Start from a repository" }));
    fireEvent.change(await screen.findByLabelText("Repository"), { target: { value: "https://github.com/a/b" } });
    fireEvent.click(screen.getByRole("button", { name: "Copy the repository" }));
    await waitFor(() => expect(calls.find((c) => c.path.endsWith("/import"))?.body).toEqual({ url: "https://github.com/a/b" }));
  });

  it("shows a commit with its files, and starts a chat from it", async () => {
    path = "/projects/p1/code/history/aaaaaaaaaa";
    const store = fakeStore();
    mockApi(
      (p) =>
        p.includes("?path=") ? { patch: "@@ -0,0 +1 @@\n+hello\n" } : p.includes("/commits/") ? { commit: commits[1], body: "First", files: [{ path: "index.html", added: 1, removed: 0 }] } : p.includes("/log") ? { commits } : overview,
      { code: true, workspaces: true },
    );
    renderPage(<ProjectCode projectId="p1" />);
    expect(await screen.findByRole("heading", { name: "Start" })).toBeInTheDocument();
    // Three or fewer files open by themselves.
    expect(await screen.findByText("hello")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Compare with now/ })).toHaveAttribute("href", "/projects/p1/code/compare/aaaaaaaaaa...main");
    fireEvent.click(screen.getByRole("button", { name: "New chat from here" }));
    expect(store.code.start()).toEqual({ sha: "aaaaaaaaaa", subject: "Start" });
    expect(store.workspaceDraft).toBe(true);
    expect(push).toHaveBeenCalledWith("/");
  });

  it("undoes a commit, or offers a chat when later work touched the same lines", async () => {
    path = "/projects/p1/code/history/bbbbbbbbbb";
    mockApi(
      (p) =>
        p.endsWith("/undo") ? new ApiError("Later commits changed the same lines", 409) : p.includes("/commits/") ? { commit: commits[0], files: [] } : p.includes("/log") ? { commits } : { ...overview, main: { ...overview.main, sha: "zzz" } },
      { code: true, workspaces: true },
    );
    const store = fakeStore();
    renderPage(<ProjectCode projectId="p1" />);
    fireEvent.click(await screen.findByRole("button", { name: "Undo this commit" }));
    fireEvent.click(await screen.findByRole("button", { name: "Undo commit" }));
    expect(await screen.findByText("Undo it in a chat?")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Undo in a new chat" }));
    expect(store.starter).toHaveBeenCalledWith(expect.stringMatching(/^Undo commit bbbbbbb \(“Add a header”\)/), true);
  });

  it("shows who changed each line of a file", async () => {
    path = "/projects/p1/code/files/src%2Fapp.py";
    mockApi(
      (p) =>
        p.includes("/files")
          ? { files: [{ path: "src/app.py", size: 30 }] }
          : p.includes("/file?")
            ? { path: "src/app.py", size: 30, binary: false }
            : p.includes("/blame")
              ? { groups: [{ sha: "aaaaaaaaaa", start: 1, lines: ["import os", "print(1)"], author: "Bo", time: now - 60, subject: "Start" }] }
              : p.includes("/lines")
                ? { versions: [{ ...commits[1], patch: "+import os" }] }
                : overview,
      { code: true },
    );
    renderPage(<ProjectCode projectId="p1" />);
    expect(await screen.findByText("print(1)")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "History" }));
    expect(await screen.findByText("Lines 1–2, through time")).toBeInTheDocument();
  });
});
