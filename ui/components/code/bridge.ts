// What the chat store asks of code mode: where a new chat's
// workspace starts when it was started from a commit ("New chat from here" on the
// project's Code tab), and the Git tab's refresh after file tools. The Git tab
// itself is components/code/code-panel.tsx; the project's Code tab is
// components/code/project-code.tsx.

import { toast as sonner } from "sonner";
import { api as defaultApi } from "@/lib/api";
import { chatCodePath } from "./model";
import { codeEvents } from "./events";

export interface CodeStart {
  sha: string;
  subject?: string;
}

export interface CodeBridge {
  /** A new chat was created: start its workspace from the chosen code. */
  applyStart(id: string): Promise<void>;
  resetDraft(): void;
  /** Files changed in the chat's workspace. */
  changed(id: string): void;
  /** The commit the next new chat's workspace starts at, or null for the latest code. */
  start(): CodeStart | null;
  setStart(start: CodeStart | null): void;
  /** For useSyncExternalStore: the start changed. */
  subscribe(listener: () => void): () => void;
}

interface Context {
  api?: { request: (path: string, options?: any) => Promise<any> };
  toast?: (message: string) => void;
}

export function createCodeBridge({ api = defaultApi, toast = (m) => sonner(m) }: Context = {}): CodeBridge {
  let start: CodeStart | null = null;
  const listeners = new Set<() => void>();
  const set = (value: CodeStart | null) => {
    start = value;
    listeners.forEach((l) => l());
  };
  return {
    /** After the new chat is created and in its project: where its workspace starts. */
    async applyStart(id) {
      const s = start;
      set(null);
      if (!s) return;
      await api.request(`${chatCodePath(id)}/start`, { method: "PUT", body: { sha: s.sha } }).catch((e: Error) => toast(e.message));
    },
    resetDraft: () => set(null),
    changed: (id) => codeEvents.emit({ id, kind: "changed" }),
    start: () => start,
    setStart: set,
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}
