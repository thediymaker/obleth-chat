"use client";

// The knowledge pieces of the chat page: which collections a chat searches (the pill
// in the chat line, the composer button, the Context panel section and the picker),
// and, under a reply, the sources it found with each [n] linked to its page. A chat
// that does not exist yet (conversationId null) keeps its picks in chat-draft.ts.

import { useQueryClient, useQuery } from "@tanstack/react-query";
import { BookOpen, Bookmark, Check, ChevronDown, ExternalLink, Orbit, Plus } from "lucide-react";
import Link from "next/link";
import { useState } from "react";
import { toast } from "sonner";
import { EmptyState, Eyebrow } from "@/components/app-shell/page";
import { chatDraft, useChatDraft } from "@/components/projects/chat-draft";
import { messageOf } from "@/components/projects/form-field";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { api } from "@/lib/api";
import { count } from "@/lib/format";
import { useFeature } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { fileUrl, knowledgeKeys, MAX_PER_CHAT, scopeLabel, useCollections } from "./api";

const chatPath = (id: string) => `/api/conversations/${encodeURIComponent(id)}/knowledge`;

/** The collections a chat searches, and a way to switch one on or off. */
export function useChatKnowledge(conversationId: string | null) {
  const client = useQueryClient();
  const enabled = useFeature("knowledge");
  const draft = useChatDraft();
  const { data: list } = useCollections(enabled);
  const saved = useQuery({
    queryKey: knowledgeKeys.chat(conversationId || ""),
    queryFn: async () => (await api.request<{ collectionIds?: string[] }>(chatPath(conversationId!))).collectionIds ?? [],
    enabled: enabled && !!conversationId,
  });
  const ids = conversationId ? (saved.data ?? []) : draft.collections;
  const collections = list?.collections ?? [];
  const names = ids.map((id) => collections.find((c) => c.id === id)?.name).filter((n): n is string => !!n);
  async function toggle(id: string) {
    const next = ids.includes(id) ? ids.filter((c) => c !== id) : [...ids, id];
    if (next.length > MAX_PER_CHAT) {
      toast(`A chat can search up to ${MAX_PER_CHAT} collections.`);
      return;
    }
    if (!conversationId) {
      chatDraft.setCollections(next);
      return;
    }
    try {
      const r = await api.request<{ collectionIds?: string[] }>(chatPath(conversationId), { method: "PUT", body: { collectionIds: next } });
      client.setQueryData(knowledgeKeys.chat(conversationId), r.collectionIds ?? []);
    } catch (e) {
      toast(messageOf(e));
    }
  }
  return { enabled, ids, names, collections, toggle };
}

/** Pick the collections a chat searches. */
export function KnowledgePicker({ open, onOpenChange, conversationId }: { open: boolean; onOpenChange: (open: boolean) => void; conversationId: string | null }) {
  const { ids, collections, toggle } = useChatKnowledge(conversationId);
  const on = new Set(ids);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[92dvh] overflow-y-auto sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Knowledge in this chat</DialogTitle>
          <DialogDescription>The chat searches the collections that are on and cites what it uses. Changes apply from the next message.</DialogDescription>
        </DialogHeader>
        {collections.length ? (
          <div className="grid divide-y rounded-md border">
            {collections.map((c) => (
              <label key={c.id} className="flex cursor-pointer items-center gap-3 px-3 py-2.5 hover:bg-accent">
                <BookOpen className="size-4 shrink-0 text-muted-foreground" />
                <span className="min-w-0 grow">
                  <span className="flex items-center gap-1.5">
                    <span className="truncate text-md font-medium">{c.name}</span>
                    {!c.mine && <Badge variant="outline">{scopeLabel(c)}</Badge>}
                  </span>
                  <span className="block text-xs text-muted-foreground">
                    {count(c.documents, "document")}
                    {c.processing ? ` · indexing ${c.processing}` : ""}
                  </span>
                </span>
                <Switch checked={on.has(c.id)} onCheckedChange={() => toggle(c.id)} aria-label={`Search ${c.name} in this chat`} />
              </label>
            ))}
          </div>
        ) : (
          <EmptyState icon={BookOpen} title="No collections yet" text="Make one on the Knowledge page and add documents to it." />
        )}
        <DialogFooter>
          <Button variant="ghost" size="sm" asChild>
            <Link href="/knowledge" onClick={() => onOpenChange(false)}>
              Knowledge
            </Link>
          </Button>
          <Button onClick={() => onOpenChange(false)}>Done</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * The chat line's knowledge pill: the collections this chat searches, or (when
 * `addable`) a way to add some. Hidden without the knowledge feature.
 */
