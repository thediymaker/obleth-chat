"use client";

// The Git tab of a chat's workspace. Each project chat's workspace is a git
// repository; Sync saves its changes as a commit, brings in what others synced and
// shares the result as the project's main line. Rendered when the side pane shows
// the "code" tab, and on the workspace's Compute page.

import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, Check, ChevronRight, Clock, Download, Folder, GitBranch, Info, RefreshCw, Sparkles, Terminal } from "lucide-react";
import Link from "next/link";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { useChat, useChatStore } from "@/components/chat/chat-provider";
import { Eyebrow } from "@/components/app-shell/page";
import { useConfirm } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { PaneLoading, PaneNote } from "@/components/workspace/pane-note";
import { api } from "@/lib/api";
import { useFeature } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { DiffStat, DiffView } from "./diff-view";
import { codeEvents } from "./events";
import { ExportDialog } from "./export-dialog";
import { MoveToProjectDialog } from "@/components/projects/chat-project";
import { type ChatCode, type ConflictRegion, LETTER, ago, chatCodePath, codeHref, conflictRegions, count, resolvePrompt, short, syncSays } from "./model";

export const chatCodeKey = (id: string) => ["chat-code", id] as const;

/** The chat's Git state; files the model changes refresh it a moment later. */
export function useChatCode(id: string) {
  const on = useFeature("code");
  const query = useQuery({ queryKey: chatCodeKey(id), queryFn: () => api.request<ChatCode>(chatCodePath(id)), enabled: on && !!id, staleTime: 15_000 });
  const { refetch, dataUpdatedAt } = query;
  const project = !!query.data?.project;
  const updated = useRef(dataUpdatedAt);
  updated.current = dataUpdatedAt;
  useEffect(() => {
    let soon: ReturnType<typeof setTimeout> | undefined;
    const off = codeEvents.subscribe((e) => {
      if (e.id !== id) return;
      if (e.kind === "changed") {
        if (!project) return;
        clearTimeout(soon);
        soon = setTimeout(() => void refetch(), 2500);
      } else if (Date.now() - updated.current > e.age) void refetch();
    });
    return () => {
      off();
      clearTimeout(soon);
    };
  }, [id, project, refetch]);
  return query;
}

/** The Git tab's label, with how many files changed (amber while a sync is paused). */
export function CodeTabLabel({ conversationId }: { conversationId: string }) {
  const d = useChatCode(conversationId).data;
  const n = d?.merging ? d.conflicts?.length : d?.changes?.length;
  return (
    <>
      Git
      {!!n && <span className={cn("ml-1 rounded-full bg-secondary px-1.5 text-2xs leading-4 font-medium", d?.merging && "bg-amber/15 text-amber")}>{n}</span>}
    </>
  );
}

