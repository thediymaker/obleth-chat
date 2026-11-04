"use client";

// One collection: try a search, its documents (upload, status, retry, remove) and who
// can search it. Old knowledge.js detail().

import { useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Check, File as FileIcon, MessageSquare, Pencil, Search, Trash2, Upload } from "lucide-react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useRef, useState } from "react";
import { toast } from "sonner";
import { usePageChrome } from "@/components/app-shell/chrome";
import { ErrorBox, Loading, Page } from "@/components/app-shell/page";
import { chatDraft } from "@/components/projects/chat-draft";
import { Confirm, messageOf } from "@/components/projects/form-field";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { api } from "@/lib/api";
import { count, when } from "@/lib/format";
import { useSession } from "@/lib/session";
import { cn } from "@/lib/utils";
import {
  ACCEPT,
  type CollectionDetail,
  collectionPath,
  DEFAULT_MAX_FILE,
  fileUrl,
  type KnowledgeDocument,
  knowledgeKeys,
  refreshKnowledge,
  type Scope,
  scopeLabel,
  type SearchResult,
  size,
  useCollection,
  useCollections,
} from "./api";
import { CollectionDialog } from "./collection-dialog";

function Status({ d }: { d: KnowledgeDocument }) {
  if (d.status === "queued") return <Badge variant="outline">Waiting</Badge>;
  if (d.status === "processing")
    return (
      <Badge variant="secondary" className="text-blue">
        {d.stage === "embedding" && d.total ? `Indexing ${Math.floor((100 * d.done) / d.total)}%` : "Reading"}
      </Badge>
    );
  if (d.status === "ready")
    return (
      <span className="inline-flex items-center gap-1 text-sm text-success">
        <Check className="size-3.5" /> Ready
      </span>
    );
  const tip = d.status === "needs_ocr" ? "These pages are images with no text layer. Text recognition is not available yet; a PDF with selectable text will work." : d.error || "";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Badge variant="outline" className="border-amber/50 text-amber" tabIndex={0}>
          {d.status === "needs_ocr" ? "Needs text recognition" : "Failed"}
        </Badge>
      </TooltipTrigger>
      {tip && <TooltipContent className="max-w-xs">{tip}</TooltipContent>}
    </Tooltip>
  );
}

function Card({ title, text, children }: { title: string; text?: string; children: React.ReactNode }) {
  return (
    <section className="grid gap-3 rounded-lg border bg-card p-4">
      <div>
        <h2 className="text-base font-semibold">{title}</h2>
        {text && <p className="text-sm text-muted-foreground">{text}</p>}
      </div>
      {children}
    </section>
  );
}

function TrySearch({ detail }: { detail: CollectionDetail }) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState("");
  const input = useRef<HTMLInputElement>(null);
  const ready = detail.documents.some((d) => d.status === "ready");
  return (
    <Card title="Try a search" text="See which passages a chat would get for a question, before you rely on it.">
      <form
        id="knowledge-search"
        className="flex gap-2"
        onSubmit={async (e) => {
          e.preventDefault();
          const q = query.trim();
          if (!q) return;
          setSearching(true);
          setError("");
          try {
            const r = await api.request<{ results?: SearchResult[] }>("/api/knowledge/search", { method: "POST", body: { query: q, collectionIds: [detail.collection.id], limit: 6 } });
            setResults(r.results ?? []);
          } catch (err) {
            setError(messageOf(err));
          } finally {
            setSearching(false);
            requestAnimationFrame(() => input.current?.focus());
          }
        }}
      >
        <Input ref={input} id="knowledge-query" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search this collection" maxLength={500} aria-label="Search this collection" />
        <Button type="submit" variant="outline" disabled={!ready || searching}>
          <Search /> Search
        </Button>
      </form>
      {error && <p className="text-sm text-destructive-foreground">{error}</p>}
      {searching ? (
        <p role="status" className="text-sm text-muted-foreground">
          Searching…
        </p>
      ) : results && !results.length ? (
        <p className="text-sm text-muted-foreground">No passages matched. A chat would answer without this collection.</p>
      ) : results ? (
        <ol className="grid gap-2">
          {results.map((r) => (
            <li key={`${r.documentId}-${r.n}`} className="grid gap-1 rounded-md border bg-background p-3">
              <div className="flex items-center justify-between gap-2">
                <a href={fileUrl(r.documentId, r.page)} target="_blank" rel="noopener noreferrer" className="truncate text-md font-medium hover:underline">
                  {r.name}
                  {r.page ? ` · page ${r.page}` : ""}
                </a>
                <Badge variant="outline">{r.matched}</Badge>
              </div>
              {r.section && <span className="text-xs text-faint">{r.section}</span>}
              <p className="text-sm text-text-2">{r.excerpt}</p>
            </li>
          ))}
        </ol>
      ) : null}
    </Card>
  );
}

