"use client";

// What a page puts in the shared chrome: its title (top bar and browser tab) and
// any actions it wants in the top bar's right side.

import { createContext, useContext, useEffect, useState } from "react";
import { useBranding } from "@/lib/branding-context";
import { pageTitle } from "@/lib/branding";

interface Chrome {
  title: string;
  actions: React.ReactNode;
  /** The top bar's label when it differs from the tab's title (a chat shows "Chat"). */
  crumb?: string;
}
const Context = createContext<{ chrome: Chrome; set: (c: Partial<Chrome>) => void } | null>(null);

export function ChromeProvider({ children }: { children: React.ReactNode }) {
  const [chrome, setChrome] = useState<Chrome>({ title: "", actions: null, crumb: "" });
  return <Context.Provider value={{ chrome, set: (c) => setChrome((old) => ({ ...old, ...c })) }}>{children}</Context.Provider>;
}

export function useChrome() {
  const value = useContext(Context);
  if (!value) throw new Error("useChrome needs the app shell");
  return value.chrome;
}

/** Sets the page's title in the top bar and the browser tab, and optional top-bar actions.
 * `crumb` labels the top bar instead of the title. Pass `actions` memoized: the page
 * re-renders with the chrome, and a new element each time would set it again. */
export function usePageChrome(title: string, actions: React.ReactNode = null, crumb = "") {
  const value = useContext(Context);
  const branding = useBranding();
  useEffect(() => {
    value?.set({ title, actions, crumb });
    document.title = pageTitle(title, branding);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- actions are new each render
  }, [title, branding, crumb]);
  useEffect(() => {
    value?.set({ actions });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [actions]);
}
