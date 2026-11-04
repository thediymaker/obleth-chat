// A unified diff drawn as rows numbered by the new file, and
// the +added −removed counts beside a file.

import { cn } from "@/lib/utils";
import { diffRows } from "./model";

export function DiffView({ text, className }: { text: string | null | undefined; className?: string }) {
  const rows = diffRows(text);
  if (!rows.length) return <p className="p-3 text-sm text-muted-foreground">No line changes.</p>;
  return (
    <div role="table" aria-label="Changes" className={cn("overflow-x-auto bg-code font-mono text-[12px] leading-[1.55]", className)}>
      {rows.map((r, i) =>
        r.kind === "hunk" || r.kind === "more" || r.kind === "note" ? (
          <div key={i} role="row" className="border-y border-line-soft bg-muted/60 px-3 py-0.5 text-muted-foreground">
            {r.kind === "hunk" ? `@@ ${r.text}` : r.text}
          </div>
        ) : (
          <div key={i} role="row" className={cn("flex min-w-max", r.kind === "add" && "bg-success/12", r.kind === "del" && "bg-destructive/12")}>
            <span className="w-11 shrink-0 pr-2 text-right text-faint select-none">{r.n}</span>
            <span className={cn("w-4 shrink-0 select-none", r.kind === "add" ? "text-success" : "text-destructive-foreground")}>{r.kind === "add" ? "+" : r.kind === "del" ? "−" : ""}</span>
            <code className="pr-3 whitespace-pre">{r.text || " "}</code>
          </div>
        ),
      )}
    </div>
  );
}

export function DiffStat({ added, removed }: { added?: number | null; removed?: number | null }) {
  return (
    <span className="shrink-0 font-mono text-xs">
      {!!added && <b className="font-medium text-success">+{added}</b>}
      {!!removed && <i className="ml-1 text-destructive-foreground not-italic">−{removed}</i>}
    </span>
  );
}
