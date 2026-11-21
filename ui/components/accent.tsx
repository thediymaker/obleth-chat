"use client";

// The accent in use: the person's own pick (kept in this browser, like the theme)
// when the site allows one, else the site's, else Chat's own. AccentStyle puts it
// on <html> as the custom properties app/globals.css reads.

import { useEffect, useSyncExternalStore } from "react";
import { ACCENTS, ACCENT_PROPERTIES, DEFAULT_ACCENT, accentHex, accentProperties } from "@/lib/accent";
import { useBranding } from "@/lib/branding-context";
import { cn } from "@/lib/utils";

const KEY = "obleth.accent";
const CHANGED = "obleth:accent";

function subscribe(change: () => void) {
  window.addEventListener("storage", change);
  window.addEventListener(CHANGED, change);
  return () => {
    window.removeEventListener("storage", change);
    window.removeEventListener(CHANGED, change);
  };
}
function saved() {
  try {
    return accentHex(window.localStorage.getItem(KEY));
  } catch {
    return "";
  }
}
function save(value: string) {
  try {
    if (value) window.localStorage.setItem(KEY, value);
    else window.localStorage.removeItem(KEY);
  } catch {
    // Private windows may refuse storage; the pick then lasts until the page closes.
  }
  window.dispatchEvent(new Event(CHANGED));
}

/** The accent shown, the person's own pick ("" when they follow the site) and how to change it. */
export function useAccent() {
  const { appearance } = useBranding();
  const own = useSyncExternalStore(subscribe, saved, () => "");
  const site = appearance.accent || DEFAULT_ACCENT;
  const choice = appearance.personalAccent ? own : "";
  return { accent: choice || site, site, choice, allowed: appearance.personalAccent, setChoice: save };
}

/** Keeps <html> carrying the accent in use. Renders nothing. */
export function AccentStyle() {
  const { accent } = useAccent();
  useEffect(() => {
    const style = document.documentElement.style;
    const set = accentProperties(accent);
    for (const name of ACCENT_PROPERTIES) {
      if (set[name]) style.setProperty(name, set[name]);
      else style.removeProperty(name);
    }
  }, [accent]);
  return null;
}

/** The accent choice in Settings: the site's own, or one of a few that read well on both themes. */
export function AccentPicker({ className }: { className?: string }) {
  const { site, choice, setChoice } = useAccent();
  const options = [{ name: "Site default", value: "", shown: site }, ...ACCENTS.filter((a) => a.value !== site).map((a) => ({ ...a, shown: a.value }))];
  return (
    <div role="radiogroup" aria-label="Accent color" className={cn("flex flex-wrap gap-1.5", className)}>
      {options.map((option) => {
        const on = choice === option.value;
        return (
          <button
            key={option.name}
            type="button"
            role="radio"
            aria-checked={on}
            aria-label={option.name}
            title={option.name}
            onClick={() => setChoice(option.value)}
            className={cn("flex size-8 items-center justify-center rounded-full border-2 border-transparent p-0.5 transition-colors hover:border-line-strong", on && "border-foreground hover:border-foreground")}
          >
            <span className="size-full rounded-full" style={{ background: option.shown }} />
          </button>
        );
      })}
    </div>
  );
}
