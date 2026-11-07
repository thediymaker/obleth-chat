"use client";

// A project's Code tab: the
// overview (latest commits, chats working on the code, exports, size), the history
// with each commit's changes and the chat it came from, the files with who changed
// each line, and comparing two versions. The code goes out as a zip or to GitHub,
// and an empty project can start from a repository.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeftRight, ChevronDown, ChevronRight, Download, File as FileIcon, GitBranch, GitCompare, Plus, Undo2, Upload, X } from "lucide-react";
import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { ErrorBox, Eyebrow } from "@/components/app-shell/page";
import { Dropdown, Pill, Quota, useConfirm } from "@/components/manage/kit";
import { useNewChatInProject, useProjectFrame } from "@/components/projects/project-frame";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { ApiError, api } from "@/lib/api";
import { useFeature } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { DiffStat, DiffView } from "./diff-view";
import { ExportDialog, ImportDialog, codeKeys } from "./export-dialog";
import {
  type BlameGroup,
  type ChangedFile,
  type ChatRef,
  type CodeOverview,
  type CodeRoute,
  type CommitDetail,
  type CommitEntry,
  type CompareResult,
  type LineVersion,
  ago,
  bytes,
  codeHref,
  codeRoute,
  count,
  initial,
  projectCodePath,
  short,
  undoInChatPrompt,
} from "./model";
import { useStartInProject } from "./start-chat";

const card = "rounded-lg border bg-card p-4";
const enc = encodeURIComponent;

function useCode<T>(projectId: string, part: readonly unknown[], url: string, enabled = true) {
  return useQuery({ queryKey: [...codeKeys.all(projectId), ...part], queryFn: () => api.request<T>(url), enabled });
}

/** A chat a commit or line came from: a link when the viewer can read it. */
function ChatLink({ chat }: { chat?: ChatRef | null }) {
  if (!chat) return null;
  return chat.readable ? (
    <Link href={`/chat/${enc(chat.id)}`} className="underline-offset-2 hover:underline">
      {chat.title || "Untitled chat"}
    </Link>
  ) : (
    <span>{chat.ownerName}’s chat</span>
  );
}

function Avatar({ name }: { name?: string | null }) {
  return <span className="grid size-6 shrink-0 place-items-center rounded-full bg-secondary text-2xs font-medium">{initial(name)}</span>;
}

function CommitRow({ projectId, e, selected = "" }: { projectId: string; e: CommitEntry; selected?: string }) {
  const merge = (e.parents?.length || 0) > 1;
  return (
    <li>
      <Link
        href={codeHref(projectId, { view: "history", sha: e.sha })}
        aria-current={selected === e.sha ? "page" : undefined}
        className={cn("flex items-center gap-2.5 rounded-md px-2 py-1.5 hover:bg-accent", selected === e.sha && "bg-secondary")}
      >
        <span className={cn("size-2 shrink-0 rounded-full border-2 border-muted-foreground", merge && "border-blue bg-blue/30")} />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-md">{e.subject}</span>
          <small className="block truncate text-xs text-muted-foreground">
            {e.author} · {ago(e.time)}
            {e.chat ? ` · ${e.chat.readable ? e.chat.title || "chat" : `${e.chat.ownerName}’s chat`}` : ""}
          </small>
        </span>
        <code className="shrink-0 font-mono text-xs text-faint">{short(e.sha)}</code>
      </Link>
    </li>
  );
}

