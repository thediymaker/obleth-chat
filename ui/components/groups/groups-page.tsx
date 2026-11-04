"use client";

// Groups: the people someone works with and what is shared with them. A project or
// collection shared with a group is open to everyone in it. Administrators create
// groups and add people under Settings › Administration › Groups.

import { useQuery } from "@tanstack/react-query";
import { BookOpen, MessageSquare, Settings } from "lucide-react";
import Link from "next/link";
import { usePageChrome } from "@/components/app-shell/chrome";
import { EmptyState, ErrorBox, Eyebrow, Loading, Page, PageHeader } from "@/components/app-shell/page";
import { projectDot } from "@/components/app-shell/sidebar";
import { Button } from "@/components/ui/button";
import { Note } from "@/components/manage/kit";
import { api } from "@/lib/api";
import { count, initials } from "@/lib/format";
import { useUser } from "@/lib/session";
import { cn } from "@/lib/utils";

export interface GroupSummary {
  id: string;
  name: string;
  members: number;
  projects?: number;
  collections?: number;
  chats?: number;
  member?: boolean;
}
export interface GroupDetail {
  group: { id: string; name: string; description?: string; members: number };
  projects: { id: string; name: string; color: string; mine: boolean; ownerName?: string; saved?: number }[];
  chats?: { id: string; title: string; mine: boolean; ownerName?: string }[];
  collections: { id: string; name: string; documents: number }[];
  members: { id: string; username: string; displayName?: string }[];
  canManage?: boolean;
}

/** "Not a member · 3 people · 1 project". */
export const groupLine = (g: GroupSummary) =>
  `${g.member ? "" : "Not a member · "}${count(g.members, "person", "people")}${g.projects ? ` · ${count(g.projects, "project")}` : ""}${g.collections ? ` · ${count(g.collections, "collection")}` : ""}${g.chats ? ` · ${count(g.chats, "chat")}` : ""}`;

function Mark({ name, small = false }: { name: string; small?: boolean }) {
  return (
    <span aria-hidden className={cn("grid shrink-0 place-items-center rounded-md bg-group font-semibold text-group-foreground", small ? "size-8 text-xs" : "size-11 text-base")}>
      {initials(name)}
    </span>
  );
}

function SharedRow({ href, icon, title, detail }: { href: string; icon: React.ReactNode; title: string; detail: string }) {
  return (
    <Link href={href} className="flex items-center gap-3 rounded-md px-2 py-2 hover:bg-accent">
      {icon}
      <span className="min-w-0 grow truncate text-md">{title}</span>
      <span className="shrink-0 text-sm text-faint">{detail}</span>
    </Link>
  );
}

