"use client";

// An app's own chat page (/a/<id>/<slug>): a plain conversation where each message
// runs the published app as the person chatting. The page keeps the conversation
// (for this tab, in sessionStorage); Chat keeps only each run, under the app's Runs.

import { useQuery } from "@tanstack/react-query";
import { AlertCircle, ArrowUp, Pencil, RotateCcw, SquarePen } from "lucide-react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect, useRef, useState } from "react";
import { Brand } from "@/components/brand";
import { Markdown } from "@/components/chat/markdown-view";
import { ThemeToggle } from "@/components/theme-toggle";
import { Button } from "@/components/ui/button";
import { ApiError } from "@/lib/api";
import { initials } from "@/lib/format";
import { useUser } from "@/lib/session";
import { appPageKey, type AppPage, chatPagePath, type ChatTurn, chatWithApp, getAppPage } from "./api";

interface Entry extends ChatTurn {
  /** A reply that failed: the error, shown instead of content. */
  error?: string;
}

/** A run's output as a reply: text as is, an object's one text field (answer, reply…), else JSON. */
export function replyText(output: unknown): string {
  if (output == null) return "";
  if (typeof output === "string") return output;
  if (typeof output === "object" && !Array.isArray(output)) {
    const o = output as Record<string, unknown>;
    for (const name of ["answer", "reply", "response", "text", "message", "content", "output"]) if (typeof o[name] === "string") return o[name] as string;
    const texts = Object.values(o).filter((v) => typeof v === "string");
    if (texts.length === 1 && Object.keys(o).length === 1) return texts[0] as string;
  }
  return "```json\n" + JSON.stringify(output, null, 2) + "\n```";
}

const storageKey = (id: string) => `obleth-chat:app-chat:${id}`;
function loadEntries(id: string): Entry[] {
  try {
    const saved = JSON.parse(sessionStorage.getItem(storageKey(id)) || "[]");
    return Array.isArray(saved) ? saved.filter((e) => e && (e.role === "user" || e.role === "assistant") && typeof e.content === "string") : [];
  } catch {
    return [];
  }
}
function saveEntries(id: string, entries: Entry[]) {
  try {
    if (entries.length) sessionStorage.setItem(storageKey(id), JSON.stringify(entries));
    else sessionStorage.removeItem(storageKey(id));
  } catch {
    // Private windows may refuse storage; the conversation still works until a reload.
  }
}

export function AppChatPage({ id, slug }: { id: string; slug?: string }) {
  const router = useRouter();
  const page = useQuery({ queryKey: appPageKey(id), queryFn: () => getAppPage(id), retry: false });
  const app = page.data;

  // The name in the address is only for people: a missing or old one is corrected.
  useEffect(() => {
    if (app && slug !== app.slug) router.replace(chatPagePath(app));
  }, [app, slug, router]);
  useEffect(() => {
    if (app) document.title = app.name;
  }, [app]);

  return (
    <main id="main" className="flex h-dvh flex-col">
      <header className="flex shrink-0 items-center gap-3 border-b px-4 py-3 sm:px-6">
        <Brand />
        <span className="ml-auto" />
        {app?.projectId && (
          <Button asChild variant="outline" size="sm">
            <Link href={`/projects/${app.projectId}/app`}>
              <Pencil /> Edit app
            </Link>
          </Button>
        )}
        <ThemeToggle />
      </header>
      {page.isPending ? (
        <Notice title="Opening the app…" />
      ) : page.error ? (
        <Notice
          title={(page.error as ApiError).status === 404 ? "This app isn’t available" : (page.error as ApiError).status === 403 ? "Apps are for administrators for now" : "Couldn’t open the app"}
          text={(page.error as ApiError).status === 404 ? "It may not be published, or the address may be wrong. Ask the person who shared it." : (page.error as Error).message}
        />
      ) : (
        <Conversation key={app!.id} app={app!} />
      )}
    </main>
  );
}

function Notice({ title, text }: { title: string; text?: string }) {
  return (
    <div role="status" className="flex flex-1 flex-col items-center justify-center gap-2 px-4 text-center">
      <p className="text-lg font-medium">{title}</p>
      {text && <p className="max-w-md text-sm text-muted-foreground">{text}</p>}
    </div>
  );
}

