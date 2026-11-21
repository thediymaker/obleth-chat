// Home's two additions: what the person means to do with a new chat (ask, build,
// research, run on a cluster), which sets up the box and its starters, and "Running
// now": the chats, jobs and workspaces that are alive without them.

import { ago, count } from "@/lib/format";

export type Intent = "ask" | "build" | "research" | "cluster";

export interface IntentInfo {
  id: Intent;
  label: string;
  placeholder: string;
  /** What choosing it sets up, in a sentence. */
  hint: string;
  /** The new chat starts with a workspace. */
  workspace: boolean;
  /** Icon, title, a line about it and the start of a prompt. */
  starters: [string, string, string, string][];
}

/** The intents this installation offers. Build and Research need workspaces; a cluster, one that is connected. */
export function intents({ workspaces, clusters }: { workspaces: boolean; clusters: { name: string; confirmActions?: boolean }[] }): IntentInfo[] {
  const cluster = clusters[0]?.name ?? "";
  const asks = clusters.length > 0 && clusters.every((c) => c.confirmActions);
  const list: (IntentInfo | false)[] = [
    {
      id: "ask",
      label: "Ask",
      placeholder: "Ask a question, write some code, follow an idea…",
      hint: "A chat. Nothing is started for you.",
      workspace: false,
      starters: [
        ["chat", "Think it through", "Find a fresh way into a tricky problem.", "Help me think through a problem. Ask me what I am working on."],
        ["book", "Learn something", "Make a complex idea feel approachable.", "Help me learn something new. Ask what topic interests me."],
        ["code", "Plan a project", "Go from a question to working code.", "Help me plan a small coding project. Start by asking what I want to build."],
      ],
    },
    workspaces && {
      id: "build",
      label: "Build",
      placeholder: "Describe the site or app you want…",
      hint: "A workspace starts with your first message, with a live preview beside the chat.",
      workspace: true,
      starters: [
        ["terminal", "Build an app", "Describe it, then watch it run in a live preview.", "Build me a site for "],
        ["branch", "Explore a repo", "Get a tour of a codebase, or a list of its issues.", "Explain this codebase to me: "],
        ["file", "Work on a file", "Attach data or a document and make something of it.", "Using the file I attach, "],
      ],
    },
    workspaces && {
      id: "research",
      label: "Research",
      placeholder: "Say what to improve, and the number that shows it got better…",
      hint: "The model offers to set it up as research: each try on its own branch, measured the same way.",
      workspace: true,
      starters: [
        ["bolt", "Make it faster", "Try several ways and keep the quickest.", "Make this as fast as it will go, measuring every try: "],
        ["grid", "Sweep a setting", "Find the value that does best.", "Find the best value of this setting by trying a range of them: "],
        ["chart", "Raise a score", "Push accuracy up over many measured tries.", "Improve the accuracy of this as far as it will go: "],
      ],
    },
    clusters.length > 0 && {
      id: "cluster",
      label: "Run on a cluster",
      placeholder: "Describe the job: what to run, on what, for how long…",
      hint: `The model works with ${clusters.length === 1 ? cluster : `${clusters.length} clusters`}${asks ? ", and asks before anything that changes a job." : "."}`,
      workspace: false,
      starters: [
        ["server", "See what is running", "Your jobs, and how busy the queue is.", `Using the ${cluster} cluster, show my jobs and how busy the queue is.`],
        ["terminal", "Submit a job", "Describe the run; the script is written for you.", `Using the ${cluster} cluster, submit a job that `],
        ["clock", "Why is it waiting?", "Find out what holds a pending job.", `Using the ${cluster} cluster, explain why my pending jobs are waiting.`],
      ],
    },
  ];
  return list.filter((i): i is IntentInfo => !!i);
}

/**
 * The intent the tabs show. The workspace switch in the box can be flipped on its
 * own, so an intent that needs a workspace falls back to Ask without one, and Ask
 * with one reads as Build.
 */
