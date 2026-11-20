// The legend and series colors of the usage bar charts, without recharts, so a page
// can show them while the chart itself loads on its own.

import { cn } from "@/lib/utils";

export interface Series {
  key: string;
  name: string;
  /** A CSS color, usually a token such as var(--chart-1). */
  color: string;
}

export const INPUT = "var(--chart-1)";
export const OUTPUT = "var(--chart-3)";

/** One legend entry per series, never color alone. */
export function Legend({ items, className }: { items: { name: string; color: string; dashed?: boolean }[]; className?: string }) {
  return (
    <ul className={cn("flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground", className)}>
      {items.map((item) => (
        <li key={item.name} className="inline-flex items-center gap-1.5">
          {item.dashed ? (
            <i aria-hidden className="inline-block w-3 border-t-2 border-dashed" style={{ borderColor: item.color }} />
          ) : (
            <i aria-hidden className="inline-block size-2.5 rounded-[2px]" style={{ background: item.color }} />
          )}
          {item.name}
        </li>
      ))}
    </ul>
  );
}
