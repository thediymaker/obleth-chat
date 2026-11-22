"use client";

// A project's Notebook tab: the project's sources with a tick each (kept per person),
// a chat that answers from the ticked ones with numbered citations, and, beside it,
// things made from the sources (studio.tsx). Three panes that stack on a phone. The
// chat is the real chat (the store, its messages and composer), shown at
// /projects/<id>/notebook/<chat>.

import { useQueryClient } from "@tanstack/react-query";
import { BookOpen, ChevronDown, ExternalLink, FilePlus2, Plus, Upload } from "lucide-react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { Fragment, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { EmptyState, ErrorBox, Eyebrow, Loading } from "@/components/app-shell/page";
import { useChat, useChatContent } from "@/components/chat/chat-provider";
import { Composer } from "@/components/chat/composer";
import { MessageView, messageSignature } from "@/components/chat/message";
import { ACCEPT, fileUrl, refreshKnowledge, uploadDocument } from "@/components/knowledge/api";
import { CitationContext, type CitationFocus, sourcesOf } from "@/components/knowledge/chat-knowledge";
import { Status } from "@/components/knowledge/collection-page";
import { Meter as Bar, messageOf, Note } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { count, when } from "@/lib/format";
import { keys } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { StudioPane } from "./studio";
import { makeCollection, meterText, type Notebook, noteBody, notebookHref, notebookKey, type Source, setOnlySources, setTicked, sourceMeta, startChat, suggestions, useNotebook } from "./api";

/** The most characters of pasted text, as the server reads a text file. */
const PASTE_LIMIT = 500_000;

export function ProjectNotebook({ projectId, chatId }: { projectId: string; chatId: string | null }) {
  const notebook = useNotebook(projectId);
  const n = notebook.data;
  const store = useChat();
  const router = useRouter();
  const client = useQueryClient();
  const starting = useRef(false);
  // The notebook's chat goes in the address, so the chat store shows it; one is made
  // with the person's model the first time.
  useEffect(() => {
    if (!n || chatId) return;
    if (n.chatId) {
      router.replace(notebookHref(projectId, n.chatId));
      return;
    }
    if (!store.model || starting.current) return;
    starting.current = true;
    startChat(projectId, store.model)
      .then((r) => {
        client.invalidateQueries({ queryKey: notebookKey(projectId) });
        router.replace(notebookHref(projectId, r.chatId));
      })
      .catch((e) => toast(messageOf(e)))
      .finally(() => {
        starting.current = false;
      });
  }, [n, chatId, projectId, store.model, router, client]);

  if (!n) return notebook.isPending ? <Loading label="Opening the notebook…" /> : <ErrorBox error={notebook.error} onRetry={() => notebook.refetch()} />;
  return (
    <div data-notebook className="grid overflow-hidden rounded-xl border bg-background lg:grid-cols-[minmax(250px,300px)_minmax(0,1fr)_minmax(240px,290px)]">
      <SourcesPane projectId={projectId} n={n} />
      <AnswersPane projectId={projectId} chatId={chatId} n={n} />
      <StudioPane projectId={projectId} n={n} />
    </div>
  );
}

// ---- Sources ----

