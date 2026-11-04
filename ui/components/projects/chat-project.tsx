"use client";

// The project pieces of the chat page: where a chat lives (the pill and its menu in
// the chat line, the crumb, the Context panel section, the composer chip of a new
// chat) and the dialogs to move it or start a project from it. A chat that does not
// exist yet (conversationId null) uses the draft from chat-draft.ts.

import { useQueryClient } from "@tanstack/react-query";
import { ArrowRight, ChevronDown, ChevronRight, Folder, Plus, X } from "lucide-react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useState } from "react";
import { toast } from "sonner";
import { Eyebrow } from "@/components/app-shell/page";
import { projectDot } from "@/components/app-shell/sidebar";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { useProjects } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { moveChat, type Project, projectLine, useConversationProject } from "./api";
import { chatDraft, useChatDraft } from "./chat-draft";
import { messageOf } from "./form-field";
import { NewProjectDialog } from "./project-dialog";

export const ProjectDot = ({ color, className }: { color: string; className?: string }) => <span className={cn("size-2 shrink-0 rounded-full", projectDot(color), className)} />;

/** The project a chat is in: the saved chat's, or a new chat's draft. Undefined while loading. */
export function useChatProject(conversationId: string | null): Project | null | undefined {
  const { data, isPending } = useConversationProject(conversationId);
  const draft = useChatDraft();
  const { data: list } = useProjects();
  if (conversationId) return isPending ? undefined : (data ?? null);
  return ((list?.projects as Project[] | undefined) ?? []).find((p) => p.id === draft.project) ?? null;
}
/** The chat's project id, for the save dialog's default. */
export function useChatProjectId(conversationId: string | null): string {
  return useChatProject(conversationId)?.id ?? "";
}

/** "Gaudi benchmarks /" before the chat's title, linking to its project. */
export function ProjectCrumb({ conversationId }: { conversationId: string | null }) {
  const p = useChatProject(conversationId);
  if (!p) return null;
  return (
    <>
      <Link href={`/projects/${encodeURIComponent(p.id)}`} className="inline-flex min-w-0 items-center gap-1.5 text-muted-foreground hover:text-foreground">
        <ProjectDot color={p.color} />
        <span className="truncate">{p.name}</span>
      </Link>
      <span className="text-faint">/</span>
    </>
  );
}

/**
 * The chat line's project pill: where the chat lives, with a menu to open the project,
 * move the chat, or start a project from it. `children` adds menu items after those
 * (such as the chat page's "Chat details").
 */
export function ProjectPill({ conversationId, children, className }: { conversationId: string | null; children?: React.ReactNode; className?: string }) {
  const p = useChatProject(conversationId);
  const router = useRouter();
  const [moving, setMoving] = useState(false);
  const [starting, setStarting] = useState(false);
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            title={p ? `In ${p.name}` : "Not in a project"}
            className={cn(
              "inline-flex h-7 max-w-56 items-center gap-1.5 rounded-full border bg-background px-2.5 text-sm text-text-2 hover:bg-accent data-[state=open]:bg-accent",
              className,
            )}
          >
            {p ? <ProjectDot color={p.color} /> : <Folder className="size-3.5" />}
            <span className="truncate">{p?.name || "Personal"}</span>
            <ChevronDown className="size-3.5 text-muted-foreground" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-60">
          {p && (
            <DropdownMenuItem onSelect={() => router.push(`/projects/${encodeURIComponent(p.id)}`)}>
              <ArrowRight /> <span className="truncate">Open {p.name}</span>
            </DropdownMenuItem>
          )}
          <DropdownMenuItem onSelect={() => setMoving(true)}>
            <Folder /> {p ? "Change project…" : "Move to project…"}
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => setStarting(true)}>
            <Plus /> Start a project from this chat
          </DropdownMenuItem>
          {children && <DropdownMenuSeparator />}
          {children}
        </DropdownMenuContent>
      </DropdownMenu>
      <MoveToProjectDialog open={moving} onOpenChange={setMoving} conversationId={conversationId} />
      <NewProjectDialog open={starting} onOpenChange={setStarting} fromChat={conversationId ?? undefined} />
    </>
  );
}

