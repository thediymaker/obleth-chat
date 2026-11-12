"use client";

// The chat side of connections, for the chat page to render: which clusters and MCP
// servers the model may use in one chat, and Auto mode (tools run without asking).
// Every active connection is on unless switched off for that chat. A new chat keeps
// its choices here until it is created; then `applyToolsDraft(id)` stores them.
//
//   <ToolsButton chatId={id} disabled={streaming} />   the composer's Tools 2/3 button
//   <ToolsContextSection chatId={id} />                the context panel's Tools block
//   useChatTools(id).auto                              whether Auto mode is on
//   applyToolsDraft(id), enableAuto(id), resetToolsDraft(), startAutoDraft()

import { useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { Link2, Zap } from "lucide-react";
import Link from "next/link";
import { useState, useSyncExternalStore } from "react";
import { toast } from "sonner";
import { Eyebrow } from "@/components/app-shell/page";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { DetailRow, FormActions, FormDialog, Pill } from "@/components/manage/kit";
import { api } from "@/lib/api";
import { useCapabilities } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { KINDS, kindOf, plural, type Connection } from "./connections";

export interface ChatTool {
  id: string;
  kind: string;
  name: string;
  shared?: boolean;
  /** How many tools it offers. */
  tools: number;
  on: boolean;
}

// The new chat's choices, before it exists.
let draft = { off: new Set<string>(), auto: false };
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};
const setDraft = (next: Partial<typeof draft>) => {
  draft = { ...draft, ...next };
  emit();
};
const useDraft = () => useSyncExternalStore(subscribe, () => draft, () => draft);

const chatPath = (id: string) => `/api/conversations/${encodeURIComponent(id)}/connections`;
export const chatToolsKey = (id: string | null) => ["chat-tools", id ?? ""] as const;

/** The tools of one chat (or of the new chat, with no id), and whether Auto mode is on. */
export function useChatTools(chatId: string | null) {
  const capabilities = useCapabilities().data;
  const enabled = !!capabilities?.features?.connections;
  const autoAllowed = !!capabilities?.features?.autoMode;
  const d = useDraft();
  const query = useQuery({
    queryKey: chatToolsKey(chatId),
    enabled,
    queryFn: async (): Promise<{ list: ChatTool[]; auto: boolean }> => {
      try {
        if (chatId) {
          const r = await api.request<{ connections?: ChatTool[]; auto?: boolean }>(chatPath(chatId));
          return { list: r.connections ?? [], auto: !!r.auto };
        }
        const r = await api.request<{ connections?: Connection[] }>("/api/connections");
        return { list: (r.connections ?? []).filter((c) => c.active).map((c) => ({ id: c.id, kind: c.kind, name: c.name, shared: c.shared, tools: c.tools.length, on: true })), auto: false };
      } catch (e) {
        if ((e as Error).name === "AbortError") throw e;
        return { list: [], auto: false };
      }
    },
  });
  const raw = query.data ?? { list: [], auto: false };
  const list = chatId ? raw.list : raw.list.map((c) => ({ ...c, on: !d.off.has(c.id) }));
  const auto = chatId ? raw.auto : d.auto;
  return { list, auto: auto && autoAllowed, autoAllowed, on: list.filter((c) => c.on).length, total: list.length };
}

/** Stores what was chosen before a new chat existed. */
export async function applyToolsDraft(id: string, queries?: QueryClient) {
  const off = [...draft.off];
  const auto = draft.auto;
  setDraft({ off: new Set(), auto: false });
  if (auto) await api.request(chatPath(id), { method: "PATCH", body: { auto: true } }).catch(() => {});
  await Promise.all(off.map((connectionId) => api.request(chatPath(id), { method: "PATCH", body: { connectionId, on: false } }).catch(() => {})));
  await queries?.invalidateQueries({ queryKey: chatToolsKey(id) });
}

/** Research turns Auto mode on for its chat, where the installation allows it. */
export async function enableAuto(id: string, autoAllowed: boolean, queries?: QueryClient) {
  if (!autoAllowed) return;
  await api.request(chatPath(id), { method: "PATCH", body: { auto: true } }).catch(() => {});
  await queries?.invalidateQueries({ queryKey: chatToolsKey(id) });
}

/** A new chat starts from every active connection again, with Auto off. */
export const resetToolsDraft = () => setDraft({ off: new Set(), auto: false });
/** A new chat that should start in Auto mode, such as research from a project. */
export const startAutoDraft = () => setDraft({ auto: true });

