// Building blocks every page uses, so pages share spacing and type.

import { LayoutGrid, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/** A page's padded column: 1440px at most, 32px around (44px on top from 1600px
 * wide, 28px 24px up to 1200px, 24px 18px on a phone). Every page is the same
 * width; `wide` is accepted and ignored. */
export function Page({ children, wide: _wide = false, className }: { children: React.ReactNode; wide?: boolean; className?: string }) {
  return (
    <div data-page className={cn("mx-auto w-full max-w-[1440px] px-[18px] py-6 min-[761px]:px-6 min-[761px]:py-7 min-[1201px]:p-8 min-[1600px]:pt-11", className)}>
      {children}
    </div>
  );
}

/** The heading block: title, one line under it, and actions on the right. */
export function PageHeader({ title, description, actions, children }: { title: React.ReactNode; description?: React.ReactNode; actions?: React.ReactNode; children?: React.ReactNode }) {
  return (
    <div className="mb-7 flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="min-w-0">
          <h1 className="text-[26px] leading-tight font-semibold tracking-[-0.6px]">{title}</h1>
          {description && <p className="mt-2 text-base text-muted-foreground">{description}</p>}
        </div>
        {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
      </div>
      {children}
    </div>
  );
}

/** Small uppercase label over a section. */
export function Eyebrow({ children, className }: { children: React.ReactNode; className?: string }) {
  return <span className={cn("text-2xs font-medium tracking-wider text-muted-foreground uppercase", className)}>{children}</span>;
}

/** Nothing here yet, with what it is for and how to start. */
export function EmptyState({ title, text, icon: Icon = LayoutGrid, action }: { title: string; text?: string; icon?: React.ComponentType<{ className?: string }>; action?: React.ReactNode }) {
  return (
    <div className="flex flex-col items-center gap-3 rounded-lg border border-dashed px-6 py-12 text-center">
      <Icon className="size-6 text-muted-foreground" />
      <h2 className="text-lg font-semibold">{title}</h2>
      {text && <p className="max-w-md text-sm text-muted-foreground">{text}</p>}
      {action}
    </div>
  );
}

/** A request that failed, with a retry. */
export function ErrorBox({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  if (!error) return null;
  const message = error instanceof Error ? error.message : String(error);
  return (
    <div role="alert" className="mb-4 flex items-center gap-3 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive-foreground">
      <span className="grow">{message}</span>
      {onRetry && (
        <Button size="sm" variant="outline" onClick={onRetry}>
          <RefreshCw /> Retry
        </Button>
      )}
    </div>
  );
}

/** A loading line for a section. */
export function Loading({ label = "Loading…" }: { label?: string }) {
  return (
    <p role="status" className="py-6 text-sm text-muted-foreground">
      {label}
    </p>
  );
}
