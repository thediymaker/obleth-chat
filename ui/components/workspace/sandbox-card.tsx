"use client";

// The person's own workspace container, now: awake or asleep, what it uses of its
// CPU, memory and disk, and when it sleeps if left alone. One reading is shared by
// Home, the status line and the Workspaces page, and is asked again every 20 seconds
// only while the container is up.

import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { meterTone, sandboxView, type SandboxUsage } from "@/lib/chat/sandbox";
import { cn } from "@/lib/utils";

export const sandboxUsageKey = ["workspace-usage"] as const;

export function useSandboxUsage(on = true) {
  return useQuery({
    queryKey: sandboxUsageKey,
    enabled: on,
    queryFn: () => api.request<SandboxUsage>("/api/workspaces/usage"),
    staleTime: 10_000,
    refetchInterval: (query) => (query.state.data?.status === "running" || query.state.data?.status === "starting" ? 20_000 : false),
  });
}

const BAR = { calm: "bg-text-2", close: "bg-amber", full: "bg-destructive" } as const;

/** The meters as rows: a name, a bar against the limit, and the numbers. */
export function SandboxMeters({ usage, className }: { usage: SandboxUsage; className?: string }) {
  const view = sandboxView(usage);
  if (!view.meters.length) return null;
  return (
    <dl className={cn("grid grid-cols-[auto_minmax(3rem,1fr)_auto] items-center gap-x-3 gap-y-1.5 text-sm leading-[normal]", className)} data-sandbox-meters>
      {view.meters.map((meter) => (
        <div key={meter.key} className="contents" data-meter={meter.key}>
          <dt className="text-muted-foreground">{meter.label}</dt>
          <dd className="h-1 overflow-hidden rounded-full bg-line-strong" role={meter.share == null ? undefined : "meter"} aria-label={meter.share == null ? undefined : `${meter.label} used`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={meter.share == null ? undefined : Math.round(meter.share * 100)}>
            {meter.share != null && <span className={cn("block h-full rounded-full", BAR[meterTone(meter.share)])} style={{ width: `${Math.max(2, meter.share * 100)}%` }} />}
          </dd>
          <dd className="text-right font-mono text-xs text-text-2 tabular-nums">{meter.text}</dd>
        </div>
      ))}
    </dl>
  );
}

/** The dot beside the container's state: the accent while it is alive. */
export function SandboxDot({ usage }: { usage: SandboxUsage }) {
  const state = usage.status;
  return <span className={cn("size-1.5 shrink-0 rounded-full", state === "running" ? "bg-signal" : state === "starting" ? "signal-breathe bg-signal" : state === "error" ? "bg-destructive" : "bg-line-strong")} aria-hidden="true" />;
}

/** The whole card, for a popover or a page. */
export function SandboxCard({ usage, className }: { usage: SandboxUsage; className?: string }) {
  const view = sandboxView(usage);
  return (
    <div className={cn("grid gap-3", className)} data-sandbox-card={view.state}>
      <div className="flex items-center gap-2 text-2xs leading-[normal] font-medium tracking-[.08em] text-text-2 uppercase">
        <SandboxDot usage={usage} />
        Your workspace · {view.label}
      </div>
      <SandboxMeters usage={usage} />
      {view.note && <p className="text-sm leading-normal text-muted-foreground">{view.note}</p>}
    </div>
  );
}
