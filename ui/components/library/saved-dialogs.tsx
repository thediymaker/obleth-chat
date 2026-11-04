"use client";

// The save dialog (new items and editing your own) and the viewer for someone else's
// item in a shared project. useLibraryDialogs() gives any page `open(id)`, `create`
// and `saveFrom` (what a chat saves: a code block, a reply, selected text), plus the
// element to render.

import { useQueryClient } from "@tanstack/react-query";
import { Copy, ExternalLink, MessageSquare, Trash2 } from "lucide-react";
import Link from "next/link";
import { useCallback, useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { Confirm, FormError, FormField, messageOf } from "@/components/projects/form-field";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { api } from "@/lib/api";
import { useProjects } from "@/lib/queries";
import { useUser } from "@/lib/session";
import { cn } from "@/lib/utils";
import { getSaved, host, KINDS, kindOf, parseTags, refreshSaved, type SavedItem, type SavedKind, savedPath, suggestTags, useSaved } from "./api";

/** What the save dialog starts from: a new item (maybe from a chat) or one being edited. */
export interface SaveDraft {
  id?: string;
  kind?: SavedKind;
  title?: string;
  body?: string;
  language?: string;
  url?: string | null;
  tags?: string[];
  searchable?: boolean;
  projectId?: string | null;
  ownerId?: string;
  source?: SavedItem["source"];
  /** The chat it is saved from: the item keeps a link back to it. */
  sourceConversationId?: string;
  sourceMessageId?: string;
}
/** What a chat hands to the save dialog. */
export interface SaveFromInput {
  kind: SavedKind;
  title?: string;
  body?: string;
  language?: string;
  conversationId?: string;
  messageId?: string;
  /** The chat's project, the dialog's default. */
  projectId?: string;
}
const NO_PROJECT = "none";

async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    toast("Copied");
  } catch {
    toast("Couldn’t copy. Select the text and copy it instead.");
  }
}
const copyItem = (item: Pick<SavedItem, "kind" | "url" | "body">) => copyText(item.kind === "link" ? item.url || item.body : item.body);