function Conversation({ app }: { app: AppPage }) {
  const user = useUser();
  const [entries, setEntries] = useState<Entry[]>([]);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  const end = useRef<HTMLDivElement>(null);
  const box = useRef<HTMLTextAreaElement>(null);
  const blocked = !app.message ? "This app has no text input, so it can’t take a chat message." : app.missing.length ? `This app also needs ${app.missing.join(", ")}, which a chat can’t give it. Call it through its API instead.` : "";

  useEffect(() => setEntries(loadEntries(app.id)), [app.id]);
  useEffect(() => {
    end.current?.scrollIntoView?.({ block: "end" });
  }, [entries, busy]);
  useEffect(() => {
    if (!busy) return;
    const started = Date.now();
    setElapsed(0);
    const timer = setInterval(() => setElapsed(Math.floor((Date.now() - started) / 1000)), 1000);
    return () => clearInterval(timer);
  }, [busy]);

  function update(next: Entry[]) {
    setEntries(next);
    saveEntries(app.id, next);
  }

  async function send(message: string, before: Entry[]) {
    // Earlier turns that worked; a failed reply is left out along with its question.
    const history: ChatTurn[] = [];
    for (let i = 0; i < before.length; i++) {
      const next = before[i + 1];
      if (before[i].role === "user" && next?.role === "assistant" && !next.error) history.push({ role: "user", content: before[i].content }, { role: "assistant", content: next.content });
    }
    const asked: Entry[] = [...before, { role: "user", content: message }];
    update(asked);
    setBusy(true);
    try {
      const reply = await chatWithApp(app.id, message, app.history ? history : []);
      update([...asked, reply.status === "completed" ? { role: "assistant", content: replyText(reply.output) } : { role: "assistant", content: "", error: reply.error || "The app didn’t finish." }]);
    } catch (e) {
      update([...asked, { role: "assistant", content: "", error: (e as Error).message }]);
    } finally {
      setBusy(false);
      box.current?.focus();
    }
  }

  function submit() {
    const message = draft.trim();
    if (!message || busy || blocked) return;
    setDraft("");
    void send(message, entries);
  }

  function retry() {
    const last = entries.at(-2);
    if (!last || last.role !== "user" || busy) return;
    void send(last.content, entries.slice(0, -2));
  }

  return (
    <>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[820px] px-4 py-8 sm:px-6">
          <div className="mb-8">
            <h1 className="text-2xl font-semibold tracking-tight">{app.name}</h1>
            {app.description && <p className="mt-2 text-md text-muted-foreground">{app.description}</p>}
          </div>
          {entries.length === 0 && !blocked && <p className="text-sm text-muted-foreground">Ask something to start. Each message runs the app.</p>}
          {entries.map((e, i) => (
            <article key={i} className="mb-8" data-role={e.role}>
              <div className="mb-2 flex items-center gap-2 text-md font-semibold">
                {e.role === "user" ? (
                  <>
                    <span aria-hidden className="inline-flex size-6 items-center justify-center rounded-full bg-secondary text-2xs font-semibold">
                      {initials(user.display_name || user.username)}
                    </span>
                    You
                  </>
                ) : (
                  app.name
                )}
              </div>
              {e.role === "user" ? (
                <div className="w-fit max-w-full rounded-lg border bg-card px-3.5 py-2.5 text-base leading-[1.75] whitespace-pre-wrap [overflow-wrap:anywhere]">{e.content}</div>
              ) : e.error ? (
                <div role="alert" className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive-foreground">
                  <AlertCircle className="mt-0.5 size-4 shrink-0" />
                  <span className="grow">{e.error}</span>
                  {i === entries.length - 1 && (
                    <Button size="xs" variant="outline" onClick={retry} disabled={busy}>
                      <RotateCcw /> Try again
                    </Button>
                  )}
                </div>
              ) : (
                <Markdown text={e.content} className="text-base leading-[1.75] text-text-2" />
              )}
            </article>
          ))}
          {busy && (
            <p role="status" className="mb-8 flex items-center gap-2 text-sm text-muted-foreground">
              <span aria-hidden className="size-4 animate-spin rounded-full border-2 border-border border-t-foreground" />
              Working{elapsed >= 3 ? ` · ${elapsed}s` : "…"}
            </p>
          )}
          <div ref={end} />
        </div>
      </div>
      <div className="shrink-0 px-4 pb-4 sm:px-6">
        <div className="mx-auto w-full max-w-[820px]">
          {blocked ? (
            <p role="note" className="rounded-lg border border-dashed border-line-strong px-3 py-2 text-sm text-text-2">
              {blocked}
            </p>
          ) : (
            <form
              className="rounded-xl border border-line-strong bg-card shadow-[0_8px_32px_var(--shadow-soft)] focus-within:border-focus-border focus-within:shadow-[0_0_0_1px_var(--focus-border),0_8px_32px_var(--shadow-soft)]"
              onSubmit={(e) => {
                e.preventDefault();
                submit();
              }}
            >
              <label htmlFor="app-chat-message" className="sr-only">
                Message {app.name}
              </label>
              <textarea
                id="app-chat-message"
                ref={box}
                rows={2}
                autoFocus
                value={draft}
                maxLength={20000}
                placeholder={`Message ${app.name}`}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                    e.preventDefault();
                    submit();
                  }
                }}
                className="max-h-[220px] min-h-[68px] w-full resize-none bg-transparent px-4 pt-3.5 pb-1 text-base leading-[1.65] outline-none placeholder:text-muted-foreground"
              />
              <div className="flex items-center gap-1.5 pt-1.5 pr-2.5 pb-2.5 pl-3">
                <Button type="button" variant="ghost" size="sm" className="text-muted-foreground" disabled={busy || !entries.length} onClick={() => update([])}>
                  <SquarePen /> New chat
                </Button>
                <span className="ml-auto" />
                <Button type="submit" size="icon" className="rounded-full" disabled={busy || !draft.trim()} aria-label="Send">
                  <ArrowUp />
                </Button>
              </div>
            </form>
          )}
          <p className="mt-2 text-center text-xs leading-normal text-faint">
            {app.history ? "The app sees this conversation." : "The app sees each message on its own."} It runs with your access. Version {app.version}.
          </p>
        </div>
      </div>
    </>
  );
}
