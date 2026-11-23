"use client";

// A project's page around its tabs: the heading with its actions, and the tabs
// (Overview, Code, Experiments, App) that the server and features allow.

import { useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Box, Code, FlaskConical, LayoutDashboard, NotebookText, Pencil, Plus, ScrollText, SquareKanban } from "lucide-react";
import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { createContext, useContext, useEffect, useState } from "react";
import { toast } from "sonner";
import { appKeys, createApp } from "@/components/apps/api";
import { usePageChrome } from "@/components/app-shell/chrome";
import { ErrorBox, Loading, Page, PageHeader, isNotFound, NotFound } from "@/components/app-shell/page";
import { SectionTabs } from "@/components/app-shell/section-tabs";
import { Button } from "@/components/ui/button";
import { count } from "@/lib/format";
import { keys, useCapabilities, useFeature } from "@/lib/queries";
import { useSession } from "@/lib/session";
import { type ProjectDetail, useProjectDetail } from "./api";
import { chatDraft } from "@/lib/chat/chat-draft";
import { ProjectDialog } from "./project-dialog";
import { ProjectMark } from "./projects-home";
import { messageOf } from "@/components/manage/kit";

const FrameContext = createContext<{ detail: ProjectDetail; edit: () => void } | null>(null);
/** The project the page is showing, and a way to open its Edit dialog. */
export function useProjectFrame() {
  const value = useContext(FrameContext);
  if (!value) throw new Error("useProjectFrame needs ProjectFrame");
  return value;
}

export type ProjectTab = "overview" | "notebook" | "log" | "code" | "research" | "app";

/** Which tabs a project shows: Notebook with knowledge, Code with the code feature, Experiments with workspaces, App for administrators on their own projects. */
export function useProjectTabs(detail?: ProjectDetail): Record<ProjectTab, boolean> {
  const notebook = useFeature("knowledge");
  const code = useFeature("code");
  const research = useFeature("workspaces");
  const { isAdmin } = useSession();
  const log = useFeature("myLog");
  return { overview: true, notebook, log, code, research, app: isAdmin && !!detail?.project.mine };
}

/** Starts a new chat that goes into the project once it is sent. */
export function useNewChatInProject() {
  const router = useRouter();
  return (projectId: string) => {
    chatDraft.reset();
    chatDraft.setProject(projectId);
    router.push("/");
  };
}

export function ProjectFrame({ children }: { children: React.ReactNode }) {
  const { id: raw } = useParams<{ id: string }>();
  const id = decodeURIComponent(raw);
  const router = useRouter();
  const client = useQueryClient();
  const { data: d, error, isPending, refetch } = useProjectDetail(id);
  const knowledge = useFeature("knowledge");
  const { isAdmin } = useSession();
  const tabs = useProjectTabs(d);
  const newChat = useNewChatInProject();
  const [editing, setEditing] = useState(false);
  const [making, setMaking] = useState(false);
  usePageChrome(d?.project.name || "Project");

  if (!d) {
    return (
      <Page>
        {isPending ? (
          <Loading />
        ) : isNotFound(error) ? (
          <NotFound what="project" to="/projects" back="all projects" />
        ) : (
          <>
            <ErrorBox error={error} onRetry={() => refetch()} />
            <Button asChild size="sm" variant="outline">
              <Link href="/projects">
                <ArrowLeft /> All projects
              </Link>
            </Button>
          </>
        )}
      </Page>
    );
  }
  const p = d.project;
  const mine = p.mine;
  const base = `/projects/${encodeURIComponent(p.id)}`;
  const facts = [p.description, count(p.chats, "chat"), knowledge ? count(p.saved, "saved item") : ""].filter(Boolean).join(" · ");
  const list = [
    { href: base, label: "Overview", icon: LayoutDashboard },
    { href: `${base}/board`, label: "Board", icon: SquareKanban },
    ...(tabs.notebook ? [{ href: `${base}/notebook`, label: "Notebook", icon: NotebookText, prefix: true }] : []),
    ...(tabs.log ? [{ href: `${base}/log`, label: "Log", icon: ScrollText }] : []),
    // History, Files and Compare live under /code and keep it highlighted.
    ...(tabs.code ? [{ href: `${base}/code`, label: "Code", icon: Code, prefix: true }] : []),
    ...(tabs.research ? [{ href: `${base}/research`, label: "Experiments", icon: FlaskConical }] : []),
    ...(tabs.app ? [{ href: `${base}/app`, label: "App", icon: Box }] : []),
  ];

  async function makeApp() {
    setMaking(true);
    try {
      const app = await createApp(p.id);
      client.setQueryData(appKeys.project(p.id), app);
      client.invalidateQueries({ queryKey: keys.project(p.id) });
      toast("App created");
      router.push(`${base}/app`);
    } catch (e) {
      toast(messageOf(e));
    } finally {
      setMaking(false);
    }
  }

  return (
    // One width for every tab, the wider one the app and the notebook's three panes
    // need, so the heading and the tabs stay where they are from tab to tab.
    <Page className="max-w-[1600px]">
      <PageHeader
        breadcrumb={
          <nav aria-label="Breadcrumb" className="-mb-1 flex items-center gap-1.5 text-sm text-muted-foreground">
            <Link href="/projects" className="hover:text-foreground">
              Projects
            </Link>
            <span className="text-faint">/</span>
            <span className="truncate text-text-2">{p.name}</span>
          </nav>
        }
        icon={<ProjectMark color={p.color} large />}
        title={p.name}
        description={facts}
        actions={
          <>
            {mine && isAdmin && !d.app && (
              <Button variant="outline" onClick={makeApp} disabled={making}>
                <Box /> Make it an app
              </Button>
            )}
            {mine && (
              <Button variant="outline" onClick={() => setEditing(true)}>
                <Pencil /> Edit
              </Button>
            )}
            <Button onClick={() => newChat(p.id)}>
              <Plus /> New chat in project
            </Button>
          </>
        }
      >
        {list.length > 1 && <SectionTabs label="Project" tabs={list} compact />}
      </PageHeader>
      <FrameContext.Provider value={{ detail: d, edit: () => setEditing(true) }}>{children}</FrameContext.Provider>
      {mine && <ProjectDialog open={editing} onOpenChange={setEditing} project={p} />}
    </Page>
  );
}

/** A tab the project can't show (its feature is off, or the app of someone else's project) goes to Overview. */
export function ProjectTabGuard({ tab, children }: { tab: ProjectTab; children: (projectId: string) => React.ReactNode }) {
  const { detail } = useProjectFrame();
  const tabs = useProjectTabs(detail);
  const { data: capabilities } = useCapabilities();
  const router = useRouter();
  const allowed = tabs[tab];
  const known = !!capabilities;
  useEffect(() => {
    if (known && !allowed) router.replace(`/projects/${encodeURIComponent(detail.project.id)}`);
  }, [known, allowed, router, detail.project.id]);
  return allowed ? children(detail.project.id) : null;
}
