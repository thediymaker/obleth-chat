"use client";

// The workspace's terminal: xterm.js over the …/workspace/terminal websocket
// (api/src/sandbox/terminal.rs: binary frames are keystrokes and output, text
// frames are JSON resize requests). One session per mounted terminal; after it
// ends, Enter reconnects. A run_in_terminal command is typed in once the shell has
// printed its prompt and gone quiet; one that arrives while the terminal is open
// gets Ctrl+C first, to stop what ran there before.

import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";
import { toast } from "sonner";
import type { WorkspaceBridge } from "./bridge";
import { terminalUrl } from "./model";

type XTerm = import("@xterm/xterm").Terminal;
type Fit = import("@xterm/addon-fit").FitAddon;

/** The terminal's colors, from Chat's theme tokens. */
export function terminalTheme(root: Element = document.documentElement) {
  const css = getComputedStyle(root),
    color = (name: string) => css.getPropertyValue(name).trim();
  return { background: color("--terminal") || color("--background"), foreground: color("--foreground"), cursor: color("--foreground"), cursorAccent: color("--terminal") || color("--background"), selectionBackground: color("--selection") };
}

interface Shell {
  term: XTerm;
  fit: Fit;
  socket: WebSocket | null;
  closed: boolean;
  typing: boolean;
  heard: number;
}

const encoder = new TextEncoder();
const pause = (ms: number) => new Promise((done) => setTimeout(done, ms));

/** Types the waiting command once the shell is quiet; `interrupt` sends Ctrl+C first. */
export async function typeCommand(bridge: WorkspaceBridge, id: string, s: Pick<Shell, "socket" | "typing" | "heard"> & { term?: { focus(): void } }, interrupt = false) {
  const open = () => s.socket?.readyState === WebSocket.OPEN;
  if (!open() || s.typing) return;
  const command = bridge.takeCommand(id);
  if (!command) return;
  s.typing = true;
  const settle = async () => {
    const began = Date.now();
    while (open() && Date.now() - began < 8000 && (!s.heard || Date.now() - s.heard < 400)) await pause(100);
  };
  try {
    await settle();
    if (interrupt && open()) {
      s.heard = 0;
      s.socket!.send(encoder.encode("\x03"));
      await settle();
    }
    if (open()) s.socket!.send(encoder.encode(`${command}\r`));
    else bridge.putCommand(id, command);
  } finally {
    s.typing = false;
  }
  s.term?.focus();
}

export function WorkspaceTerminal({ bridge, conversationId, session }: { bridge: WorkspaceBridge; conversationId: string; session: number }) {
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    let shell: Shell | null = null;
    let disposed = false;
    const cleanups: (() => void)[] = [];

    function connect(s: Shell) {
      s.term.write("\x1b[2mConnecting to your workspace…\x1b[0m\r\n");
      const socket = new WebSocket(terminalUrl(conversationId));
      socket.binaryType = "arraybuffer";
      s.socket = socket;
      socket.onopen = () => {
        try {
          s.fit.fit();
        } catch {
          // Not laid out yet.
        }
        socket.send(JSON.stringify({ type: "resize", cols: s.term.cols, rows: s.term.rows }));
        void typeCommand(bridge, conversationId, s);
      };
      socket.onmessage = (event) => {
        if (typeof event.data !== "string") {
          s.term.write(new Uint8Array(event.data as ArrayBuffer));
          s.heard = Date.now();
        }
      };
      socket.onclose = () => {
        if (s.closed) return;
        s.socket = null;
        s.term.write("\r\n\x1b[2m[The terminal closed. Press Enter to reconnect.]\x1b[0m\r\n");
      };
    }

    (async () => {
      let lib: [typeof import("@xterm/xterm"), typeof import("@xterm/addon-fit")];
      try {
        lib = await Promise.all([import("@xterm/xterm"), import("@xterm/addon-fit")]);
      } catch {
        toast("The terminal could not load.");
        return;
      }
      if (disposed) return;
      const term = new lib[0].Terminal({ cursorBlink: true, fontSize: 13, fontFamily: "ui-monospace, SFMono-Regular, Consolas, monospace", scrollback: 5000, minimumContrastRatio: 4.5, theme: terminalTheme() });
      const fit = new lib[1].FitAddon();
      term.loadAddon(fit);
      term.open(el);
      const s: Shell = (shell = { term, fit, socket: null, closed: false, typing: false, heard: 0 });
      const observer = new ResizeObserver(() => {
        try {
          fit.fit();
        } catch {
          // Hidden.
        }
      });
      observer.observe(el);
      cleanups.push(() => observer.disconnect());
      // The theme follows Chat's light and dark.
      const themes = new MutationObserver(() => {
        term.options.theme = terminalTheme();
      });
      themes.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "class"] });
      cleanups.push(() => themes.disconnect());
      term.onData((data) => {
        if (s.socket?.readyState === WebSocket.OPEN) s.socket.send(encoder.encode(data));
        else if (data === "\r" && !s.socket) connect(s);
      });
      term.onResize(({ cols, rows }) => {
        if (s.socket?.readyState === WebSocket.OPEN) s.socket.send(JSON.stringify({ type: "resize", cols, rows }));
      });
      cleanups.push(bridge.onCommand(conversationId, () => void typeCommand(bridge, conversationId, s, true)));
      connect(s);
      requestAnimationFrame(() => {
        try {
          fit.fit();
        } catch {
          // Hidden.
        }
        term.focus();
      });
    })();

    return () => {
      disposed = true;
      cleanups.forEach((c) => c());
      if (shell) {
        shell.closed = true;
        shell.socket?.close();
        shell.term.dispose();
      }
    };
  }, [bridge, conversationId, session]);
  return <div ref={box} className="h-full min-h-0 w-full bg-terminal p-2 [&_.xterm]:h-full" aria-label="Workspace terminal" />;
}