/** A version's changed files, each opening to its diff; three or fewer open at once. */
function FileList({ files, patchUrl }: { files: ChangedFile[]; patchUrl: (path: string) => string }) {
  const [open, setOpen] = useState<Record<string, string | null>>({});
  async function toggle(path: string) {
    if (open[path] !== undefined) {
      setOpen((o) => {
        const next = { ...o };
        delete next[path];
        return next;
      });
      return;
    }
    setOpen((o) => ({ ...o, [path]: null }));
    const r = await api.request<{ patch?: string }>(patchUrl(path)).catch(() => null);
    setOpen((o) => (path in o ? { ...o, [path]: r?.patch ?? "" } : o));
  }
  const key = files.map((f) => f.path).join("\n");
  useEffect(() => {
    if (files.length && files.length <= 3) files.forEach((f) => void toggle(f.path));
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once per version
  }, [key]);
  return (
    <ul className="grid gap-1">
      {files.map((f) => {
        const patch = open[f.path];
        return (
          <li key={f.path} className="overflow-hidden rounded-md border">
            <button type="button" aria-expanded={patch !== undefined} className="flex w-full items-center gap-2 px-2 py-1.5 text-left hover:bg-accent" onClick={() => toggle(f.path)}>
              {patch !== undefined ? <ChevronDown className="size-3.5 shrink-0" /> : <ChevronRight className="size-3.5 shrink-0" />}
              <span className="min-w-0 flex-1 truncate font-mono text-xs">{f.path}</span>
              <DiffStat added={f.added} removed={f.removed} />
              {f.added == null && <small className="text-xs text-faint">binary</small>}
            </button>
            {patch === undefined ? null : patch === null ? <p className="border-t px-3 py-2 text-sm text-muted-foreground">Loading…</p> : <DiffView text={patch} className="border-t" />}
          </li>
        );
      })}
    </ul>
  );
}

