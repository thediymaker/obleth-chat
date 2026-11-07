"use client";

// Data more than one page reads, under shared query keys so a change made on
// one page (a renamed chat, a new project) shows everywhere. Page-specific data
// keeps its own keys next to the page; prefix them with the page name.

import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { useSession } from "@/lib/session";

export const keys = {
  capabilities: ["capabilities"] as const,
  conversations: ["conversations"] as const,
  conversation: (id: string) => ["conversation", id] as const,
  models: ["models"] as const,
  preferences: ["preferences"] as const,
  presets: ["presets"] as const,
  projects: ["projects"] as const,
  project: (id: string) => ["project", id] as const,
};

export interface Capabilities {
  application: string;
  mode: "connected" | "preview";
  liveExecution?: boolean;
  adminGatewayUrl?: string;
  /** How the sidebar shows the Feedback button. */
  feedback?: "off" | "sidebar" | "highlighted";
  integrations?: Record<string, { configured: boolean }>;
  features: {
    knowledge?: boolean;
    workspaces?: boolean;
    previews?: boolean;
    code?: boolean;
    browser?: boolean;
    connections?: boolean;
    clusters?: boolean;
    autoMode?: boolean;
    history?: boolean;
    [feature: string]: boolean | undefined;
  };
  [key: string]: unknown;
}

/** Only fetched once someone is signed in: most queries need the session. */
export function useSignedIn() {
  return useSession().phase === "ready";
}

export function useCapabilities() {
  return useQuery({
    queryKey: keys.capabilities,
    queryFn: () => api.request<Capabilities>("/api/capabilities"),
    enabled: useSignedIn(),
    staleTime: 60_000,
  });
}
/** One feature flag, false until capabilities load. */
export function useFeature(name: keyof Capabilities["features"]): boolean {
  return !!useCapabilities().data?.features?.[name];
}

export interface ConversationSummary {
  id: string;
  title: string;
  model: string;
  updatedAt: string;
  [key: string]: unknown;
}
export function useConversations() {
  return useQuery({
    queryKey: keys.conversations,
    queryFn: async () => (await api.request<{ conversations?: ConversationSummary[] }>("/api/conversations")).conversations ?? [],
    enabled: useSignedIn(),
  });
}

export interface Model {
  id: string;
  name?: string;
  [key: string]: unknown;
}
export function useModels() {
  return useQuery({
    queryKey: keys.models,
    queryFn: async () => (await api.request<{ models?: Model[] }>("/api/models")).models ?? [],
    enabled: useSignedIn(),
    staleTime: 60_000,
  });
}

export function usePreferences() {
  return useQuery({
    queryKey: keys.preferences,
    queryFn: () => api.request<Record<string, unknown>>("/api/preferences"),
    enabled: useSignedIn(),
  });
}

export function usePresets() {
  return useQuery({
    queryKey: keys.presets,
    queryFn: () => api.request<Record<string, unknown>>("/api/presets"),
    enabled: useSignedIn(),
  });
}

export interface ProjectSummary {
  id: string;
  name: string;
  color: string;
  mine: boolean;
  [key: string]: unknown;
}
export function useProjects() {
  return useQuery({
    queryKey: keys.projects,
    queryFn: () => api.request<{ projects?: ProjectSummary[]; groups?: { id: string; name: string }[] }>("/api/projects"),
    enabled: useSignedIn(),
  });
}
