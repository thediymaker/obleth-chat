// Chat's app endpoints (api/src/apps/mod.rs), for the App tab, the chat page's
// suggested-app card and an app's own chat page (/a/<id>/<slug>).

import { api } from "@/lib/api";
import type { Definition } from "./model";
import type { TraceEntry } from "./layout";

export interface App {
  id: string;
  projectId: string;
  name: string;
  slug: string;
  description: string;
  definition: Definition;
  live: boolean;
  version: number;
  publishedAt: string | null;
  changed: boolean;
  updatedAt: string;
  endpoint: string;
}
export interface AppRun {
  id: string;
  trigger: "api" | "test" | "chat";
  version: number;
  status: "completed" | "failed" | "running";
  error: string | null;
  startedAt: string;
  ms: number | null;
  inputs?: unknown;
  output?: unknown;
  trace?: TraceEntry[];
}
export interface AppKey {
  id: string;
  name: string;
  createdBy: string | null;
  createdAt: string;
  lastUsedAt: string | null;
}
export interface TryResult {
  runId: string;
  status: "completed" | "failed";
  output: unknown;
  error: string | null;
  trace: TraceEntry[];
  ms: number;
}

export const appKeys = {
  project: (projectId: string) => ["apps", "project", projectId] as const,
  runs: (appId: string) => ["apps", "runs", appId] as const,
  keys: (appId: string) => ["apps", "keys", appId] as const,
};
const appPath = (id: string) => `/api/apps/${encodeURIComponent(id)}`;
const projectPath = (id: string) => `/api/projects/${encodeURIComponent(id)}`;

export const getProjectApp = async (projectId: string) => (await api.request<{ app: App | null }>(`${projectPath(projectId)}/app`)).app;
/** Makes the project's app, empty or with the steps a chat suggested. */
export const createApp = async (projectId: string, body?: { description?: string; definition?: Definition }) =>
  (await api.request<{ app: App }>(`${projectPath(projectId)}/app`, { method: "POST", ...(body ? { body } : {}) })).app;
export const saveApp = async (app: App, definition: Definition, details: { name?: string; description?: string } = {}) =>
  (
    await api.request<{ app: App }>(appPath(app.id), {
      method: "PUT",
      body: { name: details.name ?? app.name, slug: app.slug, description: details.description ?? app.description, definition },
    })
  ).app;
export const deleteApp = (app: App) => api.request(appPath(app.id), { method: "DELETE" });
export const publishApp = async (app: App) => (await api.request<{ app: App }>(`${appPath(app.id)}/publish`, { method: "POST" })).app;
export const unpublishApp = async (app: App) => (await api.request<{ app: App }>(`${appPath(app.id)}/unpublish`, { method: "POST" })).app;
export const tryApp = (app: App, inputs: Record<string, unknown>) => api.request<TryResult>(`${appPath(app.id)}/test`, { method: "POST", body: { inputs } });
export const listRuns = async (appId: string) => (await api.request<{ runs?: AppRun[] }>(`${appPath(appId)}/runs`)).runs ?? [];
export const getRun = async (appId: string, runId: string) => (await api.request<{ run: AppRun }>(`${appPath(appId)}/runs/${encodeURIComponent(runId)}`)).run;
export const listKeys = async (appId: string) => (await api.request<{ keys?: AppKey[] }>(`${appPath(appId)}/keys`)).keys ?? [];
export const createKey = (appId: string, name: string) => api.request<{ key: { id: string; name: string }; secret: string }>(`${appPath(appId)}/keys`, { method: "POST", body: { name } });
export const revokeKey = (appId: string, keyId: string) => api.request(`${appPath(appId)}/keys/${encodeURIComponent(keyId)}`, { method: "DELETE" });

/** A published app as its chat page shows it. */
export interface AppPage {
  id: string;
  name: string;
  slug: string;
  description: string;
  version: number;
  publishedAt: string | null;
  /** The text input a message fills; none means the app can't chat. */
  message: string | null;
  /** It has a JSON input `history` that gets the earlier turns. */
  history: boolean;
  /** Other required inputs, which a chat can't fill. */
  missing: string[];
  /** Set for the project's owner, for a way back to the App tab. */
  projectId: string | null;
}
export interface ChatTurn {
  role: "user" | "assistant";
  content: string;
}
export interface ChatReply {
  runId: string;
  status: "completed" | "failed";
  output: unknown;
  error: string | null;
}
export const appPageKey = (id: string) => ["apps", "page", id] as const;
export const getAppPage = async (id: string) => (await api.request<{ app: AppPage }>(`${appPath(id)}/page`)).app;
export const chatWithApp = (id: string, message: string, history: ChatTurn[]) => api.request<ChatReply>(`${appPath(id)}/chat`, { method: "POST", body: { message, history } });
/** The chat page's address, from the app's code and its name. */
export const chatPagePath = (app: { id: string; slug: string }) => `/a/${app.id}/${app.slug}`;

/** What the model's suggest_app tool proposes (the chat page's card). */
export interface AppSuggestion {
  name: string;
  description?: string;
  /** It answers from documents, so it wants a collection. */
  documents?: boolean;
  definition: Definition;
  [key: string]: unknown;
}

/**
 * Makes a chat's suggested app in a project: the app with its steps, and a collection
 * for its documents when it needs one and the project searches none yet. Returns the
 * collection's name, if one was made.
 */
export async function createAppFromSuggestion(projectId: string, entry: AppSuggestion, knowledgeEnabled: boolean): Promise<string> {
  await createApp(projectId, { description: entry.description || "", definition: entry.definition });
  if (!entry.documents || !knowledgeEnabled) return "";
  const detail = await api.request<{ collections: unknown[] }>(projectPath(projectId));
  if (detail.collections.length) return "";
  const { collection } = await api.request<{ collection: { id: string; name: string } }>("/api/knowledge/collections", {
    method: "POST",
    body: { name: `${entry.name} documents`.slice(0, 120), description: `Documents the ${entry.name} app answers from.`.slice(0, 1000), scope: "personal" },
  });
  await api.request(`${projectPath(projectId)}/collections`, { method: "PUT", body: { collectionIds: [collection.id] } });
  return collection.name;
}
