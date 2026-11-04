// Code in projects: the shapes api/src/code returns, and the diff and
// conflict readers the Git tab and the project's Code tab share.

export interface ChatRef {
  id: string;
  title?: string | null;
  ownerName?: string;
  readable?: boolean;
  mine?: boolean;
}

/** One commit in a log (api/src/code/history.rs entry_json). `time` is in seconds. */
export interface CommitEntry {
  sha: string;
  parents?: string[];
  author: string;
  time: number;
  subject: string;
  chat?: ChatRef | null;
  turn?: string | null;
}

export interface ChangedFile {
  path: string;
  /** added, modified, deleted or conflict. */
  status?: string;
  /** null for a binary file. */
  added?: number | null;
  removed?: number | null;
}

/** GET /api/conversations/:id/code */
export interface ChatCode {
  project: { id: string; name: string; mine?: boolean } | null;
  workspace?: boolean;
  running?: boolean;
  main?: { sha: string; commits: number } | null;
  merging?: boolean;
  conflicts?: string[];
  changes?: ChangedFile[];
  behind?: number;
  ahead?: number;
  draft?: string;
  log?: CommitEntry[];
}

/** GET /api/projects/:id/code */
export interface CodeOverview {
  main: { sha: string; commits: number; bytes: number; updatedBy?: string; updatedAt?: string } | null;
  origin?: { url: string; sha?: string | null; history?: boolean | null } | null;
  chats: { chat: ChatRef; syncedAt?: string | null; behind?: number }[];
  exports: { kind: string; target: string; url: string; at?: string; behind?: number }[];
  limits: { bytes: number };
}

export interface CommitDetail {
  commit: CommitEntry;
  body?: string;
  files: ChangedFile[];
}

export interface CompareResult {
  from: CommitEntry;
  to: CommitEntry;
  total: number;
  commits: CommitEntry[];
  files: ChangedFile[];
}

export interface BlameGroup {
  sha: string;
  start: number;
  lines: string[];
  author: string;
  time: number;
  subject: string;
  chat?: ChatRef | null;
  turn?: string | null;
}

export interface LineVersion extends CommitEntry {
  patch: string;
}

