// What the chat store asks of a chat's workspace, and the state its panel shows. The
// chat store owns the side pane (which tab shows, open or closed); this keeps each
// chat's workspace record, its preview link, its files, the command waiting for the
// terminal and the checkpoint dialog, and calls `changed()` so whatever reads the store
// draws again.

import { toast as sonner } from "sonner";
import type { StreamEvent } from "@/lib/api";
import type { Message } from "@/lib/chat/types";
import { codeEvents } from "@/components/code/events";
import { type FileEntry, type OpenFile, type Workspace, downloadUrl, imageFile, previewReload, rawUrl, startDownload, workspacePath } from "./model";

export type { Workspace } from "./model";

/** One chat's workspace as the panel shows it. */
export interface Space {
  workspace: Workspace | null;
  loaded: boolean;
  files: FileEntry[] | null;
  filesError: string;
  loadingFiles: boolean;
  file: OpenFile | null;
  /** The preview's address (after its ticket was redeemed) and origin. */
  url: string;
  origin: string;
  /** A page to open in the preview instead of the home page. */
  page: string;
  /** Bumped for every new preview link, so the frame loads it again. */
  frame: number;
  linking: boolean;
  waking: boolean;
  wakePolls: number;
  /** A run_in_terminal command waiting for the terminal. */
  command: string | null;
  /** Bumped to start a fresh terminal session. */
  terminal: number;
}

/** The checkpoint Restore dialog: asking first, then offering Undo. */
export type RestoreDialog = null | { id: string; sha: string; label: string; step: "confirm" | "done"; before?: string; undo?: boolean; busy?: boolean };

export interface WorkspaceBridge {
  /** Workspaces are configured on this server (capabilities.features.workspaces). */
  enabled(): boolean;
  /** The chat has a workspace. */
  has(id: string): boolean;
  /** Turns the chat's workspace on. */
  create(id: string): Promise<Workspace | null>;
  /** Reads whether the chat has a workspace, when it opens. */
  load(id: string): Promise<void>;
  /** Side effects of a stream event; true when the event was the workspace's. */
  onEvent(event: StreamEvent, assistant: Message, id: string): boolean;
  /** Runs a command in the chat's terminal (turn.js "Run again"). */
  runInTerminal(id: string, command: string): void;
  /** The result card and trace: open a file, the Files tab, the preview, download the project. */
  openFile(id: string, path: string): void;
  openFiles(id: string): void;
  openPreview(id: string): void;
  download(id: string): void;
  /** A checkpoint's Restore: confirm, POST …/workspace/checkpoints/<sha>/restore, offer Undo. */
  restoreCheckpoint(id: string, sha: string, label: string): void;
  reset(): void;

  // ---- For the panel and the Compute pages ----
  space(id: string): Space;
  /** Disk all of the person's workspaces may use, in bytes (0 when unknown). */
  diskLimit(): number;
  setDiskLimit(bytes: number | undefined): void;
  /** Whether the server has a preview domain (capabilities.features.previews === true). */
  previews(): boolean;
  /** The panel passes what capabilities say about previews. */
  configure(options: { previews?: boolean }): void;
  loadFiles(id: string): Promise<void>;
  /** Opens a file in the Files tab, which is showing already. */
  showFile(id: string, path: string): Promise<void>;
  closeFile(id: string): void;
  /** Asks for a preview link, redeems its ticket and shows it. */
  showPreview(id: string, force?: boolean): Promise<void>;
  /** An HTML file's Open in preview. */
  previewPage(id: string, path: string): void;
  wake(id: string): Promise<void>;
  /** The bar's refresh button for the tab showing. */
  reload(id: string, tab: string): void;
  /** Takes the waiting run_in_terminal command, if any. */
  takeCommand(id: string): string | null;
  /** Gives back a command the terminal could not type yet. */
  putCommand(id: string, command: string): void;
  /** Called with `interrupt` when a command arrives while the terminal is open. */
  onCommand(id: string, listener: () => void): () => void;
  /** Deletes the workspace (the Compute page's trash button). */
  remove(id: string): Promise<void>;
  restoreDialog(): RestoreDialog;
  confirmRestore(): Promise<void>;
  /** The done dialog's Undo: restores the state saved before. */
  undoRestore(): Promise<void>;
  closeRestore(): void;
}

