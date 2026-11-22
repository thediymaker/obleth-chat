"use client";

// The notebook's right pane: "Make from the sources" (a briefing, FAQ, study guide or
// timeline the Notebook model writes from the full text of the ticked sources), what
// the notebook keeps, and the project's notes. Without a Notebook model there are no
// tiles; an administrator is told where to name one.

import { useQueryClient } from "@tanstack/react-query";
import { Trash2 } from "lucide-react";
import Link from "next/link";
import { useState } from "react";
import { toast } from "sonner";
import { Eyebrow, Loading, ErrorBox } from "@/components/app-shell/page";
import { useChatStore } from "@/components/chat/chat-provider";
import { Markdown } from "@/components/chat/markdown-view";
import { CitationContext, type CitationFocus, citeHtml, QuotedPassage, type SourceItem } from "@/components/knowledge/chat-knowledge";
import { messageOf, Note } from "@/components/manage/kit";
import { useProjectFrame } from "@/components/projects/project-frame";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { count, when } from "@/lib/format";
import { useSession } from "@/lib/session";
import { cn } from "@/lib/utils";
import { citationItems, KINDS, meterRefusal, type Notebook, type NotebookItem, notebookKey, noteBody, removeItem, startItem, useNotebookItem } from "./api";

/** Things one person may have being made at once, as the server allows. */
const AT_ONCE = 2;