export const LETTER: Record<string, string> = { added: "A", modified: "M", deleted: "D", conflict: "!" };
export const short = (sha: string | null | undefined) => String(sha || "").slice(0, 7);
export const count = (n: number | null | undefined, one: string, many = `${one}s`) => `${Number(n || 0).toLocaleString()} ${n === 1 ? one : many}`;
export const initial = (name: string | null | undefined) => String(name || "?").trim().slice(0, 1).toUpperCase() || "?";
export const bytes = (n: number | null | undefined) => (n == null ? "" : n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1048576).toFixed(1)} MB`);
/** A project name as a repository or file name. */
export const slug = (name: string | null | undefined) =>
  String(name || "project")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 80) || "project";

/** "just now", "5 min ago", "3 h ago", "yesterday", "4 d ago"; numbers are seconds. */
export function ago(value: string | number | null | undefined, now = Date.now()) {
  if (value == null || value === "") return "";
  const t = typeof value === "number" ? value * 1000 : new Date(value).getTime(),
    m = Math.round((now - t) / 60000);
  return m < 1 ? "just now" : m < 60 ? `${m} min ago` : m < 1440 ? `${Math.round(m / 60)} h ago` : m < 2880 ? "yesterday" : `${Math.round(m / 1440)} d ago`;
}

export type DiffRow = { kind: "hunk" | "more" | "note"; text: string; n?: undefined } | { kind: "add" | "ctx"; n: number; text: string } | { kind: "del"; n: ""; text: string };

/** A unified diff as rows, numbered by the new file's lines. */
export function diffRows(text: string | null | undefined, limit = 4000): DiffRow[] {
  const rows: DiffRow[] = [];
  let line = 0;
  for (const raw of String(text || "").split("\n")) {
    if (rows.length >= limit) {
      rows.push({ kind: "more", text: "The rest of this diff is not shown." });
      break;
    }
    if (/^(diff --git|diff --cc|index |--- |\+\+\+ |new file mode|deleted file mode|similarity |rename |old mode|new mode)/.test(raw)) continue;
    const hunk = raw.match(/^@@+ [^@]*\+(\d+)(?:,\d+)? @@+ ?(.*)$/);
    if (hunk) {
      line = Number(hunk[1]);
      rows.push({ kind: "hunk", text: hunk[2] ? `line ${hunk[1]} · ${hunk[2]}` : `line ${hunk[1]}` });
      continue;
    }
    if (raw.startsWith("Binary files")) {
      rows.push({ kind: "note", text: "A binary file changed." });
      continue;
    }
    if (raw.startsWith("\\")) continue;
    if (raw.startsWith("+")) rows.push({ kind: "add", n: line++, text: raw.slice(1) });
    else if (raw.startsWith("-")) rows.push({ kind: "del", n: "", text: raw.slice(1) });
    else if (raw.startsWith(" ") || (raw === "" && rows.length && rows[rows.length - 1]!.kind !== "hunk" && line)) rows.push({ kind: "ctx", n: line++, text: raw.slice(1) });
  }
  while (rows.length && rows[rows.length - 1]!.kind === "ctx" && rows[rows.length - 1]!.text === "") rows.pop();
  return rows;
}

export interface ConflictRegion {
  line: number;
  mine: string[];
  theirs: string[];
}
/** The two sides of each conflict in a file with conflict markers. */
export function conflictRegions(text: string | null | undefined, limit = 3): ConflictRegion[] {
  const regions: ConflictRegion[] = [];
  let current: ConflictRegion | null = null,
    side: "mine" | "theirs" | "base" | null = null,
    n = 0;
  for (const line of String(text || "").split("\n")) {
    n++;
    if (line.startsWith("<<<<<<<")) {
      current = { line: n, mine: [], theirs: [] };
      side = "mine";
      continue;
    }
    if (current && line.startsWith("|||||||")) {
      side = "base";
      continue;
    }
    if (current && line.startsWith("=======")) {
      side = "theirs";
      continue;
    }
    if (current && line.startsWith(">>>>>>>")) {
      regions.push(current);
      current = null;
      if (regions.length >= limit) break;
      continue;
    }
    if (current && side && side !== "base") current[side].push(line);
  }
  return regions;
}

/** What Sync will do, as the Git tab says it under the button. */
export function syncSays(d: Pick<ChatCode, "behind" | "ahead">, changes: number) {
  const behind = d.behind || 0;
  return behind && (changes || d.ahead)
    ? `Sync saves your changes, brings in the project’s ${count(behind, "new commit")}, and shares yours with everyone.`
    : behind
      ? `Sync brings in the project’s ${count(behind, "new commit")}.`
      : changes || d.ahead
        ? "Sync saves your changes and shares them with everyone in the project."
        : "Everything here is in the project.";
}

/** The prompt "Resolve with the model" sends. */
export function resolvePrompt(files: string[]) {
  return `Sync paused because ${files.length === 1 ? `\`${files[0]}\` was` : `${files.map((f) => `\`${f}\``).join(", ")} were`} changed both in this chat and in the project. In each file, combine the two versions between the conflict markers (<<<<<<<, =======, >>>>>>>) so that both changes still work, and remove the markers. Don't run git commands. Then tell me briefly what you kept from each side, and I'll finish the sync.`;
}

/** The prompt of "Undo in a new chat", when an undo can't be made on its own. */
export function undoInChatPrompt(sha: string, subject: string) {
  return `Undo commit ${short(sha)} (“${subject}”): take back what it changed, but keep the work that came after it. Run \`git show ${short(sha)}\` to see it. Tell me what you changed, then I’ll sync.`;
}

export const projectCodePath = (id: string) => `/api/projects/${encodeURIComponent(id)}/code`;
export const chatCodePath = (id: string) => `/api/conversations/${encodeURIComponent(id)}/code`;

/** Where the project Code tab is, from the path after /projects/<id>/code. */
export type CodeRoute = { view: "overview" } | { view: "history"; sha: string } | { view: "files"; path: string } | { view: "compare"; from: string; to: string };
export function codeRoute(rest: string[]): CodeRoute {
  const [view, ...more] = rest;
  const arg = more
    .map((p) => {
      try {
        return decodeURIComponent(p);
      } catch {
        return p;
      }
    })
    .join("/");
  if (view === "history") return { view, sha: arg };
  if (view === "files") return { view, path: arg };
  if (view === "compare") {
    const [from = "", to = "main"] = arg.split("...");
    return { view, from, to: to || "main" };
  }
  return { view: "overview" };
}
export const codeHref = (projectId: string, to: CodeRoute | "history" | "files" = { view: "overview" }) => {
  const base = `/projects/${encodeURIComponent(projectId)}/code`;
  const r: CodeRoute = typeof to === "string" ? (to === "history" ? { view: "history", sha: "" } : { view: "files", path: "" }) : to;
  if (r.view === "history") return r.sha ? `${base}/history/${encodeURIComponent(r.sha)}` : `${base}/history`;
  if (r.view === "files") return r.path ? `${base}/files/${encodeURIComponent(r.path)}` : `${base}/files`;
  if (r.view === "compare") return `${base}/compare/${encodeURIComponent(r.from)}...${encodeURIComponent(r.to)}`;
  return base;
};
