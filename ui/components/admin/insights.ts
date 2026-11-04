// Usage insights for administrators: what kind of work chats were, from the labels Chat
// keeps after a chat's first reply. Nothing here shows a chat, a message or a person.

import { titleCase } from "@/components/manage/kit";

export const INSIGHT_PERIODS = [7, 28, 90] as const;
/** Kinds of use, in a fixed order, each with its project color. */
export const KINDS = [
  ["research", "Research", "green"],
  ["learning", "Learning", "blue"],
  ["work", "Work", "violet"],
  ["personal", "Personal", "gold"],
  ["play", "Play", "rose"],
  ["trying_it_out", "Trying it out", "gray"],
] as const;
export const kindColor = (color: string) => `var(--project-${color})`;
const NAMES: Record<string, Record<string, string>> = {
  activity: {
    coding: "Coding",
    data_analysis: "Data analysis",
    writing: "Writing",
    research: "Finding information",
    explaining: "Explaining a topic",
    computing_help: "Computing help",
    planning_admin: "Planning and admin",
    creative: "Creative work",
    other: "Other",
  },
  field: {
    computing: "Computing",
    engineering: "Engineering",
    physical_sciences: "Physical sciences and math",
    life_health: "Life and health sciences",
    social_sciences: "Social sciences",
    business: "Business",
    humanities_arts: "Humanities and arts",
    education: "Education",
    law_policy: "Law and policy",
    general: "No particular field",
  },
  output: { code: "Code", document: "Documents", analysis: "Analysis", answer: "Answers", creative: "Creative pieces", plan: "Plans and lists", none: "Nothing in particular" },
  kind: Object.fromEntries(KINDS.map(([id, name]) => [id, name])),
};
export const FLAGS = [
  ["seeks_harm", "Harmful or illegal requests"],
  ["jailbreak", "Attempts to get around a model’s rules"],
  ["explicit", "Sexual or explicit content"],
  ["harassment_hate", "Harassment or hate"],
  ["integrity", "Graded coursework done for them"],
  ["secrets", "Pasted keys or passwords"],
  ["self_harm", "Mentions of self-harm"],
] as const;

export const nameOf = (group: string, id: string | null | undefined) => NAMES[group]?.[id || ""] || titleCase(id || "unknown");
export const num = (n: number | null | undefined) => Number(n || 0).toLocaleString();
/** 950, 48K, 1.3M. */
export const tokens = (value: number | null | undefined) => {
  const n = Number(value || 0);
  return n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e4 ? `${Math.round(n / 1e3)}K` : n.toLocaleString();
};
export const share = (n: number | null | undefined, total: number | null | undefined) => (total ? Number(n || 0) / Number(total) : 0);
export const percent = (x: number) => (x > 0 && x < 0.005 ? "<1%" : `${Math.round(x * 100)}%`);
export const plural = (n: number | null | undefined, one: string, many: string) => `${num(n)} ${Number(n) === 1 ? one : many}`;

export type Change = null | { same: true; word: string } | { same: false; up: boolean; amount: string; word: string };
/** The change from the period before; null when there is nothing to compare. */
export function change(now: number | null | undefined, before: number | null | undefined, days: number): Change {
  const a = Number(now || 0);
  const b = Number(before || 0);
  if (!b) return null;
  const x = (a - b) / b;
  const word = days === 1 ? "yesterday" : `the ${days} days before`;
  if (Math.abs(x) < 0.005) return { same: true, word };
  return { same: false, up: x > 0, amount: percent(Math.abs(x)), word };
}
export const changeText = (c: Change) => (!c ? "" : c.same ? `Same as ${c.word}` : `${c.up ? "↑" : "↓"} ${c.amount} on ${c.word}`);

/** One chat holding at least half of a period's tokens flattens every token share. */
export const outlier = (data: { sessions?: number; largestChat?: number; tokens?: number }) =>
  Number(data.sessions) >= 3 && Number(data.largestChat) >= 0.5 * Number(data.tokens) ? share(Number(data.largestChat), Number(data.tokens)) : 0;

export interface KindCount {
  name: string;
  sessions: number;
  tokens?: number;
}
export interface Workstream {
  name: string;
  description: string;
  private?: boolean;
  sessions: number;
  tokens: number;
  kinds?: Record<string, number>;
  outputs?: Record<string, number>;
  rows?: { topic: string; kind: string; output: string; field: string; turns: number; tokens: number }[];
}
export interface InsightsSettings {
  enabled?: boolean;
  titles?: boolean;
  model?: string | null;
  reportModel?: string | null;
}
export interface Insights {
  settings?: InsightsSettings;
  gateway?: boolean;
  groups?: { id: string; name: string }[];
  sessions?: number;
  people?: number;
  tokens?: number;
  minPeople?: number;
  tooFew?: boolean;
  lastLabeledAt?: string;
  largestChat?: number;
  kinds?: KindCount[];
  fields?: KindCount[];
  activities?: KindCount[];
  outputs?: KindCount[];
  flags?: Record<string, number>;
  previous?: { sessions?: number; people?: number; tokens?: number };
  trend?: { unit: "day" | "week"; points: { at: string; kinds?: Record<string, number> }[] };
  workstreams?: {
    state?: { status?: string; building?: boolean; model?: string; foundAt?: string; error?: string; runs?: boolean };
    waiting?: number;
    list?: Workstream[];
  };
}
