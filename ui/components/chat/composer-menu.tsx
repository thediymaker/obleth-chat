"use client";

// The lists the message box opens while typing. "/" as the first thing in the box
// lists what can be done here: to the chat, to the next message, to the page. "@"
// lists what a message can point at: a file in the chat's workspace, one of the
// person's jobs, a knowledge collection, a connected tool. Picking a collection or
// a tool also turns it on for the chat, so the model can use what was named.

import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { toast } from "sonner";
import { useClusterQueues } from "@/components/clusters/status";
import { setChatTool, useChatTools } from "@/components/connections/chat-tools";
import { KINDS } from "@/components/connections/connections";
import { useChatKnowledge } from "@/components/knowledge/chat-knowledge";
import { useChatDraft } from "@/lib/chat/chat-draft";
import { EFFORTS, effortLabel } from "@/lib/chat/composer";
import { fileMention, marked, type MenuItem } from "@/lib/chat/composer-menu";
import { exportChats } from "@/lib/chat/history";
import { chatMenuItems, type ChatMenuAction } from "@/lib/chat/temporary";
import { count } from "@/lib/format";
import { cn } from "@/lib/utils";
import { OPEN_SEARCH_EVENT, useChat } from "./chat-provider";

/** A row of either list. `run` does it; what it returns takes the place of the "@…" typed. */
export interface MenuEntry extends MenuItem {
  run: () => string | void;
}

const CHAT_SLASH: Record<ChatMenuAction, [string, string]> = {
  "rename-chat": ["rename", "title name"],
  "pin-chat": ["pin", "keep top"],
  "archive-chat": ["archive", "hide restore"],
  "share-chat": ["share", "link"],
  "export-chat": ["export", "download json"],
  "delete-chat": ["delete", "remove"],
};

