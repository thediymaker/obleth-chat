"use client";

// A new chat in a project with a workspace, from the project's Code or Research tab:
// the chat goes into the project once sent, its workspace starts at a commit or the
// latest code, and the box holds the start of a prompt.

import { useRouter } from "next/navigation";
import { useChatStore } from "@/components/chat/chat-provider";
import { startAutoDraft } from "@/lib/chat/tools-draft";
import { chatDraft } from "@/lib/chat/chat-draft";
import type { CodeStart } from "@/lib/code/bridge";

export function useStartInProject() {
  const store = useChatStore();
  const router = useRouter();
  return (projectId: string, { start = null, prompt = "", auto = false }: { start?: CodeStart | null; prompt?: string; auto?: boolean } = {}) => {
    store.newChat();
    chatDraft.reset();
    chatDraft.setProject(projectId);
    if (auto) startAutoDraft();
    store.code.setStart(start);
    store.workspaceDraft = true;
    store.starter(prompt, true);
    router.push("/");
  };
}
