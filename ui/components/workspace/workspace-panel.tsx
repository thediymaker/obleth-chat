"use client";

// The workspace's tabs in the chat's side pane, and full width on its Compute page. The
// tab host (workspace-tabs.tsx) owns the tab buttons and keeps this mounted behind its
// other tabs; this draws the bar under them (the
// preview's address, disk use, status, refresh, download, open in a new tab, full
// screen) and the tab itself. The preview frame and the terminal stay alive while
// another of these tabs shows. In a chat, the preview also has the Build view's two
// tools: point at an element to talk about it, and step through the chat's versions.

import { ArrowLeft, ChevronLeft, ChevronRight, Clock, Download, ExternalLink, File, Folder, Info, Maximize2, MousePointerClick, Orbit, RefreshCw, Terminal as TerminalIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { LogoLoader } from "@/components/brand";
import { Pill } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { useChat } from "@/components/chat/chat-provider";
import { pickedElement, versionAt, versionsOf, withPicked } from "@/lib/chat/build-view";
import { useCapabilities } from "@/lib/queries";
import { cn } from "@/lib/utils";
import type { Space, WorkspaceBridge } from "@/lib/workspace/bridge";
import { PICKED_URL_LIMIT, csvFile, csvTable, diskUsage, downloadUrl, htmlFile, pickedAbove, pickedPaths, previewReload, statusLabel, statusTone, treeItem } from "@/lib/workspace/model";
import { PaneLoading, PaneNote } from "./pane-note";
import { WorkspaceTerminal } from "./terminal";
import { ago, bytes } from "@/lib/format";

export type WorkspaceTab = "preview" | "files" | "terminal";

/** Keeps the bridge's previews flag in step with capabilities. */
export function usePreviewsFlag(bridge: WorkspaceBridge) {
  const previews = useCapabilities().data?.features?.previews;
  useEffect(() => {
    bridge.configure({ previews: previews === true });
  }, [bridge, previews]);
  return bridge.previews();
}

const iconButton = "text-muted-foreground hover:text-foreground";
// A button laid over the app in the preview: a page of any color, so it carries its own ground.
const overPreview = "bg-popover text-popover-foreground shadow-md hover:bg-popover dark:bg-popover dark:hover:bg-popover";

function Bar({ id, space, tab, bridge }: { id: string; space: Space; tab: WorkspaceTab; bridge: WorkspaceBridge }) {
  const w = space.workspace!;
  const previews = bridge.previews();
  const reload = previewReload(w, space.waking);
  const usage = diskUsage(w.diskBytes, bridge.diskLimit());
  const refreshLabel = tab === "files" ? "Refresh files" : tab === "terminal" ? "Restart terminal" : reload.label;
  return (
    <div className="flex h-10 shrink-0 items-center gap-2 border-b px-3">
      {space.origin ? (
        <span className="min-w-0 flex-1 truncate font-mono text-xs text-text-2" title={space.origin}>
          {space.origin.replace(/^https?:\/\//, "")}
        </span>
      ) : (
        <span className="min-w-0 flex-1 truncate text-xs text-faint">{previews ? "No preview yet" : ""}</span>
      )}
      {usage && (
        <span className={cn("hidden text-xs whitespace-nowrap text-muted-foreground sm:inline", usage.full && "text-destructive-foreground")} title={usage.full ? "Your workspaces are full: delete files to keep working" : "Disk used by all of your workspaces"}>
          {usage.text}
        </span>
      )}
      <Pill tone={statusTone(w)}>{statusLabel(w)}</Pill>
      <Button variant="ghost" size="icon-xs" className={iconButton} aria-label={refreshLabel} title={refreshLabel} disabled={tab === "preview" && reload.disabled} onClick={() => bridge.reload(id, tab)}>
        <RefreshCw className={cn(space.waking && "animate-spin")} />
      </Button>
      {w.status === "running" && (
        <Button asChild variant="ghost" size="icon-xs" className={iconButton}>
          <a href={downloadUrl(id)} download aria-label="Download project" title="Download project (.zip, without dependencies)">
            <Download />
          </a>
        </Button>
      )}
      {space.origin && (
        <Button asChild variant="ghost" size="icon-xs" className={iconButton}>
          <a href={space.url || space.origin} target="_blank" rel="noopener" aria-label="Open preview in a new tab" title="Open in a new tab">
            <ExternalLink />
          </a>
        </Button>
      )}
    </div>
  );
}

/** What the preview shows while there is no app to show. */
function EmptyPreview({ id, space, bridge }: { id: string; space: Space; bridge: WorkspaceBridge }) {
  const w = space.workspace!;
  if (w.status === "starting")
    return (
      <PaneNote icon={Orbit} mark={<LogoLoader />} status title="Starting your workspace…">
        A private Linux environment is being prepared for this chat.
      </PaneNote>
    );
  if (w.status === "error")
    return (
      <PaneNote icon={Info} title="The workspace could not start">
        {w.errorMessage || "Try again in a moment."}
      </PaneNote>
    );
  if (w.status === "stopped")
    return (
      <PaneNote
        icon={Clock}
        title="The workspace is asleep"
        action={
          <Button size="sm" variant="outline" disabled={space.waking} onClick={() => bridge.wake(id)}>
            <RefreshCw /> Wake it up
          </Button>
        }
      >
        Your files are kept. Wake it here, or send a message and the assistant will.
      </PaneNote>
    );
  return (
    <PaneNote icon={TerminalIcon} title="Nothing to preview yet">
      Ask for something to build, like “Build a site for my lab&apos;s reading group.” The running app appears here.
    </PaneNote>
  );
}

/**
 * The chat's versions under the preview: one mark per checkpoint, oldest first, the
 * one the project is at filled in. A mark, or the arrows beside them, takes the
 * project to that version (after asking, with a way back).
 */
function VersionBar({ id, space, bridge }: { id: string; space: Space; bridge: WorkspaceBridge }) {
  const store = useChat();
  const versions = versionsOf(store.active?.id === id ? store.active.messages : undefined);
  if (versions.length < 2) return null;
  const at = versionAt(versions, space.version);
  const busy = store.streaming;
  const go = (index: number) => {
    const version = versions[index];
    if (version && index !== at) bridge.restoreCheckpoint(id, version.sha, version.label);
  };
  const now = versions[at]!;
  return (
    <div className="flex h-9 shrink-0 items-center gap-2 border-t px-3 text-xs" data-versions aria-label="Versions of this project" role="group">
      <span className="text-2xs font-medium tracking-wider text-muted-foreground uppercase max-[560px]:hidden">Versions</span>
      <Button variant="ghost" size="icon-xs" className={iconButton} aria-label="Older version" title="Older version" disabled={busy || at <= 0} onClick={() => go(at - 1)}>
        <ChevronLeft />
      </Button>
      <div className="flex min-w-0 items-center gap-1 overflow-x-auto py-1">
        {versions.map((version, index) => (
          <button
            key={version.sha}
            type="button"
            disabled={busy}
            aria-label={`${version.label}${index === at ? " (the project is here)" : ""}`}
            aria-current={index === at ? "step" : undefined}
            title={[version.label, version.stat, version.at ? ago(version.at) : ""].filter(Boolean).join(" · ")}
            onClick={() => go(index)}
            className="group grid size-4 shrink-0 place-items-center disabled:opacity-50"
          >
            <span className={cn("rounded-full transition-all", index === at ? "size-2.5 bg-foreground" : index < at ? "size-1.5 bg-text-2 group-hover:size-2.5" : "size-1.5 border border-line-strong group-hover:size-2.5")} />
          </button>
        ))}
      </div>
      <Button variant="ghost" size="icon-xs" className={iconButton} aria-label="Newer version" title="Newer version" disabled={busy || at >= versions.length - 1} onClick={() => go(at + 1)}>
        <ChevronRight />
      </Button>
      <span className="ml-auto min-w-0 truncate text-muted-foreground" data-version-now>
        <span className="text-text-2">{now.label}</span> of {versions.length}
        {now.stat && <span className="max-[900px]:hidden"> · {now.stat}</span>}
      </span>
    </div>
  );
}

function Preview({ id, space, bridge, shown }: { id: string; space: Space; bridge: WorkspaceBridge; shown: boolean }) {
  const store = useChat();
  const box = useRef<HTMLDivElement>(null);
  const frame = useRef<HTMLIFrameElement>(null);
  const previews = bridge.previews();
  const port = space.workspace?.previewPort;
  // The preview can come back after the pane opened, for example once a woken pod's
  // server is up.
  useEffect(() => {
    if (shown && previews && port && !space.url && !space.linking) void bridge.showPreview(id);
  }, [shown, previews, port, space.url, space.linking, bridge, id]);
  const live = !!(port && space.url && previews);
  // Point and say needs the chat's message box, so it is offered in the chat only,
  // and only once the page in the frame says its picker is there.
  const inChat = store.route.page === "chat" && store.route.id === id && !store.shared;
  const [canPick, setCanPick] = useState(false);
  const [picking, setPicking] = useState(false);
  useEffect(() => {
    setCanPick(false);
    setPicking(false);
  }, [space.frame, space.url]);
  const tell = (type: "pick-start" | "pick-stop") => {
    if (space.origin) frame.current?.contentWindow?.postMessage({ source: "obleth-chat", type }, space.origin);
  };
  useEffect(() => {
    if (!space.origin || !inChat) return;
    const heard = (event: MessageEvent) => {
      // Only this chat's own preview frame is listened to.
      if (event.origin !== space.origin || event.source !== frame.current?.contentWindow) return;
      const data = event.data as { source?: string; type?: string } | null;
      if (!data || data.source !== "obleth-preview") return;
      if (data.type === "ready") {
        // A page that loads anew starts without picking.
        setCanPick(true);
        setPicking(false);
      } else if (data.type === "cancelled") setPicking(false);
      else if (data.type === "picked") {
        setPicking(false);
        const element = pickedElement(data);
        if (element) {
          // Into the message box, which shows it and takes the keys.
          store.draft = withPicked(store.draft, element);
          store.emit();
        }
      }
    };
    window.addEventListener("message", heard);
    return () => window.removeEventListener("message", heard);
  }, [space.origin, inChat, store]);
  useEffect(() => {
    if (!picking) return;
    const key = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      tell("pick-stop");
      setPicking(false);
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `tell` only reads refs and the origin
  }, [picking, space.origin]);
  return (
    <div className={cn("flex min-h-0 flex-1 flex-col", !shown && "hidden")}>
      <div ref={box} className="relative min-h-0 flex-1 bg-background">
        {live ? (
          <>
            <iframe
              ref={frame}
              key={space.frame}
              title="Workspace preview"
              src={space.url}
              sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-downloads"
              referrerPolicy="no-referrer"
              className="size-full border-0 bg-white"
            />
            <div className="absolute right-2 bottom-2 flex items-center gap-1.5">
              {inChat && canPick && (
                <Button
                  variant="outline"
                  size="xs"
                  className={cn(overPreview, picking && "border-foreground dark:border-foreground")}
                  aria-pressed={picking}
                  title={picking ? "Click an element in the preview, or press Esc" : "Point at something in the preview and say what to change"}
                  data-pick
                  onClick={() => {
                    tell(picking ? "pick-stop" : "pick-start");
                    setPicking(!picking);
                  }}
                >
                  <MousePointerClick /> {picking ? "Click an element · Esc cancels" : "Point"}
                </Button>
              )}
              <Button variant="outline" size="icon-xs" className={overPreview} aria-label="Full screen" title="Full screen" onClick={() => box.current?.requestFullscreen?.().catch(() => {})}>
                <Maximize2 />
              </Button>
            </div>
          </>
        ) : !previews ? (
          <PaneNote icon={Info} title="Previews are turned off on this server">
            Your workspace runs, but its apps can&apos;t be opened here. Use the Files and Terminal tabs, or download the project.
          </PaneNote>
        ) : (
          <EmptyPreview id={id} space={space} bridge={bridge} />
        )}
      </div>
      {inChat && <VersionBar id={id} space={space} bridge={bridge} />}
    </div>
  );
}

function FileView({ id, space, bridge }: { id: string; space: Space; bridge: WorkspaceBridge }) {
  const f = space.file!;
  // Parsed once per file, not on every repaint while a reply streams.
  const table = useMemo(() => (f.content != null && csvFile(f.path) ? csvTable(f.content, f.truncated) : null), [f.content, f.path, f.truncated]);
  const head = (
    <div className="flex h-10 shrink-0 items-center gap-2 border-b px-2">
      <Button variant="ghost" size="icon-xs" aria-label="Back to files" title="Back to files" onClick={() => bridge.closeFile(id)}>
        <ArrowLeft />
      </Button>
      <code className="min-w-0 flex-1 truncate font-mono text-xs">{f.path}</code>
      {bridge.previews() && htmlFile(f.path) && (
        <Button variant="ghost" size="xs" onClick={() => bridge.previewPage(id, f.path)}>
          <ExternalLink /> Open in preview
        </Button>
      )}
    </div>
  );
  let body: React.ReactNode;
  if (f.image)
    body = (
      <div className="grid place-items-center p-4">
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img src={f.image} alt={f.path} className="max-h-[70vh] max-w-full rounded border bg-[repeating-conic-gradient(var(--muted)_0_25%,transparent_0_50%)] bg-[length:16px_16px]" />
      </div>
    );
  else if (f.content == null) body = <PaneLoading label="Opening…" />;
  else if (f.error) body = <p className="p-4 text-sm text-muted-foreground">{f.error}</p>;
  else if (table) {
    const t = table;
    body = (
      <>
        <div className="overflow-auto">
          <table className="w-full border-collapse text-sm">
            <thead className="sticky top-0 bg-card">
              <tr>
                {t.header.map((h, i) => (
                  <th key={i} className="border-b px-2 py-1 text-left font-medium whitespace-nowrap">
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {t.body.map((r, i) => (
                <tr key={i} className="border-b border-line-soft">
                  {r.map((v, j) => (
                    <td key={j} className="px-2 py-1 whitespace-nowrap">
                      {v}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        {t.more && <p className="p-3 text-sm text-muted-foreground">Showing the first 200 rows.</p>}
      </>
    );
  } else
    body = (
      <>
        <pre className="min-h-0 overflow-auto bg-code p-3 font-mono text-[12px] leading-relaxed">
          <code>{f.content}</code>
        </pre>
        {f.truncated && <p className="p-3 text-sm text-muted-foreground">Showing the first 200 KB.</p>}
      </>
    );
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {head}
      <div className="min-h-0 flex-1 overflow-auto">{body}</div>
    </div>
  );
}

function Files({ id, space, bridge }: { id: string; space: Space; bridge: WorkspaceBridge }) {
  const running = space.workspace?.status === "running";
  // The files and folders ticked for a download of just those. A ticked folder
  // brings everything in it, so its rows show ticked and cannot be changed alone.
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
  useEffect(() => {
    if ((space.files == null || space.filesStale) && !space.loadingFiles && running && !space.file) void bridge.loadFiles(id);
  }, [space.files, space.filesStale, space.loadingFiles, running, space.file, bridge, id]);
  // A refreshed list may no longer hold what was ticked.
  const listed = useMemo(() => new Set((space.files || []).map((e) => e.path)), [space.files]);
  const asking = useMemo(() => pickedPaths(new Set([...picked].filter((p) => listed.has(p)))), [picked, listed]);
  // The rows are built again only when the list or what is ticked changes, not on every
  // repaint while a reply streams.
  const rows = useMemo(() => {
    const pick = (path: string, on: boolean) =>
      setPicked((old) => {
        const next = new Set(old);
        if (on) next.add(path);
        else next.delete(path);
        return next;
      });
    return (space.files || []).map((e) => {
      const { depth, name } = treeItem(e);
      const pad = { paddingLeft: `${0.75 + depth * 1}rem` };
      const within = pickedAbove(picked, e.path);
      const box = <Checkbox aria-label={`Select ${e.path}`} checked={within || picked.has(e.path)} disabled={within} onCheckedChange={(on) => pick(e.path, on === true)} />;
      return e.type === "directory" ? (
        <li key={e.path} className="flex items-center gap-2 py-1 pr-3 text-text-2" style={pad}>
          {box}
          <Folder className="size-3.5 shrink-0 text-muted-foreground" />
          <span className="truncate">{name}</span>
        </li>
      ) : (
        <li key={e.path} className="flex items-center gap-2 pr-3 hover:bg-accent" style={pad}>
          {box}
          <button type="button" className="flex min-w-0 flex-1 items-center gap-2 py-1 text-left" onClick={() => bridge.showFile(id, e.path)}>
            <File className="size-3.5 shrink-0 text-muted-foreground" />
            <span className="min-w-0 flex-1 truncate">{name}</span>
            <small className="shrink-0 text-xs text-faint">{bytes(e.size)}</small>
          </button>
        </li>
      );
    });
  }, [space.files, picked, bridge, id]);
  if (space.file) return <FileView id={id} space={space} bridge={bridge} />;
  if (!running)
    return (
      <div className="min-h-0 flex-1 overflow-auto">
        <EmptyPreview id={id} space={space} bridge={bridge} />
      </div>
    );
  if (!space.files) return <PaneLoading label="Loading files…" />;
  if (!space.files.length)
    return (
      <PaneNote icon={File} title={space.filesError ? "The files could not load." : "No files yet."}>
        {space.filesError || "Files the assistant creates appear here."}
      </PaneNote>
    );
  const top = space.files.filter((e) => !e.path.includes("/")).map((e) => e.path);
  const all = top.length > 0 && top.every((p) => picked.has(p));
  const href = downloadUrl(id, asking);
  const tooMany = href.length > PICKED_URL_LIMIT;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 items-center gap-2 border-b px-3 text-sm">
        <Checkbox aria-label="Select everything" checked={all} onCheckedChange={(on) => setPicked(on === true ? new Set(top) : new Set())} />
        {asking.length === 0 ? (
          <span className="min-w-0 flex-1 truncate text-muted-foreground">Tick files or folders to download just those.</span>
        ) : (
          <>
            <span className="min-w-0 flex-1 truncate text-text-2">{tooMany ? "Too many picked one by one. Tick their folders instead." : `${asking.length} picked`}</span>
            {!tooMany && (
              <Button asChild variant="outline" size="xs">
                <a href={href} download aria-label="Download what is picked" title={asking.length === 1 ? "Download it: a file as itself, a folder as a .zip" : "Download them as one .zip"}>
                  <Download /> Download
                </a>
              </Button>
            )}
            <Button variant="ghost" size="xs" onClick={() => setPicked(new Set())}>
              Clear
            </Button>
          </>
        )}
      </div>
      <ul className="min-h-0 flex-1 overflow-auto py-1 text-md" aria-label="Files">
        {rows}
      </ul>
    </div>
  );
}

export function WorkspacePanel({ conversationId, tab }: { conversationId: string; tab: WorkspaceTab }) {
  const store = useChat();
  const bridge = store.workspace;
  usePreviewsFlag(bridge);
  const space = bridge.space(conversationId);
  // The terminal starts the first time its tab shows, then keeps running while hidden.
  const [terminalSeen, setTerminalSeen] = useState(tab === "terminal");
  useEffect(() => {
    if (tab === "terminal") setTerminalSeen(true);
  }, [tab]);
  const caps = useCapabilities().data;
  const on = bridge.enabled();
  useEffect(() => {
    if (on && !space.loaded) void bridge.load(conversationId);
  }, [on, space.loaded, bridge, conversationId]);
  if (!space.workspace) return space.loaded || (caps && !caps.features?.workspaces) ? <PaneNote icon={TerminalIcon} title="This chat has no workspace" /> : <PaneLoading />;
  return (
    <div className="flex h-full min-h-0 flex-col" data-conversation={conversationId}>
      <Bar id={conversationId} space={space} tab={tab} bridge={bridge} />
      <Preview id={conversationId} space={space} bridge={bridge} shown={tab === "preview"} />
      {tab === "files" && <Files id={conversationId} space={space} bridge={bridge} />}
      {terminalSeen && (
        <div className={cn("min-h-0 flex-1", tab !== "terminal" && "hidden")}>
          <WorkspaceTerminal bridge={bridge} conversationId={conversationId} session={space.terminal} />
        </div>
      )}
    </div>
  );
}
