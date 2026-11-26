"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useEffect, useRef } from "react";
import { cn } from "@/lib/utils";

export interface SectionTab {
  href: string;
  label: string;
  icon?: React.ComponentType<{ className?: string }>;
  /** Active for this path and everything under it; otherwise only the exact path. */
  prefix?: boolean;
}

/** Tabs between a section's pages, such as Library's Saved, Collections and Generated. Hidden with one tab.
 *  On a phone the bar scrolls sideways and starts with the active tab in view, so the page never pans.
 *  `current` names the active tab's href when the page decides it (Administration's URL has several shapes).
 *  `compact` is for a bar with many tabs that all have an icon: on a phone each tab but the active one shows
 *  its icon alone, and the tabs share the width, so the bar fits without scrolling. */
export function SectionTabs({ tabs, label, current, compact }: { tabs: SectionTab[]; label: string; current?: string; compact?: boolean }) {
  const path = usePathname();
  const nav = useRef<HTMLElement>(null);
  useEffect(() => {
    const active = nav.current?.querySelector<HTMLElement>('[aria-current="page"]');
    if (active && typeof active.scrollIntoView === "function") active.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [path, current]);
  if (tabs.length < 2) return null;
  return (
    <nav
      ref={nav}
      aria-label={label}
      className="-mx-4 flex gap-1 overflow-x-auto px-4 shadow-[inset_0_-1px_0_var(--border)] [scrollbar-width:none] sm:mx-0 sm:px-0 [&::-webkit-scrollbar]:hidden"
    >
      {tabs.map(({ href, label: name, icon: Icon, prefix }) => {
        const on = current !== undefined ? href === current : prefix ? path === href || path.startsWith(`${href}/`) : path === href;
        return (
          <Link
            key={href}
            href={href}
            aria-current={on ? "page" : undefined}
            className={cn(
              "inline-flex shrink-0 items-center gap-1.5 border-b-2 border-transparent px-3 py-2 text-sm whitespace-nowrap text-muted-foreground hover:text-foreground",
              on && "border-foreground text-foreground",
              compact && Icon && "max-sm:min-h-11 max-sm:min-w-10 max-sm:flex-1 max-sm:shrink max-sm:justify-center max-sm:px-1",
              compact && Icon && on && "max-sm:flex-none max-sm:px-3",
            )}
            title={compact && Icon ? name : undefined}
          >
            {Icon && <Icon className="size-4 shrink-0" />}
            {compact && Icon && !on ? <span className="max-sm:sr-only">{name}</span> : name}
          </Link>
        );
      })}
    </nav>
  );
}
