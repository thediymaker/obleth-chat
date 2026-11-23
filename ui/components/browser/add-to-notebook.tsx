"use client";

// "Add this page to a notebook" beside a chat's live browser: the page its tab shows
// now, as it is rendered there (so a page the person signed in to or reached by hand
// is kept as they see it), goes into the notebook of the chat's project, or of a
// project the person picks when the chat is in none.

import { useQueryClient } from "@tanstack/react-query";
import { BookPlus } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { messageOf } from "@/components/manage/kit";
import { addTabPage, notebookHref, notebookKey } from "@/components/notebook/api";
import { useConversationProject } from "@/components/projects/api";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { useFeature, useProjects } from "@/lib/queries";

export function AddToNotebook({ conversationId, url }: { conversationId: string; url: string }) {
  const knowledge = useFeature("knowledge");
  const client = useQueryClient();
  const project = useConversationProject(conversationId);
  const projects = useProjects();
  const [busy, setBusy] = useState(false);
  if (!knowledge || !/^https?:\/\//i.test(url) || project.isPending) return null;

  async function add(projectId: string | undefined, name: string) {
    setBusy(true);
    try {
      const added = await addTabPage(conversationId, projectId);
      client.invalidateQueries({ queryKey: notebookKey(added.projectId) });
      toast(`Added to the notebook of ${name}. It is indexed in the background.`, {
        action: { label: "Open", onClick: () => window.location.assign(notebookHref(added.projectId)) },
      });
    } catch (e) {
      toast(messageOf(e));
    } finally {
      setBusy(false);
    }
  }

  const label = busy ? "Adding…" : "Add this page to a notebook";
  if (project.data)
    return (
      <Button size="xs" variant="ghost" disabled={busy} onClick={() => add(undefined, project.data!.name)}>
        <BookPlus /> {label}
      </Button>
    );
  const list = projects.data?.projects ?? [];
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button size="xs" variant="ghost" disabled={busy}>
          <BookPlus /> {label}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" aria-label="Notebooks">
        <DropdownMenuLabel>This chat is in no project. Add the page to:</DropdownMenuLabel>
        {list.map((p) => (
          <DropdownMenuItem key={p.id} onSelect={() => void add(p.id, p.name)}>
            {p.name}
          </DropdownMenuItem>
        ))}
        {!list.length && <DropdownMenuItem disabled>{projects.isPending ? "Loading projects…" : "No projects yet"}</DropdownMenuItem>}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
