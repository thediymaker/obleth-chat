"use client";

// The steps of a reply that used workspace tools without the turn view: a folded
// list, open while a step runs or waits. The turn view (components/chat/turn-view.tsx)
// shows any reply with tool steps, so this shows only for replies saved without them.

import { ChevronDown, Orbit, Shield, Terminal } from "lucide-react";
import { useChatStore as useChat } from "@/components/chat/chat-provider";
import type { ActivityEntry, Message } from "@/lib/chat/types";
import { NamedIcon } from "@/components/chat/icon";

const HIDDEN_KINDS = new Set(["sources", "thought", "saved", "question", "workspace", "research", "project", "browser-request", "browser-handoff"]);
const TOOL_ICONS: Record<string, string> = { sync_code: "branch", write_file: "edit", edit_file: "edit", read_file: "file", list_files: "grid", run_command: "terminal", start_server: "play", server_logs: "file" };
const STEP_STATE: Record<string, string> = { running: "Running…", awaiting: "Needs your answer", error: "Failed", declined: "Declined" };

export const activitySteps = (m: Message) =>
  (Array.isArray(m.activity) ? m.activity : []).filter((s) => !HIDDEN_KINDS.has(s.kind || "") && s.tool !== "search_knowledge" && s.tool !== "save_item");

function Step({ s }: { s: ActivityEntry }) {
  const store = useChat();
  const time = STEP_STATE[s.status || ""] || (s.durationMs != null ? `${(s.durationMs / 1000).toFixed(1)}s` : "");
  return (
    <li className="flex gap-2 py-1">
      <NamedIcon name={s.connection ? "link" : TOOL_ICONS[s.tool || ""] || "terminal"} className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
      <div className="min-w-0 flex-1 text-md">
        <span>{s.summary}</span>
        {s.detail && s.status !== "running" && <pre className="mt-1 max-h-48 overflow-auto rounded bg-code px-2 py-1 font-mono text-[12px] whitespace-pre-wrap">{s.detail}</pre>}
        {s.status === "awaiting" && (
          <div className="mt-1.5 flex flex-wrap items-center gap-1.5 text-sm">
            <span className="flex-1">{s.question || `Allow ${s.connection || "this tool"} to run this?`}</span>
            {(["allow", "chat", "deny"] as const).map((d) => (
              <button key={d} type="button" className="rounded-md border px-2 py-0.5" title={d === "chat" ? `Run ${s.summary} without asking for the rest of this conversation` : undefined} onClick={() => store.decide(s.id!, d)}>
                {d === "allow" ? (s.question ? "Allow once" : "Allow") : d === "chat" ? s.remember || "Allow for this chat" : "Deny"}
              </button>
            ))}
          </div>
        )}
      </div>
      <small className="text-xs text-faint">{time}</small>
    </li>
  );
}

export function MessageActivity({ m }: { m: Message }) {
  const store = useChat();
  const steps = activitySteps(m);
  if (!steps.length) return null;
  const live = steps.find((s) => s.status === "awaiting") || steps.find((s) => s.status === "running"),
    failed = steps.filter((s) => s.status === "error").length;
  const kind = steps.every((s) => !s.connection) ? "workspace step" : "step";
  const label = live ? (live.status === "awaiting" ? `Waiting for you: ${live.summary}` : live.summary) : `${steps.length} ${kind}${steps.length === 1 ? "" : "s"}${failed ? ` · ${failed} failed` : ""}`;
  const Icon = live?.status === "running" ? Orbit : live ? Shield : Terminal;
  return (
    <details
      className="group/activity mb-3 rounded-md border"
      open={!!live || (!!m.id && store.openActivity.has(m.id))}
      onToggle={(e) => {
        if (!m.id) return;
        if ((e.target as HTMLDetailsElement).open) store.openActivity.add(m.id);
        else store.openActivity.delete(m.id);
      }}
    >
      <summary className="flex cursor-pointer list-none items-center gap-2 px-3 py-2 text-sm [&::-webkit-details-marker]:hidden">
        <Icon className="size-4 text-muted-foreground" />
        <span className="min-w-0 flex-1 truncate">{label}</span>
        <ChevronDown className="size-4 text-muted-foreground transition-transform group-open/activity:rotate-180" />
      </summary>
      <ol className="border-t px-3 py-1">
        {steps.map((s, i) => (
          <Step key={s.id || i} s={s} />
        ))}
      </ol>
    </details>
  );
}