interface Context {
  api: { request: (path: string, options?: any) => Promise<any> };
  enabled: () => boolean;
  changed: () => void;
  showTab: (id: string, tab: string) => void;
  /** Defaults to sonner's toast. */
  toast?: (message: string) => void;
  /** Whether a chat or its Compute page is open; defaults to reading the address. */
  viewing?: (id: string) => boolean;
  /** For tests: setTimeout. */
  wait?: (ms: number) => Promise<void>;
}

const FILE_TOOLS = new Set(["start_server", "write_file", "edit_file"]);
const CODE_TOOLS = new Set(["write_file", "edit_file", "run_command", "run_in_terminal", "sync_code"]);

/** The chat or its Compute page is the one on screen. */
function viewingFromAddress(id: string) {
  if (typeof location === "undefined") return false;
  const path = location.pathname.replace(/\/+$/, "");
  const [, page = "", value = ""] = path.split("/");
  let decoded = value;
  try {
    decoded = decodeURIComponent(value);
  } catch {
    // Keep it as it is.
  }
  return (page === "chat" || page === "workspaces") && decoded === id;
}

/** The preview accepts its ticket only from this page: a credentialed fetch always
 * carries Chat's origin, which a link or another page cannot. It sets the preview's
 * cookie, then the frame or a new tab opens the preview itself. */
async function enter(link: { enter: string; ticket: string }) {
  const response = await fetch(link.enter, { method: "POST", mode: "cors", credentials: "include", body: new URLSearchParams({ ticket: link.ticket }) });
  if (!response.ok) throw new Error((await response.json().catch(() => null))?.error?.message || "The preview could not be opened. Try again.");
}