export function KnowledgePill({ conversationId, addable = true, disabled = false, className }: { conversationId: string | null; addable?: boolean; disabled?: boolean; className?: string }) {
  const { enabled, names } = useChatKnowledge(conversationId);
  const [open, setOpen] = useState(false);
  if (!enabled || (!names.length && !addable)) return null;
  const label = names.length > 2 ? `${names.slice(0, 2).join(" · ")} +${names.length - 2}` : names.join(" · ");
  return (
    <>
      <button
        type="button"
        disabled={disabled}
        onClick={() => setOpen(true)}
        title="Collections this chat searches"
        className={cn(
          "inline-flex h-7 max-w-72 items-center gap-1.5 rounded-full border bg-background px-2.5 text-sm text-text-2 hover:bg-accent disabled:opacity-50",
          !names.length && "border-dashed text-muted-foreground",
          className,
        )}
      >
        {names.length ? <BookOpen className="size-3.5" /> : <Plus className="size-3.5" />}
        <span className="truncate">{names.length ? label : "Knowledge"}</span>
      </button>
      <KnowledgePicker open={open} onOpenChange={setOpen} conversationId={conversationId} />
    </>
  );
}

/** The composer's Knowledge button, with the first collection's name or a count. */
export function KnowledgeComposerButton({ conversationId, disabled = false }: { conversationId: string | null; disabled?: boolean }) {
  const { enabled, ids, names } = useChatKnowledge(conversationId);
  const [open, setOpen] = useState(false);
  if (!enabled) return null;
  const n = ids.length;
  const label = n === 1 ? names[0] || "Knowledge" : "Knowledge";
  return (
    <>
      <Button
        type="button"
        variant="ghost"
        size="sm"
        disabled={disabled}
        onClick={() => setOpen(true)}
        title="Collections this chat searches"
        aria-label={`Knowledge: ${n} collection${n === 1 ? "" : "s"}`}
        className={cn(n > 0 && "text-foreground")}
      >
        <BookOpen />
        <span className="max-w-40 truncate">{label.length > 28 ? `${label.slice(0, 27)}…` : label}</span>
        {n > 1 && <span className="rounded-full bg-secondary px-1.5 text-2xs">{n}</span>}
      </Button>
      <KnowledgePicker open={open} onOpenChange={setOpen} conversationId={conversationId} />
    </>
  );
}

/** The Context panel's Knowledge section. */
export function KnowledgeContextSection({ conversationId }: { conversationId: string | null }) {
  const { enabled, names } = useChatKnowledge(conversationId);
  const [open, setOpen] = useState(false);
  if (!enabled) return null;
  return (
    <section className="grid gap-2">
      <div className="flex items-center justify-between">
        <Eyebrow>Knowledge</Eyebrow>
        {names.length > 0 && <span className="text-xs text-faint">{names.length}</span>}
      </div>
      {names.length ? (
        names.map((n) => (
          <div key={n} className="flex items-center gap-2.5 rounded-md border bg-card px-3 py-2 text-md">
            <BookOpen className="size-4 text-muted-foreground" />
            <span className="grow truncate">{n}</span>
            <Check className="size-4 text-success" />
          </div>
        ))
      ) : (
        <p className="text-sm text-muted-foreground">Pick collections to ground this chat in your documents. Answers cite the page they came from.</p>
      )}
      <Button variant="outline" size="sm" className="justify-self-start" onClick={() => setOpen(true)}>
        <BookOpen /> Choose collections
      </Button>
      <KnowledgePicker open={open} onOpenChange={setOpen} conversationId={conversationId} />
    </section>
  );
}

