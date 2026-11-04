"use client";

// All chats (/history): find, organize and keep a copy of your conversations, and
// bring chats over from other apps.

import { Archive, ArchiveRestore, Download, MessageSquare, Pin, PinOff, Trash2, Upload } from "lucide-react";
import Link from "next/link";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { usePageChrome } from "@/components/app-shell/chrome";
import { EmptyState, ErrorBox, Eyebrow, Loading, Page, PageHeader } from "@/components/app-shell/page";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import {
  bulkConversations,
  exportChats,
  fetchHistory,
  importChats,
  importFileError,
  importSummary,
  PAGE_SIZE,
  updateConversation,
  type ArchivedFilter,
  type HistoryChat,
} from "@/lib/chat/history";
import { when } from "@/lib/format";
import { keys, useModels } from "@/lib/queries";
import { useSession } from "@/lib/session";

const FILTERS: [ArchivedFilter, string][] = [
  ["false", "Active"],
  ["true", "Archived"],
  ["all", "All conversations"],
];
const asFilter = (value: string | null): ArchivedFilter => (value === "true" || value === "all" ? value : "false");

/** Under the shared conversations key, so a change anywhere (the sidebar's menu, a
 * new chat) refreshes this list too. */
export const historyKey = (query: string, archived: ArchivedFilter, offset: number) => [...keys.conversations, "history", query, archived, offset] as const;

