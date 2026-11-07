"use client";

// Under a reply: the memories it was given ("Remembered 3 things"), and what Chat
// learned from the exchange, with Undo. The chat page renders <MemoryCards message={m} />.

import { useQueryClient } from "@tanstack/react-query";
import { Brain, ChevronDown, Sparkles } from "lucide-react";
import Link from "next/link";
import { useState, useSyncExternalStore } from "react";
import { toast } from "sonner";
import { messageOf } from "@/components/projects/form-field";
import { cn } from "@/lib/utils";
import { addMemory, deleteMemory, refreshMemory } from "./api";

interface Item {
  id: string;
  text: string;
  pinned?: boolean;
  change?: "added" | "updated" | "forgot";
}
interface Entry {
  kind: "recalled" | "learned";
  items?: Item[];
}

// Memories deleted or brought back from a reply in this session, so the card says so
// when the reply re-renders.
const done = new Map<string, "deleted" | "restored">();
const listeners = new Set<() => void>();
let version = 0;
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};
const mark = (id: string, state: "deleted" | "restored") => {
  done.set(id, state);
  version++;
  listeners.forEach((l) => l());
};

export const memoryEntries = (m: { activity?: unknown }, kind: Entry["kind"]) =>
  Array.isArray(m.activity) ? (m.activity as Entry[]).filter((e) => e?.kind === kind).flatMap((e) => e.items || []) : [];

const memoryHref = (id: string) => `/memory?m=${encodeURIComponent(id)}`;

export function MemoryCards({ message }: { message: { activity?: unknown } }) {
  useSyncExternalStore(
    subscribe,
    () => version,
    () => 0,
  );
  const client = useQueryClient();
  const [open, setOpen] = useState(false);
  const recalled = memoryEntries(message, "recalled");
  const learned = memoryEntries(message, "learned");
  if (!recalled.length && !learned.length) return null;

  async function forget(item: Item) {
    try {
      await deleteMemory(item.id);
      mark(item.id, "deleted");
      refreshMemory(client);
      toast("Memory deleted");
    } catch (e) {
      toast(messageOf(e));
    }
  }
  async function restore(item: Item) {
    try {
      const { memory } = await addMemory({ content: item.text });
      mark(item.id, "restored");
      refreshMemory(client);
      toast("Memory kept", { action: { label: "Open", onClick: () => window.location.assign(memoryHref(memory.id)) } });
    } catch (e) {
      toast(messageOf(e));
    }
  }
  return (
    <div className="my-2 grid gap-1.5">
      {!!recalled.length && (
        <div className="rounded-lg border text-sm">
          <button type="button" aria-expanded={open} onClick={() => setOpen((o) => !o)} className="flex w-full items-center gap-2 px-3 py-2 text-left text-muted-foreground hover:text-foreground">
            <Brain className="size-4 shrink-0" />
            <span className="grow">
              Remembered {recalled.length === 1 ? "1 thing" : `${recalled.length} things`} about you
            </span>
            <ChevronDown className={cn("size-4 transition-transform", open && "rotate-180")} />
          </button>
          {open && (
            <ul className="grid gap-0.5 border-t px-3 py-2">
              {recalled.map((item) => (
                <li key={item.id} className="flex items-baseline gap-2 py-0.5">
                  {done.get(item.id) === "deleted" ? (
                    <span className="grow text-muted-foreground line-through">{item.text}</span>
                  ) : (
                    <>
                      <Link href={memoryHref(item.id)} className="grow underline-offset-2 hover:underline">
                        {item.text}
                      </Link>
                      {item.pinned && <span className="text-xs text-faint">pinned</span>}
                      <button type="button" className="text-xs text-muted-foreground hover:text-foreground" onClick={() => forget(item)}>
                        Forget
                      </button>
                    </>
                  )}
                </li>
              ))}
              <li className="pt-1 text-xs">
                <Link href="/memory" className="text-muted-foreground underline-offset-2 hover:text-foreground hover:underline">
                  Manage memory
                </Link>
              </li>
            </ul>
          )}
        </div>
      )}
      {learned.map((item) => {
        const state = done.get(item.id);
        return (
          <div key={`${item.change}-${item.id}`} className="flex items-center gap-2 rounded-lg border px-3 py-2 text-sm">
            <Sparkles className="size-4 shrink-0 text-muted-foreground" />
            {state === "deleted" ? (
              <span className="grow text-muted-foreground">Forgot “{item.text}”</span>
            ) : state === "restored" ? (
              <span className="grow text-muted-foreground">Kept “{item.text}”</span>
            ) : (
              <>
                <span className="min-w-0 grow">
                  {item.change === "forgot" ? "Forgot" : item.change === "updated" ? "Updated memory:" : "Remembered"}{" "}
                  {item.change === "forgot" ? (
                    <span className="text-muted-foreground">“{item.text}”</span>
                  ) : (
                    <Link href={memoryHref(item.id)} className="font-medium underline-offset-2 hover:underline">
                      “{item.text}”
                    </Link>
                  )}
                </span>
                {item.change === "forgot" ? (
                  <button type="button" className="text-xs text-muted-foreground hover:text-foreground" onClick={() => restore(item)}>
                    Keep it
                  </button>
                ) : item.change === "updated" ? (
                  <Link href={memoryHref(item.id)} className="text-xs text-muted-foreground hover:text-foreground">
                    Edit
                  </Link>
                ) : (
                  <button type="button" className="text-xs text-muted-foreground hover:text-foreground" onClick={() => forget(item)}>
                    Undo
                  </button>
                )}
              </>
            )}
          </div>
        );
      })}
    </div>
  );
}
