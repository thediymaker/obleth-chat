"use client";

// New project, New app, Edit project and "Start a project from this chat": one form.

import { useQueryClient } from "@tanstack/react-query";
import { ChevronRight, Download, Trash2 } from "lucide-react";
import { useRouter } from "next/navigation";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { createApp } from "@/components/apps/api";
import { projectDot } from "@/components/app-shell/sidebar";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { api } from "@/lib/api";
import { count } from "@/lib/format";
import { keys, useFeature, useProjects } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { COLORS, moveChat, type Project, type ProjectColor, projectPath, refreshProjects } from "./api";
import { Confirm, FormError, FormField, messageOf } from "./form-field";

const ONLY_ME = "personal";

export interface ProjectDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The project to edit; none makes a new one. */
  project?: Project;
  /** Makes an app: a new project, then its app, then its App tab. */
  asApp?: boolean;
  /** The chat to move into the new project ("Start a project from this chat"). */
  fromChat?: string;
}

/** Copies a repository into a project's code. Throws with the reason it couldn't. */
export async function importRepository(projectId: string, url: string, token: string) {
  const r = await api.request<{ history: boolean; commits: number }>(`${projectPath(projectId)}/code/import`, { method: "POST", body: { url, ...(token ? { token } : {}) } });
  toast(r.history ? `Copied ${count(r.commits, "commit")} from the repository` : "Copied the repository’s latest version; its history was too big to keep");
  return r;
}

