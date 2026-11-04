"use client";

// The browser's cards at the end of a reply: the model's
// browser-request ("This needs the web": Not now, or Turn on browser, which turns it
// on and tells the model to go ahead), and the browser-handoff ("Your turn in the
// browser": the person does what the page needs, then hands back). Rendered after
// the question cards of every assistant message.

import { Globe, Hand } from "lucide-react";
import { useChat } from "@/components/chat/chat-provider";
import { Button } from "@/components/ui/button";
import { useBranding } from "@/lib/branding-context";
import type { Message } from "@/lib/chat/types";
import { cn } from "@/lib/utils";

const entryOf = (m: Message | undefined, kind: string) => (Array.isArray(m?.activity) ? m!.activity : []).find((e) => e.kind === kind);
const keyOf = (m: Message, index: number) => m.id || `message-${index}`;

function Closed({ icon: Icon, children }: { icon: typeof Globe; children: React.ReactNode }) {
  return (
    <div className="mt-3 flex gap-2.5 rounded-lg border px-3 py-2.5 text-md">
      <Icon className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
      <div className="grid min-w-0 flex-1 gap-1">{children}</div>
    </div>
  );
}

function Card({ icon: Icon, label, title, children, actions, className }: { icon: typeof Globe; label: string; title: string; children: React.ReactNode; actions: React.ReactNode; className?: string }) {
  return (
    <section className={cn("mt-3 flex flex-wrap gap-3 rounded-lg border bg-card p-3", className)} aria-label={label}>
      <span className="flex size-8 shrink-0 items-center justify-center rounded-md bg-secondary">
        <Icon className="size-4" />
      </span>
      <div className="min-w-0 flex-1 basis-56">
        <strong className="text-md font-semibold">{title}</strong>
        <div className="grid gap-1 text-md text-text-2">{children}</div>
      </div>
      <div className="flex items-start gap-1.5 self-end">{actions}</div>
    </section>
  );
}

export function BrowserCard({ m, index, isLast, streaming }: { m: Message; index: number; isLast: boolean; streaming: boolean }) {
  const store = useChat();
  const name = useBranding().assistantName || "The assistant";
  if (m.role !== "assistant") return null;
  const request = entryOf(m, "browser-request"),
    handoff = entryOf(m, "browser-handoff");
  if (!request && !handoff) return null;
  const bridge = store.browser;
  const id = store.active?.id || "";
  const on = bridge.on(id);
  const c = bridge.chat(id);
  const key = keyOf(m, index);
  const offer = bridge.offer(key);
  const open = isLast && m.status === "completed" && !streaming;

  let requestCard: React.ReactNode = null;
  if (request) {
    if (on || offer === "done")
      requestCard = (
        <Closed icon={Globe}>
          <p>
            <strong className="font-medium">Browser</strong> Turned on for this chat.
          </p>
        </Closed>
      );
    else if (!open || offer === "dismissed")
      requestCard = (
        <Closed icon={Globe}>
          <p>
            <strong className="font-medium">Browser</strong> {request.reason || "Asked to use the browser."}
          </p>
          {offer === "dismissed" && <p className="text-faint">Not turned on. Use the Browser button below whenever you want it.</p>}
        </Closed>
      );
    else {
      const busy = offer === "busy";
      requestCard = (
        <Card
          icon={Globe}
          label="Turn on the browser"
          title="This needs the web"
          actions={
            <>
              <Button size="sm" variant="ghost" disabled={busy} onClick={() => bridge.setOffer(key, "dismissed")}>
                Not now
              </Button>
              <Button
                size="sm"
                disabled={busy}
                onClick={() =>
                  store.run(async () => {
                    if (!id || !isLast) return;
                    bridge.setOffer(key, "busy");
                    try {
                      await bridge.setOn(id, true);
                    } catch (error) {
                      bridge.setOffer(key, null);
                      throw error;
                    }
                    bridge.setOffer(key, "done");
                    await store.send("The browser is on. Go ahead.");
                  })
                }
              >
                {busy ? "Turning on…" : "Turn on browser"}
              </Button>
            </>
          }
        >
          <p>{request.reason || "A browser for this chat, where the model searches and reads pages while you watch."}</p>
        </Card>
      );
    }
  }

  let handoffCard: React.ReactNode = null;
  if (handoff) {
    const waiting = open && on && c.controller === "user";
    handoffCard = waiting ? (
      <Card
        icon={Hand}
        label="Your turn in the browser"
        title="Your turn in the browser"
        className="border-blue/40"
        actions={
          <>
            <Button size="sm" variant="ghost" onClick={() => bridge.show(id)}>
              Show browser
            </Button>
            <Button
              size="sm"
              onClick={() =>
                store.run(async () => {
                  await bridge.setController(id, "model");
                  if (!store.streaming) await store.send("I’m done in the browser. Carry on.");
                })
              }
            >
              Hand back
            </Button>
          </>
        }
      >
        <p>{handoff.reason || "The page needs you."}</p>
        <p className="text-sm text-faint">{name} is paused and sees nothing, not even what you type, until you hand back.</p>
      </Card>
    ) : (
      <Closed icon={Hand}>
        <p>
          <strong className="font-medium">Browser</strong> {handoff.reason || "Asked you to take over."}
        </p>
        {isLast && c.controller !== "user" && <p className="text-faint">Handed back.</p>}
      </Closed>
    );
  }
  return (
    <>
      {requestCard}
      {handoffCard}
    </>
  );
}
