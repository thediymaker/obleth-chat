"use client";

// Pieces the Usage, Insights and Workspaces tabs share.

import { Dropdown } from "@/components/manage/kit";
import { cn } from "@/lib/utils";
import { KINDS, kindColor, percent, share, tokens, type Change } from "./insights";

/** "↑ 20% on the 7 days before", the arrow colored; nothing when there is nothing to compare. */
export function ChangeNote({ change }: { change: Change }) {
  if (!change) return null;
  if (change.same) return <>Same as {change.word}</>;
  return (
    <>
      <span className={cn("font-medium", change.up ? "text-success" : "text-amber")}>
        {change.up ? "↑" : "↓"} {change.amount}
      </span>{" "}
      on {change.word}
    </>
  );
}

/** The group filter: Everyone, or one group. Hidden with no groups. */
export function GroupPicker({ id, groups, value, onChange }: { id: string; groups?: { id: string; name: string }[]; value: string; onChange: (id: string) => void }) {
  if (!groups?.length) return null;
  return (
    <>
      <label className="sr-only" htmlFor={id}>
        Group
      </label>
      <Dropdown id={id} value={value} onChange={(e) => onChange(e.target.value)} className="h-7 w-auto min-w-36 text-sm">
        <option value="">Everyone</option>
        {groups.map((g) => (
          <option key={g.id} value={g.id}>
            {g.name}
          </option>
        ))}
      </Dropdown>
    </>
  );
}

/** A bar split by kind of use, by tokens or chats. */
export function KindBar({ kinds, label, by = "tokens", className }: { kinds?: { name: string; sessions?: number; tokens?: number }[]; label: string; by?: "tokens" | "sessions"; className?: string }) {
  const values = Object.fromEntries((kinds || []).map((k) => [k.name, Number(k[by]) || 0]));
  const total = Object.values(values).reduce((a, b) => a + b, 0);
  const present = KINDS.filter(([id]) => values[id]);
  const aria = present.map(([id, name]) => `${name} ${percent(share(values[id], total))}`).join(", ");
  return (
    <div role="img" aria-label={`${label}: ${aria || "nothing labelled"}`} className={cn("flex h-3 w-full gap-0.5 overflow-hidden rounded-sm bg-secondary", className)}>
      {present.map(([id, name, color]) => (
        <span
          key={id}
          title={`${name} ${percent(share(values[id], total))}${by === "tokens" ? ` · ${tokens(values[id])} tokens` : ` · ${values[id]}`}`}
          style={{ width: `${(100 * share(values[id], total)).toFixed(2)}%`, background: kindColor(color) }}
        />
      ))}
    </div>
  );
}

/** Which kinds appear, with their colors. */
export function KindLegend({ kinds, children }: { kinds: readonly (typeof KINDS)[number][]; children?: (id: string) => React.ReactNode }) {
  return (
    <ul className="flex flex-wrap gap-x-4 gap-y-1.5 text-sm">
      {kinds.map(([id, name, color]) => (
        <li key={id} className="inline-flex items-center gap-1.5">
          <i aria-hidden className="inline-block size-2.5 rounded-[2px]" style={{ background: kindColor(color) }} />
          {name}
          {children?.(id)}
        </li>
      ))}
    </ul>
  );
}