function Overview({ projectId, o, log, onExport }: { projectId: string; o: CodeOverview; log?: CommitEntry[]; onExport: () => void }) {
  const main = o.main!;
  const latest = log?.[0];
  const used = main.bytes / (o.limits.bytes || 1);
  return (
    <div className="grid grid-cols-[minmax(0,1fr)] gap-4 lg:grid-cols-[minmax(0,1fr)_20rem]">
      <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] content-start gap-4">
        <section className={card}>
          <div className="flex items-center justify-between">
            <Eyebrow>Latest</Eyebrow>
            <Link href={codeHref(projectId, "history")} className="flex items-center text-sm text-muted-foreground hover:text-foreground">
              History <ChevronRight className="size-3.5" />
            </Link>
          </div>
          <p className="mt-2 text-md">
            {count(main.commits, "commit")}
            {latest && (
              <>
                {" · latest: "}
                <Link href={codeHref(projectId, { view: "history", sha: latest.sha })} className="underline-offset-2 hover:underline">
                  {latest.subject}
                </Link>
              </>
            )}{" "}
            · {main.updatedBy || "someone"}, {ago(main.updatedAt)}
          </p>
          <ol className="mt-2 grid">
            {(log || []).slice(0, 6).map((e) => (
              <CommitRow key={e.sha} projectId={projectId} e={e} />
            ))}
          </ol>
          {o.origin && (
            <p className="mt-2 text-sm text-muted-foreground">
              Started from{" "}
              <a href={o.origin.url} target="_blank" rel="noopener" className="underline underline-offset-2">
                {o.origin.url.replace(/^https:\/\//, "")}
              </a>
              {o.origin.sha && (
                <>
                  {" "}
                  at <code className="font-mono">{short(o.origin.sha)}</code>
                </>
              )}
              {o.origin.history === false ? ", its latest version without the history" : ""}.
            </p>
          )}
        </section>
        <section className={card}>
          <Eyebrow>Chats working on the code</Eyebrow>
          <div className="mt-2 grid gap-1">
            {o.chats.length ? (
              o.chats.map((w) => (
                <div key={w.chat.id} className="flex items-center gap-2.5 py-1">
                  <Avatar name={w.chat.ownerName} />
                  <span className="min-w-0 flex-1">
                    <strong className="block truncate text-md font-medium">
                      {w.chat.readable ? (
                        <Link href={`/chat/${enc(w.chat.id)}`} className="hover:underline">
                          {w.chat.title || "Untitled chat"}
                        </Link>
                      ) : (
                        `${w.chat.ownerName}’s chat`
                      )}
                    </strong>
                    <small className="block text-xs text-muted-foreground">
                      {w.chat.mine ? "You" : w.chat.ownerName} · {w.syncedAt ? `synced ${ago(w.syncedAt)}` : "not synced yet"}
                    </small>
                  </span>
                  {w.behind ? <Pill tone="amber">{count(w.behind, "commit")} behind</Pill> : <Pill>Up to date</Pill>}
                </div>
              ))
            ) : (
              <p className="text-sm text-muted-foreground">No chat has synced yet.</p>
            )}
          </div>
          <p className="mt-2 text-sm text-muted-foreground">Every chat in the project works on its own copy. Sync shares its changes with everyone; the next Sync elsewhere brings them in.</p>
        </section>
      </div>
      <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] content-start gap-4">
        <section className={card}>
          <Eyebrow>Take it with you</Eyebrow>
          <div className="mt-2 grid gap-1.5">
            {[
              [Upload, "Push to GitHub", "With its history"],
              [Download, "Download a zip", "The code, and if you like its history and chats"],
            ].map(([Icon, title, text]) => {
              const I = Icon as typeof Upload;
              return (
                <button key={title as string} type="button" onClick={onExport} className="flex items-center gap-3 rounded-md border px-3 py-2 text-left hover:bg-accent">
                  <I className="size-4 shrink-0 text-muted-foreground" />
                  <span className="min-w-0">
                    <strong className="block text-md font-medium">{title as string}</strong>
                    <small className="block text-xs text-muted-foreground">{text as string}</small>
                  </span>
                </button>
              );
            })}
            {o.exports.map((e) => (
              <div key={`${e.kind}:${e.target}`} className="flex items-start gap-2 py-1 text-md">
                <Upload className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
                <span className="min-w-0">
                  Pushed to{" "}
                  <a href={e.url} target="_blank" rel="noopener" className="underline underline-offset-2">
                    {e.target}
                  </a>
                  <small className="block text-xs text-muted-foreground">
                    {ago(e.at)} · {e.behind ? `${count(e.behind, "commit")} since` : "up to date"}
                  </small>
                </span>
              </div>
            ))}
          </div>
          <p className="mt-2 text-sm text-muted-foreground">To put a site online, push it to GitHub, then import the repository in Vercel or Netlify, or turn on GitHub Pages.</p>
        </section>
        <section className={card}>
          <Eyebrow>Size</Eyebrow>
          <Quota value={Math.min(used, 1)} max={1} label="Project code size" className="mt-2" />
          <p className="mt-2 text-sm text-muted-foreground">
            {bytes(main.bytes)} of {Math.round(o.limits.bytes / 1048576)} MB. Data sets and build output stay out of the code.
          </p>
        </section>
      </div>
    </div>
  );
}

function Split({ list, detail }: { list: React.ReactNode; detail: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[minmax(0,1fr)] gap-4 lg:grid-cols-[minmax(0,22rem)_minmax(0,1fr)]">
      <section className={cn(card, "max-h-[70vh] overflow-auto p-2")}>{list}</section>
      <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] content-start gap-4">{detail}</div>
    </div>
  );
}