function SourcesPane({ projectId, n }: { projectId: string; n: Notebook }) {
  const client = useQueryClient();
  const [openId, setOpenId] = useState<string | null>(null);
  const [pasting, setPasting] = useState(false);
  const [adding, setAdding] = useState(false);
  const files = useRef<HTMLInputElement>(null);
  const canAdd = n.addTo.length > 0 || n.canMakeCollection;
  const ask = useAsk();

  async function tick(ids: string[], ticked: boolean) {
    try {
      await setTicked(client, projectId, ids, ticked);
    } catch (e) {
      toast(messageOf(e));
    }
  }

  /** Puts files in the person's collection in the project (made for the owner when
   *  there is none) and ticks them. */
  async function add(list: File[]) {
    if (!list.length) return;
    setAdding(true);
    try {
      const target = n.addTo[0] || (n.canMakeCollection ? (await makeCollection(projectId)).collectionId : "");
      if (!target) return;
      const added: string[] = [];
      for (const file of list) {
        try {
          const r = await uploadDocument(target, file);
          const id = (r as { document?: { id?: string } }).document?.id;
          if (id) added.push(id);
        } catch (e) {
          toast(`${file.name}: ${messageOf(e)}`);
        }
      }
      if (added.length) {
        await setTicked(client, projectId, added, true);
        toast(added.length === 1 ? "Source added. It is read and indexed in the background." : `${added.length} sources added. They are read and indexed in the background.`);
      } else client.invalidateQueries({ queryKey: notebookKey(projectId) });
      refreshKnowledge(client, target);
      client.invalidateQueries({ queryKey: keys.project(projectId) });
    } catch (e) {
      toast(messageOf(e));
    } finally {
      setAdding(false);
    }
  }

  return (
    <aside aria-label="Sources" className="grid content-start gap-3.5 border-b px-3.5 py-4 lg:border-r lg:border-b-0">
      <div className="flex items-center gap-2 px-1">
        <h2 className="text-base font-semibold">Sources</h2>
        <span className="font-mono text-xs text-muted-foreground">
          {n.ticked} of {n.total} ticked
        </span>
        {canAdd && (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="sm" className="ml-auto" disabled={adding}>
                <Plus /> Add
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" aria-label="Add sources">
              <DropdownMenuItem onSelect={() => files.current?.click()}>
                <Upload /> Upload files
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => setPasting(true)}>
                <FilePlus2 /> Paste text
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        )}
        <input
          ref={files}
          type="file"
          multiple
          accept={ACCEPT}
          className="sr-only"
          tabIndex={-1}
          aria-label="Files to add"
          onChange={(e) => {
            const list = [...(e.target.files || [])];
            e.target.value = "";
            void add(list);
          }}
        />
      </div>
      {n.meter && (
        <div className="grid gap-1.5 px-1" data-meter>
          <span className={cn("text-xs", n.meter.fits ? "text-muted-foreground" : "text-amber")}>{meterText(n.meter)}</span>
          <Bar value={Math.min(n.meter.tokens, n.meter.capacity)} max={Math.max(1, n.meter.capacity)} tone={n.meter.fits ? (n.meter.percent >= 80 ? "amber" : "blue") : "danger"} label="Share of what the notebook can read" />
        </div>
      )}
      {n.collections.length === 0 ? (
        <Note className="px-1">{canAdd ? "No sources yet. Add files or paste text, and they are ticked for you." : "This project has no sources yet. Its owner can add them here."}</Note>
      ) : (
        n.collections.map((g) => (
          <section key={g.id} className="grid gap-px" aria-label={g.name}>
            <div className="flex items-center gap-2 px-1 pb-1">
              <h3 className="min-w-0 truncate text-2xs font-medium tracking-[.08em] text-muted-foreground uppercase">
                {g.name} · {g.total}
              </h3>
              {g.documents.length > 1 && (
                <button
                  type="button"
                  className="ml-auto shrink-0 text-xs text-muted-foreground hover:text-foreground"
                  onClick={() => {
                    const all = g.documents.every((d) => d.ticked);
                    void tick(
                      g.documents.map((d) => d.id),
                      !all,
                    );
                  }}
                >
                  {g.documents.every((d) => d.ticked) ? "Untick all" : "Tick all"}
                </button>
              )}
            </div>
            {g.documents.length === 0 && <p className="px-1.5 py-1 text-sm text-muted-foreground">No documents yet.</p>}
            <ul className="grid gap-px">
              {g.documents.map((d) => (
                <SourceRow key={d.id} d={d} open={openId === d.id} onOpen={() => setOpenId(openId === d.id ? null : d.id)} onTick={(on) => tick([d.id], on)} onAsk={ask} />
              ))}
            </ul>
            {g.total > g.documents.length && (
              <Link href={`/knowledge/${encodeURIComponent(g.id)}`} className="px-1.5 pt-1 pl-8 text-xs text-muted-foreground hover:text-foreground">
                {g.total - g.documents.length} more in the collection
              </Link>
            )}
          </section>
        ))
      )}
      <PasteDialog open={pasting} onOpenChange={setPasting} onAdd={(file) => add([file])} />
    </aside>
  );
}

