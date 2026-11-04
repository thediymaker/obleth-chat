"use client";

import { createContext, useContext, useEffect, useState } from "react";
import { defaultBranding, normalizeBranding, type Branding } from "@/lib/branding";
import { BRAND_ICON_NAMES } from "@/lib/icons";

const Context = createContext<Branding>(defaultBranding);

/** The site's branding. /site.config.js loads before the app starts, so it is
 * there on the first client render; the server renders the defaults. */
export function BrandingProvider({ children }: { children: React.ReactNode }) {
  const [branding, setBranding] = useState<Branding>(defaultBranding);
  useEffect(() => {
    const value = normalizeBranding(window.OblethChatConfig, BRAND_ICON_NAMES);
    setBranding(value);
    const favicon = window.matchMedia?.("(prefers-color-scheme: dark)").matches ? value.favicon.dark : value.favicon.light;
    if (favicon) {
      let link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
      if (!link) {
        link = document.createElement("link");
        link.rel = "icon";
        document.head.appendChild(link);
      }
      link.href = favicon;
    }
  }, []);
  return <Context.Provider value={branding}>{children}</Context.Provider>;
}

export const useBranding = () => useContext(Context);