export function StudioPane({ projectId, n }: { projectId: string; n: Notebook }) {
  const { detail } = useProjectFrame();
  const { isAdmin } = useSession();
  const [open, setOpen] = useState<string | null>(null);
  const notes = detail.saved.filter((s) => s.kind === "note" || s.kind === "excerpt").slice(0, 8);
  return (
    <aside aria-label="Make from the sources" className="grid content-start gap-5 px-4 py-4 lg:border-l">
      <MakeSection projectId={projectId} n={n} isAdmin={isAdmin} />
      {n.items.length > 0 && (
        <section className="grid gap-2" aria-label="In this notebook">
          <Eyebrow>In this notebook</Eyebrow>
          <ul className="grid gap-1">
            {n.items.map((item) => (
              <ItemRow key={item.id} projectId={projectId} item={item} onOpen={() => setOpen(item.id)} />
            ))}
          </ul>
        </section>
      )}
      <section className="grid gap-2">
        <Eyebrow>Notes in this project</Eyebrow>
        {notes.length ? (
          <ul className="grid gap-1">
            {notes.map((s) => (
              <li key={s.id}>
                <Link href={`/library/${encodeURIComponent(s.id)}`} className="grid gap-0.5 rounded-md border px-3 py-2 hover:bg-accent">
                  <span className="truncate text-md">{s.title}</span>
                  <span className="text-xs text-muted-foreground">{when(String(s.createdAt || ""))}</span>
                </Link>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-sm text-muted-foreground">Answers you save as a note appear here.</p>
        )}
      </section>
      <ItemDialog projectId={projectId} id={open} onClose={() => setOpen(null)} />
    </aside>
  );
}

function MakeSection({ projectId, n, isAdmin }: { projectId: string; n: Notebook; isAdmin: boolean }) {
  const client = useQueryClient();
  const [busy, setBusy] = useState("");
  if (!n.model.set)
    return isAdmin ? (
      <Note data-admin-hint>
        Name a Notebook model in{" "}
        <Link href="/settings/admin/system" className="underline underline-offset-2">
          Administration › System
        </Link>{" "}
        to make briefings, FAQs, study guides and timelines from the sources, and to give each new source a short guide.
      </Note>
    ) : null;
  if (!n.model.available) return <Note>The Notebook model is not available to your account, so nothing can be made from the sources here.</Note>;
  if (!n.meter) return <Note>The gateway does not say how much the Notebook model can read, so nothing can be made from the sources yet.</Note>;
  const running = n.items.filter((i) => i.status === "running").length;
  const why = !n.ticked ? "Tick sources to make something from them." : !n.meter.fits ? meterRefusal(n.meter) : running >= AT_ONCE ? "Two things are being made already. The next can start when one finishes." : "";
  async function make(kind: string) {
    setBusy(kind);
    try {
      await startItem(projectId, kind);
      await client.invalidateQueries({ queryKey: notebookKey(projectId) });
    } catch (e) {
      toast(messageOf(e));
    } finally {
      setBusy("");
    }
  }
  return (
    <section className="grid gap-2.5">
      <h2 className="text-base font-semibold">Make from the sources</h2>
      <div className="grid grid-cols-2 gap-2">
        {KINDS.map(([kind, name, sub]) => (
          <button
            key={kind}
            type="button"
            disabled={!!why || !!busy}
            onClick={() => make(kind)}
            className="grid min-h-14 content-start gap-0.5 rounded-lg border bg-card px-3 py-2 text-left hover:border-line-strong hover:bg-raised disabled:cursor-not-allowed disabled:opacity-60 disabled:hover:border-border disabled:hover:bg-card"
          >
            <span className="text-md font-medium">{name}</span>
            <span className="text-xs text-muted-foreground">{sub}</span>
          </button>
        ))}
      </div>
      {why ? <p className="text-xs text-muted-foreground">{why}</p> : <p className="text-xs text-muted-foreground">Written from the full text of the {count(n.ticked, "ticked source")}, with every quotation checked against them.</p>}
    </section>
  );
}

function ItemRow({ projectId, item, onOpen }: { projectId: string; item: NotebookItem; onOpen: () => void }) {
  const client = useQueryClient();
  const alive = item.status === "running";
  const sources = count(item.sources.length, "source");
  const meta = alive ? `Reading ${sources}` : item.status === "failed" ? item.error || "It could not be made" : `${sources} · ${when(item.finishedAt || item.createdAt)}`;
  async function remove() {
    try {
      await removeItem(item.id);
      client.invalidateQueries({ queryKey: notebookKey(projectId) });
    } catch (e) {
      toast(messageOf(e));
    }
  }
  return (
    <li className="group/item flex items-start gap-1 rounded-md border" data-item={item.status}>
      <button type="button" className="flex min-w-0 flex-1 items-start gap-2.5 px-3 py-2 text-left disabled:cursor-default" disabled={item.status !== "ready"} onClick={onOpen}>
        {alive && (
          // Alive: the accent is only for what is working now.
          <span className="relative mt-1.5 flex size-2 shrink-0" aria-hidden="true">
            <span className="signal-pulse absolute inset-0 rounded-full bg-signal" />
            <span className="relative size-2 rounded-full bg-signal" />
          </span>
        )}
        <span className="grid min-w-0 gap-0.5">
          <span className="truncate text-md">{item.title}</span>
          <span className={cn("text-xs", item.status === "failed" ? "text-amber" : "text-muted-foreground")} role={alive ? "status" : undefined}>
            {meta}
          </span>
        </span>
      </button>
      {!alive && (
        <Button variant="ghost" size="icon-xs" className="mt-1.5 mr-1.5 text-muted-foreground" aria-label={`Delete ${item.title}`} onClick={remove}>
          <Trash2 />
        </Button>
      )}
    </li>
  );
}

/** A made item: its text with checked citations, which open their passage in place. */
function ItemDialog({ projectId, id, onClose }: { projectId: string; id: string | null; onClose: () => void }) {
  const { data, error, isPending } = useNotebookItem(id);
  const store = useChatStore();
  const client = useQueryClient();
  const [focus, setFocus] = useState<CitationFocus | null>(null);
  const items: SourceItem[] = data ? citationItems(data) : [];
  const reply = data ? { id: data.id, role: "assistant", content: data.body, activity: [{ kind: "sources", items }] } : null;
  function onClick(e: React.MouseEvent) {
    const link = (e.target as HTMLElement).closest?.("a.cite");
    if (!link || e.metaKey || e.ctrlKey || e.shiftKey || !data) return;
    const number = Number(link.textContent);
    if (!number) return;
    e.preventDefault();
    setFocus({ messageId: data.id, n: number });
  }
  const shown = focus && items.find((s) => s.n === focus.n);
  return (
    <Dialog open={!!id} onOpenChange={(o) => !o && (setFocus(null), onClose())}>
      <DialogContent className="max-h-[92dvh] overflow-y-auto sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>{data?.title || "From the sources"}</DialogTitle>
          <DialogDescription>{data ? `From ${data.sources.map((s) => s.name).join(", ")}. ${quotationLine(data)}` : " "}</DialogDescription>
        </DialogHeader>
        {isPending ? (
          <Loading compact />
        ) : error ? (
          <ErrorBox compact error={error} />
        ) : data && reply ? (
          <CitationContext.Provider value={{ focus, clear: () => setFocus(null) }}>
            <div onClickCapture={onClick} className="min-w-0 text-base leading-[1.75] text-text-2">
              <Markdown text={data.body} cite={(html) => citeHtml(html, reply)} />
            </div>
            {shown && <PassageCard s={shown} onClose={() => setFocus(null)} />}
          </CitationContext.Provider>
        ) : null}
        <DialogFooter>
          {data && (
            <Button
              variant="outline"
              onClick={async () => {
                try {
                  await removeItem(data.id);
                  client.invalidateQueries({ queryKey: notebookKey(projectId) });
                  onClose();
                } catch (e) {
                  toast(messageOf(e));
                }
              }}
            >
              <Trash2 /> Delete
            </Button>
          )}
          {data && (
            <Button
              onClick={() => {
                store.openDialog({ kind: "save", item: { kind: "note", title: data.title, body: noteBody(data.body || "", items), projectId } });
              }}
            >
              Save as a note
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function quotationLine(item: NotebookItem) {
  if (!item.cited && !item.unverified) return "It quotes none of them.";
  const checked = `${count(item.cited, "quotation")} checked against the sources`;
  return item.unverified ? `${checked}; ${count(item.unverified, "quotation")} not found there, listed at the end and not shown as a citation.` : `${checked}.`;
}

/** The passage a clicked citation quotes, kept in view at the foot of the text. */
function PassageCard({ s, onClose }: { s: SourceItem; onClose: () => void }) {
  return (
    <div className="sticky bottom-0 bg-background">
      <QuotedPassage s={s} onClose={onClose} />
    </div>
  );
}
