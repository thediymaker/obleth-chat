"use client";

// Home's intent tabs and "Running now". The tabs say what the new chat is for and
// set the box up for it; "Running now" lists what is alive without the person:
// chats that wait for a yes or no, chats at work, their jobs and their workspaces.

import Link from "next/link";
import { useClusterQueues } from "@/components/clusters/status";
import { useWorkspaceList } from "@/components/workspace/workspaces-page";
import { type Intent, type IntentInfo, runningNow } from "@/lib/chat/home";
import { useConversations, useFeature } from "@/lib/queries";
import { cn } from "@/lib/utils";

export function IntentTabs({ intents, value, onChange }: { intents: IntentInfo[]; value: Intent; onChange: (intent: Intent) => void }) {
  const shown = intents.find((i) => i.id === value) ?? intents[0]!;
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-0.5">
      <div role="radiogroup" aria-label="What this chat is for" className="flex flex-wrap items-center gap-1">
        {intents.map((intent) => (
          <button
            key={intent.id}
            type="button"
            role="radio"
            aria-checked={intent.id === shown.id}
            data-intent={intent.id}
            onClick={() => onChange(intent.id)}
            className={cn(
              "h-8 rounded-md border px-3 text-md whitespace-nowrap transition-colors max-[560px]:px-2.5",
              intent.id === shown.id ? "border-line-strong bg-secondary font-medium text-foreground" : "border-transparent text-muted-foreground hover:bg-raised hover:text-foreground",
            )}
          >
            {intent.label}
          </button>
        ))}
      </div>
      <p className="min-w-0 flex-1 text-sm leading-normal text-muted-foreground max-[760px]:basis-full" data-intent-hint>
        {shown.hint}
      </p>
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
  const items = runningNow({ chats, workspaces: useWorkspaces(workspacesOn), queues });
  if (!items.length) return null;
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
      </div>
    </section>
  );
}

function useWorkspaces(on: boolean) {
  const list = useWorkspaceList(on);
  return on ? list.data?.items : undefined;
}