function SourceRow({ d, open, onOpen, onTick, onAsk }: { d: Source; open: boolean; onOpen: () => void; onTick: (on: boolean) => void; onAsk: (q: string) => void }) {
  const ready = d.status === "ready";
  return (
    <li data-source={d.id}>
      <div className="flex items-start gap-2.5 rounded-md px-1.5 py-1.5 hover:bg-accent">
        <Checkbox className="mt-0.5" checked={d.ticked} onCheckedChange={(v) => onTick(v === true)} aria-label={`Use ${d.name}`} />
        <button type="button" className="grid min-w-0 flex-1 gap-0.5 text-left" aria-expanded={open} onClick={onOpen}>
          <span className={cn("line-clamp-2 text-md leading-snug break-words", d.ticked ? "text-foreground" : "text-text-2")} title={d.name}>
            {d.name}
          </span>
          {ready ? (
            <span className="text-xs text-muted-foreground">{sourceMeta(d)}</span>
          ) : (
            <span className="justify-self-start">
              <Status d={d} />
            </span>
          )}
        </button>
        <ChevronDown className={cn("mt-1 size-3.5 shrink-0 text-faint transition-transform", open && "rotate-180")} aria-hidden="true" />
      </div>
      {open && (
        <div className="mb-1.5 ml-8 grid gap-2 rounded-md border bg-card px-3 py-2.5 text-sm" data-guide>
          {d.guide?.status === "ready" ? (
            <>
              <p className="leading-relaxed text-text-2">{d.guide.summary}</p>
              {!!d.guide.questions?.length && (
                <div className="flex flex-wrap gap-1.5">
                  {d.guide.questions.map((q) => (
                    <button key={q} type="button" className="rounded-full border px-2.5 py-1 text-left text-xs text-text-2 hover:border-line-strong hover:text-foreground" onClick={() => onAsk(q)}>
                      {q}
                    </button>
                  ))}
                </div>
              )}
            </>
          ) : d.guide?.status === "running" ? (
            <p className="text-muted-foreground">A short guide to this source is being written.</p>
          ) : ready ? (
            <p className="text-muted-foreground">No guide for this source.</p>
          ) : null}
          <span className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
            <span>Added {when(d.createdAt)}</span>
            <a href={fileUrl(d.id)} target="_blank" rel="noopener noreferrer" className="inline-flex items-center gap-1 hover:text-foreground">
              Open the file <ExternalLink className="size-3" />
            </a>
          </span>
        </div>
      )}
    </li>
  );
}