/** The Context panel's Project section. Hidden when the person has no projects and the chat is in none. */
export function ProjectContextSection({ conversationId }: { conversationId: string | null }) {
  const p = useChatProject(conversationId);
  const { data } = useProjects();
  const [moving, setMoving] = useState(false);
  if (!p && !data?.projects?.length) return null;
  return (
    <section className="grid gap-2">
      <Eyebrow>Project</Eyebrow>
      {p ? (
        <Link href={`/projects/${encodeURIComponent(p.id)}`} className="flex items-center gap-2.5 rounded-md border bg-card px-3 py-2 hover:bg-accent">
          <ProjectDot color={p.color} />
          <span className="min-w-0 grow">
            <strong className="block truncate text-md font-medium">{p.name}</strong>
            {p.instructions && <span className="block text-xs text-muted-foreground">Instructions apply to this chat.</span>}
          </span>
          <ChevronRight className="size-4 text-muted-foreground" />
        </Link>
      ) : (
        <p className="text-sm text-muted-foreground">Move this chat into a project to give it the project’s instructions, collections and saved items.</p>
      )}
      <Button variant="outline" size="sm" className="justify-self-start" onClick={() => setMoving(true)}>
        <Folder /> {p ? "Change project" : "Move to project"}
      </Button>
      <MoveToProjectDialog open={moving} onOpenChange={setMoving} conversationId={conversationId} />
    </section>
  );
}

/** "New chat in <project>" over a new chat's composer, with a way out of the project. */
export function ProjectComposerChip({ onClear }: { onClear?: () => void }) {
  const p = useChatProject(null);
  if (!p) return null;
  return (
    <div className="flex items-center gap-2 text-sm text-muted-foreground">
      <ProjectDot color={p.color} />
      <span className="truncate">
        New chat in <strong className="font-medium text-foreground">{p.name}</strong>
      </span>
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label="Start outside the project"
        title="Start outside the project"
        onClick={() => {
          chatDraft.setProject(null);
          onClear?.();
        }}
      >
        <X />
      </Button>
    </div>
  );
}

/** Pick the project a chat lives in. A new chat keeps the choice until it is sent. */
export function MoveToProjectDialog({ open, onOpenChange, conversationId }: { open: boolean; onOpenChange: (open: boolean) => void; conversationId: string | null }) {
  const client = useQueryClient();
  const { data } = useProjects();
  const current = useChatProject(conversationId);
  const [busy, setBusy] = useState(false);
  const [starting, setStarting] = useState(false);
  const all = (data?.projects ?? []) as Project[];

  async function place(projectId: string | null) {
    if (!conversationId) {
      chatDraft.setProject(projectId);
      onOpenChange(false);
      return;
    }
    setBusy(true);
    try {
      const p = await moveChat(client, conversationId, projectId);
      onOpenChange(false);
      toast(p ? `Moved to ${p.name}` : "Removed from the project");
    } catch (e) {
      toast(messageOf(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="max-h-[92dvh] overflow-y-auto sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>Move to project</DialogTitle>
            <DialogDescription>The chat gets the project’s instructions and collections from the next message.</DialogDescription>
          </DialogHeader>
          {all.length > 0 && (
            <div className="grid divide-y rounded-md border">
              {all.map((x) => {
                const here = current?.id === x.id;
                return (
                  <div key={x.id} className="flex items-center gap-3 px-3 py-2.5">
                    <ProjectDot color={x.color} className="size-2.5" />
                    <div className="min-w-0 grow">
                      <h3 className="truncate text-md font-medium">{x.name}</h3>
                      <p className="truncate text-xs text-muted-foreground">{projectLine(x)}</p>
                    </div>
                    <Button size="sm" variant={here ? "outline" : "default"} disabled={here || busy} onClick={() => place(x.id)}>
                      {here ? "Here now" : "Move here"}
                    </Button>
                  </div>
                );
              })}
            </div>
          )}
          <DialogFooter>
            {current && (
              <Button variant="ghost" disabled={busy} onClick={() => place(null)}>
                Remove from project
              </Button>
            )}
            <Button
              variant="outline"
              onClick={() => {
                onOpenChange(false);
                setStarting(true);
              }}
            >
              <Plus /> New project
            </Button>
            <Button onClick={() => onOpenChange(false)}>Done</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <NewProjectDialog open={starting} onOpenChange={setStarting} />
    </>
  );
}
