"use client";

// Compute › Workspaces: every chat with a
// workspace is a project folder in the person's own container. Open one to browse
// its files, use a terminal or download it; delete one to free its space.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, MessageSquare, Play, Plus, Server, Terminal, Trash2 } from "lucide-react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { toast } from "sonner";
import { usePageChrome } from "@/components/app-shell/chrome";
import { ErrorBox, Loading, Page, PageHeader } from "@/components/app-shell/page";
import { useChat, useChatStore } from "@/components/chat/chat-provider";
import { Pill, Quota, useConfirm } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/api";
import { ago } from "@/lib/format";
import { useCapabilities, useFeature } from "@/lib/queries";
import { ComputeOff, ComputeTabs } from "./compute";
import { type Workspace, diskUsage, downloadUrl, statusLabel, statusTone } from "./model";

export const workspacesKey = ["compute-workspaces"] as const;

export function useWorkspaceList() {
  const store = useChatStore();
  return useQuery({
    queryKey: workspacesKey,
    queryFn: async () => {
      const r = await api.request<{ workspaces?: Workspace[]; diskLimit?: number }>("/api/workspaces");
      store.workspace.setDiskLimit(r.diskLimit);
      return { items: r.workspaces || [], diskLimit: r.diskLimit || 0 };
    },
  });
}

/** A new chat with a workspace and the start of a prompt (the old "starter" action). */
export function useStartBuilding() {
  const store = useChatStore();
  const router = useRouter();
  return () => {
    store.newChat();
    store.workspaceDraft = true;
    store.starter("Build me a site for ", true);
    router.push("/");
  };
}

export function WorkspacesPage() {
  usePageChrome("Compute");
  const caps = useCapabilities();
  const on = useFeature("workspaces");
  const previews = caps.data?.features?.previews === true;
  const list = useWorkspaceList();
  const queries = useQueryClient();
  const store = useChat();
  const confirm = useConfirm();
  const start = useStartBuilding();
  if (caps.data && !on)
    return (
      <Page>
        <PageHeader title="Compute">
          <ComputeTabs />
        </PageHeader>
        <ComputeOff page="workspaces" />
      </Page>
    );
  const items = list.data?.items;
  const limit = list.data?.diskLimit || 0;
  const newProject = (
    <Button size="sm" onClick={start}>
      <Plus /> New project
    </Button>
  );
  const remove = (p: Workspace) =>
    confirm.ask({
      title: "Delete this workspace?",
      text: `The project folder of “${p.title || "this conversation"}” and everything in it are deleted. The conversation stays.`,
      label: "Delete workspace",
      cancel: "Keep it",
      action: async () => {
        await store.workspace.remove(p.conversationId!);
        toast("Workspace deleted");
        await queries.invalidateQueries({ queryKey: workspacesKey });
      },
    });
  const machine = items?.[0];
  const usage = machine ? diskUsage(machine.diskBytes, limit) : null;
  return (
    <Page>
      <PageHeader title="Compute" description="Every chat with a workspace is a project folder in your own Linux container. Open one to browse its files, use a terminal or download it." actions={newProject}>
        <ComputeTabs />
      </PageHeader>
      <ErrorBox error={list.error} onRetry={() => list.refetch()} />
      {!items ? (
        list.isPending && <Loading label="Loading your workspaces…" />
      ) : !items.length ? (
        <section className="flex flex-col items-center gap-3 rounded-lg border bg-card px-6 py-12 text-center">
          <span className="grid size-10 place-items-center rounded-full border bg-background">
            <Terminal className="size-5 text-muted-foreground" />
          </span>
          <h2 className="text-lg font-semibold">Describe it, then watch it run.</h2>
          <p className="max-w-md text-sm text-muted-foreground">
            Turn on Workspace in the composer and ask for something to build. The assistant writes the files, runs the commands and starts the app, and the live preview opens beside the chat.
          </p>
          <Button onClick={start}>
            <Play /> Start building
          </Button>
        </section>
      ) : (
        <div className="grid gap-4">
          <section className="flex flex-wrap items-center gap-4 rounded-lg border bg-card p-4">
            <Server className="size-5 shrink-0 text-muted-foreground" />
            <div className="min-w-0 grow basis-64">
              <h2 className="flex items-center gap-2 text-base font-medium">
                Your container <Pill tone={statusTone(machine!)}>{statusLabel(machine!)}</Pill>
              </h2>
              <p className="mt-0.5 text-sm text-muted-foreground">
                {machine!.status === "stopped" ? "Asleep: it wakes when you open a project or the assistant needs it. Files are kept." : "It sleeps after an hour unused and keeps your files."}
              </p>
            </div>
            {usage && (
              <div className="grid w-full gap-1.5 sm:w-56">
                <span className="text-sm text-text-2">{usage.of}</span>
                <Quota value={machine!.diskBytes || 0} max={limit} label="Disk used" />
              </div>
            )}
          </section>
          <section className="divide-y rounded-lg border bg-card" aria-label="Workspaces">
            {items.map((p) => {
              const id = encodeURIComponent(p.conversationId || "");
              const live = previews && p.status === "running" && !!p.previewPort;
              return (
                <div key={p.conversationId} className="flex flex-wrap items-center gap-3 px-4 py-3">
                  <Link href={`/workspaces/${id}`} className="flex min-w-0 grow basis-56 items-center gap-3">
                    <Terminal className="size-4 shrink-0 text-muted-foreground" />
                    <span className="min-w-0">
                      <strong className="block truncate text-md font-medium">{p.title || "Untitled conversation"}</strong>
                      <small className="block text-sm text-muted-foreground">
                        {live ? "Preview running · " : ""}Used {ago(p.lastUsedAt)}
                      </small>
                    </span>
                  </Link>
                  <div className="flex items-center gap-1.5">
                    <Button asChild size="sm" variant="outline">
                      <Link href={`/workspaces/${id}`}>Open</Link>
                    </Button>
                    <Button asChild size="sm" variant="ghost">
                      <Link href={`/chat/${id}`}>
                        <MessageSquare /> Chat
                      </Link>
                    </Button>
                    {p.status === "running" && (
                      <Button asChild size="icon-sm" variant="ghost">
                        <a href={downloadUrl(p.conversationId || "")} download aria-label="Download project" title="Download project (.zip, without dependencies)">
                          <Download />
                        </a>
                      </Button>
                    )}
                    <Button size="icon-sm" variant="ghost" aria-label={`Delete the workspace of ${p.title || "this conversation"}`} title={`Delete the workspace of ${p.title || "this conversation"}`} onClick={() => remove(p)}>
                      <Trash2 />
                    </Button>
                  </div>
                </div>
              );
            })}
          </section>
        </div>
      )}
      {confirm.element}
    </Page>
  );
}
