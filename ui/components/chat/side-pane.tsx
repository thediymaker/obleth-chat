"use client";

// The pane beside a chat: its Details, the workspace's Preview, Files, Terminal and
// Code tabs, and the Browser. On a phone it covers the chat until closed.

import { FileText, GitBranch, Globe, PanelRight, Play, Settings, Terminal, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { BrowserPanel } from "@/components/browser/browser-panel";
import { CodePanel, CodeTabLabel } from "@/components/code/code-panel";
import { ToolsContextSection } from "@/components/connections/chat-tools";
import { KnowledgeContextSection } from "@/components/knowledge/chat-knowledge";
import { ProjectContextSection } from "@/components/projects/chat-project";
import { Button } from "@/components/ui/button";
import { WorkspacePanel } from "@/components/workspace/workspace-panel";
import { effortLabel, tokens } from "@/lib/chat/composer";
import { contextUse } from "@/lib/chat/context";
import { cn } from "@/lib/utils";
import { useChat } from "./chat-provider";

const TABS: Record<string, [string, typeof Play]> = {
  preview: ["Preview", Play],
  files: ["Files", FileText],
  terminal: ["Terminal", Terminal],
  code: ["Git", GitBranch],
  browser: ["Browser", Globe],
  details: ["Details", PanelRight],
};

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0 rounded-sm border bg-card px-2 py-[7px]">
      <span className="block text-2xs font-medium tracking-[.08em] text-muted-foreground uppercase">{label}</span>
      <strong className="mt-[3px] block truncate text-base font-semibold tabular-nums" title={value}>
        {value}
      </strong>
    </div>
  );
}

const SECTION = "mt-[22px] border-t border-line-soft pt-[18px] first:mt-1.5 first:border-t-0";
const EYEBROW = "mb-2.5 block text-2xs font-medium tracking-[.08em] text-muted-foreground uppercase";
const NOTE = "mt-2.5 text-xs leading-[1.6] text-faint";
function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="my-[9px] flex justify-between gap-3 text-sm text-muted-foreground">
      <span>{label}</span>
      <span className="text-right text-text-2 tabular-nums [overflow-wrap:anywhere]">{children}</span>
    </div>
  );
}

/** Everything about the chat: context use, the engine, its project, knowledge and tools. */
export function ChatDetails() {
  const store = useChat();
  const c = store.active;
  if (!c) return null;
  const use = contextUse(c, { windowSize: Number(store.policyModel()?.metadata?.contextWindow) || 0, systemPrompt: store.systemPrompt, open: true });
  const selected = store.modelName(store.model || c.model),
    saved = c.messages?.length || 0;
  const resolved = [...(c.messages || [])].reverse().find((m) => m.role === "assistant" && m.settings?.resolvedModel)?.settings?.resolvedModel;
  return (
    <div className="px-5 pb-6 leading-[normal]">
      <section className={SECTION}>
        <span className={EYEBROW}>Context</span>
        {use.used || use.unknown ? (
          <>
            <div className="mb-2.5 flex items-baseline gap-2">
              <strong className="text-[26px] font-medium tabular-nums">{use.unknown ? "—" : tokens(use.used)}</strong>
              <span className="text-sm text-muted-foreground">{use.windowSize ? `of ${tokens(use.windowSize)} tokens` : "tokens"}</span>
            </div>
            {use.windowSize > 0 && !use.unknown && (
              <div className="mt-1.5 mb-1.5 h-1 overflow-hidden rounded bg-control" role="meter" aria-label="Context used" aria-valuemin={0} aria-valuemax={use.windowSize} aria-valuenow={Math.min(use.used, use.windowSize)}>
                <div className={cn("h-full", use.share > 0.9 ? "bg-destructive" : use.share > 0.75 ? "bg-amber" : "bg-text-2")} style={{ width: `${use.share * 100}%` }} />
              </div>
            )}
            <Row label="Messages">about {tokens(use.messages)}</Row>
            {!use.unknown && <Row label="Tools, instructions and pages">about {tokens(use.rest)}</Row>}
            <Row label="Last reply">
              {tokens(use.output)} out{use.steps > 1 ? ` · ${use.steps} steps` : ""}
            </Row>
            {use.unknown && <p className={NOTE}>The size shows after the next reply.</p>}
          </>
        ) : (
          <p className={NOTE}>Shows after the first reply.</p>
        )}
        <p className={NOTE}>
          This chat holds {(use.bytes / 1024).toFixed(1)} of 100 KiB of text and {saved} of 200 messages.
        </p>
      </section>
      <section className={SECTION}>
        <div className="flex items-center justify-between">
          <span className={cn(EYEBROW, "mb-0")}>Engine</span>
          <Button variant="ghost" size="sm" onClick={() => store.openDialog({ kind: "parameters" })}>
            <Settings /> Adjust
          </Button>
        </div>
        <div className="mt-2.5 grid grid-cols-4 gap-1.5">
          <Stat label="Model" value={selected} />
          <Stat label="Thinking" value={effortLabel(store.reasoningEffort)} />
          <Stat label="Temperature" value={String(store.temperature)} />
          <Stat label="Output" value={store.maxTokens ? Number(store.maxTokens).toLocaleString() : "No limit"} />
        </div>
        {resolved && store.modelName(resolved) !== selected && <p className={NOTE}>Last answer came from {store.modelName(resolved)}.</p>}
        <p className={NOTE}>Instructions: {store.systemPrompt ? "set for this chat" : "your defaults"}.</p>
      </section>
      <div className={cn(SECTION, "empty:hidden")}>
        <ProjectContextSection conversationId={c.id} />
      </div>
      <div className="empty:hidden">
        <KnowledgeContextSection conversationId={c.id} />
      </div>
      <div className="empty:hidden">
        <ToolsContextSection chatId={c.id} />
      </div>
    </div>
  );
}

