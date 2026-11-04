"use client";

// The strip under every signed-in page: how many
// models this account can use, today's requests and tokens against any daily
// limits, the site's name and the keyboard shortcuts.

import { useBranding } from "@/lib/branding-context";
import { cn } from "@/lib/utils";
import { useChat } from "./chat-provider";

const compact = new Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 });
const count = (value: unknown) => compact.format(Number(value) || 0);

interface DailyUsage {
  today?: { requests?: number; totalTokens?: number };
  limits?: { dailyRequests?: number; dailyTokens?: number };
}

/** "3 / 100 requests · 12K tokens", or "—" before the first count. */
export function usageText(usage: DailyUsage | null | undefined) {
  const today = usage?.today,
    limits = usage?.limits || {};
  if (!today) return "—";
  return `${count(today.requests)}${limits.dailyRequests ? ` / ${count(limits.dailyRequests)}` : ""} requests · ${count(today.totalTokens)}${limits.dailyTokens ? ` / ${count(limits.dailyTokens)}` : ""} tokens`;
}

export function modelsText(store: { catalogError: string; models: { id: string }[] }) {
  if (store.catalogError) return "Unavailable";
  return store.models.length ? `${store.models.filter((m) => m.id !== "auto").length} available` : "—";
}

function Metric({ label, value, optional }: { label: string; value: string; optional?: boolean }) {
  return (
    <span className={cn("flex items-baseline gap-1.5 whitespace-nowrap", optional && "max-[1000px]:hidden")} data-status={label.toLowerCase()}>
      <span className="text-2xs font-medium tracking-[.08em] text-muted-foreground uppercase">{label}</span>
      <span className="truncate text-sm font-medium text-foreground tabular-nums">{value}</span>
    </span>
  );
}

/** 40px, on the sidebar's color. */
export function StatusBar() {
  const store = useChat();
  const branding = useBranding();
  return (
    <footer className="flex h-10 shrink-0 leading-[normal] items-center justify-between gap-4 border-t bg-sidebar px-6 text-sm text-muted-foreground max-[760px]:px-4">
      <div className="flex min-w-0 items-center gap-4">
        <Metric label="Models" value={modelsText(store)} />
        <span className="h-3.5 w-px shrink-0 bg-border max-[1000px]:hidden" />
        <Metric label="Today" value={usageText(store.usage as DailyUsage | null)} optional />
      </div>
      <div className="flex shrink-0 items-center gap-4 whitespace-nowrap max-[760px]:hidden">
        <span>{branding.siteName}</span>
        <button type="button" className="rounded-sm border px-1.5 py-0.5 text-xs text-muted-foreground hover:text-foreground" aria-label="Keyboard shortcuts" onClick={() => store.openDialog({ kind: "shortcuts" })}>
          ⌘ K
        </button>
      </div>
    </footer>
  );
}