function PasteDialog({ open, onOpenChange, onAdd }: { open: boolean; onOpenChange: (open: boolean) => void; onAdd: (file: File) => Promise<void> }) {
  const [title, setTitle] = useState("");
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Paste text as a source</DialogTitle>
          <DialogDescription>It is kept as a text file in the project’s collection, indexed, and ticked for you.</DialogDescription>
        </DialogHeader>
        <form
          id="notebook-paste"
          className="grid gap-3"
          onSubmit={async (e) => {
            e.preventDefault();
            const body = text.trim();
            if (!body) return;
            const name = (title.trim() || body.split("\n")[0]!.trim().slice(0, 60) || "Pasted text").replace(/[\\/:*?"<>|]+/g, " ").trim();
            setBusy(true);
            try {
              await onAdd(new File([body], `${name}.md`, { type: "text/markdown" }));
              setTitle("");
              setText("");
              onOpenChange(false);
            } finally {
              setBusy(false);
            }
          }}
        >
          <Input id="notebook-paste-title" aria-label="Title" placeholder="Title" maxLength={120} value={title} onChange={(e) => setTitle(e.target.value)} />
          <Textarea id="notebook-paste-text" aria-label="Text" placeholder="Paste the text here" rows={10} maxLength={PASTE_LIMIT} value={text} onChange={(e) => setText(e.target.value)} />
        </form>
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="submit" form="notebook-paste" disabled={busy || !text.trim()}>
            Add source
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ---- Answers ----

/** Sends a question to the notebook's chat. */
function useAsk() {
  const store = useChat();
  return (question: string) => {
    if (store.streaming) {
      toast("Wait for the answer being written, or stop it.");
      return;
    }
    void store.run(() => store.send(question));
  };
}

function AnswersPane({ projectId, chatId, n }: { projectId: string; chatId: string | null; n: Notebook }) {
  const store = useChat();
  const router = useRouter();
  const client = useQueryClient();
  const ask = useAsk();
  const [focus, setFocus] = useState<CitationFocus | null>(null);
  const [only, setOnly] = useState(n.onlySources);
  const [starting, setStarting] = useState(false);
  const c = chatId && store.active?.id === chatId ? store.active : null;
  useEffect(() => setOnly(n.onlySources), [n.onlySources, chatId]);
  const chips = suggestions(n);
  const empty = !c?.messages?.length;

  async function toggleOnly(on: boolean) {
    if (!chatId) return;
    setOnly(on);
    try {
      await setOnlySources(chatId, on);
      client.invalidateQueries({ queryKey: notebookKey(projectId) });
    } catch (e) {
      setOnly(!on);
      toast(messageOf(e));
    }
  }
  async function newChat() {
    if (!store.model) return;
    setStarting(true);
    try {
      const r = await startChat(projectId, store.model);
      client.invalidateQueries({ queryKey: notebookKey(projectId) });
      router.push(notebookHref(projectId, r.chatId));
    } catch (e) {
      toast(messageOf(e));
    } finally {
      setStarting(false);
    }
  }
  // A clicked [n] shows its passage under the answer; with a modifier key it opens the file.
  function onClick(e: React.MouseEvent) {
    const link = (e.target as HTMLElement).closest?.("a.cite");
    if (!link || e.metaKey || e.ctrlKey || e.shiftKey || !c) return;
    const index = Number(link.closest("[data-message]")?.getAttribute("data-message"));
    const m = c.messages[index];
    const number = Number(link.textContent);
    if (!m?.id || !number) return;
    e.preventDefault();
    setFocus(focus?.messageId === m.id && focus.n === number ? null : { messageId: m.id, n: number });
  }

  return (
    <section aria-label="Ask the sources" className="@container flex min-w-0 flex-col border-b lg:border-b-0">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b px-5 py-2.5 text-sm text-muted-foreground">
        <span>Answers come from the ticked sources, and each one cites the passages it used.</span>
        <label className="ml-auto flex items-center gap-2 text-foreground">
          <Switch checked={only} disabled={!chatId} onCheckedChange={(v) => void toggleOnly(v)} aria-label="Only from the sources" />
          Only from the sources
        </label>
        <Button variant="ghost" size="sm" onClick={newChat} disabled={starting || !store.model || store.streaming}>
          <Plus /> New chat
        </Button>
      </div>
      <div className="flex flex-1 justify-center px-5 pt-5">
        <div className="grid w-full max-w-[760px] grid-cols-[minmax(0,1fr)] content-start gap-4">
          {chips.length > 0 && (
            <section className="grid gap-2.5 rounded-xl border bg-card px-4 py-3.5" aria-label="Suggested questions">
              <Eyebrow>Ask the sources</Eyebrow>
              <div className="flex flex-wrap gap-1.5">
                {chips.map((q) => (
                  <button key={q} type="button" data-suggestion className="min-h-8 rounded-full border px-3 py-1 text-left text-sm text-text-2 hover:border-line-strong hover:text-foreground" onClick={() => ask(q)}>
                    {q}
                  </button>
                ))}
              </div>
            </section>
          )}
          {!c ? (
            store.chatMissing ? (
              <EmptyState icon={BookOpen} title="This chat is gone" text="Start a new chat to ask the ticked sources." />
            ) : (
              <Loading compact label="Opening the notebook’s chat…" />
            )
          ) : (
            <CitationContext.Provider value={{ focus, clear: () => setFocus(null) }}>
              {/* Captured, so a clicked [n] is handled before the link opens the file. */}
              <div onClickCapture={onClick}>
                <NotebookMessages projectId={projectId} />
              </div>
            </CitationContext.Provider>
          )}
          {c && empty && (
            <p className="py-6 text-center text-md text-muted-foreground">
              {n.ticked ? `Ask a question about the ${count(n.ticked, "ticked source")}.` : "Tick sources on the left, then ask a question about them."}
            </p>
          )}
        </div>
      </div>
      <div className="sticky bottom-0 z-[5] mx-auto grid w-full max-w-[800px] gap-2 bg-[linear-gradient(transparent,var(--background)_18px)] px-5 pt-4 pb-3">
        {store.error && (
          <p role="alert" className="text-sm text-amber">
            {store.error}
          </p>
        )}
        <Composer lean placeholder={n.ticked ? `Ask the ${count(n.ticked, "ticked source")}` : "Tick sources to ask about them"} />
      </div>
    </section>
  );
}

function NotebookMessages({ projectId }: { projectId: string }) {
  const store = useChatContent();
  const messages = store.active?.messages || [];
  function saveNote(index: number) {
    const m = messages[index]!;
    const question = [...messages.slice(0, index)].reverse().find((x) => x.role === "user")?.content || "";
    const title = question.trim().replace(/\s+/g, " ").slice(0, 80) || "Note from the notebook";
    store.saveFromChat({ kind: "note", title, body: noteBody(String(m.content || ""), sourcesOf(m)?.items ?? []) }, index, projectId);
  }
  return (
    <div id="messages">
      {messages.map((m, index) => (
        <Fragment key={m.id || `pending-${index}`}>
          <MessageView m={m} index={index} signature={messageSignature(store, m, index, messages.length)} version={index >= messages.length - 2 ? store.version : 0} />
          {m.role === "assistant" && m.id && m.status === "completed" && String(m.content || "").trim() && !store.active?.temporary && (
            <div className="-mt-5 mb-8">
              <Button variant="outline" size="sm" onClick={() => saveNote(index)}>
                Save as a note
              </Button>
            </div>
          )}
        </Fragment>
      ))}
    </div>
  );
}
