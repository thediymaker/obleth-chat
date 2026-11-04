"use client";

import { Monitor, Moon, Sun } from "lucide-react";
import { useTheme } from "next-themes";
import { useEffect, useState } from "react";
import { cn } from "@/lib/utils";

const OPTIONS = [
  ["system", "System", Monitor],
  ["light", "Light", Sun],
  ["dark", "Dark", Moon],
] as const;

/** System, light or dark, as a segmented switch. Shared by sign-in, the top bar and settings. */
export function ThemeToggle({ labels = false, className }: { labels?: boolean; className?: string }) {
  const { theme, setTheme } = useTheme();
  // The saved choice is only known in the browser.
  const [mounted, setMounted] = useState(false);
  useEffect(() => setMounted(true), []);
  return (
    <div role="radiogroup" aria-label="Color theme" className={cn("inline-flex items-center gap-0.5 rounded-md border bg-background p-0.5", className)}>
      {OPTIONS.map(([value, label, Icon]) => {
        const on = mounted && (theme ?? "system") === value;
        return (
          <button
            key={value}
            type="button"
            role="radio"
            aria-checked={on}
            aria-label={label}
            title={`${label} theme`}
            onClick={() => setTheme(value)}
            className={cn(
              "inline-flex h-6 items-center gap-1.5 rounded-sm px-1.5 text-xs text-muted-foreground transition-colors hover:text-foreground",
              on && "bg-secondary text-foreground",
            )}
          >
            <Icon className="size-3.5" />
            {labels && <span>{label}</span>}
          </button>
        );
      })}
    </div>
  );
}