function CommitView({ projectId, sha, overview, onUndo }: { projectId: string; sha: string; overview?: CodeOverview | null; onUndo: (c: CommitEntry) => void }) {
  const workspaces = useFeature("workspaces");
  const startIn = useStartInProject();
  const q = useCode<CommitDetail>(projectId, ["commit", sha], `${projectCodePath(projectId)}/commits/${enc(sha)}`);
  if (!q.data) return <section className={card}>{q.error ? <ErrorBox error={q.error} onRetry={() => q.refetch()} /> : <p className="text-sm text-muted-foreground">Loading…</p>}</section>;
  const { commit: c, body, files } = q.data;
  const latest = overview?.main?.sha === c.sha;
  const parents = c.parents?.length || 0;
  return (
    <section className={cn(card, "grid gap-3")}>
      <div className="flex items-center justify-between gap-2">
        <code className="font-mono text-xs">{short(c.sha)}</code>
        <span className="text-xs text-muted-foreground">{new Date(c.time * 1000).toLocaleString([], { dateStyle: "medium", timeStyle: "short" })}</span>
      </div>
      <div>
        <h2 className="text-lg font-semibold">{c.subject}</h2>
        <p className="text-sm text-muted-foreground">
          {c.author}
          {parents > 1 ? " · combined two lines of work" : ""}
        </p>
      </div>
      {body && <p className="text-md whitespace-pre-wrap">{body}</p>}
      {c.chat && (
        <div>
          <Eyebrow>From the chat</Eyebrow>
          <p className="text-md">
            <ChatLink chat={c.chat} />
            {!c.chat.readable && <span className="text-muted-foreground"> · not shared with you</span>}
          </p>
        </div>
      )}
      {(!latest || workspaces || parents === 1) && (
        <div className="flex flex-wrap gap-1.5">
          {!latest && (
            <Button asChild size="sm" variant="ghost">
              <Link href={codeHref(projectId, { view: "compare", from: c.sha, to: "main" })}>
                <GitCompare /> Compare with now
              </Link>
            </Button>
          )}
          {workspaces && (
            <Button size="sm" variant="ghost" onClick={() => startIn(projectId, { start: { sha: c.sha, subject: c.subject } })}>
              <Plus /> New chat from here
            </Button>
          )}
          {parents === 1 && (
            <Button size="sm" variant="ghost" onClick={() => onUndo(c)}>
              <Undo2 /> Undo this commit
            </Button>
          )}
        </div>
      )}
      <Eyebrow>{count(files.length, "file")}</Eyebrow>
      <FileList key={c.sha} files={files} patchUrl={(path) => `${projectCodePath(projectId)}/commits/${enc(c.sha)}?path=${enc(path)}`} />
    </section>
  );
}

function History({ projectId, sha, log, overview, onUndo }: { projectId: string; sha: string; log?: CommitEntry[]; overview?: CodeOverview | null; onUndo: (c: CommitEntry) => void }) {
  if (!log) return <p className="text-sm text-muted-foreground">Loading history…</p>;
  return (
    <Split
      list={
        <>
          <ol>
            {log.map((e) => (
              <CommitRow key={e.sha} projectId={projectId} e={e} selected={sha} />
            ))}
          </ol>
          {log.length >= 200 && <p className="p-2 text-sm text-muted-foreground">Showing the latest 200 commits.</p>}
        </>
      }
      detail={
        sha ? (
          <CommitView key={sha} projectId={projectId} sha={sha} overview={overview} onUndo={onUndo} />
        ) : (
          <section className={card}>
            <p className="text-sm text-muted-foreground">Pick a commit to see what it changed and the chat it came from.</p>
          </section>
        )
      }
    />
  );
}

