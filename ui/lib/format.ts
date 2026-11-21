// The one set of formatters every page uses, so a count, a token amount, a size
// or a time reads the same on every screen: numbers group by locale, thousands
// are "K", sizes are 1024-based KB/MB/GB (the wording the server uses), list rows
// say "5 min ago" and detail views give the date.

/** What a missing value shows. */
export const NONE = "—";

/** The period pickers' default: the last 28 days, four whole weeks, which the API's
 * usage and workspace periods (1, 7, 28 and 90 days) take. */
export const PERIOD_DAYS = 28;

/** "1,234". */
export const num = (n: number | null | undefined) => Number(n || 0).toLocaleString();

/** "3 chats", "1 chat", "1,200 saved items". */
export const count = (n: number | null | undefined, one: string, many = `${one}s`) => `${num(n)} ${Number(n) === 1 ? one : many}`;

/** A token amount or any large count at a glance: 950, 1.2K, 48K, 1.3M. */
export function compact(value: number | null | undefined): string {
  const n = Number(value) || 0;
  if (n < 1000) return String(n);
  if (n < 1e4) return `${(n / 1000).toFixed(1)}K`;
  if (n < 1e6) return `${Math.round(n / 1000)}K`;
  return `${(n / 1e6).toFixed(1)}M`;
}

/** A size in bytes: "512 B", "2.0 KB", "3.0 MB", "1.5 GB" (1024-based, as the server says it). */
export function bytes(value: number | null | undefined): string {
  if (value == null) return "";
  const n = Number(value) || 0;
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  return `${(n / 1024 ** 3).toFixed(1)} GB`;
}

/** A span in milliseconds, compact: "0.4s", "12s", "1m 40s". */
export function duration(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return "";
  if (ms < 10000) return `${(ms / 1000).toFixed(1)}s`;
  const s = Math.round(ms / 1000);
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
}

/** A span in seconds, in words: "5 minutes", "3 hours 1 minute", "2 days". */
export function durationWords(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  const unit = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
  if (s < 60) return unit(s, "second");
  const m = Math.round(s / 60);
  if (m < 60) return unit(m, "minute");
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${unit(h, "hour")} ${unit(m % 60, "minute")}` : unit(h, "hour");
  const d = Math.floor(h / 24);
  return h % 24 ? `${unit(d, "day")} ${unit(h % 24, "hour")}` : unit(d, "day");
}

/** A date and time: "Nov 3, 2026, 10:04 AM". Detail views use this; list rows use `ago`. */
export const when = (value: string | number | Date | null | undefined) =>
  value ? new Date(value).toLocaleString([], { dateStyle: "medium", timeStyle: "short" }) : NONE;

/** A relative time: "just now", "5 min ago", "3 h ago", "yesterday", "5 days ago", then a date.
 *  A number is milliseconds, or seconds with `{ seconds: true }` (commit times). */
export function ago(value: string | number | Date | null | undefined, now = Date.now(), { seconds = false } = {}): string {
  if (value == null || value === "") return "";
  const date = typeof value === "number" && seconds ? new Date(value * 1000) : new Date(value);
  const time = date.getTime();
  if (Number.isNaN(time)) return "";
  const secs = Math.round((now - time) / 1000);
  if (secs < 45) return "just now";
  const minutes = Math.round(secs / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.round(hours / 24);
  if (days === 1) return "yesterday";
  if (days < 7) return `${days} days ago`;
  return date.toLocaleDateString(undefined, { month: "short", day: "numeric", ...(date.getFullYear() === new Date(now).getFullYear() ? {} : { year: "numeric" }) });
}

/** Initials for an avatar. */
export const initials = (name: string) =>
  name
    .split(/[\s._@-]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0]!.toUpperCase())
    .join("") || "?";
