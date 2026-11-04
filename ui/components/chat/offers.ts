// Making the app the model suggested (its suggest_app card): in the chat's own project
// when it can take one, else a new one, with the app and its collection made by
// components/apps/api.ts.

import { appKeys, createAppFromSuggestion, type AppSuggestion } from "@/components/apps/api";
import { moveChatToNewProject } from "@/components/projects/api";
import type { ChatStore } from "@/lib/chat/store";
import type { ActivityEntry } from "@/lib/chat/types";
import { keys } from "@/lib/queries";

// The app builder's step types and their names.
export const APP_STEP_TYPES: Record<string, string> = {
  agent: "Agent",
  search: "Search knowledge",
  code: "Code",
  decide: "Decide",
  if: "If",
  for_each: "For each",
  http: "HTTP request",
  response: "Response",
};

/** Only administrators can make apps for now. */
export const appsAllowed = (store: ChatStore) => store.isAdmin;

/** `chatProject` is the chat's own project id, or "". */
export async function startApp(store: ChatStore, id: string, entry: ActivityEntry, chatProject: string) {
  const queries = store.queries!;
  const project =
    (entry.inProject && chatProject) ||
    (await moveChatToNewProject(queries, id, { name: String(entry.name || "App").slice(0, 120), description: String(entry.description || "").slice(0, 1000), color: "green" })).id;
  const collection = await createAppFromSuggestion(project, entry as AppSuggestion, store.feature("knowledge"));
  queries.invalidateQueries({ queryKey: appKeys.project(project) });
  queries.invalidateQueries({ queryKey: keys.projects });
  return { project, collection };
}
