// The workspace's data shapes (api/src/sandbox) and the small rules the panel and
// the Compute pages share.

/** A conversation's workspace: a project folder in its owner's pod (api/src/sandbox Sandbox). */
export interface Workspace {
  id?: string;
  conversationId?: string;
  /** starting, running, stopped (asleep) or error: the pod's. */
  status?: string;
  previewPort?: number | null;
  errorMessage?: string | null;
  /** The owner's whole workspace volume, when last measured. */
  diskBytes?: number | null;
  createdAt?: string;
  lastUsedAt?: string;
  /** On GET /api/workspaces: the conversation's title. */
  title?: string;
  [key: string]: unknown;
}

/** One entry of GET …/workspace/files. */
export interface FileEntry {
  path: string;
  type?: "file" | "directory" | string;
  size?: number;
}

/** The file the Files tab shows. */
export interface OpenFile {
  path: string;
  /** Image files show from …/workspace/raw. */
  image?: string;
  /** null while it loads. */
  content?: string | null;
  truncated?: boolean;
  error?: string;
}

/** The preview's reload button reloads a running app; with none running it wakes the
 * workspace, and Chat restarts the app once the workspace is up. */
export function previewReload(w: Workspace, waking = false) {
  if (w.previewPort) return { label: "Reload preview", wake: false, disabled: false };
  const label = ({ stopped: "Wake the workspace", error: "Start the workspace again" } as Record<string, string>)[w.status || ""] || "Start the preview";
  return { label, wake: true, disabled: waking || w.status === "starting" };
}

export const statusLabel = (w: Pick<Workspace, "status">) =>
  ({ starting: "Starting", running: "Running", stopped: "Asleep", error: "Error" } as Record<string, string>)[w.status || ""] || w.status || "";

export const statusTone = (w: Pick<Workspace, "status">): "accent" | "amber" | "" => (w.status === "running" ? "accent" : w.status === "error" ? "amber" : "");

export const formatBytes = (n: number | null | undefined) =>
  n == null ? "" : n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1048576).toFixed(1)} MB`;

export const GiB = 2 ** 30;
/** "1.2 / 10 GB", and whether every workspace together is over the limit. */
export function diskUsage(used: number | null | undefined, limit: number) {
  if (used == null || !limit) return null;
  return { text: `${(used / GiB).toFixed(1)} / ${Math.round(limit / GiB)} GB`, of: `${(used / GiB).toFixed(1)} of ${Math.round(limit / GiB)} GB used`, full: used > limit };
}

export const imageFile = (path: string) => /\.(png|jpe?g|gif|webp|svg)$/i.test(path);
export const htmlFile = (path: string) => /\.html?$/i.test(path);
export const csvFile = (path: string) => /\.csv$/i.test(path);

/** A small CSV reader for the file viewer: quoted fields, up to `limit` rows. */
export function csvRows(text: string, limit: number): string[][] {
  const rows: string[][] = [];
  let row: string[] = [],
    field = "",
    quoted = false;
  for (let i = 0; i < text.length && rows.length < limit; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') {
        field += '"';
        i++;
      } else if (c === '"') quoted = false;
      else field += c;
    } else if (c === '"') quoted = true;
    else if (c === ",") {
      row.push(field);
      field = "";
    } else if (c === "\n" || c === "\r") {
      if (c === "\r" && text[i + 1] === "\n") i++;
      row.push(field);
      rows.push(row);
      row = [];
      field = "";
    } else field += c;
  }
  if ((field || row.length) && rows.length < limit) {
    row.push(field);
    rows.push(row);
  }
  return rows;
}

/** The CSV as a header, up to 199 body rows, and whether there was more. */
export function csvTable(text: string, truncated = false) {
  const rows = csvRows(text, 201);
  const [header = [], ...body] = rows;
  return { header, body: body.slice(0, 199), more: rows.length > 200 || truncated };
}

/** A file tree entry's depth and name. */
export const treeItem = (e: FileEntry) => ({ depth: e.path.split("/").length - 1, name: e.path.split("/").pop() || e.path });

export const workspacePath = (id: string) => `/api/conversations/${encodeURIComponent(id)}/workspace`;
/** The whole project as a zip; with `paths`, only those files and folders (one file comes as itself). */
export const downloadUrl = (id: string, paths: string[] = []) =>
  `${workspacePath(id)}/download${paths.length ? `?${paths.map((p) => `path=${encodeURIComponent(p)}`).join("&")}` : ""}`;

/** Whether a folder above `path` is among `picked`, so `path` comes with it. */
export const pickedAbove = (picked: ReadonlySet<string>, path: string) => {
  for (let at = path.lastIndexOf("/"); at > 0; at = path.lastIndexOf("/", at - 1)) if (picked.has(path.slice(0, at))) return true;
  return false;
};

/** What to ask for: the picked files and folders, without what a picked folder already holds. */
export const pickedPaths = (picked: ReadonlySet<string>) => [...picked].filter((p) => !pickedAbove(picked, p)).sort();

/** The most a download's address may name; past it, folders are picked instead of their files. */
export const PICKED_URL_LIMIT = 6000;
export const rawUrl = (id: string, path: string) => `${workspacePath(id)}/raw?path=${encodeURIComponent(path)}`;
export const terminalUrl = (id: string, loc: { protocol: string; host: string } = location) =>
  `${loc.protocol === "https:" ? "wss" : "ws"}://${loc.host}${workspacePath(id)}/terminal`;

/** Starts a download of a same-origin URL the way a link with `download` does. */
export function startDownload(href: string) {
  const link = document.createElement("a");
  link.href = href;
  link.download = "";
  document.body.append(link);
  link.click();
  link.remove();
}
