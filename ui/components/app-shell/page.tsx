// Building blocks every page uses, so pages share spacing and type.
//
// Empty, loading and error states: as a page's body (nothing here yet, a feature that
// is off, a page not found) use EmptyState, FeatureOff or NotFound; inside a Section
// or card use a Note (manage/kit), and `compact` Loading and ErrorBox. Empty-state
// titles have no full stop.

import { ArrowLeft, LayoutGrid, RefreshCw, SearchX } from "lucide-react";
import Link from "next/link";
import { Button } from "@/components/ui/button";
import { ApiError } from "@/lib/api";
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

/** Nothing here yet, with what it is for and how to start. `card` draws it as a solid
 *  card with the icon in a ring, for a page's one big block such as a feature that is
 *  off; `children` go under the action. */
export function EmptyState({
  title,
  text,
  icon: Icon = LayoutGrid,
  action,
  variant = "dashed",
  className,
  children,
}: {
  title: string;
  text?: React.ReactNode;
  icon?: React.ComponentType<{ className?: string }>;
  action?: React.ReactNode;
  variant?: "dashed" | "card";
  className?: string;
  children?: React.ReactNode;
}) {
  return (
    <div className={cn("flex flex-col items-center gap-3 rounded-lg border px-6 py-12 text-center", variant === "dashed" ? "border-dashed" : "bg-card", className)}>
      {variant === "card" ? (
        <span className="grid size-10 place-items-center rounded-full border bg-background">
          <Icon className="size-5 text-muted-foreground" />
        </span>
      ) : (
        <Icon className="size-6 text-muted-foreground" />
      )}
      <h2 className="text-lg font-semibold">{title}</h2>
      {text && <p className="max-w-lg text-sm text-muted-foreground">{text}</p>}
      {action && <div className="flex flex-wrap justify-center gap-2">{action}</div>}
      {children}
    </div>
  );
}

/** Whether a request failed because the server is not set up for its feature. */
export const isFeatureOff = (error: unknown) => error instanceof ApiError && error.code === "feature_off";

/** A feature this server has turned off, in one wording everywhere. `what` is the
 *  feature as a subject ("Memory", "Workspaces"); `plural` picks "are" and "them". */
export function FeatureOff({ what, plural = false, icon, children }: { what: string; plural?: boolean; icon?: React.ComponentType<{ className?: string }>; children?: React.ReactNode }) {
  return (
    <EmptyState variant="card" icon={icon} title={`${what} ${plural ? "are" : "is"} turned off on this server`} text={`Ask an administrator to turn ${plural ? "them" : "it"} on.`}>
      {children}
    </EmptyState>
  );
}

/** Whether a request failed because the thing is not there (or not shared with this person). */
export const isNotFound = (error: unknown) => !!error && (error as { status?: number }).status === 404;

/** A page or item that does not exist, or is not shared with this person: one block for
 *  every kind, with a way back and no Retry (asking again will not make it appear). The
 *  wording never says which of the two it is. */
export function NotFound({ what, to, back, action }: { what: string; to: string; back: string; action?: React.ReactNode }) {
  return (
    <EmptyState
      icon={SearchX}
      title={`${what[0]!.toUpperCase()}${what.slice(1)} not found`}
      text={`This ${what} does not exist, or it is not shared with you.`}
      action={
        action ?? (
          <Button asChild size="sm" variant="outline">
            <Link href={to}>
              <ArrowLeft /> Back to {back}
            </Link>
          </Button>
        )
      }
    />
  );
}

/** A request that failed, with a retry; `compact` inside a card. A feature that is off
 *  gets no Retry, since asking again will not turn it on. */
export function ErrorBox({ error, onRetry, compact = false }: { error: unknown; onRetry?: () => void; compact?: boolean }) {
  if (!error) return null;
  const message = error instanceof Error ? error.message : String(error);
  return (
    <div role="alert" className={cn("flex items-center gap-3 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive-foreground", !compact && "mb-4")}>
      <span className="grow">{message}</span>
      {onRetry && !isFeatureOff(error) && (
        <Button size="sm" variant="outline" onClick={onRetry}>
          <RefreshCw /> Retry
        </Button>
      )}
    </div>
  );
}

/** A loading line for a page, or `compact` inside a card or list. */
export function Loading({ label = "Loading…", compact = false }: { label?: string; compact?: boolean }) {
  return (
    <p role="status" className={cn("text-sm text-muted-foreground", compact ? "py-1" : "py-6")}>
      {label}
    </p>
  );
}
