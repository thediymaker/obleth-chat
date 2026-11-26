"use client";

// The scratch pad's endpoints (api/src/scratch.rs): a person's own jottings, newest first.

import { useQuery, type QueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { useSignedIn } from "@/lib/queries";

export interface Jotting {
  id: string;
  text: string;
  /** Null when it is in no project, or one the person may no longer use. */
  projectId: string | null;
  projectName: string | null;
  projectColor: string | null;
  createdAt: string;
  updatedAt: string;
}

/** Jottings fetched at first; "Show more" asks for this many more. */
export const PAGE = 20;
export const scratchKey = ["scratch"] as const;
const path = (id: string) => `/api/scratch/${encodeURIComponent(id)}`;

export function useJottings(limit: number, enabled: boolean) {
  return useQuery({
    queryKey: [...scratchKey, limit],
    queryFn: () => api.request<{ items: Jotting[]; more: boolean }>(`/api/scratch?limit=${limit}`),
    enabled: useSignedIn() && enabled,
  });
}

export async function addJotting(text: string, projectId: string | null) {
  return (await api.request<{ jotting: Jotting }>("/api/scratch", { method: "POST", body: { text, projectId } })).jotting;
}

export async function changeJotting(id: string, body: { text?: string; projectId?: string | null }) {
  return (await api.request<{ jotting: Jotting }>(path(id), { method: "PATCH", body })).jotting;
}

export function deleteJotting(id: string) {
  return api.request(path(id), { method: "DELETE" });
}

/** After a change: every page of the list. */
export function refreshJottings(client: QueryClient) {
  return client.invalidateQueries({ queryKey: scratchKey });
}
