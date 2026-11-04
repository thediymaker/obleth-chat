import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { codeEvents } from "@/components/code/events";
import { createWorkspaceBridge } from "./bridge";

function setup(handler: (path: string, options: any) => unknown = () => ({})) {
  const calls: { path: string; method: string }[] = [];
  const api = {
    request: vi.fn(async (path: string, options: any = {}) => {
      calls.push({ path, method: options.method || "GET" });
      const r = await handler(path, options);
      if (r instanceof Error) throw r;
      return r;
    }),
  };
  const showTab = vi.fn();
  const changed = vi.fn();
  const toast = vi.fn();
  const bridge = createWorkspaceBridge({ api, enabled: () => true, changed, showTab, toast, viewing: () => false, wait: async () => {} });
  return { bridge, calls, showTab, changed, toast };
}
const running = { status: "running", previewPort: 5173, conversationId: "c1" };
const tool = (data: object) => ({ event: "tool", data });
const assistant = { role: "assistant", content: "" };

beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn(async () => new Response(null, { status: 204 })));
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the workspace bridge", () => {
  it("loads a chat's workspace and its disk limit", async () => {
    const { bridge, calls } = setup(() => ({ workspace: running, diskLimit: 10 * 2 ** 30 }));
    const refresh = vi.fn();
    const off = codeEvents.subscribe(refresh);
    await bridge.load("c1");
    off();
    expect(calls).toEqual([{ path: "/api/conversations/c1/workspace", method: "GET" }]);
    expect(bridge.has("c1")).toBe(true);
    expect(bridge.diskLimit()).toBe(10 * 2 ** 30);
    // A running workspace asks the Git tab to check again.
    expect(refresh).toHaveBeenCalledWith({ id: "c1", kind: "refresh", age: 30000 });
  });

  it("turns a workspace on and shows its preview tab", async () => {
    const { bridge, showTab } = setup(() => ({ workspace: { status: "starting" } }));
    await bridge.create("c1");
    expect(bridge.space("c1").workspace?.status).toBe("starting");
    expect(showTab).toHaveBeenCalledWith("c1", "preview");
  });

  it("acts on the reply's tool steps", async () => {
    const { bridge, showTab } = setup(() => ({ workspace: running }));
    await bridge.load("c1");
    bridge.space("c1").files = [{ path: "a.txt" }];
    const changed = vi.fn();
    const off = codeEvents.subscribe(changed);
    expect(bridge.onEvent(tool({ id: "t1", tool: "write_file", status: "running" }), assistant, "c1")).toBe(true);
    expect(bridge.space("c1").files).toBeNull();
    expect(changed).not.toHaveBeenCalled();
    bridge.onEvent(tool({ id: "t1", tool: "write_file", status: "completed" }), assistant, "c1");
    expect(changed).toHaveBeenCalledWith({ id: "c1", kind: "changed" });
    off();
    bridge.onEvent(tool({ id: "t2", tool: "run_in_terminal", status: "completed", command: "npm test" }), assistant, "c1");
    expect(showTab).toHaveBeenLastCalledWith("c1", "terminal");
    expect(bridge.takeCommand("c1")).toBe("npm test");
    expect(bridge.takeCommand("c1")).toBeNull();
    expect(bridge.onEvent({ event: "draft", data: {} }, assistant, "c1")).toBe(true);
    expect(bridge.onEvent({ event: "delta", data: {} }, assistant, "c1")).toBe(false);
  });

  it("tells an open terminal about a new command", async () => {
    const { bridge } = setup(() => ({ workspace: running }));
    await bridge.load("c1");
    const heard = vi.fn();
    const off = bridge.onCommand("c1", heard);
    bridge.runInTerminal("c1", "ls");
    expect(heard).toHaveBeenCalledTimes(1);
    off();
    bridge.runInTerminal("c1", "pwd");
    expect(heard).toHaveBeenCalledTimes(1);
  });

  it("loads a workspace a delegated worker turned on", async () => {
    const { bridge, calls } = setup(() => ({ workspace: running }));
    bridge.onEvent(tool({ tool: "delegate", workspace: true }), assistant, "c1");
    await vi.waitFor(() => expect(calls.length).toBe(1));
  });

  it("opens the preview when the app starts: a link, its ticket redeemed, then the frame", async () => {
    const { bridge, showTab, calls } = setup((path) =>
      path.endsWith("/preview") ? { enter: "https://p.example/__obleth/enter", ticket: "t", url: "https://p.example/", origin: "https://p.example" } : { workspace: { status: "running", previewPort: null } },
    );
    await bridge.load("c1");
    bridge.onEvent({ event: "preview", data: { port: 5173 } }, assistant, "c1");
    expect(showTab).toHaveBeenCalledWith("c1", "preview");
    await vi.waitFor(() => expect(bridge.space("c1").url).toBe("https://p.example/"));
    expect(bridge.space("c1").workspace?.previewPort).toBe(5173);
    expect(calls.at(-1)).toEqual({ path: "/api/conversations/c1/workspace/preview", method: "POST" });
    const [url, init] = (fetch as any).mock.calls[0];
    expect(url).toBe("https://p.example/__obleth/enter");
    expect(init).toMatchObject({ method: "POST", mode: "cors", credentials: "include" });
    expect(String(init.body)).toBe("ticket=t");
    expect(bridge.space("c1").frame).toBe(1);
  });

  it("opens an HTML file in the preview at its page", async () => {
    const { bridge, calls } = setup((path) => (path.includes("/preview") ? { enter: "https://p/__obleth/enter", ticket: "t", url: "https://p/about.html", origin: "https://p" } : { workspace: running }));
    await bridge.load("c1");
    bridge.previewPage("c1", "about.html");
    await vi.waitFor(() => expect(bridge.space("c1").url).toBe("https://p/about.html"));
    expect(calls.at(-1)?.path).toBe("/api/conversations/c1/workspace/preview?path=about.html");
  });

  it("says why the preview could not open", async () => {
    (fetch as any).mockImplementation(async () => new Response(JSON.stringify({ error: { message: "Ticket expired" } }), { status: 403 }));
    const { bridge, toast } = setup((path) => (path.includes("/preview") ? { enter: "https://p/__obleth/enter", ticket: "t", url: "https://p/", origin: "https://p" } : { workspace: running }));
    await bridge.load("c1");
    await bridge.showPreview("c1");
    expect(toast).toHaveBeenCalledWith("Ticket expired");
    expect(bridge.space("c1").url).toBe("");
  });

  it("reads files, and shows images from raw", async () => {
    const { bridge } = setup((path) =>
      path.endsWith("/files") ? { entries: [{ path: "src", type: "directory" }, { path: "src/a.py", type: "file", size: 10 }] } : path.includes("/file?") ? { content: "print(1)", truncated: false } : { workspace: running },
    );
    await bridge.load("c1");
    await bridge.loadFiles("c1");
    expect(bridge.space("c1").files).toHaveLength(2);
    await bridge.showFile("c1", "src/a.py");
    expect(bridge.space("c1").file).toEqual({ path: "src/a.py", content: "print(1)", truncated: false });
    await bridge.showFile("c1", "logo.png");
    expect(bridge.space("c1").file).toEqual({ path: "logo.png", image: "/api/conversations/c1/workspace/raw?path=logo.png" });
    bridge.reload("c1", "files");
    expect(bridge.space("c1").files).toBeNull();
    expect(bridge.space("c1").file).toBeNull();
  });

  it("wakes an asleep workspace from the reload button", async () => {
    const { bridge, calls } = setup((_path, options) => ({ workspace: { status: options.method === "POST" ? "stopped" : "stopped", previewPort: null } }));
    await bridge.load("c1");
    bridge.reload("c1", "preview");
    await vi.waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
    await vi.waitFor(() => expect(bridge.space("c1").waking).toBe(false));
    // Chat starts the pod after answering, so it shows as starting.
    expect(bridge.space("c1").workspace?.status).toBe("starting");
  });

  it("restarts the terminal from the reload button", async () => {
    const { bridge } = setup(() => ({ workspace: running }));
    await bridge.load("c1");
    bridge.reload("c1", "terminal");
    expect(bridge.space("c1").terminal).toBe(1);
  });

  it("wakes the workspace before a download", async () => {
    let status = "stopped";
    const { bridge, toast, calls } = setup((_p, options) => {
      if (options.method === "POST") status = "starting";
      else if (status === "starting") status = "running";
      return { workspace: { status } };
    });
    await bridge.load("c1");
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    bridge.download("c1");
    await vi.waitFor(() => expect(click).toHaveBeenCalled());
    expect(toast).toHaveBeenCalledWith("Waking the workspace to download the project…");
    expect(calls.filter((c) => c.method === "POST")).toHaveLength(1);
    click.mockRestore();
  });

  it("restores a checkpoint, then offers to undo it", async () => {
    const { bridge, calls, toast } = setup((path) => (path.includes("/restore") ? { before: "b4" } : path.includes("/preview") ? new Error("No preview") : { workspace: running }));
    await bridge.load("c1");
    bridge.restoreCheckpoint("c1", "abc", "step 3");
    expect(bridge.restoreDialog()).toMatchObject({ step: "confirm", sha: "abc", label: "step 3" });
    await bridge.confirmRestore();
    expect(calls).toContainEqual({ path: "/api/conversations/c1/workspace/checkpoints/abc/restore", method: "POST" });
    expect(bridge.restoreDialog()).toMatchObject({ step: "done", before: "b4" });
    await bridge.undoRestore();
    expect(calls).toContainEqual({ path: "/api/conversations/c1/workspace/checkpoints/b4/restore", method: "POST" });
    expect(bridge.restoreDialog()).toBeNull();
    expect(toast).toHaveBeenCalledWith("Back to where you were");
  });

  it("closes the restore dialog with the reason it failed", async () => {
    const { bridge, toast } = setup((path) => (path.includes("/restore") ? new Error("Wait for the reply to finish") : { workspace: running }));
    await bridge.load("c1");
    bridge.restoreCheckpoint("c1", "abc", "");
    await bridge.confirmRestore();
    expect(bridge.restoreDialog()).toBeNull();
    expect(toast).toHaveBeenCalledWith("Wait for the reply to finish");
  });

  it("keeps checking while the workspace wakes, when its chat is open", async () => {
    vi.useFakeTimers();
    try {
      let n = 0;
      const api = { request: vi.fn(async () => ({ workspace: { status: n++ < 2 ? "starting" : "running", previewPort: 3000 } })) };
      const bridge = createWorkspaceBridge({ api, enabled: () => true, changed: () => {}, showTab: () => {}, viewing: () => true });
      await bridge.load("c1");
      await vi.advanceTimersByTimeAsync(2000);
      await vi.advanceTimersByTimeAsync(2000);
      await vi.advanceTimersByTimeAsync(10000);
      // Starting, starting, then running with a preview: no more checks.
      expect(api.request).toHaveBeenCalledTimes(3);
    } finally {
      vi.useRealTimers();
    }
  });

  it("deletes a workspace", async () => {
    const { bridge, calls } = setup(() => ({ workspace: running }));
    await bridge.load("c1");
    await bridge.remove("c1");
    expect(calls.at(-1)).toEqual({ path: "/api/conversations/c1/workspace", method: "DELETE" });
    expect(bridge.has("c1")).toBe(false);
  });
});
