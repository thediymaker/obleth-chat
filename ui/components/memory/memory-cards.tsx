"use client";

// Under a reply: the memories it was given ("Remembered 3 things"), and what Chat
// learned from the exchange, with Undo. A memory learned in a chat in a project shows
// where it went, for this project or everywhere, and changes with one click. The chat
// page renders <MemoryCards message={m} />.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Brain, Check, ChevronDown, Globe, Sparkles } from "lucide-react";
import Link from "next/link";
import { useState, useSyncExternalStore } from "react";
import { toast } from "sonner";
import { projectDot } from "@/components/app-shell/sidebar";
import { api } from "@/lib/api";
import { useProjects, useSignedIn } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { addMemory, changeMemory, deleteMemory, MEMORY_KEY, refreshMemory, type MemoryState } from "./api";
import { messageOf } from "@/components/manage/kit";
import { scopeMeaning } from "./scope";

interface Item {
  id: string;
  text: string;
  pinned?: boolean;
  change?: "added" | "updated" | "forgot";
  /** Learned in a chat in a project: where it went (null: everywhere)… */
  projectId?: string | null;
  /** …and the chat's project, the other place it can go. */
  chatProjectId?: string;
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
// Where memories were moved from a reply in this session, until the Memory list says.
const moved = new Map<string, string | null>();
const markMoved = (id: string, projectId: string | null) => {
  moved.set(id, projectId);
  version++;
  listeners.forEach((l) => l());
};
const unmarkMoved = (id: string) => {
  moved.delete(id);
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
  const scoped = learned.some((i) => i.change === "added" && i.chatProjectId);
  const signedIn = useSignedIn();
  // Where each memory is now, so the card is right after a reload or a move elsewhere.
  const memories = useQuery({ queryKey: MEMORY_KEY, queryFn: () => api.request<MemoryState>("/api/memory"), enabled: signedIn && scoped });
  const projects = useProjects().data?.projects;
  if (!recalled.length && !learned.length) return null;

  /** The project the memory is in now, or null for everywhere. */
  function placeOf(item: Item): string | null {
    if (moved.has(item.id)) return moved.get(item.id) ?? null;
    const now = memories.data?.memories.find((m) => m.id === item.id);
    if (now) return now.projectId;
    return item.projectId ?? null;
  }
  async function move(item: Item, projectId: string | null, name: string) {
    try {
      await changeMemory(item.id, { projectId });
      markMoved(item.id, projectId);
      toast(projectId ? "Kept for this project" : "Moved to everywhere", { description: scopeMeaning(projectId ? name : null) });
      // Once the list has it, the list is right again, also after a move elsewhere.
      await refreshMemory(client);
      if (client.getQueryData<MemoryState>(MEMORY_KEY)?.memories.some((m) => m.id === item.id)) unmarkMoved(item.id);
    } catch (e) {
      toast(messageOf(e));
    }
  }

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
        const project = item.change === "added" && item.chatProjectId ? projects?.find((p) => p.id === item.chatProjectId) : undefined;
        const row = (
          <div className="flex min-w-0 items-center gap-2 text-sm">
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
                  <button type="button" className="shrink-0 text-xs text-muted-foreground hover:text-foreground" onClick={() => restore(item)}>
                    Keep it
                  </button>
                ) : item.change === "updated" ? (
                  <Link href={memoryHref(item.id)} className="shrink-0 text-xs text-muted-foreground hover:text-foreground">
                    Edit
                  </Link>
                ) : (
                  <button type="button" className="shrink-0 text-xs text-muted-foreground hover:text-foreground" onClick={() => forget(item)}>
                    Undo
                  </button>
                )}
              </>
            )}
          </div>
        );
        if (!project || state) return <div key={`${item.change}-${item.id}`} className="rounded-lg border px-3 py-2">{row}</div>;
        const place = placeOf(item);
        const here = place === project.id;
        const choice = (selected: boolean, label: React.ReactNode, title: string, onClick: () => void) => (
          <button
            type="button"
            role="radio"
            aria-checked={selected}
            title={title}
            onClick={selected ? undefined : onClick}
            className={cn(
              "inline-flex max-w-full min-w-0 items-center gap-1.5 rounded-full border px-2.5 py-0.5",
              selected ? "border-foreground/25 bg-accent font-medium text-foreground" : "text-muted-foreground hover:bg-accent/60 hover:text-foreground",
            )}
          >
            {label}
          </button>
        );
        return (
          <div key={`${item.change}-${item.id}`} className="grid grid-cols-[minmax(0,1fr)] gap-1.5 rounded-lg border px-3 py-2">
            {row}
            <div role="radiogroup" aria-label="Where Chat uses this memory" className="flex min-w-0 flex-wrap items-center gap-1.5 pl-6 text-xs">
              {choice(
                here,
                <>
                  {here ? <Check className="size-3 shrink-0" aria-hidden="true" /> : <span className={cn("size-2 shrink-0 rounded-full", projectDot(project.color))} aria-hidden="true" />}
                  <span className="truncate">For this project</span>
                </>,
                `${project.name}. ${scopeMeaning(project.name)}`,
                () => move(item, project.id, project.name),
              )}
              {choice(
                !here,
                <>
                  {!here ? <Check className="size-3 shrink-0" aria-hidden="true" /> : <Globe className="size-3 shrink-0" aria-hidden="true" />}
                  <span className="truncate">Everywhere</span>
                </>,
                scopeMeaning(null),
                () => move(item, null, project.name),
              )}
              <span className="min-w-0 basis-full truncate text-muted-foreground sm:flex-1 sm:basis-0" title={scopeMeaning(here ? project.name : null)}>
                {scopeMeaning(here ? project.name : null)}
              </span>
            </div>
          </div>
        );
      })}
    </div>
  );
}
