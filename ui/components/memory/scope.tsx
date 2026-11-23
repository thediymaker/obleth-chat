"use client";

// Where a memory is used: everywhere (every chat of the person's) or one project (only
// their chats in it). A label for it, and a menu to choose it.

import { ChevronDown, Globe } from "lucide-react";
import { projectDot } from "@/components/app-shell/sidebar";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuLabel, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import type { ProjectSummary } from "@/lib/queries";
import { cn } from "@/lib/utils";
import type { Memory } from "./api";

export const EVERYWHERE = "Everywhere";
export const UNAVAILABLE = "A project you can no longer use";

/** What a scope means for the chats that get the memory, as a sentence. */
export const scopeMeaning = (projectName: string | null) => (projectName ? `Only your chats in ${projectName} get it.` : "Every chat of yours can get it.");

/** A memory's project with its colour dot, or Everywhere. Truncates inside its box. */
export function ScopeLabel({ memory, className }: { memory: Pick<Memory, "projectId" | "projectName" | "projectColor" | "projectUnavailable">; className?: string }) {
  const cls = cn("inline-flex min-w-0 items-center gap-1.5", className);
  if (memory.projectId && memory.projectName)
    return (
      <span className={cls} title={`${memory.projectName}. ${scopeMeaning(memory.projectName)}`} data-scope="project">
        <span className={cn("size-2 shrink-0 rounded-full", projectDot(memory.projectColor || "gray"))} aria-hidden="true" />
        <span className="truncate">{memory.projectName}</span>
      </span>
    );
  if (memory.projectUnavailable)
    return (
      <span className={cls} title={`${UNAVAILABLE}. No chat gets it until you move it.`} data-scope="unavailable">
        <span className="size-2 shrink-0 rounded-full border border-muted-foreground" aria-hidden="true" />
        <span className="truncate">{UNAVAILABLE}</span>
      </span>
    );
  return (
    <span className={cls} title={`${EVERYWHERE}. ${scopeMeaning(null)}`} data-scope="everywhere">
      <Globe className="size-3 shrink-0" aria-hidden="true" />
      <span className="truncate">{EVERYWHERE}</span>
    </span>
  );
}

/** A button showing the chosen scope that opens Everywhere and the person's projects. */
export function ScopeMenu({
  projects,
  value,
  onChange,
  label,
  trigger,
  disabled,
}: {
  projects: ProjectSummary[];
  /** A project id, or null for everywhere. */
  value: string | null;
  onChange: (projectId: string | null) => void;
  /** The menu's heading and the button's accessible name. */
  label: string;
  /** Shown on the button instead of the chosen scope, such as "Move". */
  trigger?: React.ReactNode;
  disabled?: boolean;
}) {
  const current = projects.find((p) => p.id === value) ?? null;
  const shown = current ? current.name : EVERYWHERE;
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <Button type="button" variant="outline" size="sm" disabled={disabled} className="max-w-full min-w-0 justify-start" title={trigger ? label : `${shown}. ${scopeMeaning(current?.name ?? null)}`} aria-label={trigger ? label : `${label}: ${shown}`}>
          {trigger ?? (
            <>
              {current ? <span className={cn("size-2 shrink-0 rounded-full", projectDot(current.color))} aria-hidden="true" /> : <Globe className="shrink-0" aria-hidden="true" />}
              <span className="truncate">{shown}</span>
              <ChevronDown className="ml-auto shrink-0" />
            </>
          )}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="max-h-80 w-72 max-w-[calc(100vw-2rem)] overflow-y-auto">
        <DropdownMenuLabel className="text-xs font-normal text-muted-foreground">{label}</DropdownMenuLabel>
        <DropdownMenuRadioGroup value={value ?? "everywhere"} onValueChange={(v) => onChange(v === "everywhere" ? null : v)}>
          <DropdownMenuRadioItem value="everywhere" className="min-w-0">
            <Globe className="shrink-0" aria-hidden="true" />
            <span className="grid min-w-0">
              <span className="truncate">{EVERYWHERE}</span>
              <span className="truncate text-xs text-muted-foreground">Every chat of yours</span>
            </span>
          </DropdownMenuRadioItem>
          {!!projects.length && <DropdownMenuSeparator />}
          {projects.map((p) => (
            <DropdownMenuRadioItem key={p.id} value={p.id} className="min-w-0" title={p.name}>
              <span className={cn("size-2 shrink-0 rounded-full", projectDot(p.color))} aria-hidden="true" />
              <span className="truncate">{p.name}</span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        {!projects.length && <p className="px-2 py-1.5 text-sm text-muted-foreground">You have no projects yet.</p>}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