export function ProjectDialog({ open, onOpenChange, project, asApp = false, fromChat }: ProjectDialogProps) {
  const router = useRouter();
  const client = useQueryClient();
  const { data } = useProjects();
  const codeOn = useFeature("code");
  const groups0 = data?.groups ?? [];
  const editing = !!project;
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [instructions, setInstructions] = useState("");
  const [color, setColor] = useState<ProjectColor>("blue");
  const [share, setShare] = useState(ONLY_ME);
  const [repo, setRepo] = useState("");
  const [token, setToken] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState("");
  const [deleting, setDeleting] = useState(false);

  useEffect(() => {
    if (!open) return;
    setName(project?.name ?? "");
    setDescription(project?.description ?? "");
    setInstructions(project?.instructions ?? "");
    setColor(project?.color ?? (asApp ? "green" : COLORS[(data?.projects?.length ?? 0) % COLORS.length]!));
    setShare(project?.scope === "group" && project.groupId ? project.groupId : ONLY_ME);
    setRepo("");
    setToken("");
    setError("");
    setBusy("");
    // eslint-disable-next-line react-hooks/exhaustive-deps -- reset each time it opens
  }, [open, project?.id]);

  // The group it is shared with stays a choice even after its owner leaves that group.
  const groups =
    project?.scope === "group" && project.groupId && !groups0.some((g) => g.id === project.groupId) ? [...groups0, { id: project.groupId, name: project.groupName || "Current group" }] : groups0;
  const withRepo = !editing && !fromChat && !asApp && codeOn;

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setError("");
    setBusy("saving");
    const body = {
      name: name.trim(),
      description: description.trim(),
      instructions,
      color,
      scope: share === ONLY_ME ? "personal" : "group",
      ...(share === ONLY_ME ? {} : { groupId: share }),
    };
    try {
      const r = await api.request<{ project: Project }>(editing ? projectPath(project.id) : "/api/projects", { method: editing ? "PATCH" : "POST", body });
      const id = r.project.id;
      refreshProjects(client, id);
      const url = withRepo ? repo.trim() : "";
      // Started from a repository: the project exists either way; a failed copy is
      // offered again from its Code tab.
      if (url) {
        setBusy("copying");
        let failed = "";
        try {
          await importRepository(id, url, token.trim());
        } catch (e) {
          failed = messageOf(e);
        }
        onOpenChange(false);
        router.push(`/projects/${encodeURIComponent(id)}/code`);
        if (failed) toast(`Project created, but the repository wasn’t copied: ${failed}`);
        return;
      }
      // A new app: the project exists either way; a failed app is offered again from its tab.
      if (!editing && asApp) {
        onOpenChange(false);
        try {
          await createApp(id);
          toast("App created");
        } catch (e) {
          toast(`Project created, but the app wasn’t: ${messageOf(e)}`);
        }
        router.push(`/projects/${encodeURIComponent(id)}/app`);
        return;
      }
      // Started from a chat: the chat moves into the new project and stays open.
      if (!editing && fromChat) {
        const moved = await moveChat(client, fromChat, id);
        onOpenChange(false);
        toast(moved ? `Moved to ${moved.name}` : "Removed from the project");
        return;
      }
      onOpenChange(false);
      toast(editing ? "Saved your changes" : "Project created");
      if (!editing) router.push(`/projects/${encodeURIComponent(id)}`);
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy("");
    }
  }

  async function remove() {
    await api.request(projectPath(project!.id), { method: "DELETE" });
    onOpenChange(false);
    toast("Project deleted");
    client.invalidateQueries({ queryKey: keys.projects });
    client.removeQueries({ queryKey: keys.project(project!.id) });
    router.push("/projects");
  }

  const title = editing ? "Edit project" : asApp ? "New app" : "New project";
  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="max-h-[92dvh] overflow-y-auto sm:max-w-xl">
          <DialogHeader>
            <DialogTitle>{title}</DialogTitle>
            <DialogDescription className={asApp && !editing ? "" : "sr-only"}>
              {asApp && !editing
                ? "An app is a project whose steps answer over an API. Name it here, then lay out its steps, try it and publish it from its App tab."
                : "A project gives its chats standing instructions and the collections to search."}
            </DialogDescription>
          </DialogHeader>
          <form id="project-form" onSubmit={submit} className="grid gap-4">
            <FormField id="project-name" label="Name">
              <Input id="project-name" value={name} onChange={(e) => setName(e.target.value)} maxLength={120} required placeholder="Gaudi benchmarks" autoFocus />
            </FormField>
            <FormField id="project-description" label="Description">
              <Input id="project-description" value={description} onChange={(e) => setDescription(e.target.value)} maxLength={1000} placeholder="What is it for?" />
            </FormField>
            <FormField id="project-instructions" label="Instructions for every chat" help="Added to the start of each chat in the project.">
              <Textarea
                id="project-instructions"
                value={instructions}
                onChange={(e) => setInstructions(e.target.value)}
                rows={6}
                maxLength={20000}
                placeholder="We are benchmarking LLM inference on Gaudi 3. Report tokens per second with batch size and model."
              />
            </FormField>
            <div className="grid gap-1.5">
              <span id="project-color-label" className="text-sm text-text-2">
                Color
              </span>
              <div role="radiogroup" aria-labelledby="project-color-label" className="flex gap-2">
                {COLORS.map((c) => (
                  <button
                    key={c}
                    type="button"
                    role="radio"
                    aria-checked={color === c}
                    aria-label={c}
                    title={c}
                    onClick={() => setColor(c)}
                    className={cn("flex size-7 items-center justify-center rounded-full border border-transparent", color === c && "border-foreground/60")}
                  >
                    <span className={cn("size-4 rounded-full", projectDot(c))} />
                  </button>
                ))}
              </div>
            </div>
            <FormField
              id="project-share"
              label="Shared with"
              help={groups.length ? "Group members see the project and can start their own chats in it." : "You are not in any groups. An administrator adds people to groups."}
            >
              <Select value={share} onValueChange={setShare}>
                <SelectTrigger id="project-share" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={ONLY_ME}>Only me</SelectItem>
                  {groups.map((g) => (
                    <SelectItem key={g.id} value={g.id}>
                      {g.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </FormField>
            {withRepo && (
              <div className="grid gap-3 rounded-md border border-dashed p-3">
                <FormField id="code-repo-url" label="Start from a repository (optional)" help="Chat copies its default branch once, with its history, as the project’s code.">
                  <Input
                    id="code-repo-url"
                    type="url"
                    value={repo}
                    onChange={(e) => setRepo(e.target.value)}
                    maxLength={300}
                    autoComplete="off"
                    spellCheck={false}
                    placeholder="https://github.com/owner/name"
                  />
                </FormField>
                <Collapsible>
                  <CollapsibleTrigger className="group flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground">
                    <ChevronRight className="size-3.5 transition-transform group-data-[state=open]:rotate-90" /> Private repository?
                  </CollapsibleTrigger>
                  <CollapsibleContent className="pt-3">
                    <FormField id="code-repo-token" label="Access token" help="Used for this copy only and never kept. It needs read access to the repository’s contents.">
                      <Input id="code-repo-token" type="password" value={token} onChange={(e) => setToken(e.target.value)} autoComplete="off" spellCheck={false} placeholder="github_pat_…" />
                    </FormField>
                  </CollapsibleContent>
                </Collapsible>
              </div>
            )}
            <FormError message={error} />
            {editing && (
              <div className="flex items-center gap-3 rounded-md border border-destructive/30 p-3">
                <div className="grow">
                  <strong className="block text-md font-medium">Delete project</strong>
                  <small className="text-xs text-muted-foreground">Its chats and saved items are kept, outside any project.</small>
                </div>
                <Button type="button" size="sm" variant="outline" className="text-destructive-foreground" onClick={() => setDeleting(true)}>
                  <Trash2 /> Delete
                </Button>
              </div>
            )}
            <DialogFooter>
              <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
                Cancel
              </Button>
              <Button type="submit" disabled={!!busy}>
                {busy === "copying" ? (
                  <>
                    <Download /> Copying the repository…
                  </>
                ) : editing ? (
                  "Save changes"
                ) : asApp ? (
                  "Create app"
                ) : (
                  "Create project"
                )}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
      {project && (
        <Confirm
          open={deleting}
          onOpenChange={setDeleting}
          title="Delete this project?"
          text="The project, its instructions and its collection choices are deleted. Its chats and saved items are kept, outside any project."
          cancel="Keep project"
          confirm="Delete project"
          onConfirm={remove}
        />
      )}
    </>
  );
}

/** For the chat page: "Start a project from this chat" when `fromChat` is set, else New project. */
export function NewProjectDialog(props: Omit<ProjectDialogProps, "project" | "asApp">) {
  return <ProjectDialog {...props} />;
}
