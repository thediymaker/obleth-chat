"use client";

// The card under a reply that Chat also put to other models (lib/chat/compare): who
// else was asked and why, lit while they are still answering, and the way to read
// the answers side by side and keep one.

import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { compareGround, compareState } from "@/lib/chat/compare";
import type { Message } from "@/lib/chat/types";
import { useChatStore } from "./chat-provider";
import { CompareDialog } from "./compare-dialog";

const list = (words: string[]) => (words.length < 3 ? words.join(" and ") : `${words.slice(0, -1).join(", ")} and ${words.at(-1)}`);

export function CompareCard({ m }: { m: Message }) {
  const store = useChatStore();
  const [open, setOpen] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const chatId = store.active?.id;
  const note = m.settings?.compare;
  // One of the other answers, kept: it has no note of its own, only its place among the versions.
  const kept = typeof m.settings?.compareOf === "string";
  const versions = m.versions?.length || 1;
  const state = compareState(note, versions, now);
  const pending = !!state?.pending;
  // While answers may still come, look for them; the reply's versions say when they are in.
  useEffect(() => {
    if (!pending || !chatId) return;
    const timer = setInterval(() => {
      setNow(Date.now());
      if (!store.streaming && !store.switchingVersion) void store.reconcileConversation(chatId);
    }, 4000);
    return () => clearInterval(timer);
  }, [pending, chatId, store]);
  if (!m.id || !chatId || store.shared || m.status !== "completed") return null;
  if (!state && !(kept && versions > 1)) return null;
  const names = list((note?.models || []).map((id) => store.modelName(id)));
  const text = state
    ? state.arrived
      ? `${compareGround(note?.why)}, so Chat also asked ${names}.`
      : `${compareGround(note?.why)}, so ${names} ${(note?.models || []).length > 1 ? "are" : "is"} answering this too.`
    : `One of ${versions} answers to this message.`;
  const ready = !state || state.arrived > 0;
  return (
    <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-2 rounded-lg border bg-card px-3 py-2" data-compare-card={pending ? "waiting" : "ready"}>
      {pending && (
        <span className="relative inline-flex size-2 shrink-0" aria-hidden="true">
          <span className="signal-pulse absolute inset-0 rounded-full bg-signal" />
          <span className="absolute inset-0 rounded-full bg-signal" />
        </span>
      )}
      <p className="min-w-0 flex-1 basis-56 text-sm leading-normal text-text-2" role={pending ? "status" : undefined}>
        {text}
      </p>
      {ready && (
        <Button size="sm" variant="outline" onClick={() => setOpen(true)}>
          Compare the answers
        </Button>
      )}
      {open && <CompareDialog chatId={chatId} messageId={m.id} open onOpenChange={setOpen} waiting={pending} />}
    </div>
  );
}
