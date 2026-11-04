"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { cn } from "@/lib/utils";

export interface SectionTab {
  href: string;
  label: string;
  icon?: React.ComponentType<{ className?: string }>;
  /** Active for this path and everything under it; otherwise only the exact path. */
  prefix?: boolean;
}

/** Tabs between a section's pages, such as Library's Saved, Collections and Generated. Hidden with one tab. */
export function SectionTabs({ tabs, label }: { tabs: SectionTab[]; label: string }) {
  const path = usePathname();
  if (tabs.length < 2) return null;
  return (
    <nav aria-label={label} className="flex gap-1 border-b">
      {tabs.map(({ href, label: name, icon: Icon, prefix }) => {
        const on = prefix ? path === href || path.startsWith(`${href}/`) : path === href;
        return (
          <Link
            key={href}
            href={href}
            aria-current={on ? "page" : undefined}
            className={cn(
              "-mb-px inline-flex items-center gap-1.5 border-b-2 border-transparent px-3 py-2 text-sm text-muted-foreground hover:text-foreground",
              on && "border-foreground text-foreground",
            )}
          >
            {Icon && <Icon className="size-4" />}
            {name}
          </Link>
        );
      })}
    </nav>
  );
}