export function HistoryPage() {
  usePageChrome("All chats");
  const params = useSearchParams();
  const router = useRouter();
  const pathname = usePathname();
  const queries = useQueryClient();
  const signedIn = useSession().phase === "ready";
  const { data: models = [] } = useModels();
  const [query, setQuery] = useState(() => params.get("query")?.trim() ?? "");
  const [archived, setArchived] = useState<ArchivedFilter>(() => asFilter(params.get("archived")));
  const [draftQuery, setDraftQuery] = useState(query);
  const [draftArchived, setDraftArchived] = useState<ArchivedFilter>(archived);
  const [offset, setOffset] = useState(0);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [importing, setImporting] = useState(false);
  const [busy, setBusy] = useState(false);

  // A search opened from elsewhere (the search dialog's See all results) arrives in the
  // URL.
  const paramKey = params.toString();
  useEffect(() => {
    const q = params.get("query")?.trim() ?? "";
    const a = asFilter(params.get("archived"));
    setQuery(q);
    setArchived(a);
    setDraftQuery(q);
    setDraftArchived(a);
    setOffset(0);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- the string stands for the
    // params
  }, [paramKey]);

  const list = useQuery({
    queryKey: historyKey(query, archived, offset),
    queryFn: ({ signal }) => fetchHistory(query, archived, offset, PAGE_SIZE, signal),
    enabled: signedIn,
    placeholderData: (previous) => previous,
  });
  const data = list.data;
  const chats = data?.conversations ?? [];

  // A new page of results starts with nothing selected.
  useEffect(() => {
    setSelected(new Set());
  }, [data]);
  // The last page emptied (its chats were deleted or archived): show the page before it.
  useEffect(() => {
    if (!data || list.isPlaceholderData || offset === 0 || data.conversations.length) return;
    setOffset(Math.max(0, Math.floor(Math.max(0, data.total - 1) / PAGE_SIZE) * PAGE_SIZE));
  }, [data, offset, list.isPlaceholderData]);

  const refresh = () => queries.invalidateQueries({ queryKey: keys.conversations });
  const modelName = (id: string) => models.find((m) => m.id === id)?.name || id;
  const n = selected.size;

  function search(event: React.FormEvent) {
    event.preventDefault();
    const q = draftQuery.trim();
    setQuery(q);
    setArchived(draftArchived);
    setOffset(0);
    const next = new URLSearchParams();
    if (q) next.set("query", q);
    if (draftArchived !== "false") next.set("archived", draftArchived);
    const target = next.toString() ? `${pathname}?${next}` : pathname;
    if (target !== `${pathname}${paramKey ? `?${paramKey}` : ""}`) router.replace(target, { scroll: false });
    // Searching again for the same thing looks again.
    if (q === query && draftArchived === archived && offset === 0) list.refetch();
  }
  async function update(chat: HistoryChat, body: Record<string, unknown>) {
    try {
      await updateConversation(chat.id, body);
      queries.invalidateQueries({ queryKey: keys.conversation(chat.id) });
      await refresh();
    } catch (error) {
      if ((error as Error).name !== "AbortError") toast((error as Error).message);
    }
  }
  async function bulk(action: "archive" | "unarchive" | "delete") {
    setBusy(true);
    try {
      await bulkConversations([...selected], action);
      setConfirmDelete(false);
      await refresh();
      toast(action === "delete" ? "Conversations deleted" : action === "archive" ? "Conversations archived" : "Conversations restored");
    } catch (error) {
      if ((error as Error).name !== "AbortError") toast((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function exportSelected() {
    try {
      await exportChats([...selected]);
    } catch (error) {
      toast((error as Error).message);
    }
  }
  const toggle = (id: string, on: boolean) =>
    setSelected((old) => {
      const next = new Set(old);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });

  const empty = query
    ? { title: "No conversations found", text: "Try another search or include archived conversations." }
    : { title: archived === "true" ? "Nothing archived" : "A fresh start", text: "Your conversations will appear here as you use Chat." };

  return (
    <Page>
      <Eyebrow className="mb-1 block">Only you</Eyebrow>
      <PageHeader
        title="Your conversations"
        description="Find, organize, and keep a copy of your work."
        actions={
          <Button size="sm" variant="outline" onClick={() => setImporting(true)}>
            <Upload /> Import
          </Button>
        }
      />
      <form onSubmit={search} className="mb-[22px] flex flex-wrap items-center gap-2.5" role="search">
        <Label htmlFor="history-query" className="sr-only">
          Search conversations
        </Label>
        <Input
          id="history-query"
          name="query"
          className="h-10 min-w-0 flex-1 basis-56 text-base"
          placeholder="Search titles and messages…"
          maxLength={256}
          value={draftQuery}
          onChange={(e) => setDraftQuery(e.target.value)}
        />
        <Label htmlFor="history-archived" className="sr-only">
          Conversation visibility
        </Label>
        <Select value={draftArchived} onValueChange={(v) => setDraftArchived(v as ArchivedFilter)}>
          <SelectTrigger id="history-archived" className="w-[180px] text-base data-[size=default]:h-10" aria-label="Conversation visibility">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {FILTERS.map(([value, label]) => (
              <SelectItem key={value} value={value}>
                {label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Button type="submit" variant="outline" className="h-10">
          Search
        </Button>
      </form>
      <ErrorBox error={list.error} onRetry={() => list.refetch()} />
      <div className="mt-[22px] mb-3 flex flex-wrap items-center justify-between gap-[15px]">
        <label className="flex items-center gap-[9px] text-md text-muted-foreground">
          <Checkbox
            id="history-select-all"
            aria-label="Select page"
            checked={chats.length > 0 && n === chats.length}
            disabled={!chats.length}
            onCheckedChange={(on) => setSelected(new Set(on === true ? chats.map((c) => c.id) : []))}
          />
          {n ? `${n} selected` : "Select page"}
        </label>
        <div className="flex flex-wrap gap-1.5">
          <Button size="sm" variant="outline" disabled={!n} onClick={exportSelected}>
            <Download /> Export
          </Button>
          <Button size="sm" variant="outline" disabled={!n || busy} onClick={() => bulk(archived === "true" ? "unarchive" : "archive")}>
            {archived === "true" ? <ArchiveRestore /> : <Archive />} {archived === "true" ? "Restore" : "Archive"}
          </Button>
          <Button size="sm" variant="destructive" disabled={!n} onClick={() => setConfirmDelete(true)}>
            <Trash2 /> Delete
          </Button>
        </div>
      </div>
      {list.isFetching && !data && <Loading label="Loading conversations…" />}
      {list.isFetching && data && (
        <p role="status" className="sr-only">
          Loading conversations…
        </p>
      )}
      {chats.length ? (
        <>
          <ul className="border-t" aria-label="Conversations">
            {chats.map((chat) => (
              <li key={chat.id} className="group/item flex flex-wrap items-center gap-x-[15px] gap-y-1.5 border-b py-3">
                <Checkbox aria-label={`Select ${chat.title}`} checked={selected.has(chat.id)} onCheckedChange={(on) => toggle(chat.id, on === true)} />
                <Link href={`/chat/${encodeURIComponent(chat.id)}`} title={chat.title} className="min-w-0 flex-1 basis-48">
                  <span className="flex items-center gap-2 text-base leading-[1.6] font-medium decoration-muted-foreground underline-offset-[3px] group-hover/item:underline">
                    {chat.pinned && <Pin className="size-3.5 shrink-0 text-muted-foreground" aria-label="Pinned" />}
                    <span className="truncate">{chat.title}</span>
                    {chat.archived && <Badge variant="secondary">Archived</Badge>}
                  </span>
                  <span className="mt-1 flex flex-wrap gap-x-2 text-sm text-faint">
                    <span className="truncate">{modelName(chat.model)}</span>
                    <span aria-hidden>·</span>
                    <span>{when(chat.updatedAt || chat.createdAt)}</span>
                  </span>
                </Link>
                <div className="ml-auto flex gap-1">
                  <Button size="sm" variant="ghost" onClick={() => update(chat, { pinned: !chat.pinned })}>
                    {chat.pinned ? <PinOff /> : <Pin />} {chat.pinned ? "Unpin" : "Pin"}
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => update(chat, { archived: !chat.archived })}>
                    {chat.archived ? <ArchiveRestore /> : <Archive />} {chat.archived ? "Restore" : "Archive"}
                  </Button>
                </div>
              </li>
            ))}
          </ul>
          <div className="mt-6 flex flex-wrap items-center justify-between gap-[15px] text-md text-muted-foreground">
            <span>
              {offset + 1}–{offset + chats.length} of {data?.total ?? chats.length}
            </span>
            <div className="flex gap-1.5">
              <Button size="sm" variant="outline" disabled={!offset} onClick={() => setOffset((o) => Math.max(0, o - PAGE_SIZE))}>
                Previous
              </Button>
              <Button size="sm" variant="outline" disabled={!data?.hasMore} onClick={() => setOffset((o) => o + PAGE_SIZE)}>
                Next
              </Button>
            </div>
          </div>
        </>
      ) : (
        data && !list.isFetching && <EmptyState icon={MessageSquare} title={empty.title} text={empty.text} />
      )}
      <AlertDialog open={confirmDelete} onOpenChange={setConfirmDelete}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              Delete {n} conversation{n === 1 ? "" : "s"}?
            </AlertDialogTitle>
            <AlertDialogDescription>This permanently deletes the selected conversations and their messages.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep conversations</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={busy}
              onClick={(event) => {
                event.preventDefault();
                bulk("delete");
              }}
            >
              Delete conversations
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <ImportDialog open={importing} onOpenChange={setImporting} onImported={refresh} />
    </Page>
  );
}

/** Bring chats over from Chat, Open WebUI, ChatGPT or Claude, then say what came over. */
export function ImportDialog({ open, onOpenChange, onImported }: { open: boolean; onOpenChange: (open: boolean) => void; onImported: () => unknown }) {
  const [file, setFile] = useState<File | null>(null);
  const [error, setError] = useState("");
  const [sending, setSending] = useState(false);
  const [summary, setSummary] = useState<string[] | null>(null);
  // A reply that arrives after the page closed (or the person signed out) shows nothing.
  const alive = useRef<AbortController | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    alive.current = controller;
    return () => controller.abort();
  }, []);

  function reset(next: boolean) {
    if (!next && sending) return;
    if (!next) {
      setFile(null);
      setError("");
    }
    onOpenChange(next);
  }
  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!file || sending) return;
    const tooLarge = importFileError(file);
    if (tooLarge) {
      setError(tooLarge);
      return;
    }
    const controller = alive.current;
    setError("");
    setSending(true);
    try {
      const result = await importChats(file, controller?.signal);
      if (!controller || controller.signal.aborted) return;
      setFile(null);
      onOpenChange(false);
      await onImported();
      if (controller.signal.aborted) return;
      setSummary(importSummary(result));
    } catch (e) {
      if (controller?.signal.aborted || (e as Error).name === "AbortError") return;
      setError((e as Error).message);
    } finally {
      if (!controller?.signal.aborted) setSending(false);
    }
  }
  return (
    <>
      <Dialog open={open} onOpenChange={reset}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Import conversations</DialogTitle>
            <DialogDescription>
              Bring your chats over from Chat, Open WebUI, ChatGPT or Claude. They are added to your account, and your existing conversations are kept.
            </DialogDescription>
          </DialogHeader>
          <form id="history-import-form" onSubmit={submit} className="grid gap-3">
            <div className="grid gap-1.5">
              <Label htmlFor="history-file">Export file</Label>
              <Input
                id="history-file"
                name="history-file"
                type="file"
                accept=".json,.zip,application/json,application/zip"
                aria-describedby="history-file-help"
                onChange={(e) => {
                  setFile(e.target.files?.[0] ?? null);
                  setError("");
                }}
              />
              <p id="history-file-help" className="text-sm text-muted-foreground">
                Open WebUI: Settings, Data Controls, Export Chats. ChatGPT and Claude: the zip from Export data, or the conversations.json file inside it. Up to 200 MB.
              </p>
            </div>
            {sending && (
              <p role="status" className="text-sm text-muted-foreground">
                Importing… A large export can take a few minutes.
              </p>
            )}
            {error && (
              <p role="alert" className="text-sm text-destructive-foreground">
                {error}
              </p>
            )}
            <DialogFooter>
              <Button type="submit" disabled={!file || sending}>
                Import conversations
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
      <Dialog open={!!summary} onOpenChange={(next) => !next && setSummary(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Import finished</DialogTitle>
            <DialogDescription className="sr-only">What the import brought over</DialogDescription>
          </DialogHeader>
          <div className="grid gap-2 text-base">
            {summary?.map((line) => (
              <p key={line}>{line}</p>
            ))}
          </div>
          <DialogFooter>
            <Button onClick={() => setSummary(null)}>Done</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
