"use client";

// Chat's project endpoints (api/src/projects.rs) and the queries pages share.

import { useQuery, type QueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { keys, type ProjectSummary } from "@/lib/queries";

export const COLORS = ["blue", "green", "gold", "rose", "violet", "gray"] as const;
export type ProjectColor = (typeof COLORS)[number];
export const MAX_COLLECTIONS = 8;

export interface Project extends ProjectSummary {
  description: string;
  instructions: string;
  color: ProjectColor;
  scope: "personal" | "group";
  groupId: string | null;
  groupName: string | null;
  ownerName: string;
  chats: number;
  saved: number;
  updatedAt: string;
}
export interface SavedSummary {
  id: string;
  ownerId: string;
  kind: "code" | "note" | "link" | "excerpt";
  title: string;
  language: string;
  tags: string[];
  pinned: boolean;
  [key: string]: unknown;
}
export interface ProjectDetail {
  project: Project;
  collections: { id: string; name: string; documents: number }[];
  chats: { id: string; title: string; updatedAt: string }[];
  saved: SavedSummary[];
  /** The app's code, for the owner; null when the project has none. */
  app: string | null;
}

export const projectPath = (id: string) => `/api/projects/${encodeURIComponent(id)}`;
const chatPath = (id: string) => `/api/conversations/${encodeURIComponent(id)}/project`;
export const conversationProjectKey = (id: string) => ["conversation-project", id] as const;

/** Who sees a project, as its card says. */
export const sharing = (p: Pick<Project, "scope" | "groupName">) => (p.scope === "group" ? `Shared with ${p.groupName || "a group"}` : "Only you");
/** A project's line in lists: who sees it, or whose it is. */
export const projectLine = (p: Project) => (p.mine ? sharing(p) : `${p.groupName || "Shared"} · ${p.ownerName}`);

export function useProjectDetail(id: string) {
  return useQuery({ queryKey: keys.project(id), queryFn: () => api.request<ProjectDetail>(projectPath(id)), enabled: !!id });
}

/** The project a chat is in, or null. */
export function useConversationProject(conversationId: string | null | undefined) {
  return useQuery({
    queryKey: conversationProjectKey(conversationId || ""),
    queryFn: async () => (await api.request<{ project: Project | null }>(chatPath(conversationId!))).project,
    enabled: !!conversationId,
  });
}

/** After a project changes: the list, the sidebar and that project's page. */
export function refreshProjects(client: QueryClient, id?: string) {
  client.invalidateQueries({ queryKey: keys.projects });
  if (id) client.invalidateQueries({ queryKey: keys.project(id) });
}

/** Moves a chat into a project, or out of any with null. Returns the project it is in now. */
export async function moveChat(client: QueryClient, conversationId: string, projectId: string | null): Promise<Project | null> {
  const r = await api.request<{ project: Project | null }>(chatPath(conversationId), { method: "PUT", body: { projectId: projectId || null } });
  client.setQueryData(conversationProjectKey(conversationId), r.project);
  client.invalidateQueries({ queryKey: keys.projects });
  client.invalidateQueries({ queryKey: ["project"] });
  return r.project;
}

/** Moves a chat into a new personal project. */
export async function moveChatToNewProject(
  client: QueryClient,
  conversationId: string,
  { name, description = "", color = "blue" }: { name: string; description?: string; color?: ProjectColor },
): Promise<Project> {
  const made = await api.request<{ project: Project }>("/api/projects", { method: "POST", body: { name, description, color, scope: "personal" } });
  return (await moveChat(client, conversationId, made.project.id))!;
}

/** Moves a chat into a new project named after the research it will run. */
export function moveChatToResearchProject(client: QueryClient, conversationId: string, goal: string) {
  const name =
    String(goal || "")
      .trim()
      .replace(/[.\s]+$/, "")
      .slice(0, 120) || "Research";
  return moveChatToNewProject(client, conversationId, { name, description: "Research started from a chat.", color: "violet" });
}
