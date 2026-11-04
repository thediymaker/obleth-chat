"use client";

// Above a new chat that starts from a project's code at a commit: where its workspace
// starts, with a button to start from the latest code instead. Rendered in the
// composer, under the project chip.

import { GitBranch, X } from "lucide-react";
import { useSyncExternalStore } from "react";
import { useChat } from "@/components/chat/chat-provider";
import { short } from "./model";

export function CodeChip() {
  const store = useChat();
  const code = store.code;
  const start = useSyncExternalStore(code.subscribe, code.start, () => null);
  if (!start || store.active) return null;
  return (
    <div className="flex max-w-full items-center gap-1.5 self-start rounded-full border bg-card py-0.5 pr-0.5 pl-2.5 text-sm text-text-2">
      <GitBranch className="size-3.5 shrink-0 text-muted-foreground" />
      <span className="min-w-0 truncate">
        Workspace starts at <code className="font-mono text-xs">{short(start.sha)}</code> {start.subject || ""}
      </span>
      <button type="button" className="grid size-5 shrink-0 place-items-center rounded-full text-muted-foreground hover:bg-accent hover:text-foreground" aria-label="Start from the latest code" title="Start from the latest code" onClick={() => code.setStart(null)}>
        <X className="size-3" />
      </button>
    </div>
  );
}
