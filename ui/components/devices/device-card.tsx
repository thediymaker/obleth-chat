"use client";

// The device-request card at the end of a reply: the model asked for a device
// plugged into the person's computer ("This needs a device at your computer":
// Not now, or Attach device, which opens the browser's port chooser, names the
// device, and tells the model to go ahead). Rendered after the browser cards of
// every assistant message.

import { Cpu } from "lucide-react";
import { useState } from "react";
import { useChat } from "@/components/chat/chat-provider";
import { AddDeviceDialog, type Picked } from "@/components/devices/devices-page";
import { Button } from "@/components/ui/button";
import type { Message } from "@/lib/chat/types";

const keyOf = (m: Message, index: number) => m.id || `message-${index}`;

export function DeviceCard({ m, index, isLast, streaming }: { m: Message; index: number; isLast: boolean; streaming: boolean }) {
  const store = useChat();
  const [picked, setPicked] = useState<Picked | null>(null);
  const [error, setError] = useState("");
  if (m.role !== "assistant") return null;
  const request = (Array.isArray(m.activity) ? m.activity : []).find((e) => e.kind === "device-request");
  if (!request) return null;
  const bridge = store.devices;
  const key = keyOf(m, index);
  const offer = bridge.offer(key);
  const open = isLast && m.status === "completed" && !streaming;
  const name = String(request.name || "").trim();

  if (offer === "done")
    return (
      <div className="mt-3 flex gap-2.5 rounded-lg border px-3 py-2.5 text-md">
        <Cpu className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <p>
          <strong className="font-medium">Device</strong> Attached for your chats.
        </p>
      </div>
    );
  if (!open || offer === "dismissed")
    return (
      <div className="mt-3 flex gap-2.5 rounded-lg border px-3 py-2.5 text-md">
        <Cpu className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div className="grid min-w-0 flex-1 gap-1">
          <p>
            <strong className="font-medium">Device</strong> {request.reason || "Asked for a device at your computer."}
          </p>
          {offer === "dismissed" && <p className="text-faint">Not attached. Add it under Compute › Devices whenever you want it.</p>}
        </div>
      </div>
    );
  const busy = offer === "busy";
  const supported = bridge.supported();
  return (
    <>
      <section className="mt-3 flex flex-wrap gap-3 rounded-lg border bg-card p-3" aria-label="Attach a device">
        <span className="flex size-8 shrink-0 items-center justify-center rounded-md bg-secondary">
          <Cpu className="size-4" />
        </span>
        <div className="min-w-0 flex-1 basis-56">
          <strong className="text-md font-semibold">This needs a device at your computer</strong>
          <div className="grid gap-1 text-md text-text-2">
            <p>{request.reason || "A board or serial device plugged into this computer, reached through this browser."}</p>
            {!supported && <p className="text-amber">This browser has no Web Serial support. Open the chat in Chrome or Edge to attach it.</p>}
            {error && <p className="text-destructive-foreground">{error}</p>}
          </div>
        </div>
        <div className="flex items-start gap-1.5 self-end">
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => bridge.setOffer(key, "dismissed")}>
            Not now
          </Button>
          <Button
            size="sm"
            disabled={busy || !supported}
            onClick={async () => {
              setError("");
              bridge.setOffer(key, "busy");
              try {
                setPicked(await bridge.pick());
              } catch (e) {
                bridge.setOffer(key, null);
                if ((e as Error).name !== "NotFoundError") setError((e as Error).message);
              }
            }}
          >
            {busy ? "Attaching…" : "Attach device"}
          </Button>
        </div>
      </section>
      <AddDeviceDialog
        picked={picked}
        suggested={name || undefined}
        onClose={() => {
          setPicked(null);
          if (bridge.offer(key) === "busy") bridge.setOffer(key, null);
        }}
        onAdded={(item) =>
          store.run(async () => {
            setPicked(null);
            bridge.setOffer(key, "done");
            if (isLast) await store.send(`${item.name} is attached. Go ahead.`);
          })
        }
      />
    </>
  );
}