export function SaveDialog({ open, onOpenChange, draft, title }: { open: boolean; onOpenChange: (open: boolean) => void; draft: SaveDraft; title: string }) {
  const client = useQueryClient();
  const user = useUser();
  const { data: projects } = useProjects();
  // The person's tags, for suggestions.
  const { data: known } = useSaved({}, open);
  const [kind, setKind] = useState<SavedKind>("note");
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [body, setBody] = useState("");
  const [language, setLanguage] = useState("");
  const [project, setProject] = useState(NO_PROJECT);
  const [tags, setTags] = useState("");
  const [searchable, setSearchable] = useState(true);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [used, setUsed] = useState<string[]>([]);

  useEffect(() => {
    if (!open) return;
    setKind(draft.kind || "note");
    setName(draft.title || "");
    setUrl(draft.url || "");
    setBody(draft.body || "");
    setLanguage(draft.language || "");
    setProject(draft.projectId || NO_PROJECT);
    setTags((draft.tags || []).map((t) => `#${t}`).join(" "));
    setSearchable(draft.searchable !== false);
    setError("");
    setUsed([]);
  }, [open, draft]);

  const all = projects?.projects ?? [];
  // Someone else's item keeps its project: only its owner moves it.
  const projectField = all.length > 0 && !(draft.id && draft.ownerId && draft.ownerId !== user.id);
  const suggestions = useMemo(() => suggestTags(draft, known?.tags ?? []).filter((t) => !used.includes(t)), [draft, known, used]);

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    const payload: Record<string, unknown> = { kind, title: name.trim(), body, tags: parseTags(tags), searchable };
    if (kind === "code") payload.language = language.trim();
    if (kind === "link") payload.url = url.trim();
    if (projectField) payload.projectId = project === NO_PROJECT ? null : project;
    if (!draft.id && draft.sourceConversationId) {
      payload.sourceConversationId = draft.sourceConversationId;
      if (draft.sourceMessageId) payload.sourceMessageId = draft.sourceMessageId;
    }
    try {
      await api.request(draft.id ? savedPath(draft.id) : "/api/saved", { method: draft.id ? "PATCH" : "POST", body: payload });
      onOpenChange(false);
      toast(draft.id ? "Saved your changes" : "Saved to Library");
      refreshSaved(client);
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="max-h-[92dvh] overflow-y-auto sm:max-w-xl">
          <DialogHeader>
            <DialogTitle className="truncate pr-6">{title}</DialogTitle>
            <DialogDescription className="sr-only">Save something to Library, where later chats can find it.</DialogDescription>
          </DialogHeader>
          <form id="saved-form" onSubmit={submit} className="grid gap-4">
            <FormField id="saved-title" label="Title">
              <Input id="saved-title" value={name} onChange={(e) => setName(e.target.value)} maxLength={200} placeholder="What is it?" />
            </FormField>
            <div className="grid gap-1.5">
              <span id="saved-kind-label" className="text-sm text-text-2">
                Kind
              </span>
              <div role="radiogroup" aria-labelledby="saved-kind-label" className="grid grid-cols-2 gap-2 sm:grid-cols-4">
                {(Object.entries(KINDS) as [SavedKind, (typeof KINDS)[SavedKind]][]).map(([k, [Icon, , label]]) => (
                  <button
                    key={k}
                    type="button"
                    role="radio"
                    aria-checked={kind === k}
                    onClick={() => setKind(k)}
                    className={cn(
                      "flex items-center justify-center gap-1.5 rounded-md border px-2 py-2 text-sm text-muted-foreground hover:bg-accent",
                      kind === k && "border-line-strong bg-secondary text-foreground",
                    )}
                  >
                    <Icon className="size-4" /> {label}
                  </button>
                ))}
              </div>
            </div>
            {kind === "link" && (
              <FormField id="saved-url" label="Address">
                <Input id="saved-url" type="url" value={url} onChange={(e) => setUrl(e.target.value)} maxLength={2048} placeholder="https://" />
              </FormField>
            )}
            <FormField id="saved-body" label={kind === "link" ? "Notes" : "Content"}>
              <Textarea id="saved-body" value={body} onChange={(e) => setBody(e.target.value)} rows={kind === "code" ? 10 : 6} maxLength={65536} className={cn(kind === "code" && "font-mono text-sm")} />
            </FormField>
            {kind === "code" && (
              <FormField id="saved-language" label="Language">
                <Input id="saved-language" value={language} onChange={(e) => setLanguage(e.target.value)} maxLength={40} placeholder="python" />
              </FormField>
            )}
            {projectField && (
              <FormField id="saved-project" label="Project" help="Chats in the project can find it, and so can everyone the project is shared with.">
                <Select value={project} onValueChange={setProject}>
                  <SelectTrigger id="saved-project" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={NO_PROJECT}>No project</SelectItem>
                    {all.map((p) => (
                      <SelectItem key={p.id} value={p.id}>
                        {p.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </FormField>
            )}
            <FormField id="saved-tags" label="Tags" help="Separate tags with spaces.">
              <Input id="saved-tags" value={tags} onChange={(e) => setTags(e.target.value)} maxLength={400} placeholder="#research #python" />
            </FormField>
            {suggestions.length > 0 && (
              <div className="-mt-2 flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
                <span>Suggested</span>
                {suggestions.map((t) => (
                  <button
                    key={t}
                    type="button"
                    className="rounded-full border px-2 py-0.5 hover:bg-accent hover:text-foreground"
                    onClick={() => {
                      setTags((v) => `${v} #${t}`.trim());
                      setUsed((u) => [...u, t]);
                    }}
                  >
                    + {t}
                  </button>
                ))}
              </div>
            )}
            <label htmlFor="saved-searchable" className="flex items-start gap-2.5">
              <Checkbox id="saved-searchable" checked={searchable} onCheckedChange={(v) => setSearchable(!!v)} className="mt-0.5" />
              <span className="grid gap-0.5">
                <strong className="text-md font-medium">Chats can find it</strong>
                <small className="text-xs text-muted-foreground">Indexed like a document, so a later chat can find it when you ask about it.</small>
              </span>
            </label>
            {draft.source ? (
              <p className="flex items-center gap-1.5 text-sm text-muted-foreground">
                <MessageSquare className="size-3.5" /> From{" "}
                <Link className="underline-offset-2 hover:underline" href={`/chat/${encodeURIComponent(draft.source.conversationId)}`} onClick={() => onOpenChange(false)}>
                  {draft.source.title || "a chat"}
                </Link>
              </p>
            ) : draft.sourceConversationId ? (
              <p className="text-sm text-muted-foreground">Keeps a link back to this chat.</p>
            ) : null}
            <FormError message={error} />
            {draft.id && (
              <div className="flex flex-wrap items-center gap-3 rounded-md border border-destructive/30 p-3">
                <div className="grow">
                  <strong className="block text-md font-medium">Delete</strong>
                  <small className="text-xs text-muted-foreground">Removes it from Library and from chat searches.</small>
                </div>
                <Button type="button" size="sm" variant="outline" onClick={() => copyItem({ kind, url, body })}>
                  <Copy /> Copy
                </Button>
                <Button type="button" size="sm" variant="outline" className="text-destructive-foreground" onClick={() => setDeleting(true)}>
                  <Trash2 /> Delete
                </Button>
              </div>
            )}
            <DialogFooter>
              <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
                Cancel
              </Button>
              <Button type="submit" disabled={busy}>
                {draft.id ? "Save changes" : "Save"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
      {draft.id && (
        <Confirm
          open={deleting}
          onOpenChange={setDeleting}
          title={`Delete “${draft.title || "this item"}”?`}
          text="It is removed from Library and from chat searches."
          confirm="Delete"
          onConfirm={async () => {
            await api.request(savedPath(draft.id!), { method: "DELETE" });
            onOpenChange(false);
            toast("Deleted");
            refreshSaved(client);
          }}
        />
      )}
    </>
  );
}

/** Someone else's item in a shared project: read it, copy it or keep your own copy. */
export function SavedViewer({ open, onOpenChange, item }: { open: boolean; onOpenChange: (open: boolean) => void; item: SavedItem | null }) {
  const client = useQueryClient();
  const { data: projects } = useProjects();
  const [busy, setBusy] = useState(false);
  if (!item) return null;
  const [Icon, , label] = kindOf(item.kind);
  const project = projects?.projects?.find((p) => p.id === item.projectId);
  async function run(work: () => Promise<unknown>) {
    setBusy(true);
    try {
      await work();
    } catch (e) {
      toast(messageOf(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[92dvh] overflow-y-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle className="pr-6">{item.title}</DialogTitle>
          <DialogDescription>
            Saved by {item.ownerName || "someone else"}
            {project ? ` in ${project.name}` : ""}. Only they can change it.
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-3">
          <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
            <Icon className="size-3.5" /> {label}
          </span>
          {item.url && (
            <a href={item.url} target="_blank" rel="noopener noreferrer" className="inline-flex items-center gap-1.5 text-sm text-blue hover:underline">
              <ExternalLink className="size-3.5" /> {host(item.url)}
            </a>
          )}
          {item.kind === "code" ? (
            <pre className="max-h-96 overflow-auto rounded-md bg-code p-3 font-mono text-sm">
              <code>{item.body}</code>
            </pre>
          ) : (
            item.body && <p className="text-md whitespace-pre-wrap">{item.body}</p>
          )}
          {item.tags.length > 0 && (
            <div className="flex flex-wrap gap-1.5 text-xs text-muted-foreground">
              {item.tags.map((t) => (
                <span key={t}>#{t}</span>
              ))}
            </div>
          )}
        </div>
        <DialogFooter>
          {project?.mine && (
            <Button
              variant="outline"
              size="sm"
              className="text-destructive-foreground"
              disabled={busy}
              onClick={() =>
                run(async () => {
                  await api.request(`/api/projects/${encodeURIComponent(project.id)}/saved/${encodeURIComponent(item.id)}`, { method: "DELETE" });
                  onOpenChange(false);
                  toast("Removed from the project");
                  refreshSaved(client);
                })
              }
            >
              Remove from project
            </Button>
          )}
          <Button variant="outline" size="sm" onClick={() => copyItem(item)}>
            <Copy /> Copy
          </Button>
          <Button
            size="sm"
            disabled={busy}
            onClick={() =>
              run(async () => {
                await api.request("/api/saved", {
                  method: "POST",
                  body: { kind: item.kind, title: item.title, body: item.body, language: item.language || undefined, url: item.url || undefined, tags: item.tags, searchable: item.searchable },
                });
                onOpenChange(false);
                toast("Saved a copy to your Library");
                refreshSaved(client);
              })
            }
          >
            Save my own copy
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * The Library's dialogs for any page: `open(id)` shows a saved item (yours to edit,
 * someone else's to read), `create(kind)` starts a note or a link, and `saveFrom`
 * opens the save dialog for something in a chat. Render `element` once.
 */
export function useLibraryDialogs(items: SavedItem[] = []) {
  const user = useUser();
  const [edit, setEdit] = useState<{ draft: SaveDraft; title: string } | null>(null);
  const [editOpen, setEditOpen] = useState(false);
  const [view, setView] = useState<SavedItem | null>(null);
  const [viewOpen, setViewOpen] = useState(false);

  const show = useCallback(
    (item: SavedItem) => {
      if (item.ownerId && item.ownerId !== user.id) {
        setView(item);
        setViewOpen(true);
      } else {
        setEdit({ draft: item, title: item.title });
        setEditOpen(true);
      }
    },
    [user.id],
  );
  const open = useCallback(
    async (id: string) => {
      try {
        show(items.find((i) => i.id === id) || (await getSaved(id)));
      } catch (e) {
        toast(messageOf(e));
      }
    },
    [items, show],
  );
  const create = useCallback((kind: SavedKind = "note", extra: Partial<SaveDraft> = {}) => {
    setEdit({ draft: { kind, ...extra }, title: kind === "link" ? "Save a link" : "New note" });
    setEditOpen(true);
  }, []);
  const saveFrom = useCallback(({ kind, title = "", body = "", language = "", conversationId, messageId, projectId = "" }: SaveFromInput) => {
    setEdit({
      draft: { kind, title, body, language, tags: language ? [language.toLowerCase()] : [], sourceConversationId: conversationId, sourceMessageId: messageId, projectId },
      title: "Save to Library",
    });
    setEditOpen(true);
  }, []);
  const element = (
    <>
      {edit && <SaveDialog open={editOpen} onOpenChange={setEditOpen} draft={edit.draft} title={edit.title} />}
      <SavedViewer open={viewOpen} onOpenChange={setViewOpen} item={view} />
    </>
  );
  return { open, create, saveFrom, element };
}
