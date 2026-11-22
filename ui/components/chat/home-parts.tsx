"use client";

// Home's intent tabs and "Running now". The tabs say what the new chat is for and
// set the box up for it; "Running now" lists what is alive without the person:
// chats that wait for a yes or no, chats at work, their jobs, their workspaces and
// the container those run in, with what it uses now.

import Link from "next/link";
import { useClusterQueues } from "@/components/clusters/status";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { SandboxCard, useSandboxUsage } from "@/components/workspace/sandbox-card";
import { useWorkspaceList } from "@/components/workspace/workspaces-page";
import { type Intent, type IntentInfo, runningNow } from "@/lib/chat/home";
import { useConversations, useFeature } from "@/lib/queries";
import { cn } from "@/lib/utils";

export function IntentTabs({ intents, value, onChange }: { intents: IntentInfo[]; value: Intent; onChange: (intent: Intent) => void }) {
  const shown = intents.find((i) => i.id === value) ?? intents[0]!;
  return (
    <div role="radiogroup" aria-label="What this chat is for" className="flex flex-wrap items-center gap-1 px-0.5">
      {intents.map((intent) => (
        // What choosing it sets up shows on hover or focus, and is read out with the name.
        <Tooltip key={intent.id}>
          <TooltipTrigger asChild>
            <button
              type="button"
              role="radio"
              aria-checked={intent.id === shown.id}
              aria-description={intent.hint}
              data-intent={intent.id}
              onClick={() => onChange(intent.id)}
              className={cn(
                "h-8 rounded-md border px-3 text-md whitespace-nowrap transition-colors max-[560px]:px-2.5",
                intent.id === shown.id ? "border-line-strong bg-secondary font-medium text-foreground" : "border-transparent text-muted-foreground hover:bg-raised hover:text-foreground",
              )}
            >
              {intent.label}
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom" className="max-w-72">
            {intent.hint}
          </TooltipContent>
        </Tooltip>
      ))}
    </div>
  );
}

// Amber asks for the person; the accent marks what is alive, and pulses for a reply being written.
const TONE = {
  waiting: ["text-amber", "bg-amber"],
  working: ["text-text-2", "bg-signal"],
  jobs: ["text-text-2", "bg-signal"],
  workspace: ["text-text-2", "bg-signal"],
} as const;

/** Nothing when nothing is alive: Home stays as quiet as it was. */
export function RunningNow() {
  const workspacesOn = useFeature("workspaces");
  const { data: chats } = useConversations();
  const queues = useClusterQueues();
  // The person's container, while it is up: what it uses, and when it sleeps.
  const { data: usage } = useSandboxUsage(workspacesOn);
  const sandbox = usage && (usage.status === "running" || usage.status === "starting") ? usage : null;
  const items = runningNow({ chats, workspaces: useWorkspaces(workspacesOn), queues, limit: sandbox ? 5 : 6 });
  if (!items.length && !sandbox) return null;
  return (
    <section className="mt-10 max-[760px]:mt-8" aria-labelledby="running-now" data-running-now>
      <h2 id="running-now" className="mb-3.5 text-base leading-[normal] font-medium tracking-normal">
        Running now
      </h2>
      <div className="grid grid-cols-3 gap-3 max-[900px]:grid-cols-2 max-[560px]:grid-cols-1">
        {items.map((item) => (
          <Link key={item.key} href={item.href} data-running={item.kind} className="flex min-w-0 flex-col rounded-lg border bg-card px-4 py-3.5 transition-colors hover:border-line-strong hover:bg-raised">
            <span className={cn("mb-2 flex items-center gap-2 text-2xs font-medium tracking-[.08em] uppercase", TONE[item.kind][0])}>
              <span className="relative inline-flex size-1.5 shrink-0" aria-hidden="true">
                {item.kind === "working" && <span className="signal-pulse absolute inset-0 rounded-full bg-signal" />}
                <span className={cn("absolute inset-0 rounded-full", TONE[item.kind][1])} />
              </span>
              <span className="truncate">{item.eyebrow}</span>
            </span>
            <span className="truncate text-base leading-[normal] font-medium">{item.title}</span>
            {item.detail && <span className="mt-1 truncate text-sm leading-[normal] text-muted-foreground">{item.detail}</span>}
          </Link>
        ))}
        {sandbox && (
          <Link href="/workspaces" data-running="sandbox" aria-label="Your workspace container" className="flex min-w-0 flex-col rounded-lg border bg-card px-4 py-3.5 transition-colors hover:border-line-strong hover:bg-raised">
            <SandboxCard usage={sandbox} />
          </Link>
        )}
      </div>
    </section>
  );
}

function useWorkspaces(on: boolean) {
  const list = useWorkspaceList(on);
  return on ? list.data?.items : undefined;
}
