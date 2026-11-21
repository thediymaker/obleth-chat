import { describe, expect, it } from "vitest";
import { intents, runningNow, shownIntent } from "./home";

describe("intents", () => {
  it("offers only what the installation can do", () => {
    expect(intents({ workspaces: false, clusters: [] }).map((i) => i.id)).toEqual(["ask"]);
    expect(intents({ workspaces: true, clusters: [] }).map((i) => i.id)).toEqual(["ask", "build", "research"]);
    expect(intents({ workspaces: true, clusters: [{ name: "Sol" }] }).map((i) => i.id)).toEqual(["ask", "build", "research", "cluster"]);
  });
  it("names the cluster, and says that it asks only when every cluster does", () => {
    const one = intents({ workspaces: false, clusters: [{ name: "Sol", confirmActions: true }] }).at(-1)!;
    expect(one.hint).toBe("The model works with Sol, and asks before anything that changes a job.");
    expect(one.starters[1]![3]).toBe("Using the Sol cluster, submit a job that ");
    const two = intents({ workspaces: false, clusters: [{ name: "Sol", confirmActions: true }, { name: "Phx" }] }).at(-1)!;
    expect(two.hint).toBe("The model works with 2 clusters.");
  });
});

describe("shownIntent", () => {
  const all = intents({ workspaces: true, clusters: [{ name: "Sol" }] });
  it("follows the workspace switch in the box", () => {
    expect(shownIntent("build", true, all)).toBe("build");
    expect(shownIntent("build", false, all)).toBe("ask");
    expect(shownIntent("research", false, all)).toBe("ask");
    expect(shownIntent("ask", true, all)).toBe("build");
    expect(shownIntent("cluster", true, all)).toBe("cluster");
  });
  it("falls back to Ask for an intent that is not offered", () => {
    expect(shownIntent("cluster", false, intents({ workspaces: true, clusters: [] }))).toBe("ask");
    expect(shownIntent("ask", true, intents({ workspaces: false, clusters: [] }))).toBe("ask");
  });
});

describe("runningNow", () => {
  const now = Date.parse("2026-11-20T12:00:00Z");
  it("puts what waits for the person first, then work, jobs and workspaces", () => {
    const items = runningNow({
      now,
      chats: [
        { id: "a", title: "Quiet" },
        { id: "b", title: "Sweep", working: true },
        { id: "c", title: "Cancel a job", working: true, waiting: true, waitingTool: "cancel_job" },
      ],
      queues: [
        { cluster: { id: "s", name: "Sol" }, running: 2, pending: 1, jobs: [{ name: "sweep-32" }, { name: "sweep-64" }, { name: "sweep-128" }] },
        { cluster: { id: "p", name: "Phx" }, running: 0, pending: 0, jobs: [] },
      ],
      workspaces: [
        { conversationId: "w1", status: "running", title: "Signup site", previewPort: 3000, lastUsedAt: "2026-11-20T11:56:00Z" },
        { conversationId: "b", status: "running", title: "Sweep", lastUsedAt: "2026-11-20T11:59:00Z" },
        { conversationId: "w2", status: "stopped", title: "Asleep" },
      ],
    });
    expect(items.map((i) => [i.kind, i.eyebrow, i.title, i.detail, i.href])).toEqual([
      ["waiting", "Waiting on you", "Cancel a job", "Asks to run cancel_job", "/chat/c"],
      ["working", "Working", "Sweep", "A reply is being written", "/chat/b"],
      ["jobs", "Jobs · Sol", "2 running · 1 waiting", "sweep-32, sweep-64 and 1 other", "/clusters"],
      ["workspace", "Workspace · preview is up", "Signup site", "Port 3000 · used 4 min ago", "/chat/w1"],
    ]);
  });
  it("is empty when nothing is alive", () => {
    expect(runningNow({ chats: [{ id: "a", title: "Quiet" }], workspaces: [{ conversationId: "a", status: "stopped" }] })).toEqual([]);
  });
  it("lists the two workspaces used last", () => {
    const spaces = ["1", "2", "3"].map((n) => ({ conversationId: n, status: "running", title: n, lastUsedAt: `2026-11-20T11:0${n}:00Z` }));
    expect(runningNow({ now, workspaces: spaces }).map((i) => i.title)).toEqual(["3", "2"]);
  });
});