// ---- Under replies ----

export interface SourceItem {
  n: number;
  documentId: string;
  name: string;
  page?: number | null;
  section?: string | null;
  excerpt: string;
  saved?: boolean;
}
export interface SourcesEntry {
  id?: string;
  kind: "sources";
  collections?: string[];
  queries?: string[];
  items?: SourceItem[];
}
export interface ReplyLike {
  id?: string;
  role: string;
  content?: string;
  activity?: unknown;
}
const activity = (m: ReplyLike) => (Array.isArray(m.activity) ? (m.activity as Record<string, unknown>[]) : []);
export const sourcesOf = (m: ReplyLike) => activity(m).find((e) => e.kind === "sources") as SourcesEntry | undefined;
const searchingNow = (m: ReplyLike) => activity(m).find((e) => e.tool === "search_knowledge" && e.status === "running") as { summary?: string } | undefined;
/** Where a numbered source opens: the file at its page, or the saved item in Library. */
export const sourceHref = (s: SourceItem) => (s.saved ? `/library/${encodeURIComponent(s.documentId)}` : fileUrl(s.documentId, s.page));
const escapeHtml = (text: string) => text.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
const CITE = /\[(\d{1,2}(?:\s*,\s*\d{1,2})*)\]/g;

/** Links each [n] in a reply's rendered HTML to its source. Leaves code alone. */
export function citeHtml(html: string, m: ReplyLike): string {
  const entry = sourcesOf(m);
  if (!entry?.items?.length) return html;
  const items = new Map(entry.items.map((s) => [s.n, s]));
  let code = 0;
  return html
    .split(/(<[^>]+>)/)
    .map((part) => {
      if (part.startsWith("<")) {
        if (/^<(pre|code|svg)\b/i.test(part)) code++;
        else if (/^<\/(pre|code|svg)>/i.test(part)) code = Math.max(0, code - 1);
        return part;
      }
      if (code) return part;
      return part.replace(CITE, (whole, list: string) => {
        const ns = list.split(",").map((n) => Number(n.trim()));
        if (!ns.every((n) => items.has(n))) return whole;
        return ns
          .map((n) => {
            const s = items.get(n)!;
            const title = `${s.saved ? "Saved: " : ""}${escapeHtml(s.name)}${s.page ? `, page ${s.page}` : ""}${s.section ? `, ${escapeHtml(s.section)}` : ""}`;
            const target = s.saved ? "" : ' target="_blank" rel="noopener noreferrer"';
            return `<a class="cite" href="${escapeHtml(sourceHref(s))}"${target} title="${title}">${n}</a>`;
          })
          .join("");
      });
    })
    .join("");
}

const quote = (q: string, max = 80) => `“${q.length > max ? `${q.slice(0, max - 1)}…` : q}”`;

function Passage({ s }: { s: SourceItem & { cited: boolean } }) {
  const external = !s.saved;
  return (
    <li className={cn("flex gap-3 rounded-md px-2 py-2", s.cited && "bg-muted/60")}>
      <span className={cn("flex size-5 shrink-0 items-center justify-center rounded-full border text-2xs", s.cited && "border-line-strong text-foreground")}>{s.n}</span>
      <div className="min-w-0 grow">
        <div className="flex items-start justify-between gap-2">
          <strong className="min-w-0 text-sm font-medium">
            {s.saved && <Bookmark className="mr-1 inline size-3.5" />}
            {s.name}
            {s.page ? <span className="font-normal text-faint"> page {s.page}</span> : null}
            {s.section ? <span className="font-normal text-faint"> · {s.section}</span> : null}
          </strong>
          <Button variant="ghost" size="xs" asChild>
            <a href={sourceHref(s)} {...(external ? { target: "_blank", rel: "noopener noreferrer" } : {})}>
              {s.saved ? "Open in Library" : s.page ? "Open at page" : "Open"}
              {external && <ExternalLink />}
            </a>
          </Button>
        </div>
        <p className="mt-0.5 text-sm text-muted-foreground">{s.excerpt}</p>
      </div>
    </li>
  );
}