/** What "/" lists, the things that fit this moment first. */
export function useCommands({ home, attach, knowledge }: { home: boolean; attach: () => void; knowledge: (() => void) | null }): MenuEntry[] {
  const store = useChat();
  const draft = useChatDraft();
  const id = store.route.id;
  const chat = id && store.active?.id === id ? store.active : null;
  const own = !!chat && !store.shared;
  const last = chat?.messages?.at(-1);
  const list: (MenuEntry | false | null | undefined)[] = [
    store.streaming && { id: "stop", group: "This chat", slash: "stop", label: "Stop the reply", keywords: "cancel halt", run: () => void store.run(() => store.stopCurrent()) },
    own &&
      !store.streaming &&
      last?.role === "assistant" &&
      !!last.id && {
        id: "regenerate",
        group: "This chat",
        slash: "regenerate",
        label: "Write the last reply again",
        keywords: "retry redo again",
        run: () => void store.run(() => store.forkAndSend(last.id!, "regenerate")),
      },
    own && { id: "details", group: "This chat", slash: "details", label: "Chat details", keywords: "context usage info window", run: () => store.toggleDetails(id) },
    store.workspace.enabled() &&
      !store.streaming &&
      (!id || own) && {
        id: "workspace",
        group: id ? "This chat" : "The new chat",
        slash: "workspace",
        label: !id ? (store.workspaceDraft ? "Start without a workspace" : "Start with a workspace") : store.workspace.has(id) ? "Show or hide the workspace" : "Give this chat a workspace",
        keywords: "sandbox preview files terminal build",
        run: () => void store.run(() => store.toggleWorkspace()),
      },
    store.browser.enabled() &&
      !store.streaming &&
      (!id || own) && {
        id: "browser",
        group: id ? "This chat" : "The new chat",
        slash: "browser",
        label: !id ? (store.browserDraft ? "Start without a browser" : "Start with a browser") : store.browser.on(id) ? "Show the browser, or turn it off" : "Give this chat a browser",
        keywords: "web page site",
        run: () => void store.run(() => store.toggleBrowser()),
      },
    home && !draft.project && { id: "temporary", group: "The new chat", slash: "temporary", label: store.temporaryDraft ? "Keep this chat after all" : "Make this a temporary chat", keywords: "incognito private forget", run: () => store.toggleTemporary() },
    !store.streaming && !store.attachments.full() && { id: "attach", group: "Add to the message", slash: "attach", label: "Attach files", keywords: "upload file", run: attach },
    knowledge && !store.streaming && { id: "knowledge", group: "Add to the message", slash: "knowledge", label: "Choose knowledge collections", keywords: "library documents search", run: knowledge },
    ...(own
      ? chatMenuItems(chat).map(([, label, action]): MenuEntry | false => {
          const run: Record<ChatMenuAction, () => void> = {
            "rename-chat": () => store.openDialog({ kind: "rename", id, title: chat.title }),
            "pin-chat": () => void store.run(() => store.update(id, { pinned: !chat.pinned })),
            "archive-chat": () => void store.run(() => store.update(id, { archived: !chat.archived })),
            "share-chat": () => store.openDialog({ kind: "share", id }),
            "export-chat": () => void exportChats([id]).catch((e: Error) => toast(e.message)),
            "delete-chat": () => store.openDialog({ kind: "delete", id }),
          };
          // Deleting waits until the reply has stopped, as in the chat's menu.
          if (action === "delete-chat" && store.streaming) return false;
          return { id: action, group: "Keep or share", slash: CHAT_SLASH[action][0], label: `${label.replace("…", "")} this chat`, keywords: CHAT_SLASH[action][1], run: run[action] };
        })
      : []),
    {
      id: "new",
      group: "Go",
      slash: "new",
      label: "New chat",
      keywords: "start fresh clear",
      run: () => store.startNew(),
    },
    { id: "search", group: "Go", slash: "search", label: "Search chats", keywords: "find history", run: () => void window.dispatchEvent(new Event(OPEN_SEARCH_EVENT)) },
    { id: "shortcuts", group: "Go", slash: "shortcuts", label: "Keyboard shortcuts", keywords: "keys help", run: () => store.openDialog({ kind: "shortcuts" }) },
    ...(store.streaming || store.shared
      ? []
      : EFFORTS.filter((e) => e !== store.reasoningEffort).map(
          (effort): MenuEntry => ({
            id: `effort-${effort}`,
            group: "Thinking effort",
            slash: "effort",
            label: effortLabel(effort),
            keywords: "thinking reasoning",
            run: () => {
              store.setEffort(effort);
              store.saveEffort();
            },
          }),
        )),
    ...(store.streaming || store.shared
      ? []
      : store.models
          .filter((m) => m.id !== store.model)
          .map((m): MenuEntry => ({ id: `model-${m.id}`, group: "Model", slash: "model", label: store.modelName(m.id), keywords: m.id, run: () => void store.selectModel(m.id) }))),
    ...(store.streaming || store.shared ? [] : store.presets.map((p): MenuEntry => ({ id: `preset-${p.id}`, group: "Preset", slash: "preset", label: p.name, keywords: p.model, run: () => store.applyPreset(p) }))),
  ];
  return list.filter((entry): entry is MenuEntry => !!entry);
}

