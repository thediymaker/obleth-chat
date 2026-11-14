"use client";

// The serial monitor's screen: a terminal that draws what the device prints, so
// colours, progress lines redrawn in place and full-screen programs show as they
// would on a console. Keys typed into it go to the device as they are, Ctrl+C and
// arrows included, while the port is open.

import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";
import { terminalTheme } from "@/components/workspace/terminal";
import type { SerialDevice } from "@/lib/devices/serial";

const encoder = new TextEncoder();

export function SerialTerminal({ device }: { device: SerialDevice }) {
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    let disposed = false;
    const cleanups: (() => void)[] = [];
    (async () => {
      let lib: [typeof import("@xterm/xterm"), typeof import("@xterm/addon-fit")];
      try {
        lib = await Promise.all([import("@xterm/xterm"), import("@xterm/addon-fit")]);
      } catch {
        return;
      }
      if (disposed) return;
      // Many boards end lines with a bare newline; convertEol keeps them from stepping right.
      const term = new lib[0].Terminal({ convertEol: true, fontSize: 12, fontFamily: "ui-monospace, SFMono-Regular, Consolas, monospace", scrollback: 5000, minimumContrastRatio: 4.5, theme: terminalTheme() });
      const fit = new lib[1].FitAddon();
      term.loadAddon(fit);
      term.open(el);
      cleanups.push(() => term.dispose());
      const refit = () => {
        try {
          fit.fit();
        } catch {
          // Hidden or not laid out yet.
        }
      };
      refit();
      const observer = new ResizeObserver(refit);
      observer.observe(el);
      cleanups.push(() => observer.disconnect());
      const themes = new MutationObserver(() => {
        term.options.theme = terminalTheme();
      });
      themes.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "class"] });
      cleanups.push(() => themes.disconnect());
      // What it printed before this mounted, then each new piece as it arrives.
      term.write(device.recent());
      cleanups.push(device.onOutput((text) => term.write(text)));
      const typed = term.onData((data) => {
        if (device.isOpen) void device.write(encoder.encode(data)).catch(() => {});
      });
      cleanups.push(() => typed.dispose());
    })();
    return () => {
      disposed = true;
      cleanups.reverse().forEach((c) => c());
    };
  }, [device]);
  return <div ref={box} className="h-64 overflow-hidden rounded-md bg-terminal px-2 py-1" aria-label="Device output" />;
}
