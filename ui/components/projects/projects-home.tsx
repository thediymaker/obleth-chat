"use client";

import { Box, Folder, Plus } from "lucide-react";
import Link from "next/link";
import { useState } from "react";
import { usePageChrome } from "@/components/app-shell/chrome";
import { EmptyState, ErrorBox, Loading, Page, PageHeader } from "@/components/app-shell/page";
import { projectDot } from "@/components/app-shell/sidebar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { count, when } from "@/lib/format";
import { useFeature, useProjects } from "@/lib/queries";
import { useSession } from "@/lib/session";
import { cn } from "@/lib/utils";
import { type Project, sharing } from "./api";
import { ProjectDialog } from "./project-dialog";

export function ProjectMark({ color, large = false }: { color: string; large?: boolean }) {
  return (
    <span className={cn("relative flex shrink-0 items-center justify-center overflow-hidden rounded-lg border", large ? "size-12" : "size-9")}>
      <span className={cn("absolute inset-0 opacity-15", projectDot(color))} />
      <Folder className={cn("relative", large ? "size-5" : "size-4")} style={{ color: `var(--project-${color})` }} />
    </span>
  );
}

function ProjectCard({ p, knowledge }: { p: Project; knowledge: boolean }) {
  const facts = [count(p.chats, "chat"), knowledge ? count(p.saved, "saved item") : ""].filter(Boolean).join(" · ");
  return (
    <Link
      href={`/projects/${encodeURIComponent(p.id)}`}
      className="group flex min-h-44 flex-col gap-3 rounded-lg border bg-card p-4 transition-colors hover:border-line-strong hover:bg-raised"
    >
      <div className="flex items-start justify-between gap-2">
        <ProjectMark color={p.color} />
        <Badge variant={p.scope === "group" ? "secondary" : "outline"} className="max-w-[60%] truncate">
          {p.mine ? sharing(p) : `${p.groupName || "Shared"} · ${p.ownerName}`}
        </Badge>
      </div>
      <h2 className="text-base font-semibold">{p.name}</h2>
      <p className="line-clamp-2 grow text-sm text-muted-foreground">{p.description || (p.instructions ? p.instructions.slice(0, 140) : "No description yet.")}</p>
      <div className="flex items-center justify-between gap-2 text-xs text-faint">
        <span>{facts}</span>
        <span>{when(p.updatedAt)}</span>
      </div>
    </Link>
  );
}

export function ProjectsHome() {
  usePageChrome("Projects");
  const { data, error, isPending, refetch } = useProjects();
  const knowledge = useFeature("knowledge");
  const { isAdmin } = useSession();
  const [open, setOpen] = useState(false);
  const [asApp, setAsApp] = useState(false);
  const start = (app: boolean) => {
    setAsApp(app);
    setOpen(true);
  };
  const all = (data?.projects ?? []) as Project[];
  const newProject = (
    <Button onClick={() => start(false)}>
      <Plus /> New project
    </Button>
  );
  return (
    <Page wide>
      <PageHeader
        title="Projects worth coming back to."
        description="Instructions, documents and saved things that every chat in the project starts with."
        actions={
          <>
            {isAdmin && (
              <Button variant="outline" onClick={() => start(true)}>
                <Box /> New app
              </Button>
            )}
            {newProject}
          </>
        }
      />
      <ErrorBox error={error} onRetry={() => refetch()} />
      {isPending ? (
        <Loading label="Loading projects…" />
      ) : !all.length ? (
        !error && (
          <EmptyState
            icon={Folder}
            title="No projects yet"
            text="A project gives its chats standing instructions and documents to search, and keeps what you save along the way. Share it with a group to work on it together."
            action={newProject}
          />
        )
      ) : (
        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {all.map((p) => (
            <ProjectCard key={p.id} p={p} knowledge={knowledge} />
          ))}
        </div>
      )}
      <ProjectDialog open={open} asApp={asApp} onOpenChange={setOpen} />
    </Page>
  );
}
