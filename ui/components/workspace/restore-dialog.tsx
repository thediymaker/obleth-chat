"use client";

// A checkpoint's Restore: asks first, restores, then offers Undo. The turn view opens
// it with store.workspace.restoreCheckpoint(id, sha, label); the composer's
// Workspace button renders it, so it is there on every chat page.

import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { useChat } from "@/components/chat/chat-provider";

export function RestoreDialog() {
  const store = useChat();
  const bridge = store.workspace;
  const d = bridge.restoreDialog();
  return (
    <AlertDialog open={!!d} onOpenChange={(open) => !open && bridge.closeRestore()}>
      <AlertDialogContent>
        {d?.step === "done" ? (
          <>
            <AlertDialogHeader>
              <AlertDialogTitle>Checkpoint restored</AlertDialogTitle>
              <AlertDialogDescription>The project is back at {d.label || "the checkpoint"}. The version you had is saved, so you can go back to it.</AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel
                disabled={d.busy || !d.before}
                onClick={(e) => {
                  e.preventDefault();
                  void bridge.undoRestore();
                }}
              >
                Undo
              </AlertDialogCancel>
              <AlertDialogAction onClick={() => bridge.closeRestore()}>Done</AlertDialogAction>
            </AlertDialogFooter>
          </>
        ) : (
          <>
            <AlertDialogHeader>
              <AlertDialogTitle>Restore {d?.label || "this checkpoint"}?</AlertDialogTitle>
              <AlertDialogDescription>
                The project goes back to how it was at this checkpoint: changed files are restored and files added since are removed. Installed packages stay. The current state is saved first, so you can come back to it.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel disabled={d?.busy}>Cancel</AlertDialogCancel>
              <AlertDialogAction
                disabled={d?.busy}
                onClick={(e) => {
                  e.preventDefault();
                  void bridge.confirmRestore();
                }}
              >
                {d?.busy ? "Restoring…" : "Restore"}
              </AlertDialogAction>
            </AlertDialogFooter>
          </>
        )}
      </AlertDialogContent>
    </AlertDialog>
  );
}
