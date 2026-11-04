"use client";

// The Browser tab of the chat's side pane: a live view of this chat's tab in the
// person's Chrome, streamed as JPEG frames over …/browser/live. The person can take
// over (their clicks and keys then go to the page and the model is paused), type an
// address, and hand back.

import { ArrowLeft, ArrowRight, Globe, Hand, Info, Orbit, Power, RotateCw } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { useChat, useChatStore } from "@/components/chat/chat-provider";
import { Button } from "@/components/ui/button";
import { useBranding } from "@/lib/branding-context";
import { cn } from "@/lib/utils";
import { liveUrl } from "./bridge";
import { buttonOf, keyMessage, modifiers, pagePoint, viewport } from "./input";

type Note = "connecting" | "starting" | "closed" | "status" | null;

interface Live {
  socket: WebSocket;
  w: number;
  h: number;
  closed: boolean;
  frames: number;
}

export function BrowserPanel({ conversationId: id }: { conversationId: string }) {
  const store = useChat();
  const handle = useChatStore();
  const bridge = store.browser;
  const c = bridge.chat(id);
  const name = useBranding().assistantName || "The assistant";
  const canvas = useRef<HTMLCanvasElement>(null);
  const screen = useRef<HTMLDivElement>(null);
  const live = useRef<Live | null>(null);
  const fitted = useRef("");
  const [note, setNote] = useState<Note>("status");
  const [address, setAddress] = useState("");
  const [editing, setEditing] = useState(false);
  const mine = c.controller === "user";
  const busy = !mine && store.streaming && store.streamChatId === id;

  const disconnect = useCallback(() => {
    const l = live.current;
    if (!l) return;
    l.closed = true;
    try {
      l.socket.close();
    } catch {
      // Already closed.
    }
    live.current = null;
  }, []);

  const fit = useCallback((force = false) => {
    const l = live.current,
      el = screen.current;
    if (!l || !el || l.socket.readyState !== WebSocket.OPEN) return;
    const size = viewport(el.clientWidth, el.clientHeight);
    if (!size) return;
    const key = `${size.width}x${size.height}`;
    if (key === fitted.current && !force) return;
    fitted.current = key;
    l.socket.send(JSON.stringify({ t: "viewport", w: size.width, h: size.height }));
  }, []);

  const connect = useCallback(
    (wake = false) => {
      disconnect();
      const socket = new WebSocket(liveUrl(id, wake));
      socket.binaryType = "arraybuffer";
      const mineLive: Live = { socket, w: 1280, h: 720, closed: false, frames: 0 };
      live.current = mineLive;
      setNote(wake ? "starting" : "connecting");
      socket.onopen = () => {
        if (live.current === mineLive) fit(true);
      };
      socket.onmessage = async (event) => {
        if (typeof event.data === "string") {
          let message: any;
          try {
            message = JSON.parse(event.data);
          } catch {
            return;
          }
          if (message.t === "size" && message.w > 0 && message.h > 0) {
            mineLive.w = message.w;
            mineLive.h = message.h;
          }
          if (message.t === "state") bridge.update(id, { controller: message.controller || "model", url: message.url || "", title: message.title || "", status: "running" });
          return;
        }
        let bitmap: ImageBitmap;
        try {
          bitmap = await createImageBitmap(new Blob([event.data], { type: "image/jpeg" }));
        } catch {
          return;
        }
        const el = canvas.current;
        if (live.current !== mineLive || !el) {
          bitmap.close();
          return;
        }
        if (el.width !== bitmap.width || el.height !== bitmap.height) {
          el.width = bitmap.width;
          el.height = bitmap.height;
        }
        el.getContext("2d")?.drawImage(bitmap, 0, 0);
        bitmap.close();
        if (!mineLive.frames++) {
          bridge.update(id, { status: "running" });
          setNote(null);
        }
      };
      socket.onclose = () => {
        if (mineLive.closed || live.current !== mineLive) return;
        live.current = null;
        void bridge.load(id).then(() => setNote(bridge.chat(id).status === "running" ? "closed" : "status"));
      };
    },
    [bridge, disconnect, fit, id],
  );

  // Open: read the tab's state, then watch it if the browser is up.
  useEffect(() => {
    let gone = false;
    (async () => {
      if (!bridge.chat(id).loaded) await bridge.load(id);
      if (!gone && bridge.chat(id).status === "running" && !live.current) connect();
    })();
    const off = bridge.onActivity(id, () => {
      if (!live.current) connect();
    });
    return () => {
      gone = true;
      off();
      disconnect();
    };
  }, [bridge, connect, disconnect, id]);

  useEffect(() => {
    const el = screen.current;
    if (!el) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const observer = new ResizeObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(() => fit(), 150);
    });
    observer.observe(el);
    return () => {
      observer.disconnect();
      clearTimeout(timer);
    };
  }, [fit]);

  // Leave the address alone while the person types in it.
  useEffect(() => {
    if (!editing || !mine) setAddress(c.url && c.url !== "about:blank" ? c.url : "");
  }, [c.url, editing, mine]);

  // The person's mouse and keys reach the tab only while they have control.
  const controlling = () => !!live.current && live.current.socket.readyState === WebSocket.OPEN && bridge.chat(id).controller === "user";
  const send = (message: object) => {
    if (controlling()) live.current!.socket.send(JSON.stringify(message));
  };
  const point = (e: React.MouseEvent | React.WheelEvent) => pagePoint(canvas.current!.getBoundingClientRect(), canvas.current!, live.current!, e);
  const moved = useRef(0);

  // Wheel needs a non-passive listener to keep the pane from scrolling.
  useEffect(() => {
    const el = canvas.current;
    if (!el) return;
    const wheel = (e: WheelEvent) => {
      if (!controlling()) return;
      e.preventDefault();
      send({ t: "mouse", type: "mouseWheel", ...pagePoint(el.getBoundingClientRect(), el, live.current!, e), deltaX: e.deltaX, deltaY: e.deltaY, modifiers: modifiers(e) });
    };
    el.addEventListener("wheel", wheel, { passive: false });
    return () => el.removeEventListener("wheel", wheel);
  });

  async function take() {
    try {
      await bridge.setController(id, "user");
      canvas.current?.focus();
    } catch (e) {
      toast((e as Error).message);
    }
  }
  async function handBack() {
    try {
      await bridge.setController(id, "model");
    } catch (e) {
      toast((e as Error).message);
      return;
    }
    const messages = handle.active?.messages || [];
    const last = messages[messages.length - 1];
    if (!handle.streaming && last?.activity?.some((e) => e.kind === "browser-handoff")) void handle.send("I’m done in the browser. Carry on.");
  }
  function wake() {
    bridge.update(id, { status: "starting" });
    connect(true);
  }

  const kind = note === "status" ? (c.status === "error" ? "error" : c.status === "starting" ? "starting" : "asleep") : note;
  const overlay =
    kind &&
    {
      connecting: (
        <>
          <Orbit className="size-5 animate-spin text-muted-foreground [animation-duration:2.5s]" />
          <strong className="font-medium">Connecting to the browser…</strong>
        </>
      ),
      starting: (
        <>
          <Orbit className="size-5 animate-spin text-muted-foreground [animation-duration:2.5s]" />
          <strong className="font-medium">Starting your browser…</strong>
          <p className="text-sm text-muted-foreground">It takes a few seconds the first time.</p>
        </>
      ),
      closed: (
        <>
          <Globe className="size-5 text-muted-foreground" />
          <strong className="font-medium">The live view closed.</strong>
          <Button size="sm" variant="outline" onClick={wake}>
            Reconnect
          </Button>
        </>
      ),
      error: (
        <>
          <Info className="size-5 text-muted-foreground" />
          <strong className="font-medium">The browser could not start.</strong>
          <p className="text-sm text-muted-foreground">{c.errorMessage || "Try again in a moment."}</p>
          <Button size="sm" variant="outline" onClick={wake}>
            Try again
          </Button>
        </>
      ),
      asleep: (
        <>
          <Globe className="size-5 text-muted-foreground" />
          <strong className="font-medium">The browser is asleep.</strong>
          <p className="text-sm text-muted-foreground">It wakes when {name} browses, or open it to look around yourself.</p>
          <Button size="sm" variant="outline" onClick={wake}>
            Open the browser
          </Button>
        </>
      ),
    }[kind];

  const nav = (Icon: typeof ArrowLeft, t: string, title: string) => (
    <Button type="button" variant="ghost" size="icon-xs" aria-label={title} title={title} disabled={!mine} onClick={() => send({ t })}>
      <Icon />
    </Button>
  );
  return (
    <div className="flex h-full min-h-0 flex-col" data-conversation={id}>
      <div role="status" className={cn("flex flex-wrap items-center gap-2 border-b px-3 py-2 text-sm", mine && "bg-blue/10", busy && "bg-success/5")}>
        {mine ? <Hand className="size-4 shrink-0 text-blue" /> : busy ? <span aria-hidden className="size-2 shrink-0 animate-pulse rounded-full bg-success" /> : <Globe className="size-4 shrink-0 text-muted-foreground" />}
        <strong className="font-medium">{mine ? "You have control" : busy ? `${name} is browsing` : "This chat’s tab"}</strong>
        <span className="min-w-0 flex-1 basis-40 text-muted-foreground">{mine ? `Clicks and keys go to the page. ${name} is paused and sees nothing.` : busy ? "Watch along, or take over." : `${name} uses it when it browses.`}</span>
        {mine ? (
          <Button size="xs" onClick={handBack}>
            Hand back
          </Button>
        ) : (
          <Button size="xs" variant="outline" onClick={take}>
            <Hand /> Take over
          </Button>
        )}
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Turn off the browser in this chat"
          title="Turn off the browser in this chat"
          onClick={async () => {
            try {
              await bridge.setOn(id, false);
              toast("Browser off for this chat");
            } catch (e) {
              toast((e as Error).message);
            }
          }}
        >
          <Power />
        </Button>
      </div>
      <form
        className="flex items-center gap-1 border-b px-2 py-1.5"
        autoComplete="off"
        onSubmit={(e) => {
          e.preventDefault();
          const url = address.trim();
          if (url) {
            send({ t: "go", url });
            canvas.current?.focus();
          }
        }}
      >
        {nav(ArrowLeft, "back", "Back")}
        {nav(ArrowRight, "forward", "Forward")}
        {nav(RotateCw, "reload", "Reload")}
        <label className="sr-only" htmlFor={`browser-url-${id}`}>
          Address
        </label>
        <input
          id={`browser-url-${id}`}
          className="h-7 min-w-0 flex-1 rounded-md border bg-background px-2 font-mono text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/50 read-only:text-muted-foreground"
          value={address}
          onChange={(e) => setAddress(e.target.value)}
          onFocus={() => setEditing(true)}
          onBlur={() => setEditing(false)}
          placeholder={mine ? "Type an address and press Enter" : "Take over to type an address"}
          readOnly={!mine}
          spellCheck={false}
        />
      </form>
      <div ref={screen} className="relative min-h-0 flex-1 bg-muted/40">
        <canvas
          ref={canvas}
          width={1280}
          height={720}
          tabIndex={0}
          aria-label="The chat's browser tab"
          className={cn("size-full object-contain outline-none", mine && "cursor-default ring-2 ring-blue/50 ring-inset", kind && "invisible")}
          onMouseDown={(e) => {
            if (!controlling()) return;
            canvas.current?.focus();
            e.preventDefault();
            send({ t: "mouse", type: "mousePressed", ...point(e), button: buttonOf(e), clickCount: e.detail || 1, modifiers: modifiers(e) });
          }}
          onMouseUp={(e) => {
            if (!controlling()) return;
            e.preventDefault();
            send({ t: "mouse", type: "mouseReleased", ...point(e), button: buttonOf(e), clickCount: e.detail || 1, modifiers: modifiers(e) });
          }}
          onMouseMove={(e) => {
            if (!controlling() || Date.now() - moved.current < 40) return;
            moved.current = Date.now();
            send({ t: "mouse", type: "mouseMoved", ...point(e), modifiers: modifiers(e) });
          }}
          onContextMenu={(e) => {
            if (controlling()) e.preventDefault();
          }}
          onKeyDown={(e) => {
            if (!controlling()) return;
            const message = keyMessage(e);
            if (!message) return;
            e.preventDefault();
            send(message);
          }}
          onKeyUp={(e) => {
            if (!controlling()) return;
            e.preventDefault();
            send(keyMessage(e, true)!);
          }}
          onPaste={(e) => {
            if (!controlling()) return;
            e.preventDefault();
            const text = e.clipboardData?.getData("text/plain");
            if (text) send({ t: "text", text });
          }}
        />
        {overlay && <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 p-6 text-center">{overlay}</div>}
      </div>
      <p className="truncate border-t px-3 py-1.5 text-xs text-faint">Chrome · yours · public internet only{c.title ? ` · ${c.title}` : ""}</p>
    </div>
  );
}
