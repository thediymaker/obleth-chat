"use client";

// Which collections every chat in a project searches. Old projects.js collectionsPicker.

import { useQueryClient } from "@tanstack/react-query";
import { BookOpen } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { EmptyState, ErrorBox, Loading } from "@/components/app-shell/page";
import { useCollections } from "@/components/knowledge/api";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { api } from "@/lib/api";
import { count } from "@/lib/format";
import { keys } from "@/lib/queries";
import { MAX_COLLECTIONS, type ProjectDetail, projectPath } from "./api";
import { messageOf } from "./form-field";

export function CollectionsPicker({ open, onOpenChange, detail }: { open: boolean; onOpenChange: (open: boolean) => void; detail: ProjectDetail }) {
  const client = useQueryClient();
  const { data, error, isPending, refetch } = useCollections(open);
  const [busy, setBusy] = useState("");
  const on = new Set(detail.collections.map((c) => c.id));

  async function toggle(id: string) {
    const current = detail.collections.map((c) => c.id);
    const next = current.includes(id) ? current.filter((c) => c !== id) : [...current, id];
    if (next.length > MAX_COLLECTIONS) {
      toast(`A project can use up to ${MAX_COLLECTIONS} collections.`);
      return;
    }
    setBusy(id);
    try {
      await api.request(`${projectPath(detail.project.id)}/collections`, { method: "PUT", body: { collectionIds: next } });
      await client.invalidateQueries({ queryKey: keys.project(detail.project.id) });
    } catch (e) {
      toast(messageOf(e));
    } finally {
      setBusy("");
    }
  }

  const all = data?.collections ?? [];
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[92dvh] overflow-y-auto sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Collections for this project</DialogTitle>
          <DialogDescription>
            Every chat in the project searches these, along with any it picks itself.
            {detail.project.scope === "group" &&
              " Group members can read your own collections through the project. A collection someone else shared with you reaches only the people it is already shared with."}
          </DialogDescription>
        </DialogHeader>
        <ErrorBox error={error} onRetry={() => refetch()} />
        {isPending ? (
          <Loading />
        ) : all.length ? (
          <div className="grid divide-y rounded-md border">
            {all.map((c) => (
              <div key={c.id} className="flex items-center gap-3 px-3 py-2.5">
                <BookOpen className="size-4 shrink-0 text-muted-foreground" />
                <div className="min-w-0 grow">
                  <h3 className="truncate text-md font-medium">{c.name}</h3>
                  <p className="text-xs text-muted-foreground">{count(c.documents, "document")}</p>
                </div>
                <Button size="sm" variant={on.has(c.id) ? "outline" : "default"} aria-pressed={on.has(c.id)} disabled={busy === c.id} onClick={() => toggle(c.id)}>
                  {on.has(c.id) ? "On" : "Off"}
                </Button>
              </div>
            ))}
          </div>
        ) : (
          !error && <EmptyState icon={BookOpen} title="No collections yet" text="Make one in Library → Collections and add documents to it." />
        )}
        <DialogFooter>
          <Button onClick={() => onOpenChange(false)}>Done</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