function Pad({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={cn("px-4 py-3", className)}>{children}</div>;
}

export function CodePanel({ conversationId: id }: { conversationId: string }) {
  const store = useChat();
  const handle = useChatStore();
  const query = useChatCode(id);
  const d = query.data;
  const [busy, setBusy] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [view, setView] = useState<{ path: string; diff: string | null; truncated?: boolean; error?: string } | null>(null);
  const [regions, setRegions] = useState<Record<string, ConflictRegion[] | null>>({});
  const [exporting, setExporting] = useState(false);
  const [moving, setMoving] = useState(false);
  const confirm = useConfirm();
  const conflicts = d?.merging ? d.conflicts || [] : [];
  const conflictKey = conflicts.join("\n");

  // The two sides of each conflicted file, read from the workspace.
  useEffect(() => {
    for (const path of conflictKey ? conflictKey.split("\n") : []) {
      if (regions[path] !== undefined) continue;
      setRegions((r) => ({ ...r, [path]: null }));
      api
        .request<{ content: string }>(`/api/conversations/${encodeURIComponent(id)}/workspace/file?path=${encodeURIComponent(path)}`)
        .then((f) => conflictRegions(f.content))
        .catch(() => [])
        .then((list) => setRegions((r) => ({ ...r, [path]: list })));
    }
  }, [conflictKey, id, regions]);

  async function run<T>(name: string, request: () => Promise<T>, done?: (result: T) => void) {
    if (busy) return;
    setBusy(name);
    try {
      const result = await request();
      setBusy("");
      done?.(result);
    } catch (e) {
      toast((e as Error).message);
    } finally {
      setBusy("");
      await query.refetch();
    }
  }
  const post = <T,>(action: string, body?: unknown) => api.request<T>(`${chatCodePath(id)}/${action}`, { method: "POST", body });
  const sentMessage = () => message ?? undefined;
  const sync = () =>
    run(
      "sync",
      () => post<{ state?: string; shared?: boolean }>("sync", { message: sentMessage() }),
      (r) => {
        setMessage(null);
        setView(null);
        if (r.state === "conflict") {
          setRegions({});
          toast("Sync paused: some files changed in both places");
        } else toast(r.shared ? "Synced. Everyone in the project has your changes." : "Up to date with the project");
      },
    );
  const save = () =>
    run(
      "save",
      () => post<{ saved?: boolean }>("save", { message: sentMessage() }),
      (r) => {
        setMessage(null);
        toast(r.saved ? "Saved in this chat. Sync shares it." : "Nothing to save");
      },
    );
  const undo = (path: string | null) =>
    confirm.ask(
      path
        ? {
            title: "Undo the changes to this file?",
            text: `${path} goes back to how it was at the last save. A new file is removed.`,
            label: "Undo changes",
            cancel: "Keep it",
            action: () =>
              run("undo", () => post("undo", { path }), () => {
                setView(null);
                setMessage(null);
                toast("Changes undone");
              }),
          }
        : {
            title: "Undo every change?",
            text: "Every file goes back to how it was at the last save, and new files are removed. Installed packages and build output stay.",
            label: "Undo all",
            cancel: "Keep them",
            action: () =>
              run("undo", () => post("undo", { path: null }), () => {
                setView(null);
                setMessage(null);
                toast("All changes undone");
              }),
          },
    );
  async function openDiff(path: string) {
    setView({ path, diff: null });
    try {
      const r = await api.request<{ diff: string; truncated?: boolean }>(`${chatCodePath(id)}/diff?path=${encodeURIComponent(path)}`);
      setView((v) => (v?.path === path ? { path, diff: r.diff, truncated: r.truncated } : v));
    } catch (e) {
      setView((v) => (v?.path === path ? { path, diff: "", error: (e as Error).message } : v));
    }
  }
  const terminal = () => store.workspace.runInTerminal(id, "git status");
  const reload = () => {
    setRegions({});
    void query.refetch();
  };

  if (!d)
    return query.error ? (
      <PaneNote
        icon={Info}
        title="Git could not load"
        action={
          <Button size="sm" variant="outline" onClick={reload}>
            <RefreshCw /> Retry
          </Button>
        }
      >
        {(query.error as Error).message}
      </PaneNote>
    ) : (
      <PaneLoading />
    );
  if (!d.project)
    return (
      <>
      <PaneNote
        icon={GitBranch}
        title="Git works in project chats"
        action={
          <Button size="sm" variant="outline" onClick={() => setMoving(true)}>
            <Folder /> Move to project
          </Button>
        }
      >
        Move this chat into a project. Sync then shares its code with the project’s other chats and brings theirs in, with every change kept in the project’s history.
      </PaneNote>
      <MoveToProjectDialog
        open={moving}
        onOpenChange={(open) => {
          setMoving(open);
          if (!open) void query.refetch();
        }}
        conversationId={id}
      />
      </>
    );
  if (!d.running)
    return (
      <PaneNote
        icon={Clock}
        title={d.workspace ? "The workspace is asleep" : "No workspace yet"}
        action={
          d.workspace && (
            <Button size="sm" disabled={!!busy} onClick={sync}>
              <RefreshCw className={cn(busy === "sync" && "animate-spin")} /> {busy === "sync" ? "Syncing…" : "Sync"}
            </Button>
          )
        }
      >
        {d.workspace ? "Its files are kept. Sync wakes it, saves its changes and brings in the project’s latest code." : "Turn on Workspace and ask for something to build. Its code can then be synced with the project."}
      </PaneNote>
    );

  const head = (
    <div className="flex h-10 shrink-0 items-center gap-2 border-b px-3 text-sm">
      <GitBranch className="size-3.5 shrink-0 text-muted-foreground" />
      <Link href={codeHref(d.project.id)} className="min-w-0 flex-1 truncate font-medium hover:underline">
        {d.project.name}
      </Link>
      <span className="shrink-0 text-xs text-faint">main{d.main ? ` · ${count(d.main.commits, "commit")}` : ""}</span>
      <Button variant="ghost" size="icon-xs" aria-label="Check again" title="Check again" disabled={query.isFetching} onClick={reload}>
        <RefreshCw className={cn(query.isFetching && "animate-spin")} />
      </Button>
    </div>
  );

  let body: React.ReactNode;
  if (d.merging) {
    const combined = (d.changes || []).filter((f) => f.status !== "conflict");
    body = (
      <div className="grid gap-3 py-3">
        <div className="mx-4 rounded-md border border-amber/40 bg-amber/10 p-3 text-sm">
          <strong className="flex items-center gap-1.5 font-medium text-amber">
            <Info className="size-4" /> Sync paused
          </strong>
          <p className="mt-1">
            Your changes are saved in this workspace. Someone synced changes to the same lines, so {conflicts.length ? count(conflicts.length, "file needs", "files need") : "nothing more needs"} a decision before they can be combined.
          </p>
          <p className="mt-1 text-muted-foreground">Nothing is lost whichever way you go. Stop syncing puts every file back the way it was before Sync.</p>
        </div>
        <div className="grid gap-2 px-4">
          <div className="flex flex-wrap gap-1.5">
            {!!conflicts.length && (
              <Button size="sm" disabled={!!busy} onClick={() => handle.send(resolvePrompt(conflicts))}>
                <Sparkles /> Resolve with the model
              </Button>
            )}
            <Button
              size="sm"
              variant={conflicts.length ? "outline" : "default"}
              disabled={!!busy}
              onClick={() =>
                run(
                  "finish",
                  () => post<{ state?: string }>("finish"),
                  (r) => {
                    if (r.state === "conflict") {
                      setRegions({});
                      toast("More changes came in that need a decision");
                    } else toast("Synced. Everyone in the project has the combined code.");
                  },
                )
              }
            >
              <Check /> {busy === "finish" ? "Finishing…" : "Finish syncing"}
            </Button>
            <Button size="sm" variant="ghost" onClick={terminal}>
              <Terminal /> Open in terminal
            </Button>
            <Button
              size="sm"
              variant="ghost"
              disabled={!!busy}
              onClick={() =>
                confirm.ask({
                  title: "Stop syncing?",
                  text: "Every file goes back to how it was before Sync, with your changes saved. Nothing is shared, and the next Sync tries again.",
                  label: "Stop syncing",
                  cancel: "Keep going",
                  action: () =>
                    run("stop", () => post("stop"), () => {
                      setRegions({});
                      toast("Sync stopped. Your changes are as they were.");
                    }),
                })
              }
            >
              Stop syncing
            </Button>
          </div>
          <small className="text-xs text-muted-foreground">
            {conflicts.length ? "Finish once each file is combined: by the model, by hand, or with Keep mine or Keep the project’s." : "Every file is settled. Finish syncing to share the result."}
          </small>
        </div>
        {conflicts.map((path) => {
          const list = regions[path];
          return (
            <div key={path} className="mx-4 grid gap-2 rounded-md border p-3">
              <div className="flex flex-wrap items-center gap-2">
                <code className="min-w-0 flex-1 truncate font-mono text-xs">{path}</code>
                {(["ours", "theirs"] as const).map((side) => (
                  <Button
                    key={side}
                    size="xs"
                    variant="outline"
                    disabled={!!busy}
                    onClick={() =>
                      run("choose", () => post("choose", { path, side }), () =>
                        setRegions((r) => {
                          const next = { ...r };
                          delete next[path];
                          return next;
                        }),
                      )
                    }
                  >
                    {side === "ours" ? "Keep mine" : "Keep the project’s"}
                  </Button>
                ))}
              </div>
              {list == null ? (
                <PaneLoading />
              ) : list.length ? (
                list.map((x, i) => (
                  <div key={i} className="grid gap-2 sm:grid-cols-2">
                    {[
                      [`Yours · line ${x.line}`, x.mine],
                      ["The project’s", x.theirs],
                    ].map(([label, lines]) => (
                      <div key={label as string} className="min-w-0">
                        <Eyebrow>{label as string}</Eyebrow>
                        <pre className="mt-1 max-h-48 overflow-auto rounded bg-code p-2 font-mono text-[12px]">{(lines as string[]).join("\n") || <span className="text-faint">(nothing)</span>}</pre>
                      </div>
                    ))}
                  </div>
                ))
              ) : (
                <p className="text-sm text-muted-foreground">Changed or removed on both sides.</p>
              )}
            </div>
          );
        })}
        {!!combined.length && (
          <p className="flex items-start gap-1.5 px-4 text-sm text-muted-foreground">
            <Check className="mt-0.5 size-3.5 shrink-0" />
            Combined on their own: {combined.map((f) => f.path).join(", ")}
          </p>
        )}
      </div>
    );
  } else if (view) {
    const f = (d.changes || []).find((x) => x.path === view.path);
    body = (
      <div className="flex min-h-0 flex-col">
        <div className="flex h-10 shrink-0 items-center gap-2 border-b px-2">
          <Button variant="ghost" size="icon-xs" aria-label="Back to changes" title="Back to changes" onClick={() => setView(null)}>
            <ArrowLeft />
          </Button>
          <code className="min-w-0 flex-1 truncate font-mono text-xs">{view.path}</code>
          {f && <DiffStat added={f.added} removed={f.removed} />}
          <Button size="xs" variant="ghost" disabled={!!busy} onClick={() => undo(view.path)}>
            Undo this file
          </Button>
        </div>
        {view.diff == null ? (
          <PaneLoading />
        ) : view.error ? (
          <Pad className="text-sm text-muted-foreground">{view.error}</Pad>
        ) : (
          <>
            <DiffView text={view.diff} />
            {view.truncated && <Pad className="text-sm text-muted-foreground">Showing the first 200 KB.</Pad>}
          </>
        )}
      </div>
    );
  } else {
    const changes = d.changes || [];
    const text = message ?? (changes.length ? d.draft || "" : "");
    const log = (d.log || []).slice(0, 8);
    const ahead = d.ahead || 0;
    const say = syncSays(d, changes.length);
    body = (
      <div className="grid gap-1">
        {d.behind ? (
          <p className="mx-4 mt-3 flex items-center gap-2 rounded-md border border-blue/30 bg-blue/10 px-3 py-2 text-sm">
            <Download className="size-4 shrink-0 text-blue" />
            {count(d.behind, "new commit")} in the project since this chat last synced.
          </p>
        ) : !d.main ? (
          <p className="mx-4 mt-3 flex items-center gap-2 rounded-md border px-3 py-2 text-sm text-muted-foreground">
            <Info className="size-4 shrink-0" />
            Nobody has synced yet. The first Sync starts the project’s code.
          </p>
        ) : null}
        <section className="grid gap-1 py-2">
          <div className="flex items-center justify-between px-4">
            <Eyebrow>Changes{changes.length ? ` · ${count(changes.length, "file")}` : ""}</Eyebrow>
            {!!changes.length && (
              <Button variant="link" size="xs" className="h-auto px-0 text-muted-foreground" disabled={!!busy} onClick={() => undo(null)}>
                Undo all
              </Button>
            )}
          </div>
          {changes.length ? (
            <ul>
              {changes.map((f) => (
                <li key={f.path}>
                  <button type="button" className="flex w-full items-center gap-2 px-4 py-1 text-left text-md hover:bg-accent" onClick={() => openDiff(f.path)}>
                    <span
                      title={f.status}
                      className={cn(
                        "w-4 shrink-0 text-center font-mono text-xs font-semibold",
                        f.status === "added" && "text-success",
                        f.status === "deleted" && "text-destructive-foreground",
                        f.status === "conflict" && "text-amber",
                        (!f.status || f.status === "modified") && "text-blue",
                      )}
                    >
                      {LETTER[f.status || ""] || "M"}
                    </span>
                    <span className="min-w-0 flex-1 truncate font-mono text-xs">{f.path}</span>
                    <DiffStat added={f.added} removed={f.removed} />
                  </button>
                </li>
              ))}
            </ul>
          ) : (
            <p className="px-4 text-sm text-muted-foreground">No changes since the last save{ahead ? `. ${count(ahead, "saved commit")} not synced yet.` : "."}</p>
          )}
        </section>
        {changes.length || ahead || d.behind || !d.main ? (
          <section className="grid gap-2 border-t px-4 py-3">
            {!!changes.length && (
              <>
                <label htmlFor={`code-message-${id}`} className="text-sm font-medium">
                  What changed
                </label>
                <Textarea id={`code-message-${id}`} rows={2} maxLength={2000} value={text} placeholder={d.draft} onChange={(e) => setMessage(e.target.value)} className="min-h-14 text-md" />
                <small className="text-xs text-muted-foreground">{message == null ? "Drafted from the changes. Edit it freely." : "Kept with the commit, along with a link to this chat."}</small>
              </>
            )}
            <div className="flex flex-wrap gap-1.5">
              <Button size="sm" disabled={!!busy} onClick={sync}>
                <RefreshCw className={cn(busy === "sync" && "animate-spin")} /> {busy === "sync" ? "Syncing…" : "Sync"}
              </Button>
              {!!changes.length && (
                <Button size="sm" variant="ghost" disabled={!!busy} onClick={save}>
                  {busy === "save" ? "Saving…" : "Save without syncing"}
                </Button>
              )}
            </div>
            <small className="text-xs text-muted-foreground">{say}</small>
          </section>
        ) : (
          <p className="px-4 py-2 text-sm text-muted-foreground">{say}</p>
        )}
        {!!log.length && (
          <section className="grid gap-1 border-t py-3">
            <div className="flex items-center justify-between px-4">
              <Eyebrow>History</Eyebrow>
              <Link href={codeHref(d.project.id, "history")} className="flex items-center text-sm text-muted-foreground hover:text-foreground">
                All <ChevronRight className="size-3.5" />
              </Link>
            </div>
            <ol>
              {log.map((e, i) => (
                <li key={e.sha} className="flex items-center gap-2.5 px-4 py-1">
                  <span className={cn("size-2 shrink-0 rounded-full border-2", i < ahead ? "border-amber" : "border-muted-foreground")} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-md">{e.subject}</span>
                    <small className="block text-xs text-muted-foreground">
                      {e.author} · {ago(e.time)}
                      {i < ahead ? " · not synced" : ""}
                    </small>
                  </span>
                  {i === 0 && ahead > 0 && (
                    <Button
                      variant="link"
                      size="xs"
                      className="h-auto px-0 text-muted-foreground"
                      title="Make this save’s changes unsaved again"
                      disabled={!!busy}
                      onClick={() =>
                        run("uncommit", () => post("uncommit"), () => {
                          setMessage(null);
                          toast("Save undone. Its changes are back under Changes.");
                        })
                      }
                    >
                      Undo
                    </Button>
                  )}
                  <code className="shrink-0 font-mono text-xs text-faint">{short(e.sha)}</code>
                </li>
              ))}
            </ol>
          </section>
        )}
        <div className="flex flex-wrap items-center justify-between gap-2 border-t px-4 py-3">
          <span className="text-xs text-faint">Kept by Chat while the project runs</span>
          <span className="flex gap-1">
            <Button size="xs" variant="ghost" onClick={() => setExporting(true)}>
              Export…
            </Button>
            <Button size="xs" variant="ghost" onClick={terminal}>
              <Terminal /> Open in terminal
            </Button>
          </span>
        </div>
        <ExportDialog projectId={d.project.id} name={d.project.name} open={exporting} onOpenChange={setExporting} />
      </div>
    );
  }
  return (
    <div className="flex h-full min-h-0 flex-col" data-conversation={id}>
      {head}
      <div className="min-h-0 flex-1 overflow-auto">{body}</div>
      {confirm.element}
    </div>
  );
}