export function GroupsPage({ id = "" }: { id?: string }) {
  usePageChrome("Groups");
  const me = useUser();
  const list = useQuery({ queryKey: ["groups"], queryFn: () => api.request<{ groups?: GroupSummary[]; canManage?: boolean }>("/api/groups") });
  const all = list.data?.groups;
  const chosen = id || all?.[0]?.id || "";
  const detail = useQuery({
    queryKey: ["group", chosen],
    queryFn: () => api.request<GroupDetail>(`/api/groups/${encodeURIComponent(chosen)}`),
    enabled: !!chosen && !!all,
  });
  const canManage = !!list.data?.canManage;
  const error = list.error || detail.error;
  const retry = () => {
    list.refetch();
    if (chosen) detail.refetch();
  };
  const head = (
    <PageHeader
      title="Work with your people."
      description="A group shares chats, projects and collections with everyone in it. Chats stay private until you share one."
      actions={
        canManage && (
          <Button variant="outline" asChild>
            <Link href="/settings/admin/groups">
              <Settings /> Manage groups
            </Link>
          </Button>
        )
      }
    />
  );
  if (!all)
    return (
      <Page wide>
        {head}
        <ErrorBox error={error} onRetry={retry} />
        {!error && <Loading label="Loading groups…" />}
      </Page>
    );
  if (!all.length)
    return (
      <Page wide>
        {head}
        <ErrorBox error={error} onRetry={retry} />
        <EmptyState
          title={canManage ? "No groups yet" : "You are not in any groups"}
          text={
            canManage
              ? "Create a group and add people under Settings → Administration → Groups."
              : "An administrator adds people to groups. Once you are in one, projects and collections shared with it appear here."
          }
        />
      </Page>
    );
  const d = detail.data;
  const current = d?.group.id || chosen;
  return (
    <Page wide>
      {head}
      <ErrorBox error={error} onRetry={retry} />
      <div className="grid items-start gap-6 lg:grid-cols-[240px_minmax(0,1fr)_280px]">
        {/* Administrators see every group; the ones they are not in are marked. */}
        <nav aria-label={canManage ? "Groups" : "Your groups"} className="grid content-start gap-1 rounded-lg border bg-card p-3">
          <Eyebrow className="px-1 pb-1">{canManage ? "Groups" : "Your groups"}</Eyebrow>
          {all.map((g) => (
            <Link
              key={g.id}
              href={`/groups/${encodeURIComponent(g.id)}`}
              aria-current={g.id === current ? "page" : undefined}
              className={cn("flex items-center gap-2.5 rounded-md p-1.5 hover:bg-accent", g.id === current && "bg-accent")}
            >
              <Mark name={g.name} small />
              <span className="grid min-w-0">
                <strong className="truncate text-md font-medium">{g.name}</strong>
                <small className="truncate text-xs text-muted-foreground">{groupLine(g)}</small>
              </span>
            </Link>
          ))}
        </nav>
        {d ? (
          <>
            <section className="grid min-w-0 content-start gap-4 rounded-lg border bg-card p-4 sm:p-5">
              <div className="flex items-center gap-3">
                <Mark name={d.group.name} />
                <div className="min-w-0">
                  <h2 className="text-lg font-semibold">{d.group.name}</h2>
                  <p className="text-sm text-muted-foreground">{d.group.description || `${count(d.group.members, "person", "people")} in this group.`}</p>
                </div>
              </div>
              <Eyebrow>Shared with the group</Eyebrow>
              {d.projects.length || d.chats?.length || d.collections.length ? (
                <div className="grid">
                  {d.projects.map((p) => (
                    <SharedRow
                      key={p.id}
                      href={`/projects/${encodeURIComponent(p.id)}`}
                      icon={<span className={cn("size-2.5 shrink-0 rounded-full", projectDot(p.color))} />}
                      title={p.name}
                      detail={`project · ${p.mine ? "yours" : p.ownerName}${p.saved ? ` · ${count(p.saved, "saved item")}` : ""}`}
                    />
                  ))}
                  {(d.chats || []).map((c) => (
                    <SharedRow key={c.id} href={`/chat/${encodeURIComponent(c.id)}`} icon={<MessageSquare className="size-4 shrink-0 text-muted-foreground" />} title={c.title} detail={`chat · ${c.mine ? "yours" : c.ownerName}`} />
                  ))}
                  {d.collections.map((c) => (
                    <SharedRow key={c.id} href={`/knowledge/${encodeURIComponent(c.id)}`} icon={<BookOpen className="size-4 shrink-0 text-muted-foreground" />} title={c.name} detail={`collection · ${count(c.documents, "doc")}`} />
                  ))}
                </div>
              ) : (
                <Note>Nothing yet. Share a chat from its Share button, a project from its Edit dialog, or a collection from its page, and everyone here can use it.</Note>
              )}
            </section>
            <section className="grid content-start gap-2 rounded-lg border bg-card p-4">
              <Eyebrow>People · {d.members.length}</Eyebrow>
              {d.members.map((m) => (
                <div key={m.id} className="flex items-center gap-2.5 py-1">
                  <span aria-hidden className="grid size-7 shrink-0 place-items-center rounded-full bg-secondary text-2xs font-semibold">
                    {initials(m.displayName || m.username)}
                  </span>
                  <span className="min-w-0 grow truncate text-md">{m.id === me.id ? "You" : m.displayName || m.username}</span>
                  <span className="shrink-0 text-sm text-faint">{m.username}</span>
                </div>
              ))}
              {d.canManage ? (
                <Button size="sm" variant="outline" className="mt-2 w-fit" asChild>
                  <Link href="/settings/admin/groups">
                    <Settings /> Manage members
                  </Link>
                </Button>
              ) : (
                <Note className="mt-2">An administrator adds and removes people.</Note>
              )}
            </section>
          </>
        ) : (
          !error && <Loading label="Loading the group…" />
        )}
      </div>
    </Page>
  );
}