function Sharing({ detail }: { detail: CollectionDetail }) {
  const client = useQueryClient();
  const { isAdmin } = useSession();
  const { data } = useCollections();
  const c = detail.collection;
  const groups0 = data?.groups ?? [];
  // The group it is shared with stays a choice even after its owner leaves that group.
  const groups = c.scope === "group" && c.groupId && !groups0.some((g) => g.id === c.groupId) ? [...groups0, { id: c.groupId, name: c.groupName || "Current group" }] : groups0;
  const [busy, setBusy] = useState(false);
  async function setScope(scope: Scope, groupId?: string) {
    setBusy(true);
    const body = { scope, ...(scope === "group" ? { groupId: groupId || c.groupId || groups[0]?.id } : {}) };
    try {
      await api.request(collectionPath(c.id), { method: "PATCH", body });
      toast(scope === "personal" ? "Only you can search it now" : scope === "everyone" ? "Everyone can search it now" : "Group members can search it now");
    } catch (e) {
      toast(messageOf(e));
    }
    await refreshKnowledge(client, c.id);
    setBusy(false);
  }
  if (!c.mine)
    return (
      <p className="text-sm text-muted-foreground">
        {scopeLabel(c)} · shared by {c.ownerName}. You can search it in chats, but only the owner changes it.
      </p>
    );
  const choices: [Scope, string, string, boolean][] = [
    ["personal", "Only you", "Private, like your files", false],
    ["group", "A group", groups.length ? "Members search it; only you edit" : "You are not in any group yet", !groups.length],
    ["everyone", "Everyone", isAdmin ? "Every account can search it" : "Administrators only", !isAdmin],
  ];
  return (
    <div role="radiogroup" aria-label="Who can search it" className="grid gap-2">
      {choices.map(([value, label, note, disabled]) => (
        <label
          key={value}
          className={cn(
            "flex cursor-pointer items-start gap-2.5 rounded-md border p-2.5 hover:bg-accent",
            c.scope === value && "border-line-strong bg-secondary",
            (disabled || busy) && "cursor-not-allowed opacity-60 hover:bg-transparent",
          )}
        >
          <input type="radio" name="knowledge-scope" value={value} checked={c.scope === value} disabled={disabled || busy} onChange={() => setScope(value)} className="mt-1 accent-current" />
          <span className="grid">
            <strong className="text-md font-medium">{label}</strong>
            <small className="text-xs text-muted-foreground">{note}</small>
          </span>
        </label>
      ))}
      {groups.length > 0 && c.scope === "group" && (
        <Select value={c.groupId || undefined} onValueChange={(v) => setScope("group", v)} disabled={busy}>
          <SelectTrigger aria-label="Group" className="w-full">
            <SelectValue placeholder="Choose a group" />
          </SelectTrigger>
          <SelectContent>
            {groups.map((g) => (
              <SelectItem key={g.id} value={g.id}>
                {g.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
    </div>
  );
}

function Documents({ detail, maxFileBytes }: { detail: CollectionDetail; maxFileBytes: number }) {
  const client = useQueryClient();
  const c = detail.collection;
  const [uploading, setUploading] = useState<string[]>([]);
  const [dropping, setDropping] = useState(false);
  const [removing, setRemoving] = useState<KnowledgeDocument | null>(null);
  const refresh = () => refreshKnowledge(client, c.id);

  async function add(files: File[]) {
    if (!c.mine) return;
    for (const file of files) {
      if (file.size > maxFileBytes) {
        toast(`${file.name} is larger than ${size(maxFileBytes)}.`);
        continue;
      }
      setUploading((u) => [...u, file.name]);
      try {
        await api.request(`${collectionPath(c.id)}/documents?name=${encodeURIComponent(file.name)}`, { method: "POST", raw: file, contentType: file.type || "application/octet-stream" });
      } catch (e) {
        if ((e as Error).name !== "AbortError") toast(`${file.name}: ${messageOf(e)}`);
      } finally {
        setUploading((u) => u.filter((n) => n !== file.name));
      }
      await refresh();
    }
  }
  async function retry(d: KnowledgeDocument) {
    try {
      await api.request(`/api/knowledge/documents/${encodeURIComponent(d.id)}/retry`, { method: "POST" });
    } catch (e) {
      toast(messageOf(e));
    }
    await refresh();
  }

  const docs = detail.documents;
  return (
    <section className="grid gap-3">
      <h2 className="text-base font-semibold">Documents</h2>
      {c.mine && (
        <label
          htmlFor="knowledge-files"
          onDragOver={(e) => {
            if (!e.dataTransfer.types.includes("Files")) return;
            e.preventDefault();
            setDropping(true);
          }}
          onDragLeave={(e) => {
            if (!e.currentTarget.contains(e.relatedTarget as Node)) setDropping(false);
          }}
          onDrop={(e) => {
            e.preventDefault();
            setDropping(false);
            add([...e.dataTransfer.files]);
          }}
          className={cn(
            "flex cursor-pointer items-center gap-3 rounded-lg border border-dashed bg-card px-4 text-left transition-colors hover:border-line-strong hover:bg-raised focus-within:ring-2 focus-within:ring-ring/50",
            docs.length ? "py-3" : "flex-col py-10 text-center",
            dropping && "border-foreground bg-accent",
          )}
        >
          <Upload className="size-5 shrink-0 text-muted-foreground" />
          <span className="grid gap-0.5">
            <strong className="text-md font-medium">Drop files here or choose them</strong>
            <small className="text-xs text-muted-foreground">
              PDF, Word (.docx), PowerPoint (.pptx), Excel (.xlsx), HTML, Markdown, text and code · up to {size(maxFileBytes)} each
            </small>
          </span>
          <input
            id="knowledge-files"
            className="sr-only"
            type="file"
            multiple
            accept={ACCEPT}
            onChange={(e) => {
              const files = [...(e.target.files ?? [])];
              e.target.value = "";
              add(files);
            }}
          />
        </label>
      )}
      {uploading.length > 0 && (
        <p role="status" className="text-sm text-muted-foreground">
          Uploading {uploading.join(", ")}…
        </p>
      )}
      {docs.length ? (
        <div className="overflow-hidden rounded-lg border bg-card">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead className="hidden sm:table-cell">Pages</TableHead>
                <TableHead className="hidden sm:table-cell">Passages</TableHead>
                <TableHead className="hidden md:table-cell">Size</TableHead>
                <TableHead>Status</TableHead>
                <TableHead className="hidden lg:table-cell">Added</TableHead>
                <TableHead className="w-0">
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {docs.map((d) => (
                <TableRow key={d.id}>
                  <TableCell className="max-w-44 whitespace-normal sm:max-w-64">
                    <a href={fileUrl(d.id)} target="_blank" rel="noopener noreferrer" className="inline-flex max-w-full items-center gap-2 hover:underline">
                      <FileIcon className="size-4 shrink-0 text-muted-foreground" />
                      <span className="truncate">{d.name}</span>
                    </a>
                    {d.status === "failed" && d.error && <small className="block text-xs text-amber">{d.error}</small>}
                  </TableCell>
                  <TableCell className="hidden sm:table-cell">{d.pages ?? "—"}</TableCell>
                  <TableCell className="hidden sm:table-cell">{d.status === "ready" ? d.passages.toLocaleString() : "—"}</TableCell>
                  <TableCell className="hidden md:table-cell">{size(d.size)}</TableCell>
                  <TableCell>
                    <Status d={d} />
                  </TableCell>
                  <TableCell className="hidden text-muted-foreground lg:table-cell">{when(d.createdAt)}</TableCell>
                  <TableCell>
                    {c.mine && (
                      <div className="flex items-center justify-end gap-1">
                        {d.status === "failed" && (
                          <Button variant="ghost" size="xs" onClick={() => retry(d)}>
                            Retry
                          </Button>
                        )}
                        <Button variant="ghost" size="icon-sm" aria-label={`Remove ${d.name}`} title="Remove" onClick={() => setRemoving(d)}>
                          <Trash2 />
                        </Button>
                      </div>
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      ) : (
        !c.mine && <p className="text-sm text-muted-foreground">No documents yet.</p>
      )}
      <Confirm
        open={!!removing}
        onOpenChange={(o) => !o && setRemoving(null)}
        title={`Remove ${removing?.name ?? ""}?`}
        text="The file and its passages are deleted. Chats stop citing it."
        cancel="Keep it"
        confirm="Remove"
        onConfirm={async () => {
          await api.request(`/api/knowledge/documents/${encodeURIComponent(removing!.id)}`, { method: "DELETE" });
          await refresh();
          toast("Document removed");
        }}
      />
    </section>
  );
}

export function CollectionPage({ id }: { id: string }) {
  const router = useRouter();
  const { data: detail, error, isPending, refetch } = useCollection(id);
  const { data: list } = useCollections();
  const [editing, setEditing] = useState(false);
  usePageChrome(detail?.collection.name || "Collection");
  if (!detail)
    return (
      <Page wide>
        {isPending ? (
          <Loading label="Opening the collection…" />
        ) : (
          <>
            <ErrorBox error={error} onRetry={() => refetch()} />
            <Button asChild size="sm" variant="outline">
              <Link href="/knowledge">
                <ArrowLeft /> All collections
              </Link>
            </Button>
          </>
        )}
      </Page>
    );
  const c = detail.collection;
  const docs = detail.documents;
  const passages = docs.reduce((n, d) => n + (d.status === "ready" ? d.passages : 0), 0);
  const bytes = docs.reduce((n, d) => n + d.size, 0);
  return (
    <Page wide>
      <nav aria-label="Breadcrumb" className="mb-3 flex items-center gap-1.5 text-sm text-muted-foreground">
        <Link href="/knowledge" className="hover:text-foreground">
          Library
        </Link>
        <span className="text-faint">/</span>
        <span className="truncate text-text-2">{c.name}</span>
      </nav>
      <div className="mb-6 flex flex-wrap items-start justify-between gap-4">
        <div className="min-w-0">
          <Badge variant="outline" className="mb-2.5">
            {c.mine ? scopeLabel(c) : `${scopeLabel(c)} · read only`}
          </Badge>
          <h1 className="text-[26px] leading-tight font-semibold tracking-tight">{c.name}</h1>
          {c.description && <p className="mt-1.5 text-sm text-muted-foreground">{c.description}</p>}
          <p className="mt-1 text-sm text-faint">
            {count(docs.length, "document")} · {count(passages, "passage")} · {size(bytes)}
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="outline"
            onClick={() => {
              chatDraft.reset();
              chatDraft.setCollections([c.id]);
              router.push("/");
            }}
          >
            <MessageSquare /> Ask in a new chat
          </Button>
          {c.mine && (
            <Button variant="ghost" onClick={() => setEditing(true)}>
              <Pencil /> Edit
            </Button>
          )}
        </div>
      </div>
      <ErrorBox error={error} onRetry={() => refetch()} />
      <div className="grid grid-cols-1 items-start gap-6 lg:grid-cols-[minmax(0,1fr)_320px]">
        <div className="grid gap-6">
          <TrySearch key={c.id} detail={detail} />
          <Documents detail={detail} maxFileBytes={list?.maxFileBytes || DEFAULT_MAX_FILE} />
        </div>
        <aside className="grid gap-4">
          <Card title="Who can search it">
            <Sharing detail={detail} />
          </Card>
          <Card title="How it searches">
            <dl className="grid gap-1.5 text-sm">
              {[
                ["Meaning", "Embeddings"],
                ["Words", "Full-text match"],
                ["Passage size", "About 250 words"],
                ["Per question", "Best 6 passages"],
              ].map(([k, v]) => (
                <div key={k} className="flex justify-between gap-2">
                  <dt className="text-muted-foreground">{k}</dt>
                  <dd>{v}</dd>
                </div>
              ))}
            </dl>
            <p className="text-xs text-muted-foreground">Models that use tools can search again with their own words.</p>
          </Card>
        </aside>
      </div>
      {c.mine && <CollectionDialog open={editing} onOpenChange={setEditing} detail={detail} />}
    </Page>
  );
}