export function createWorkspaceBridge({ api, enabled, changed, showTab, toast = (m) => sonner(m), viewing = viewingFromAddress, wait = (ms) => new Promise((done) => setTimeout(done, ms)) }: Context): WorkspaceBridge {
  const spaces = new Map<string, Space>();
  const commandListeners = new Map<string, Set<() => void>>();
  let diskLimit = 0;
  let previews = true;
  let polling: ReturnType<typeof setTimeout> | undefined;
  let reloading: ReturnType<typeof setTimeout> | undefined;
  let dialog: RestoreDialog = null;
  const path = workspacePath;

  const entry = (id: string): Space => {
    let space = spaces.get(id);
    if (!space)
      spaces.set(
        id,
        (space = { workspace: null, loaded: false, files: null, filesError: "", loadingFiles: false, file: null, url: "", origin: "", page: "", frame: 0, linking: false, waking: false, wakePolls: 0, command: null, terminal: 0 }),
      );
    return space;
  };

  async function load(id: string) {
    if (!enabled() || !id) return;
    const space = entry(id);
    try {
      const result = await api.request(path(id));
      space.workspace = result.workspace || null;
      space.loaded = true;
      if (result.diskLimit) diskLimit = result.diskLimit;
      if (space.workspace?.status === "running") codeEvents.emit({ id, kind: "refresh", age: 30_000 });
      watch(id);
    } catch {
      space.loaded = true;
      // A check that fails, as while Chat restarts, is tried again: a workspace that
      // is starting keeps its status until Chat says it runs or could not start.
      watch(id);
    }
    changed();
  }

  async function create(id: string) {
    const space = entry(id);
    const result = await api.request(path(id), { method: "POST" });
    space.workspace = result.workspace || null;
    space.loaded = true;
    watch(id);
    showTab(id, "preview");
    changed();
    return space.workspace;
  }

  /** Keeps checking while the pod wakes, then briefly while Chat restarts its dev server. */
  function watch(id: string) {
    clearTimeout(polling);
    const space = spaces.get(id);
    if (!space?.workspace || !viewing(id)) return;
    const w = space.workspace,
      waking = w.status === "starting" || w.status === "stopped";
    if (waking) space.wakePolls = 0;
    else if (w.status !== "running" || w.previewPort || (space.wakePolls = space.wakePolls + 1) > 20) return;
    polling = setTimeout(() => void load(id), 2000);
  }

  async function showPreview(id: string, force = false) {
    const space = entry(id);
    if (!previews || (!space.workspace?.previewPort && !force) || space.linking) return;
    space.linking = true;
    try {
      const page = space.page ? `?path=${encodeURIComponent(space.page)}` : "";
      const link = await api.request(`${path(id)}/preview${page}`, { method: "POST" });
      await enter(link);
      space.url = link.url;
      space.origin = link.origin;
      space.frame++;
    } catch (error) {
      toast((error as Error).message);
    } finally {
      space.linking = false;
      changed();
    }
  }

  async function loadFiles(id: string) {
    const space = entry(id);
    if (space.loadingFiles) return;
    space.loadingFiles = true;
    try {
      space.files = (await api.request(`${path(id)}/files`)).entries || [];
      space.filesError = "";
    } catch (error) {
      space.files = [];
      space.filesError = (error as Error).message;
    } finally {
      space.loadingFiles = false;
      changed();
    }
  }

  async function showFile(id: string, filePath: string) {
    const space = entry(id);
    if (imageFile(filePath)) {
      space.file = { path: filePath, image: rawUrl(id, filePath) };
      changed();
      return;
    }
    space.file = { path: filePath, content: null };
    changed();
    try {
      const result = await api.request(`${path(id)}/file?path=${encodeURIComponent(filePath)}`);
      if (space.file?.path === filePath) space.file = { path: filePath, content: result.content ?? "", truncated: !!result.truncated };
    } catch (error) {
      if (space.file?.path === filePath) space.file = { path: filePath, content: "", error: (error as Error).message };
    }
    changed();
  }

  async function wake(id: string) {
    const space = entry(id);
    if (space.waking) return;
    space.waking = true;
    space.url = "";
    space.origin = "";
    changed();
    try {
      await create(id);
      // Chat starts the pod after answering, so the record may still say asleep.
      if (space.workspace?.status === "stopped") space.workspace.status = "starting";
      space.wakePolls = 0;
      watch(id);
    } catch (error) {
      toast((error as Error).message);
    } finally {
      space.waking = false;
      changed();
    }
  }

  /** Downloads the project, waking an asleep workspace first. */
  async function download(id: string) {
    const space = entry(id);
    if (!space.workspace) return;
    if (space.workspace.status !== "running") {
      toast("Waking the workspace to download the project…");
      try {
        await create(id);
      } catch (error) {
        toast((error as Error).message);
        return;
      }
      for (let i = 0; i < 45 && space.workspace?.status !== "running"; i++) {
        await wait(2000);
        await load(id);
      }
      if (space.workspace?.status !== "running") {
        toast("The workspace has not woken yet. Try again in a moment.");
        return;
      }
    }
    startDownload(downloadUrl(id));
  }

  function runInTerminal(id: string, command: string) {
    const space = entry(id);
    if (!space.workspace || !command) return;
    space.command = command;
    showTab(id, "terminal");
    changed();
    commandListeners.get(id)?.forEach((l) => l());
  }

  async function restore(undo: boolean) {
    const d = dialog;
    if (!d || d.busy) return;
    const sha = undo ? d.before || "" : d.sha;
    dialog = { ...d, busy: true };
    changed();
    let result: { before?: string } | undefined;
    try {
      result = await api.request(`${path(d.id)}/checkpoints/${encodeURIComponent(sha)}/restore`, { method: "POST" });
    } catch (error) {
      dialog = null;
      changed();
      toast((error as Error).message);
      return;
    }
    const space = spaces.get(d.id);
    if (space) {
      space.files = null;
      space.file = null;
    }
    void showPreview(d.id, true);
    if (undo) {
      dialog = null;
      toast("Back to where you were");
    } else dialog = { ...d, step: "done", before: result?.before || "", busy: false };
    changed();
  }

  return {
    enabled,
    has: (id) => !!spaces.get(id)?.workspace,
    create,
    load,
    onEvent(event, _assistant, id) {
      if (event.event === "tool") {
        const data = event.data || {};
        const space = spaces.get(id);
        if (FILE_TOOLS.has(data.tool) && space) space.files = null;
        if (CODE_TOOLS.has(data.tool) && data.status !== "running") codeEvents.emit({ id, kind: "changed" });
        // A plain web server has no live reload, so reload the preview after each change.
        if (data.reload) {
          clearTimeout(reloading);
          reloading = setTimeout(() => void showPreview(id), 600);
        }
        if (data.tool === "run_in_terminal" && data.status === "completed") runInTerminal(id, data.command);
        // Workers that needed files turned the chat's workspace on.
        if (data.tool === "delegate" && data.workspace && !spaces.get(id)?.workspace) void load(id);
        if (space && space.files === null) changed();
        return true;
      }
      if (event.event === "draft") return true;
      if (event.event === "preview") {
        const space = entry(id);
        if (space.workspace) space.workspace = { ...space.workspace, previewPort: event.data?.port, status: "running" };
        showTab(id, "preview");
        void showPreview(id, true);
        changed();
        return true;
      }
      return false;
    },
    runInTerminal,
    openFile(id, filePath) {
      const space = spaces.get(id);
      if (!space?.workspace) return;
      space.files = null;
      showTab(id, "files");
      void showFile(id, filePath);
    },
    openFiles(id) {
      const space = spaces.get(id);
      if (!space?.workspace) return;
      space.file = null;
      space.files = null;
      showTab(id, "files");
      changed();
    },
    openPreview(id) {
      const space = spaces.get(id);
      if (!space?.workspace || !previews) return;
      showTab(id, "preview");
      void showPreview(id, true);
    },
    download: (id) => void download(id),
    restoreCheckpoint(id, sha, label) {
      dialog = { id, sha, label, step: "confirm" };
      changed();
    },
    reset() {
      clearTimeout(polling);
      clearTimeout(reloading);
      spaces.clear();
      dialog = null;
    },

    space: entry,
    diskLimit: () => diskLimit,
    setDiskLimit(bytes) {
      if (bytes) diskLimit = bytes;
    },
    previews: () => previews,
    configure(options) {
      if (options.previews !== undefined && options.previews !== previews) {
        previews = options.previews;
        changed();
      }
    },
    loadFiles,
    showFile,
    closeFile(id) {
      entry(id).file = null;
      changed();
    },
    showPreview,
    previewPage(id, page) {
      const space = entry(id);
      if (!space.workspace) return;
      space.page = page;
      space.file = null;
      showTab(id, "preview");
      void showPreview(id, true);
    },
    wake,
    reload(id, tab) {
      const space = entry(id);
      if (tab === "terminal") space.terminal++;
      else if (tab === "files") {
        space.files = null;
        space.file = null;
      } else if (space.workspace && previewReload(space.workspace).wake) void wake(id);
      else void showPreview(id);
      changed();
    },
    takeCommand(id) {
      const space = spaces.get(id);
      const command = space?.command || null;
      if (space) space.command = null;
      return command;
    },
    putCommand(id, command) {
      entry(id).command = command;
    },
    onCommand(id, listener) {
      let set = commandListeners.get(id);
      if (!set) commandListeners.set(id, (set = new Set()));
      set.add(listener);
      return () => {
        set.delete(listener);
      };
    },
    async remove(id) {
      await api.request(path(id), { method: "DELETE" });
      spaces.delete(id);
      changed();
    },
    restoreDialog: () => dialog,
    confirmRestore: () => restore(false),
    undoRestore: () => restore(true),
    closeRestore() {
      if (dialog?.busy) return;
      dialog = null;
      changed();
    },
  };
}