/** The dialog that switches each connection, and Auto mode, for this chat. */
export function ToolsChooser({ chatId, open, onOpenChange }: { chatId: string | null; open: boolean; onOpenChange: (open: boolean) => void }) {
  const queries = useQueryClient();
  const features = useCapabilities().data?.features;
  const clusters = !!features?.clusters;
  const devices = !!features?.devices;
  const { list, auto, autoAllowed } = useChatTools(chatId);
  const patch = (fn: (old: { list: ChatTool[]; auto: boolean }) => { list: ChatTool[]; auto: boolean }) =>
    queries.setQueryData<{ list: ChatTool[]; auto: boolean }>(chatToolsKey(chatId), (old) => (old ? fn(old) : old));
  async function switchOne(item: ChatTool) {
    const on = !item.on;
    try {
      if (chatId) await api.request(chatPath(chatId), { method: "PATCH", body: { connectionId: item.id, on } });
      else {
        const off = new Set(draft.off);
        if (on) off.delete(item.id);
        else off.add(item.id);
        setDraft({ off });
      }
      if (chatId) patch((old) => ({ ...old, list: old.list.map((c) => (c.id === item.id ? { ...c, on } : c)) }));
    } catch (e) {
      toast((e as Error).message);
    }
  }
  async function switchAuto() {
    const next = !auto;
    try {
      if (chatId) {
        await api.request(chatPath(chatId), { method: "PATCH", body: { auto: next } });
        patch((old) => ({ ...old, auto: next }));
      } else setDraft({ auto: next });
    } catch (e) {
      toast((e as Error).message);
    }
  }
  return (
    <FormDialog open={open} onOpenChange={onOpenChange} title="Tools in this chat" description="The model can use the clusters, devices and MCP servers that are on. Changes apply from the next message.">
      {autoAllowed && (
        <div className="flex items-center gap-3 rounded-md border p-3">
          <Zap className="size-4 shrink-0 text-amber" />
          <div className="min-w-0 grow">
            <h3 className="text-md font-medium">Auto mode</h3>
            <p className="text-sm text-muted-foreground">Every tool runs without asking, including ones that submit, apply or delete. For long research runs you won’t be watching.</p>
          </div>
          <Switch checked={auto} onCheckedChange={switchAuto} aria-label="Auto mode" />
        </div>
      )}
      <div className="grid gap-2">
        {list.map((c) => {
          const Icon = KINDS[kindOf(c)]?.icon ?? Link2;
          return (
            <div key={c.id} className="flex items-center gap-3">
              <Icon className="size-4 shrink-0 text-muted-foreground" />
              <div className="min-w-0 grow">
                <h3 className="flex items-center gap-1.5 text-md font-medium">
                  {c.name} {c.shared && <Pill>Shared</Pill>}
                </h3>
                <p className="text-sm text-muted-foreground">{plural(c.tools, "tool")}</p>
              </div>
              <Switch checked={c.on} onCheckedChange={() => switchOne(c)} aria-label={`Use ${c.name} in this chat`} />
            </div>
          );
        })}
      </div>
      <FormActions>
        {clusters && (
          <Button variant="ghost" size="sm" asChild>
            <Link href="/clusters">Clusters</Link>
          </Button>
        )}
        {devices && (
          <Button variant="ghost" size="sm" asChild>
            <Link href="/devices">Devices</Link>
          </Button>
        )}
        <Button variant="ghost" size="sm" asChild>
          <Link href="/settings/mcp">MCP servers</Link>
        </Button>
        <Button onClick={() => onOpenChange(false)}>Done</Button>
      </FormActions>
    </FormDialog>
  );
}

/** The composer's Tools button: how many are on, and Auto when it is. Nothing when there are none. */
export function ToolsButton({ chatId, disabled, className }: { chatId: string | null; disabled?: boolean; className?: string }) {
  const [open, setOpen] = useState(false);
  const { on, total, auto } = useChatTools(chatId);
  if (!total) return null;
  return (
    <>
      <Button
        type="button"
        variant="ghost"
        size="sm"
        disabled={disabled}
        onClick={() => setOpen(true)}
        title={`Clusters, devices and MCP servers the model can use in this chat${auto ? ". Auto: tools run without asking" : ""}`}
        aria-label={`Tools: ${on} of ${total} on${auto ? ", Auto mode" : ""}`}
        className={cn(on > 0 && "text-foreground", !on && "text-muted-foreground", className)}
      >
        <Link2 />
        <span className="hidden sm:inline">Tools</span>
        <span className="tabular-nums text-muted-foreground">
          {on}/{total}
        </span>
        {auto && <Pill tone="amber">Auto</Pill>}
      </Button>
      <ToolsChooser chatId={chatId} open={open} onOpenChange={setOpen} />
    </>
  );
}

/** The Tools block of the chat's context panel. */
export function ToolsContextSection({ chatId }: { chatId: string | null }) {
  const [open, setOpen] = useState(false);
  const { list, auto } = useChatTools(chatId);
  if (!list.length) return null;
  return (
    <section className="grid gap-1">
      <Eyebrow>Tools</Eyebrow>
      {auto && <DetailRow label="Auto mode">Runs without asking</DetailRow>}
      {list.map((c) => (
        <DetailRow key={c.id} label={c.name}>
          {c.on ? plural(c.tools, "tool") : "Off"}
        </DetailRow>
      ))}
      <Button size="sm" variant="outline" className="mt-1 w-fit" onClick={() => setOpen(true)}>
        <Link2 /> Choose tools
      </Button>
      <ToolsChooser chatId={chatId} open={open} onOpenChange={setOpen} />
    </section>
  );
}
