"use client";

// New project, New app, Edit project and "Start a project from this chat": one form.

import { useQueryClient } from "@tanstack/react-query";
import { Download, Trash2 } from "lucide-react";
import { useRouter } from "next/navigation";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { createApp } from "@/components/apps/api";
import { RepoFields, importRepo } from "@/components/code/export-dialog";
import { projectDot } from "@/components/app-shell/sidebar";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { api } from "@/lib/api";
import { keys, useFeature, useProjects } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { COLORS, moveChat, type Project, type ProjectColor, projectPath, refreshProjects } from "./api";
import { Confirm, Field, FormActions, FormDialog, FormError, messageOf } from "@/components/manage/kit";

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
  /** What a new project's form starts with, such as a project the model suggested. */
  initial?: { name?: string; description?: string; instructions?: string };
  /** Called once a chat has moved into the new project. */
  onMoved?: (project: Project) => void;
}

export function ProjectDialog({ open, onOpenChange, project, asApp = false, fromChat, initial, onMoved }: ProjectDialogProps) {
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
  const [error, setError] = useState("");
  const [busy, setBusy] = useState("");
  const [deleting, setDeleting] = useState(false);

  useEffect(() => {
    if (!open) return;
    setName(project?.name ?? initial?.name ?? "");
    setDescription(project?.description ?? initial?.description ?? "");
    setInstructions(project?.instructions ?? initial?.instructions ?? "");
    setColor(project?.color ?? (asApp ? "green" : COLORS[(data?.projects?.length ?? 0) % COLORS.length]!));
    setShare(project?.scope === "group" && project.groupId ? project.groupId : ONLY_ME);
    setError("");
    setBusy("");
    // eslint-disable-next-line react-hooks/exhaustive-deps -- reset each time it opens
  }, [open, project?.id]);

  // The group it is shared with stays a choice even after its owner leaves that group.
  const groups =
    project?.scope === "group" && project.groupId && !groups0.some((g) => g.id === project.groupId) ? [...groups0, { id: project.groupId, name: project.groupName || "Current group" }] : groups0;
  const withRepo = !editing && !fromChat && !asApp && codeOn;

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
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
      const url = withRepo ? String(form.get("code-repo-url") || "").trim() : "";
      // Started from a repository: the project exists either way; a failed copy is
      // offered again from its Code tab.
      if (url) {
        setBusy("copying");
        let failed = "";
        try {
          await importRepo(id, url, String(form.get("code-repo-token") || "").trim());
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
        if (moved) onMoved?.(moved);
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
      <FormDialog
        open={open}
        onOpenChange={onOpenChange}
        title={title}
        description={asApp && !editing ? "An app is a project whose steps answer over an API. Name it here, then lay out its steps, try it and publish it from its App tab." : undefined}
        className="sm:max-w-xl"
      >
        <form id="project-form" onSubmit={submit} className="grid gap-4">
          <Field id="project-name" label="Name">
            <Input id="project-name" value={name} onChange={(e) => setName(e.target.value)} maxLength={120} required placeholder="Gaudi benchmarks" autoFocus />
          </Field>
          <Field id="project-description" label="Description">
            <Input id="project-description" value={description} onChange={(e) => setDescription(e.target.value)} maxLength={1000} placeholder="What is it for?" />
          </Field>
          <Field id="project-instructions" label="Instructions for every chat" help="Added to the start of each chat in the project.">
            <Textarea
              id="project-instructions"
              value={instructions}
              onChange={(e) => setInstructions(e.target.value)}
              rows={6}
              maxLength={20000}
              placeholder="We are benchmarking LLM inference on Gaudi 3. Report tokens per second with batch size and model."
            />
          </Field>
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
          <Field
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
          </Field>
          {withRepo && (
            <div className="rounded-md border border-dashed p-3">
              <RepoFields optional />
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
          <FormActions>
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
          </FormActions>
        </form>
      </FormDialog>
      {project && (
        <Confirm
          open={deleting}
          onOpenChange={setDeleting}
          title="Delete this project?"
          text="The project, its instructions and its collection choices are deleted. Its chats and saved items are kept, outside any project."
          cancel="Keep project"
          label="Delete project"
          action={remove}
        />
      )}
    </>
  );
}

/** For the chat page: "Start a project from this chat" when `fromChat` is set, else New project. */
export function NewProjectDialog(props: Omit<ProjectDialogProps, "project" | "asApp">) {
  return <ProjectDialog {...props} />;
}
