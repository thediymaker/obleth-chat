import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { fakeStore } from "@/components/workspace/test-store";
import { ProjectResearch } from "./project-research";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("@/components/chat/chat-provider", () => import("@/components/workspace/test-store"));
const push = vi.fn();
vi.mock("next/navigation", () => ({ useRouter: () => ({ push, replace: vi.fn() }), usePathname: () => "/projects/p1/research" }));
vi.mock("sonner", () => ({ toast: vi.fn() }));
let mine = true;
vi.mock("@/components/projects/project-frame", () => ({ useProjectFrame: () => ({ detail: { project: { id: "p1", name: "Decode", mine } } }) }));

const research = {
  research: { goal: "Make decode faster", metric: "ms", direction: "min", runCommand: "python bench.py" },
  workspace: { conversationId: "c9", title: "Speed chat" },
  experiments: [
    { id: "base", parentId: null, title: "Baseline", frozen: true, branch: "research/base" },
    { id: "a", parentId: "base", title: "Cache the keys", branch: "research/a", description: "Keep keys warm" },
  ],
  runs: [
    { id: "r1", experimentId: "base", number: 1, status: "completed", metric: 100, commit: "abc1234", startedAt: "2026-11-03T10:00:00Z", finishedAt: "2026-11-03T10:02:00Z" },
    { id: "r2", experimentId: "a", number: 2, status: "running", commit: "def5678", startedAt: "2026-11-03T10:05:00Z", metrics: {} },
  ],
};

beforeEach(() => {
  fakeStore();
  mine = true;
});

describe("the Research tab", () => {
  it("shows the goal, the tree and a node's run, and stops a run", async () => {
    const calls = mockApi((path) => (path.startsWith("/api/research/runs/r2/cancel") ? {} : path.startsWith("/api/research/runs/") ? { run: { log: "step 1\nstep 2", canCancel: true, diff: "+fast\n-slow" } } : research), { workspaces: true });
    renderPage(<ProjectResearch projectId="p1" />);
    expect(await screen.findByText("Make decode faster")).toBeInTheDocument();
    expect(screen.getByText("ms ↓ lower is better")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Runs in “Speed chat”/ })).toHaveAttribute("href", "/chat/c9");
    expect(screen.getByText("1 running")).toBeInTheDocument();
    // The best node opens first: the baseline, with its one measured run.
    expect(screen.getByRole("heading", { name: "Baseline" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Cache the keys/ }));
    expect(await screen.findByRole("heading", { name: "Cache the keys" })).toBeInTheDocument();
    expect(await screen.findByText(/step 2/)).toBeInTheDocument();
    expect(screen.getByText("+fast")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Stop this run" }));
    await waitFor(() => expect(calls).toContainEqual({ path: "/api/research/runs/r2/cancel", method: "POST", body: undefined }));
  });

  it("starts research in a new chat in the project", async () => {
    const store = fakeStore();
    mockApi(() => ({ research: null }), { workspaces: true });
    renderPage(<ProjectResearch projectId="p1" />);
    fireEvent.click(await screen.findByRole("button", { name: "Start research in a chat" }));
    expect(store.newChat).toHaveBeenCalled();
    expect(store.workspaceDraft).toBe(true);
    expect(store.starter).toHaveBeenCalledWith("Let’s set up research in this project. The goal: ", true);
    expect(push).toHaveBeenCalledWith("/");
  });

  it("tells others only the owner can start research", async () => {
    mine = false;
    mockApi(() => ({ research: null }), { workspaces: true });
    renderPage(<ProjectResearch projectId="p1" />);
    expect(await screen.findByText("Only the project’s owner can start research in it.")).toBeInTheDocument();
  });
});