const WIDTH_KEY = "obleth-chat.workspace-width";

/** The pane's width, dragged from its left edge and remembered in this browser:
 * at least 360px for the pane and 480px for the chat. */
function usePaneWidth() {
  const [width, setWidthState] = useState<number | null>(null);
  const clamp = useCallback((w: number) => {
    const sidebar = document.querySelector('aside[aria-label="Main navigation"]')?.getBoundingClientRect().width || 0;
    return Math.round(Math.max(360, Math.min(w, window.innerWidth - sidebar - 480)));
  }, []);
  useEffect(() => {
    try {
      const saved = Number(localStorage.getItem(WIDTH_KEY));
      if (saved > 0) setWidthState(saved);
    } catch {
      // Storage unavailable: the default width.
    }
  }, []);
  const setWidth = useCallback(
    (w: number | null, save = false) => {
      const next = w == null ? null : clamp(w);
      setWidthState(next);
      if (save)
        try {
          if (next == null) localStorage.removeItem(WIDTH_KEY);
          else localStorage.setItem(WIDTH_KEY, String(next));
        } catch {
          // Storage unavailable.
        }
    },
    [clamp],
  );
  return { width: width == null ? null : clamp(width), setWidth };
}

function ResizeHandle({ pane, width, setWidth }: { pane: React.RefObject<HTMLElement | null>; width: number | null; setWidth: (w: number | null, save?: boolean) => void }) {
  const [dragging, setDragging] = useState(false);
  const latest = useRef(width);
  latest.current = width;
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize the pane"
      tabIndex={0}
      title="Drag to resize. Double-click to reset."
      className={cn(
        "absolute inset-y-0 -left-1 z-10 hidden w-2 cursor-col-resize touch-none outline-none min-[1001px]:block",
        "after:absolute after:inset-y-0 after:left-[3px] after:w-0.5 after:transition-colors hover:after:bg-ring focus-visible:after:bg-ring",
        dragging && "after:bg-ring",
      )}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        setDragging(true);
        document.body.style.userSelect = "none";
      }}
      onPointerMove={(e) => {
        if (e.currentTarget.hasPointerCapture(e.pointerId)) setWidth(window.innerWidth - e.clientX);
      }}
      onPointerUp={(e) => {
        if (!dragging) return;
        setDragging(false);
        document.body.style.userSelect = "";
        if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
        setWidth(latest.current, true);
      }}
      onPointerCancel={() => {
        setDragging(false);
        document.body.style.userSelect = "";
      }}
      onDoubleClick={() => setWidth(null, true)}
      onKeyDown={(e) => {
        const step = e.shiftKey ? 96 : 32,
          now = pane.current?.getBoundingClientRect().width || 0;
        if (e.key === "ArrowLeft") setWidth(now + step, true);
        else if (e.key === "ArrowRight") setWidth(now - step, true);
        else return;
        e.preventDefault();
      }}
    />
  );
}

export function SidePane({ id }: { id: string }) {
  const store = useChat();
  const ref = useRef<HTMLElement>(null);
  const { width, setWidth } = usePaneWidth();
  if (!store.paneVisible(id)) return null;
  const pane = store.pane(id);
  const tabs = store.paneTabs(id);
  const tab = pane.tab;
  // Details alone need less room than a preview or a page.
  const detailsOnly = tabs.length === 1 && tabs[0] === "details";
  return (
    <aside
      ref={ref}
      aria-label="Chat side pane"
      style={width && !detailsOnly ? ({ "--pane-w": `${width}px` } as React.CSSProperties) : undefined}
      className={cn(
        // min(48vw, 920px), min(440px, 42vw) for Details alone; over the chat up to
        // 1000px wide.
        "fixed inset-x-0 top-14 bottom-0 z-30 flex flex-col bg-sidebar max-[760px]:top-[52px] min-[1001px]:relative min-[1001px]:inset-auto min-[1001px]:z-auto min-[1001px]:shrink-0 min-[1001px]:border-l",
        detailsOnly ? "min-[1001px]:w-[min(440px,42vw)]" : width ? "min-[1001px]:w-[var(--pane-w)]" : "min-[1001px]:w-[min(48vw,920px)]",
      )}
    >
      {!detailsOnly && <ResizeHandle pane={ref} width={width} setWidth={setWidth} />}
      {/* The tab bar, with underlined tabs. */}
      <div className="flex min-h-11 shrink-0 items-center gap-3 border-b pr-2 pl-4 leading-[normal]">
        <div className="flex min-w-0 flex-1 gap-4 overflow-x-auto" role="tablist">
          {tabs.map((t) => {
            const [label] = TABS[t] || [t];
            return (
              <button
                key={t}
                type="button"
                role="tab"
                aria-selected={tab === t}
                onClick={() => store.showTab(id, t)}
                className={cn(
                  "-mb-px flex shrink-0 items-center gap-[7px] border-b border-transparent py-3 text-md text-muted-foreground hover:text-foreground",
                  tab === t && "border-foreground text-foreground",
                )}
              >
                {t === "code" ? <CodeTabLabel conversationId={id} /> : label}
              </button>
            );
          })}
        </div>
        <Button variant="ghost" size="icon-sm" className="size-8" aria-label="Close the pane" onClick={() => store.closePane(id)}>
          <X />
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {tab === "details" ? (
          <ChatDetails />
        ) : tab === "browser" ? (
          <BrowserPanel conversationId={id} />
        ) : tab === "code" ? (
          <CodePanel conversationId={id} />
        ) : (
          <WorkspacePanel conversationId={id} tab={tab as "preview" | "files" | "terminal"} />
        )}
      </div>
    </aside>
  );
}
