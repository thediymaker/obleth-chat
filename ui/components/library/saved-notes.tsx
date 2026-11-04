"use client";

// Under a reply: what the model saved with its save_item tool, with Undo. Old
// library.js cards(). The chat page renders <SavedNotes message={m} />.

import { useQueryClient } from "@tanstack/react-query";
import { Bookmark } from "lucide-react";
import { useSyncExternalStore } from "react";
import { toast } from "sonner";
import { messageOf } from "@/components/projects/form-field";
import { api } from "@/lib/api";
import { cn } from "@/lib/utils";
import { refreshSaved, savedPath } from "./api";
import { useLibraryDialogs } from "./saved-dialogs";

export interface SavedEntry {
  kind: "saved";
  status?: string;
  savedId: string;
  title: string;
  tags?: string[];
  detail?: string;
}

// Items undone in this session stay "Removed" when the reply re-renders.
const undone = new Set<string>();
const listeners = new Set<() => void>();
let version = 0;
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};
const markUndone = (id: string) => {
  undone.add(id);
  version++;
  listeners.forEach((l) => l());
};

export const savedEntries = (m: { activity?: unknown }) => (Array.isArray(m.activity) ? (m.activity as SavedEntry[]).filter((e) => e?.kind === "saved") : []);

export function SavedNotes({ message }: { message: { activity?: unknown } }) {
  useSyncExternalStore(
    subscribe,
    () => version,
    () => 0,
  );
  const client = useQueryClient();
  const library = useLibraryDialogs();
  const entries = savedEntries(message);
  if (!entries.length) return null;
  async function undo(id: string) {
    try {
      await api.request(savedPath(id), { method: "DELETE" }).catch((e: { status?: number }) => {
        if (e.status !== 404) throw e;
      });
      markUndone(id);
      refreshSaved(client);
      toast("Removed from Library");
    } catch (e) {
      toast(messageOf(e));
    }
  }
  return (
    <div className="my-2 grid gap-1.5">
      {entries.map((e, i) => (
        <div key={e.savedId || i} className={cn("flex items-center gap-2 rounded-lg border px-3 py-2 text-sm", e.status === "error" && "border-amber/40")}>
          <Bookmark className="size-4 shrink-0 text-muted-foreground" />
          {e.status === "error" ? (
            <span className="grow text-amber">Could not save: {e.detail || ""}</span>
          ) : undone.has(e.savedId) ? (
            <span className="grow text-muted-foreground">Removed “{e.title}” from Library</span>
          ) : (
            <>
              <span className="min-w-0 grow">
                Saved{" "}
                <button type="button" className="font-medium underline-offset-2 hover:underline" onClick={() => library.open(e.savedId)}>
                  “{e.title}”
                </button>{" "}
                to Library{" "}
                {(e.tags || []).map((t) => (
                  <span key={t} className="ml-1 text-xs text-faint">
                    #{t}
                  </span>
                ))}
              </span>
              <button type="button" className="text-xs text-muted-foreground hover:text-foreground" onClick={() => undo(e.savedId)}>
                Undo
              </button>
            </>
          )}
        </div>
      ))}
      {library.element}
    </div>
  );
}
