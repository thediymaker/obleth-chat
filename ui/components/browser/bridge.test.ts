import { describe, expect, it, vi } from "vitest";
import { createBrowserBridge, liveUrl } from "./bridge";

function setup(handler: (path: string, options: any) => unknown = () => ({})) {
  const api = { request: vi.fn(async (path: string, options: any = {}) => handler(path, options)) };
  const showTab = vi.fn();
  const changed = vi.fn();
  return { api, showTab, changed, bridge: createBrowserBridge({ api, enabled: () => true, changed, showTab }) };
}
const step = (data: object) => ({ event: "tool", data });

describe("the browser bridge", () => {
  it("reads whether a chat may browse", async () => {
    const { bridge } = setup(() => ({ on: true, status: "running", controller: "user", url: "https://x.org", title: "X" }));
    await bridge.load("c1");
    expect(bridge.on("c1")).toBe(true);
    expect(bridge.chat("c1")).toMatchObject({ status: "running", controller: "user", url: "https://x.org", title: "X", loaded: true });
  });

  it("turns the browser on and shows its tab", async () => {
    const { bridge, api, showTab } = setup((_p, o) => ({ on: o.body.on }));
    await bridge.setOn("c1", true);
    expect(api.request).toHaveBeenCalledWith("/api/conversations/c1/browser", { method: "PUT", body: { on: true } });
    expect(showTab).toHaveBeenCalledWith("c1", "browser");
    await bridge.setOn("c1", false);
    expect(bridge.on("c1")).toBe(false);
    expect(showTab).toHaveBeenCalledTimes(1);
  });

  it("shows the tab at the first browser step, and once only", () => {
    const { bridge, showTab } = setup();
    const heard = vi.fn();
    bridge.onActivity("c1", heard);
    bridge.observe(step({ tool: "browser_open", url: "https://a.org", title: "A" }), "c1");
    bridge.observe(step({ tool: "browser_click" }), "c1");
    bridge.observe(step({ tool: "write_file" }), "c1");
    expect(showTab).toHaveBeenCalledTimes(1);
    expect(heard).toHaveBeenCalledTimes(2);
    expect(bridge.chat("c1")).toMatchObject({ on: true, status: "running", url: "https://a.org", title: "A", controller: "model" });
  });

  it("hands the tab to the person at a handoff", () => {
    const { bridge } = setup();
    bridge.observe(step({ kind: "browser-handoff", reason: "Sign in" }), "c1");
    expect(bridge.chat("c1").controller).toBe("user");
  });

  it("takes over and hands back", async () => {
    const { bridge, api } = setup();
    await bridge.setController("c1", "user");
    expect(api.request).toHaveBeenCalledWith("/api/conversations/c1/browser/control", { method: "POST", body: { controller: "user" } });
    expect(bridge.chat("c1").controller).toBe("user");
  });

  it("remembers a card's answer", () => {
    const { bridge } = setup();
    bridge.setOffer("m1", "dismissed");
    expect(bridge.offer("m1")).toBe("dismissed");
    bridge.setOffer("m1", null);
    expect(bridge.offer("m1")).toBeUndefined();
    bridge.setOffer("m1", "done");
    bridge.reset();
    expect(bridge.offer("m1")).toBeUndefined();
  });

  it("opens the live view on the same host, waking it when asked", () => {
    expect(liveUrl("c1", false, { protocol: "https:", host: "chat.edu" })).toBe("wss://chat.edu/api/conversations/c1/browser/live");
    expect(liveUrl("c1", true, { protocol: "http:", host: "localhost" })).toBe("ws://localhost/api/conversations/c1/browser/live?wake=1");
  });
});
