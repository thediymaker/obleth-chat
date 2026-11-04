"use client";

// The workspace's tabs in the chat's side pane, and full width on its Compute page. The
// side pane owns the tab buttons and closing; this draws the bar under them (the
// preview's address, disk use, status, refresh, download, open in a new tab, full
// screen) and the tab itself. The preview frame and the terminal stay alive while
// another of these tabs shows.

import { ArrowLeft, Clock, Download, ExternalLink, File, Folder, Info, Maximize2, Orbit, RefreshCw, Terminal as TerminalIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Pill } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { useChat } from "@/components/chat/chat-provider";
import { useCapabilities } from "@/lib/queries";
import { cn } from "@/lib/utils";
import type { Space, WorkspaceBridge } from "./bridge";
import { csvFile, csvTable, diskUsage, downloadUrl, formatBytes, htmlFile, previewReload, statusLabel, statusTone, treeItem } from "./model";
import { PaneLoading, PaneNote } from "./pane-note";
import { WorkspaceTerminal } from "./terminal";

export type WorkspaceTab = "preview" | "files" | "terminal";

/** Keeps the bridge's previews flag in step with capabilities. */
export function usePreviewsFlag(bridge: WorkspaceBridge) {
  const previews = useCapabilities().data?.features?.previews;
  useEffect(() => {
    bridge.configure({ previews: previews !== false });
  }, [bridge, previews]);
  return bridge.previews();
}

const iconButton = "text-muted-foreground hover:text-foreground";

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
      <PaneNote icon={Orbit} spin status title="Starting your workspace…">
        A private Linux environment is being prepared for this conversation.
      </PaneNote>
    );
  if (w.status === "error")
    return (
      <PaneNote icon={Info} title="The workspace could not start.">
        {w.errorMessage || "Try again in a moment."}
      </PaneNote>
    );
  if (w.status === "stopped")
    return (
      <PaneNote
        icon={Clock}
        title="The workspace is asleep."
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
    <PaneNote icon={TerminalIcon} title="Nothing to preview yet.">
      Ask for something to build, like “Build a site for my lab&apos;s reading group.” The running app appears here.
    </PaneNote>
  );
}

function Preview({ id, space, bridge, shown }: { id: string; space: Space; bridge: WorkspaceBridge; shown: boolean }) {
  const box = useRef<HTMLDivElement>(null);
  const previews = bridge.previews();
  const port = space.workspace?.previewPort;
  // The preview can come back after the pane opened, for example once a woken pod's
  // server is up.
  useEffect(() => {
    if (shown && previews && port && !space.url && !space.linking) void bridge.showPreview(id);
  }, [shown, previews, port, space.url, space.linking, bridge, id]);
  const live = !!(port && space.url && previews);
  return (
    <div ref={box} className={cn("relative min-h-0 flex-1 bg-background", !shown && "hidden")}>
      {live ? (
        <>
          <iframe
            key={space.frame}
            title="Workspace preview"
            src={space.url}
            sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-downloads"
            referrerPolicy="no-referrer"
            className="size-full border-0 bg-white"
          />
          <Button
            variant="outline"
            size="icon-xs"
            className="absolute right-2 bottom-2 opacity-70 hover:opacity-100"
            aria-label="Full screen"
            title="Full screen"
            onClick={() => box.current?.requestFullscreen?.().catch(() => {})}
          >
            <Maximize2 />
          </Button>
        </>
      ) : !previews ? (
        <PaneNote icon={Info} title="Previews are not set up on this server.">
          Your workspace runs, but its apps can&apos;t be opened here. Use the Files and Terminal tabs, or download the project.
        </PaneNote>
      ) : (
        <EmptyPreview id={id} space={space} bridge={bridge} />
      )}
    </div>
  );
}

function FileView({ id, space, bridge }: { id: string; space: Space; bridge: WorkspaceBridge }) {
  const f = space.file!;
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
  else if (csvFile(f.path)) {
    const t = csvTable(f.content, f.truncated);
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
  useEffect(() => {
    if (space.files == null && !space.loadingFiles && running && !space.file) void bridge.loadFiles(id);
  }, [space.files, space.loadingFiles, running, space.file, bridge, id]);
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
  return (
    <ul className="min-h-0 flex-1 overflow-auto py-1 text-md" aria-label="Files">
      {space.files.map((e) => {
        const { depth, name } = treeItem(e);
        const pad = { paddingLeft: `${0.75 + depth * 1}rem` };
        return e.type === "directory" ? (
          <li key={e.path} className="flex items-center gap-2 py-1 pr-3 text-text-2" style={pad}>
            <Folder className="size-3.5 shrink-0 text-muted-foreground" />
            <span className="truncate">{name}</span>
          </li>
        ) : (
          <li key={e.path}>
            <button type="button" className="flex w-full items-center gap-2 py-1 pr-3 text-left hover:bg-accent" style={pad} onClick={() => bridge.showFile(id, e.path)}>
              <File className="size-3.5 shrink-0 text-muted-foreground" />
              <span className="min-w-0 flex-1 truncate">{name}</span>
              <small className="shrink-0 text-xs text-faint">{formatBytes(e.size)}</small>
            </button>
          </li>
        );
      })}
    </ul>
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
  if (!space.workspace) return space.loaded || (caps && !caps.features?.workspaces) ? <PaneNote icon={TerminalIcon} title="This conversation has no workspace." /> : <PaneLoading />;
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
