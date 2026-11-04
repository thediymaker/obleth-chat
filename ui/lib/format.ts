// Small formatting helpers shared by pages.

/** "3 chats", "1 chat". */
export const count = (n: number | null | undefined, one: string, many = `${one}s`) =>
  `${Number(n || 0).toLocaleString()} ${n === 1 ? one : many}`;

/** A date and time: "Nov 3, 2026, 10:04 AM". */
export const when = (value: string | number | Date | null | undefined) =>
  value ? new Date(value).toLocaleString([], { dateStyle: "medium", timeStyle: "short" }) : "—";

/** A relative time such as "just now", "5 min ago", "yesterday", or a date. */
export function ago(value: string | number | Date | null | undefined, now = Date.now()): string {
  if (!value) return "";
  const date = new Date(value);
  const time = date.getTime();
  if (Number.isNaN(time)) return "";
  const seconds = Math.round((now - time) / 1000);
  if (seconds < 45) return "just now";
  const minutes = Math.round(seconds / 60);
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
