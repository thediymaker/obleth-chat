// A centered note inside a side-pane tab: what is going on and, when there is
// one, the one thing to do about it. Shared by the workspace, browser and Git tabs.

import { cn } from "@/lib/utils";

export function PaneNote({
  icon: Icon,
  title,
  children,
  action,
  status = false,
  spin = false,
  className,
}: {
  icon: React.ComponentType<{ className?: string }>;
  title: string;
  children?: React.ReactNode;
  action?: React.ReactNode;
  /** A live status (role="status"), such as starting. */
  status?: boolean;
  spin?: boolean;
  className?: string;
}) {
  return (
    <div role={status ? "status" : undefined} className={cn("flex h-full min-h-48 flex-col items-center justify-center gap-2 p-6 text-center", className)}>
      <span className="mb-1 grid size-9 place-items-center rounded-full border bg-card">
        <Icon className={cn("size-4 text-muted-foreground", spin && "animate-spin [animation-duration:2.5s]")} />
      </span>
      <strong className="text-base font-medium">{title}</strong>
      {children && <div className="max-w-sm text-sm text-muted-foreground">{children}</div>}
      {action && <div className="mt-2 flex flex-wrap justify-center gap-2">{action}</div>}
    </div>
  );
}

/** "Loading…" inside a pane. */
export function PaneLoading({ label = "Loading…" }: { label?: string }) {
  return (
    <p role="status" className="p-4 text-sm text-muted-foreground">
      {label}
    </p>
  );
}
