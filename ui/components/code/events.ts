// What the workspace tells the chat's Git tab: files changed (the count follows a
// moment later) or the workspace is running (check again when the state is old).
// The Git tab's query listens while it, or its tab label, is on screen.

export type CodeEvent = { id: string; kind: "changed" } | { id: string; kind: "refresh"; age: number };

const listeners = new Set<(event: CodeEvent) => void>();

export const codeEvents = {
  emit(event: CodeEvent) {
    listeners.forEach((l) => l(event));
  },
  subscribe(listener: (event: CodeEvent) => void) {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  },
};
