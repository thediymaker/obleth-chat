// Starting research from the model's suggest_research card in a chat: a project that
// can hold it, a workspace, and Auto mode. The wiring is real; the project's Research
// tab is components/research/project-research.tsx.

import { moveChatToResearchProject } from "@/components/projects/api";
import type { ChatStore } from "@/lib/chat/store";
import type { ActivityEntry } from "@/lib/chat/types";

export const researchAllowed = (store: ChatStore) => store.feature("workspaces");

export async function startResearch(store: ChatStore, id: string, entry: ActivityEntry) {
  if (!entry.inProject && store.queries) await moveChatToResearchProject(store.queries, id, entry.goal);
  if (!store.workspace.has(id)) await store.workspace.create(id);
  await store.enableAuto(id);
}