function Compare({ projectId, from, to, log }: { projectId: string; from: string; to: string; log?: CommitEntry[] }) {
  const router = useRouter();
  const q = useCode<CompareResult>(projectId, ["compare", from, to], `${projectCodePath(projectId)}/compare?from=${enc(from)}&to=${enc(to)}`, !!from);
  if (!from)
    return (
      <section className={card}>
        <p className="text-sm text-muted-foreground">Pick two versions to compare.</p>
      </section>
    );
  const same = (value: string, e: CommitEntry) => value === e.sha || (value.length >= 4 && e.sha.startsWith(value));
  const choose = (side: "from" | "to", value: string) => router.push(codeHref(projectId, { view: "compare", from: side === "from" ? value : from, to: side === "to" ? value : to }));
  const picker = (side: "from" | "to", value: string) => {
    const selected = value === "main" ? "main" : log?.find((e) => same(value, e))?.sha || value;
    return (
      <label className="grid min-w-0 flex-1 gap-1">
        <Eyebrow>{side === "from" ? "From" : "To"}</Eyebrow>
        <Dropdown aria-label={side === "from" ? "From" : "To"} value={selected} onChange={(e) => choose(side, e.target.value)}>
          <option value="main">Now (main)</option>
          {!log?.some((e) => e.sha === selected) && selected !== "main" && <option value={selected}>{short(selected)}</option>}
          {(log || []).map((e) => (
            <option key={e.sha} value={e.sha}>
              {short(e.sha)} · {e.subject.slice(0, 60)}
            </option>
          ))}
        </Dropdown>
      </label>
    );
  };
  const d = q.data;
  const backwards = d && !d.total && d.files.length;
  const swap = codeHref(projectId, { view: "compare", from: to, to: from });
  return (
    <div className="grid gap-4">
      <section className={cn(card, "grid gap-3")}>
        <div className="flex items-end gap-2">
          {picker("from", from)}
          <Button asChild variant="ghost" size="icon-sm" aria-label="Swap" title="Swap">
            <Link href={swap}>
              <ArrowLeftRight />
            </Link>
          </Button>
          {picker("to", to)}
        </div>
        <ErrorBox error={q.error} onRetry={() => q.refetch()} />
        {!d ? (
          !q.error && <p className="text-sm text-muted-foreground">Comparing…</p>
        ) : (
          <>
            <p className="text-md">
              {d.files.length ? `${count(d.files.length, "file")} changed` : "The same code"}
              {d.total ? ` over ${count(d.total, "commit")}` : ""}, from{" "}
              <Link href={codeHref(projectId, { view: "history", sha: d.from.sha })} className="underline-offset-2 hover:underline">
                {d.from.subject}
              </Link>{" "}
              to{" "}
              <Link href={codeHref(projectId, { view: "history", sha: d.to.sha })} className="underline-offset-2 hover:underline">
                {d.to.subject}
              </Link>
              .
            </p>
            {!!backwards && (
              <p className="text-sm text-muted-foreground">
                “To” is older than “From”, so this shows the changes taken back.{" "}
                <Link href={swap} className="underline underline-offset-2">
                  Swap them
                </Link>
                .
              </p>
            )}
          </>
        )}
      </section>
      {d && (
        <Split
          list={
            d.commits.length ? (
              <>
                <Eyebrow className="px-2">Commits in between</Eyebrow>
                <ol className="mt-1">
                  {d.commits.map((e) => (
                    <CommitRow key={e.sha} projectId={projectId} e={e} />
                  ))}
                </ol>
                {d.total > d.commits.length && <p className="p-2 text-sm text-muted-foreground">Showing the latest {d.commits.length}.</p>}
              </>
            ) : (
              <p className="p-2 text-sm text-muted-foreground">No commits in between.</p>
            )
          }
          detail={
            <section className={card}>
              {d.files.length ? (
                <>
                  <Eyebrow>Files</Eyebrow>
                  <div className="mt-2">
                    <FileList key={`${from}...${to}`} files={d.files} patchUrl={(path) => `${projectCodePath(projectId)}/compare?from=${enc(from)}&to=${enc(to)}&path=${enc(path)}`} />
                  </div>
                </>
              ) : (
                <p className="text-sm text-muted-foreground">No files differ.</p>
              )}
            </section>
          }
        />
      )}
    </div>
  );
}