export function shownIntent(chosen: Intent, workspaceOn: boolean, offered: IntentInfo[]): Intent {
  const has = (id: Intent) => offered.some((i) => i.id === id);
  const info = offered.find((i) => i.id === chosen);
  if (!info) return "ask";
  if (info.workspace && !workspaceOn) return "ask";
  if (chosen === "ask" && workspaceOn && has("build")) return "build";
  return chosen;
}

export interface RunningItem {
  key: string;
  kind: "waiting" | "working" | "jobs" | "workspace";
  /** The small line over the title. */
  eyebrow: string;
  title: string;
  detail: string;
  href: string;
}

interface Chat {
  id: string;
  title: string;
  working?: boolean;
  waiting?: boolean;
  waitingTool?: string | null;
}
interface Space {
  conversationId?: string;
  status?: string;
  title?: string;
  previewPort?: number | null;
  lastUsedAt?: string;
}
interface Queue {
  cluster: { id: string; name: string };
  running: number;
  pending: number;
  jobs: { name: string }[];
}

const chatHref = (id: string) => `/chat/${encodeURIComponent(id)}`;

/**
 * What is alive right now, the things that wait for the person first: chats asking
 * for a yes or no, chats at work, jobs on clusters, and the workspaces used last.
 */
export function runningNow({ chats = [], workspaces = [], queues = [], now = Date.now(), limit = 6 }: { chats?: Chat[]; workspaces?: Space[]; queues?: Queue[]; now?: number; limit?: number }): RunningItem[] {
  const waiting = chats
    .filter((c) => c.working && c.waiting)
    .map((c): RunningItem => ({ key: `chat-${c.id}`, kind: "waiting", eyebrow: "Waiting on you", title: c.title, detail: c.waitingTool ? `Asks to run ${c.waitingTool}` : "Asks for a yes or no", href: chatHref(c.id) }));
  const working = chats.filter((c) => c.working && !c.waiting).map((c): RunningItem => ({ key: `chat-${c.id}`, kind: "working", eyebrow: "Working", title: c.title, detail: "A reply is being written", href: chatHref(c.id) }));
  const jobs = queues
    .filter((q) => q.running + q.pending > 0)
    .map((q): RunningItem => {
      const names = q.jobs.map((j) => j.name).filter(Boolean);
      return {
        key: `jobs-${q.cluster.id}`,
        kind: "jobs",
        eyebrow: `Jobs · ${q.cluster.name}`,
        title: [q.running ? `${q.running} running` : "", q.pending ? `${q.pending} waiting` : ""].filter(Boolean).join(" · "),
        detail: names.length > 2 ? `${names.slice(0, 2).join(", ")} and ${count(names.length - 2, "other")}` : names.join(", "),
        href: "/clusters",
      };
    });
  // A chat already listed as working says enough; its workspace is not listed again.
  const listed = new Set([...waiting, ...working].map((i) => i.key));
  const spaces = workspaces
    .filter((w) => w.conversationId && (w.status === "running" || w.status === "starting") && !listed.has(`chat-${w.conversationId}`))
    .sort((a, b) => Date.parse(b.lastUsedAt || "") - Date.parse(a.lastUsedAt || ""))
    .slice(0, 2)
    .map((w): RunningItem => ({
      key: `space-${w.conversationId}`,
      kind: "workspace",
      eyebrow: w.status === "starting" ? "Workspace · starting" : w.previewPort ? "Workspace · preview is up" : "Workspace · awake",
      title: w.title || "Untitled chat",
      detail: [w.previewPort ? `Port ${w.previewPort}` : "", w.lastUsedAt ? `used ${ago(w.lastUsedAt, now)}` : ""].filter(Boolean).join(" · "),
      href: chatHref(w.conversationId!),
    }));
  return [...waiting, ...working, ...jobs, ...spaces].slice(0, limit);
}