/** What "@" lists for this chat (or the new one, with no id). Reads only while `open`. */
export function useMentions(chatId: string | null, open: boolean): MenuEntry[] {
  const store = useChat();
  const queries = useQueryClient();
  const tools = useChatTools(chatId);
  const knowledge = useChatKnowledge(chatId);
  const queues = useClusterQueues(open);
  const space = chatId && store.workspace.enabled() && store.workspace.has(chatId) ? store.workspace.space(chatId) : null;
  const stale = !!space && (space.files == null || space.filesStale) && !space.loadingFiles;
  useEffect(() => {
    if (open && stale && chatId) void store.workspace.loadFiles(chatId);
  }, [open, stale, chatId, store]);
  if (!open) return [];
  const files = (space?.files ?? [])
    .filter((f) => f.type !== "directory")
    .map((f): MenuEntry => ({ id: `file-${f.path}`, group: "Files in the workspace", label: f.path, mono: true, run: () => fileMention(f.path) }));
  const jobs = queues.flatMap((q) =>
    q.jobs.map(
      (j): MenuEntry => ({
        id: `job-${q.cluster.id}-${j.id}`,
        group: "Your jobs",
        label: j.name,
        note: [j.state.toLowerCase(), q.cluster.name].filter(Boolean).join(" · "),
        keywords: `${j.id} ${q.cluster.name} job`,
        run: () => (j.name && j.name !== j.id ? `job ${j.id} (${j.name}) on ${q.cluster.name}` : `job ${j.id} on ${q.cluster.name}`),
      }),
    ),
  );
  const collections = knowledge.enabled
    ? knowledge.collections.map((c): MenuEntry => {
        const on = knowledge.ids.includes(c.id);
        return {
          id: `knowledge-${c.id}`,
          group: "Knowledge",
          label: c.name,
          note: on ? "searched by this chat" : count(c.documents, "document"),
          keywords: "collection library",
          run: () => {
            if (!on) void knowledge.toggle(c.id);
            return `the ${c.name} collection`;
          },
        };
      })
    : [];
  const connected = tools.list.map((c): MenuEntry => {
    const kind = KINDS[c.kind as keyof typeof KINDS]?.label ?? "Tool";
    return {
      id: `tool-${c.id}`,
      group: "Tools",
      label: c.name,
      note: c.on ? kind : `${kind} · off for this chat`,
      keywords: `${c.kind} tool connection cluster`,
      run: () => {
        if (!c.on) void setChatTool(chatId, c.id, true, queries).catch((e: Error) => toast(e.message));
        return c.kind === "slurm" || c.kind === "kubernetes" ? `the ${c.name} cluster` : c.name;
      },
    };
  });
  return [...files, ...jobs, ...collections, ...connected];
}

/** The list itself, over or under the box. The box keeps the focus; the keys come from it. */
export function ComposerMenu({ id, label, items, active, query, below, onActive, onPick }: { id: string; label: string; items: MenuEntry[]; active: number; query: string; below: boolean; onActive: (index: number) => void; onPick: (item: MenuEntry) => void }) {
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => {
    list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: "nearest" });
  }, [active, items]);
  return (
    <div
      className={cn("absolute left-0 z-30 w-[min(28rem,100%)] rounded-xl border border-line-strong bg-popover text-popover-foreground shadow-[0_16px_48px_var(--shadow-soft)]", below ? "top-full mt-2" : "bottom-full mb-2")}
      // A press in the list keeps the caret where it is.
      onMouseDown={(e) => e.preventDefault()}
    >
      <div ref={list} id={id} role="listbox" aria-label={label} className="max-h-72 overflow-y-auto overscroll-contain p-1.5">
        {items.map((item, index) => {
          const [before, match, after] = marked(item.label, item.slash ? "" : query);
          const heading = item.group !== items[index - 1]?.group;
          return (
            <div key={item.id} className="contents">
              {heading && (
                <div role="presentation" className={cn("px-2.5 pb-1 text-2xs font-medium tracking-[.08em] text-muted-foreground uppercase", index ? "pt-2.5" : "pt-1.5")}>
                  {item.group}
                </div>
              )}
              <div
                id={`${id}-${index}`}
                role="option"
                aria-selected={index === active}
                data-menu-item={item.id}
                className={cn("flex min-h-9 cursor-default items-center gap-2.5 rounded-md px-2.5 text-md text-text-2", index === active && "bg-secondary text-foreground")}
                onMouseMove={() => index !== active && onActive(index)}
                onClick={() => onPick(item)}
              >
                {item.slash && <span className="w-[5.5rem] shrink-0 truncate font-mono text-sm text-muted-foreground">/{item.slash}</span>}
                <span className={cn("min-w-0 flex-1 truncate", item.mono && "font-mono text-sm")}>
                  {before}
                  {match && <strong className="font-semibold text-foreground">{match}</strong>}
                  {after}
                </span>
                {item.note && <span className="shrink-0 text-xs text-muted-foreground">{item.note}</span>}
              </div>
            </div>
          );
        })}
      </div>
      <p className="border-t border-line-soft px-3 py-1.5 text-xs text-muted-foreground">↑ ↓ to move · ⏎ to {items[active]?.slash ? "do it" : "add it"} · esc to close</p>
    </div>
  );
}
