import { describe, expect, it, vi } from "vitest";
import { createWorkspaceBridge } from "./bridge";
import { typeCommand } from "./terminal";

const decode = (data: Uint8Array) => new TextDecoder().decode(data);

async function withCommand(command: string) {
  const bridge = createWorkspaceBridge({ api: { request: async () => ({ workspace: { status: "running" } }) }, enabled: () => true, changed: () => {}, showTab: () => {}, viewing: () => false });
  await bridge.load("c1");
  bridge.runInTerminal("c1", command);
  return bridge;
}
const socket = () => ({ readyState: WebSocket.OPEN, send: vi.fn() }) as unknown as WebSocket & { send: ReturnType<typeof vi.fn> };

describe("run_in_terminal", () => {
  it("types the command once the shell has gone quiet", async () => {
    const bridge = await withCommand("npm run dev");
    const s = { socket: socket(), typing: false, heard: Date.now() - 1000, term: { focus: vi.fn() } };
    await typeCommand(bridge, "c1", s);
    expect(s.socket.send).toHaveBeenCalledTimes(1);
    expect(decode(s.socket.send.mock.calls[0][0])).toBe("npm run dev\r");
    expect(s.term.focus).toHaveBeenCalled();
    expect(bridge.takeCommand("c1")).toBeNull();
  });

  it("stops what ran before with Ctrl+C in a terminal that was open", async () => {
    vi.useFakeTimers();
    try {
      const bridge = await withCommand("pytest");
      const s = { socket: socket(), typing: false, heard: Date.now() - 1000 };
      const done = typeCommand(bridge, "c1", s, true);
      await vi.advanceTimersByTimeAsync(9000);
      await done;
      expect(s.socket.send.mock.calls.map((c) => decode(c[0]))).toEqual(["\x03", "pytest\r"]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the command for later when the terminal is not connected", async () => {
    const bridge = await withCommand("ls");
    await typeCommand(bridge, "c1", { socket: null, typing: false, heard: 0 });
    expect(bridge.takeCommand("c1")).toBe("ls");
  });
});