function Blame({ projectId, path }: { projectId: string; path: string }) {
  const info = useCode<{ path: string; size?: number; binary?: boolean; tooLarge?: boolean }>(projectId, ["file", path], `${projectCodePath(projectId)}/file?path=${enc(path)}`);
  const readable = !!info.data && !info.data.binary && !info.data.tooLarge;
  const blame = useCode<{ groups: BlameGroup[] }>(projectId, ["blame", path], `${projectCodePath(projectId)}/blame?path=${enc(path)}`, readable);
  const [range, setRange] = useState<{ start: number; end: number } | null>(null);
  const versions = useCode<{ versions: LineVersion[] }>(projectId, ["lines", path, range?.start, range?.end], `${projectCodePath(projectId)}/lines?path=${enc(path)}&start=${range?.start}&end=${range?.end}`, !!range);
  const head = (
    <div className="flex items-center justify-between gap-2">
      <code className="min-w-0 flex-1 truncate font-mono text-xs">{path}</code>
      {info.data && <span className="text-xs text-muted-foreground">{bytes(info.data.size)}</span>}
    </div>
  );
  if (!info.data)
    return (
      <section className={cn(card, "grid gap-2")}>
        {head}
        {info.error ? <ErrorBox error={info.error} onRetry={() => info.refetch()} /> : <p className="text-sm text-muted-foreground">Loading…</p>}
      </section>
    );
  if (!readable)
    return (
      <section className={cn(card, "grid gap-2")}>
        {head}
        <p className="text-sm text-muted-foreground">{info.data.binary ? "A binary file." : "Too large to show here."} It is in the zip and on GitHub when you export.</p>
      </section>
    );
  return (
    <>
      {range && (
        <section className={cn(card, "grid gap-3")}>
          <div className="flex items-center justify-between">
            <Eyebrow>
              Lines {range.start}–{range.end}, through time
            </Eyebrow>
            <Button variant="ghost" size="icon-xs" aria-label="Close" title="Close" onClick={() => setRange(null)}>
              <X />
            </Button>
          </div>
          {!versions.data ? (
            <p className="text-sm text-muted-foreground">Loading…</p>
          ) : versions.data.versions.length ? (
            versions.data.versions.map((e) => (
              <div key={e.sha} className="grid gap-1.5">
                <p className="text-md">
                  <Link href={codeHref(projectId, { view: "history", sha: e.sha })} className="underline-offset-2 hover:underline">
                    {e.subject}
                  </Link>
                  <small className="text-xs text-muted-foreground">
                    {" "}
                    · {e.author} · {ago(e.time)}
                    {e.chat && (
                      <>
                        {" · "}
                        <ChatLink chat={e.chat} />
                      </>
                    )}
                  </small>
                </p>
                <DiffView text={e.patch} className="rounded-md border" />
              </div>
            ))
          ) : (
            <p className="text-sm text-muted-foreground">No earlier versions.</p>
          )}
        </section>
      )}
      <section className={cn(card, "grid gap-2")}>
        {head}
        <Eyebrow>Who changed each line</Eyebrow>
        {!blame.data ? (
          blame.error ? (
            <ErrorBox error={blame.error} onRetry={() => blame.refetch()} />
          ) : (
            <p className="text-sm text-muted-foreground">Reading who changed each line…</p>
          )
        ) : (
          <div className="overflow-hidden rounded-md border">
            {blame.data.groups.map((g) => {
              const end = g.start + g.lines.length - 1;
              return (
                <div key={`${g.sha}:${g.start}`} className="border-b last:border-b-0">
                  <div className="flex items-center gap-2 bg-muted/50 px-2 py-1.5">
                    <Avatar name={g.author} />
                    <span className="min-w-0 flex-1">
                      <Link href={codeHref(projectId, { view: "history", sha: g.sha })} className="block truncate text-sm hover:underline">
                        {g.subject}
                      </Link>
                      <small className="block truncate text-xs text-muted-foreground">
                        {g.author} · {ago(g.time)}
                        {g.chat && (
                          <>
                            {" · "}
                            <ChatLink chat={g.chat} />
                          </>
                        )}
                      </small>
                    </span>
                    <Button variant="link" size="xs" className="h-auto px-0 text-muted-foreground" title={`Every version of lines ${g.start}–${end}`} onClick={() => setRange({ start: g.start, end })}>
                      History
                    </Button>
                  </div>
                  <div className="overflow-x-auto bg-code font-mono text-[12px] leading-[1.55]">
                    {g.lines.map((l, i) => (
                      <div key={i} className="flex min-w-max">
                        <span className="w-11 shrink-0 pr-2 text-right text-faint select-none">{g.start + i}</span>
                        <code className="pr-3 whitespace-pre">{l || " "}</code>
                      </div>
                    ))}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </section>
    </>
  );
}

function Files({ projectId, path }: { projectId: string; path: string }) {
  const q = useCode<{ files: { path: string; size?: number }[] }>(projectId, ["files"], `${projectCodePath(projectId)}/files`);
  if (!q.data) return q.error ? <ErrorBox error={q.error} onRetry={() => q.refetch()} /> : <p className="text-sm text-muted-foreground">Loading files…</p>;
  return (
    <Split
      list={
        <ul>
          {q.data.files.map((f) => (
            <li key={f.path}>
              <Link
                href={codeHref(projectId, { view: "files", path: f.path })}
                aria-current={f.path === path ? "page" : undefined}
                className={cn("flex items-center gap-2 rounded-md px-2 py-1 hover:bg-accent", f.path === path && "bg-secondary")}
              >
                <FileIcon className="size-3.5 shrink-0 text-muted-foreground" />
                <span className="min-w-0 flex-1 truncate font-mono text-xs">{f.path}</span>
                <small className="shrink-0 text-xs text-faint">{bytes(f.size)}</small>
              </Link>
            </li>
          ))}
        </ul>
      }
      detail={
        path ? (
          <Blame key={path} projectId={projectId} path={path} />
        ) : (
          <section className={card}>
            <p className="text-sm text-muted-foreground">Pick a file to see who changed each line, and in which chat.</p>
          </section>
        )
      }
    />
  );
}

export function ProjectCode({ projectId }: { projectId: string }) {
  const { detail } = useProjectFrame();
  const p = detail.project;
  const pathname = usePathname();
  const router = useRouter();
  const queries = useQueryClient();
  const confirm = useConfirm();
  const newChat = useNewChatInProject();
  const startIn = useStartInProject();
  const workspaces = useFeature("workspaces");
  const base = `/projects/${enc(projectId)}/code`;
  const rest = pathname.startsWith(base) ? pathname.slice(base.length).split("/").filter(Boolean) : [];
  const route: CodeRoute = codeRoute(rest);
  const overview = useCode<CodeOverview>(projectId, ["overview"], projectCodePath(projectId));
  const log = useCode<{ commits: CommitEntry[] }>(projectId, ["log"], `${projectCodePath(projectId)}/log?limit=200`, route.view !== "files");
  const [exporting, setExporting] = useState(false);
  const [importing, setImporting] = useState(false);
  const [undoInChat, setUndoInChat] = useState<{ commit: CommitEntry; message: string } | null>(null);
  const o = overview.data;

  const undo = (c: CommitEntry) =>
    confirm.ask({
      title: "Undo this commit?",
      text: `A new commit takes back what “${c.subject}” changed. Everything after it stays, and the history keeps both. Each chat brings the undo in with its next Sync.`,
      label: "Undo commit",
      cancel: "Keep it",
      action: async () => {
        try {
          const r = await api.request<{ sha: string }>(`${projectCodePath(projectId)}/commits/${enc(c.sha)}/undo`, { method: "POST" });
          toast("Undone. Chats get it with their next Sync.");
          await queries.invalidateQueries({ queryKey: codeKeys.all(projectId) });
          router.push(codeHref(projectId, { view: "history", sha: r.sha }));
        } catch (e) {
          // Later commits touched the same lines: a chat can combine the undo by hand.
          if (e instanceof ApiError && e.status === 409 && /same lines/.test(e.message) && workspaces) {
            setUndoInChat({ commit: c, message: e.message });
            return;
          }
          throw e;
        }
      },
    });

  const tabs: [CodeRoute["view"], string, string][] = [
    ["overview", "Overview", base],
    ["history", "History", `${base}/history`],
    ["files", "Files", `${base}/files`],
    ...(route.view === "compare" ? [["compare", "Compare", pathname] as [CodeRoute["view"], string, string]] : []),
  ];
  const top = (
    <div className="mb-4 flex flex-wrap items-center justify-between gap-2">
      <nav aria-label="Code" className="flex gap-1 rounded-md bg-muted p-0.5">
        {tabs.map(([view, label, href]) => (
          <Link key={view} href={href} aria-current={route.view === view ? "page" : undefined} className={cn("rounded px-3 py-1 text-sm text-muted-foreground hover:text-foreground", route.view === view && "bg-background text-foreground shadow-xs")}>
            {label}
          </Link>
        ))}
      </nav>
      {o?.main && (
        <Button size="sm" variant="outline" onClick={() => setExporting(true)}>
          <Download /> Export
        </Button>
      )}
    </div>
  );

  let content: React.ReactNode;
  if (!o) content = overview.error ? null : <p role="status" className="text-sm text-muted-foreground">Loading the code…</p>;
  else if (!o.main)
    content = (
      <section className="flex flex-col items-center gap-3 rounded-lg border bg-card px-6 py-12 text-center">
        <span className="grid size-10 place-items-center rounded-full border bg-background">
          <GitBranch className="size-5 text-muted-foreground" />
        </span>
        <h2 className="text-lg font-semibold">No code yet</h2>
        <p className="max-w-lg text-sm text-muted-foreground">
          Each chat in this project can build in its own workspace. When someone presses Sync in a chat’s Git tab, its code lands here and the other chats bring it in with their next Sync. Every change remembers the chat that made it.
        </p>
        <div className="flex flex-wrap justify-center gap-2">
          <Button size="sm" onClick={() => newChat(projectId)}>
            <Plus /> New chat in project
          </Button>
          {p.mine && (
            <Button size="sm" variant="outline" onClick={() => setImporting(true)}>
              <GitBranch /> Start from a repository
            </Button>
          )}
        </div>
      </section>
    );
  else if (route.view === "history") content = <History projectId={projectId} sha={route.sha} log={log.data?.commits} overview={o} onUndo={undo} />;
  else if (route.view === "files") content = <Files projectId={projectId} path={route.path} />;
  else if (route.view === "compare") content = <Compare projectId={projectId} from={route.from} to={route.to} log={log.data?.commits} />;
  else content = <Overview projectId={projectId} o={o} log={log.data?.commits} onExport={() => setExporting(true)} />;

  return (
    <div>
      {top}
      <ErrorBox error={overview.error || log.error} onRetry={() => queries.invalidateQueries({ queryKey: codeKeys.all(projectId) })} />
      {content}
      <ExportDialog projectId={projectId} name={p.name} overview={o} open={exporting} onOpenChange={setExporting} />
      <ImportDialog projectId={projectId} open={importing} onOpenChange={setImporting} />
      {confirm.element}
      {undoInChat && (
        <UndoInChat
          message={undoInChat.message}
          onCancel={() => setUndoInChat(null)}
          onGo={() => {
            const c = undoInChat.commit;
            setUndoInChat(null);
            startIn(projectId, { prompt: undoInChatPrompt(c.sha, c.subject) });
          }}
        />
      )}
    </div>
  );
}

function UndoInChat({ message, onCancel, onGo }: { message: string; onCancel: () => void; onGo: () => void }) {
  return (
    <AlertDialog open onOpenChange={(open) => !open && onCancel()}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Undo it in a chat?</AlertDialogTitle>
          <AlertDialogDescription>
            {message}. A new chat with the latest code can take it back and keep what came after; the model does the combining and you check it before syncing.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={onGo}>Undo in a new chat</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
