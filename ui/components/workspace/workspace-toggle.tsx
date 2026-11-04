"use client";

// The composer dock's Workspace button. The wiring
// is real: on a new chat it marks the next chat to get a workspace; in a chat it
// turns one on, or shows and hides its tabs. It also holds the checkpoint Restore
// dialog, so the dialog is on every page with a composer.

import { Terminal } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useChat } from "@/components/chat/chat-provider";
import { cn } from "@/lib/utils";
import { RestoreDialog } from "./restore-dialog";

export function WorkspaceToggle() {
  const store = useChat();
  if (!store.workspace.enabled()) return null;
  const id = store.route.id;
  const on = id ? store.workspace.has(id) : store.workspaceDraft;
  const shown = on && !!id && ["preview", "files", "terminal"].some((tab) => store.showing(id, tab));
  return (
    <>
    <Button
      type="button"
      variant="ghost"
      size="xs"
      aria-pressed={on}
      className={cn("rounded-full", on && "bg-secondary text-foreground", shown && "ring-1 ring-line-strong")}
      title={on ? "This conversation has a workspace: the model can create files, run commands and show a live preview" : "Give the model a private Linux workspace with a live preview"}
      disabled={store.streaming}
      onClick={() => store.run(() => store.toggleWorkspace())}
    >
      <Terminal />
      <span className="hidden sm:inline">Workspace</span>
    </Button>
    <RestoreDialog />
    </>
  );
}