/**
 * One card for everything a reply looked up: a single line while closed (what it is
 * searching now, or how much it found), then the searches and the passages the answer
 * cited, with the rest behind a second toggle. Nothing when the reply searched nothing.
 */
export function KnowledgeSources({ message }: { message: ReplyLike }) {
  const entry = sourcesOf(message);
  const [open, setOpen] = useState(false);
  const [more, setMore] = useState(false);
  if (!entry) return null;
  const items = entry.items || [];
  const queries = entry.queries || [];
  const where = (entry.collections || []).join(", ");
  const live = searchingNow(message);
  const used = new Set([...String(message.content || "").matchAll(CITE)].flatMap((x) => x[1]!.split(",").map((n) => Number(n.trim()))));
  const saved = items.filter((s) => s.saved).length;
  const passages = items.length - saved;
  const found = !items.length
    ? "nothing matched"
    : [passages && `${count(passages, "passage")} from ${count(new Set(items.filter((s) => !s.saved).map((s) => s.documentId)).size, "document")}`, saved && count(saved, "saved item")]
        .filter(Boolean)
        .join(" and ");
  const line = live
    ? `Searching ${where} for ${quote(String(live.summary || "").replace(/^Searched for “|”$/g, ""), 60)}`
    : queries.length > 1
      ? `Searched ${where} · ${count(queries.length, "search", "searches")} · ${found}`
      : `Searched ${where}${queries[0] ? ` for ${quote(queries[0])}` : ""} · ${found}`;
  const Icon = live ? Orbit : BookOpen;
  const head = (
    <>
      <Icon className={cn("size-4 shrink-0 text-muted-foreground", live && "animate-spin [animation-duration:3s]")} />
      <span className={cn("min-w-0 grow truncate", live && "text-text-2")}>{line}</span>
    </>
  );
  if (!items.length && !live && queries.length <= 1) return <div className="my-2 flex items-center gap-2 rounded-lg border px-3 py-2 text-sm text-muted-foreground">{head}</div>;
  const marked = items.map((s) => ({ ...s, cited: used.has(s.n) }));
  const cited = marked.filter((s) => s.cited);
  const rest = marked.filter((s) => !s.cited);
  const shown = cited.length ? cited : rest.slice(0, 6);
  const hidden = cited.length ? rest : rest.slice(6);
  return (
    <div className="my-2 rounded-lg border text-sm" data-sources={message.id || "live"}>
      <button type="button" aria-expanded={open} onClick={() => setOpen(!open)} className="flex w-full items-center gap-2 px-3 py-2 text-left text-muted-foreground hover:text-foreground">
        {head}
        <ChevronDown className={cn("size-4 shrink-0 transition-transform", open && "rotate-180")} />
      </button>
      {open && (
        <div className="grid gap-2 border-t px-2 py-2">
          {queries.length > 1 && (
            <div className="px-2">
              <Eyebrow>Searches</Eyebrow>
              <ol className="mt-1 list-decimal pl-5 text-sm text-text-2">
                {queries.map((q, i) => (
                  <li key={i}>{quote(q, 140)}</li>
                ))}
              </ol>
            </div>
          )}
          {shown.length > 0 && (
            <ol className="grid">
              {shown.map((s) => (
                <Passage key={s.n} s={s} />
              ))}
            </ol>
          )}
          {hidden.length > 0 && (
            <div>
              <button type="button" aria-expanded={more} onClick={() => setMore(!more)} className="flex items-center gap-1 px-2 py-1 text-xs text-muted-foreground hover:text-foreground">
                <ChevronDown className={cn("size-3.5 transition-transform", more && "rotate-180")} />
                {count(hidden.length, "more passage")}
                {cited.length ? " found but not cited" : ""}
              </button>
              {more && (
                <ol className="grid">
                  {hidden.map((s) => (
                    <Passage key={s.n} s={s} />
                  ))}
                </ol>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
