// Usage and limits for administrators: requests and tokens over a period, by model and by
// person, who is near a daily allowance, and, from Insights labels, what the tokens went to.

export const USAGE_PERIODS = [
  [1, "Today"],
  [7, "7 days"],
  [28, "28 days"],
  [90, "90 days"],
] as const;
export const PURPOSES: Record<string, string> = { chat: "Chat replies", follow_up: "Suggested follow-ups", agent: "Agents" };
export const SORTS = [
  ["totalTokens", "Tokens"],
  ["requests", "Requests"],
  ["errors", "Errors"],
  ["lastActive", "Last active"],
] as const;
export const METRICS = [
  ["tokens", "Tokens"],
  ["requests", "Requests"],
  ["people", "People"],
] as const;
export type Metric = (typeof METRICS)[number][0];
export type Sort = (typeof SORTS)[number][0];
export const periodName = (days: number) => (days === 1 ? "today" : `the last ${days} days`);

export interface Counts {
  requests?: number;
  promptTokens?: number;
  completionTokens?: number;
  totalTokens?: number;
  errors?: number;
  activeGenerations?: number;
  incompleteUsage?: number;
  reservedTokens?: number;
  people?: number;
}
export interface Limits {
  dailyRequests?: number | null;
  dailyTokens?: number | null;
  concurrentRequests?: number | null;
}
export interface UsagePerson extends Counts {
  id: string;
  username: string;
  displayName?: string;
  enabled?: boolean;
  lastActive?: string;
  override?: boolean;
  today?: Counts & { fraction?: number };
  limits?: Limits;
  atLimit?: boolean;
}
export interface KindsRow {
  name: string;
  sessions?: number;
  tokens?: number;
}
export interface UsageOverview {
  days?: number;
  start: string;
  groups?: { id: string; name: string }[];
  models?: string[];
  totals?: Counts;
  previous?: Counts;
  series?: (Counts & { at: string })[];
  byModel?: (Counts & { name: string | null })[];
  byPurpose?: (Counts & { name: string })[];
  people?: UsagePerson[];
  limited?: UsagePerson[];
  hasMore?: boolean;
  limits?: Limits;
  overrides?: number;
  atLimit?: number;
  nearLimit?: number;
  mix?: {
    enabled?: boolean;
    tooFew?: boolean;
    sessions?: number;
    people?: number;
    tokens?: number;
    minPeople?: number;
    kinds?: KindsRow[];
    byModel?: { model: string | null; sessions: number; tokens: number; kinds?: KindsRow[] }[];
    heavy?: { top: { people: number; tokens: number; kinds?: KindsRow[] }; rest: { people: number; tokens: number; kinds?: KindsRow[] } };
  };
}

export interface Bucket {
  key: string;
  label: string;
  input: number;
  output: number;
  tokens: number;
  requests: number;
  people: number;
  errors: number;
}
/** Every day (or hour, for today) in the period, with zeros where nothing was used. */
export function buckets(series: UsageOverview["series"], days: number, start: string): Bucket[] {
  const by = new Map((series || []).map((b) => [b.at, b]));
  const from = new Date(start);
  const n = days === 1 ? 24 : days;
  return Array.from({ length: n }, (_, i) => {
    const at = new Date(from.getTime() + i * (days === 1 ? 3600e3 : 86400e3));
    const key = days === 1 ? at.toISOString().slice(0, 13) : at.toISOString().slice(0, 10);
    const b = by.get(key) || ({} as Counts);
    const label = days === 1 ? `${String(at.getUTCHours()).padStart(2, "0")}:00` : at.toLocaleDateString(undefined, { month: "short", day: "numeric", timeZone: "UTC" });
    const input = Number(b.promptTokens) || 0;
    const output = Number(b.completionTokens) || 0;
    return { key, label, input, output, tokens: input + output, requests: Number(b.requests) || 0, people: Number(b.people) || 0, errors: Number(b.errors) || 0 };
  });
}

/** The people, sorted by the chosen column (most first, or most recent). */
export function sortPeople(list: UsagePerson[], sort: Sort) {
  return [...list].sort((a, b) => (sort === "lastActive" ? String(b.lastActive).localeCompare(String(a.lastActive)) : (Number(b[sort]) || 0) - (Number(a[sort]) || 0)));
}

/** Today against the tightest daily limit: which one, how much is used, and of how much. */
export function allowance(today: UsagePerson["today"], limits: Limits | undefined) {
  const parts = (
    [
      [today?.requests, limits?.dailyRequests, "requests"],
      [Number(today?.totalTokens || 0) + Number(today?.reservedTokens || 0), limits?.dailyTokens, "tokens"],
    ] as [number | undefined, number | null | undefined, "requests" | "tokens"][]
  ).filter(([, limit]) => limit != null) as [number | undefined, number, "requests" | "tokens"][];
  if (!parts.length) return null;
  const ratio = ([used, limit]: [number | undefined, number, string]) => (limit ? Number(used || 0) / limit : 0);
  const [used, limit, what] = parts.reduce((a, b) => (ratio(b) > ratio(a) ? b : a));
  const f = Number(today?.fraction) || 0;
  return { used: Number(used) || 0, limit, what, level: f >= 1 ? "full" : f >= 0.8 ? "near" : "" };
}
