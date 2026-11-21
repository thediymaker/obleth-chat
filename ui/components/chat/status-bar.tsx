"use client";

// The line under every signed-in page. In a chat it says what is happening now: a
// reply at work and for how long (or that it waits for the person), the chat's
// workspace, the model and how much of its window the chat uses. Then the person's
// jobs on their clusters, when any run or wait. After that, how many models this account can use, today's requests and tokens against any daily
// limits, the site's name and the keyboard shortcuts.

import Link from "next/link";
import { useEffect, useState } from "react";
import { useClusterQueues } from "@/components/clusters/status";
import { clock, effortLabel } from "@/lib/chat/composer";
import { contextUse } from "@/lib/chat/context";
import { isLive } from "@/lib/chat/turn";
import { useBranding } from "@/lib/branding-context";
import { useUsage, type OwnUsage } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { useChat } from "./chat-provider";
import { compact, count } from "@/lib/format";

/** "3 / 100 requests · 12K tokens", or "—" before the first count. */
export function usageText(usage: OwnUsage | null | undefined) {
  const today = usage?.today,
    limits = usage?.limits || {};
  if (!today) return "—";
  const requests = limits.dailyRequests ? `${compact(today.requests)} / ${compact(limits.dailyRequests)} requests` : count(today.requests, "request");
  return `${requests} · ${compact(today.totalTokens)}${limits.dailyTokens ? ` / ${compact(limits.dailyTokens)}` : ""} tokens`;
}

export function modelsText(store: { catalogError: string; models: { id: string }[] }) {
  if (store.catalogError) return "Unavailable";
  return store.models.length ? `${store.models.filter((m) => m.id !== "auto").length} available` : "—";
}

const WORKSPACE: Record<string, [string, string]> = {
  running: ["Workspace awake", "bg-signal"],
  starting: ["Workspace starting", "signal-breathe bg-signal"],
  stopped: ["Workspace asleep", "bg-line-strong"],
  error: ["Workspace error", "bg-destructive"],
};

/** What the reply being written is doing, for the line's first segment. */
export function runText(status: { waiting: boolean; began: number; now: number }) {
  if (status.waiting) return "Needs your say";
  return Number.isFinite(status.began) ? `Working ${clock(status.now - status.began)}` : "Working";
}

/** "2 running · 1 waiting" across the person's clusters, or "" when none do. */
export function jobsText(queues: { running: number; pending: number }[]) {
  const running = queues.reduce((n, q) => n + q.running, 0),
    pending = queues.reduce((n, q) => n + q.pending, 0);
  return [running ? `${running} running` : "", pending ? `${pending} waiting` : ""].filter(Boolean).join(" · ");
}

const SEGMENT = "flex h-full shrink-0 items-center gap-2 border-r border-sidebar-border px-3.5 whitespace-nowrap";

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <span className={cn(SEGMENT, "max-[1200px]:hidden")} data-status={label.toLowerCase()}>
      <span className="text-muted-foreground">{label.toLowerCase()}</span>
      <span className="truncate text-text-2 tabular-nums">{value}</span>
    </span>
  );
}

/** 32px, on the sidebar's color, in the monospace face. */
export function StatusBar() {
  const store = useChat();
  const branding = useBranding();
  const { data: usage } = useUsage();
  const queues = useClusterQueues();
  const jobs = jobsText(queues);
  const id = store.route.id;
  const chat = id && store.active?.id === id ? store.active : null;
  const last = chat?.messages?.at(-1);
  const live = !!last && last.role === "assistant" && isLive(last) && store.streaming;
  const waiting = live && (last.activity || []).some((step) => step.status === "awaiting");
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!live) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [live]);

  const space = chat && store.workspace.enabled() && store.workspace.has(id) ? WORKSPACE[store.workspace.space(id).workspace?.status || ""] : undefined;
  const use = chat ? contextUse(chat, { windowSize: Number(store.policyModel()?.metadata?.contextWindow) || 0, systemPrompt: store.systemPrompt, open: true }) : null;
  const share = use && use.windowSize && !use.unknown && use.used ? Math.min(1, use.share) : null;
  const effort = store.reasoningEffort && store.reasoningEffort !== "none" ? effortLabel(store.reasoningEffort) : "";
  return (
    <footer className="flex h-8 shrink-0 items-stretch overflow-hidden border-t border-sidebar-border bg-sidebar font-mono text-xs leading-[normal] text-text-2">
      {live && (
        <span role="status" className={cn("flex h-full shrink-0 items-center px-3.5 font-medium whitespace-nowrap", waiting ? "bg-amber text-background" : "bg-signal text-signal-foreground")} data-status="run">
          {runText({ waiting, began: Date.parse(last._began || last.createdAt || ""), now })}
        </span>
      )}
      {space && (
        <span className={cn(SEGMENT, "max-[760px]:hidden")} data-status="workspace">
          <span className={cn("size-1.5 rounded-full", space[1])} aria-hidden="true" />
          {space[0].toLowerCase()}
        </span>
      )}
      {chat && (
        <span className={cn(SEGMENT, "max-[1000px]:hidden")} data-status="model">
          <span className="max-w-56 truncate">{store.modelName(store.model || chat.model)}</span>
          {effort && <span className="text-muted-foreground">· {effort.toLowerCase()}</span>}
        </span>
      )}
      {share != null && (
        <button type="button" className={cn(SEGMENT, "hover:text-foreground")} title="Chat details (⌘.)" data-status="context" onClick={() => store.toggleDetails(id)}>
          <span className="text-muted-foreground">context</span>
          <span className="h-1 w-14 overflow-hidden rounded-full bg-line-strong" aria-hidden="true">
            <span className={cn("block h-full", share > 0.9 ? "bg-destructive" : share > 0.75 ? "bg-amber" : "bg-text-2")} style={{ width: `${share * 100}%` }} />
          </span>
          <span className="tabular-nums">{Math.round(share * 100)}%</span>
        </button>
      )}
      {jobs && (
        <Link href="/clusters" className={cn(SEGMENT, "hover:text-foreground max-[1000px]:hidden")} title="Your jobs on your clusters" data-status="jobs">
          <span className={cn("size-1.5 rounded-full", queues.some((q) => q.running) ? "bg-signal" : "bg-line-strong")} aria-hidden="true" />
          <span className="text-muted-foreground">jobs</span>
          <span className="tabular-nums">{jobs}</span>
        </Link>
      )}
      <Metric label="Models" value={modelsText(store)} />
      <Metric label="Today" value={usageText(usage)} />
      <span className="ml-auto flex shrink-0 items-center gap-3 px-3.5 whitespace-nowrap text-muted-foreground max-[760px]:hidden">
        <span className="font-sans">{branding.siteName}</span>
        <button type="button" className="rounded-sm border px-1.5 py-px text-2xs hover:text-foreground" aria-label="Keyboard shortcuts" onClick={() => store.openDialog({ kind: "shortcuts" })}>
          ⌘ K
        </button>
      </span>
    </footer>
  );
}
