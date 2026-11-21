"use client";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ThemeProvider, useTheme } from "next-themes";
import { useEffect, useState, useSyncExternalStore } from "react";
import { AccentStyle } from "@/components/accent";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Toaster } from "@/components/ui/sonner";
import { normalizeBranding } from "@/lib/branding";
import { BrandingProvider } from "@/lib/branding-context";
import { siteAppearance } from "@/lib/site-appearance";
import { SessionProvider } from "@/lib/session";

const THEME_KEY = "obleth.theme";

/**
 * The site's default theme can change after the page started: the session answer
 * brings what an administrator set. Someone who never picked a theme then moves to it,
 * and still has not picked one. Renders nothing.
 */
function SiteDefaultTheme() {
  // Only what the administrator set: the file's own default was there when the page started.
  const wanted = useSyncExternalStore(siteAppearance.subscribe, siteAppearance.get, siteAppearance.server).defaultTheme;
  const { theme, setTheme } = useTheme();
  useEffect(() => {
    try {
      if (!wanted || window.localStorage.getItem(THEME_KEY) != null || theme === wanted) return;
      setTheme(wanted);
      // setTheme notes the theme as the person's choice; it is the site's.
      window.localStorage.removeItem(THEME_KEY);
    } catch {
      // Without storage the theme stays as the page started.
    }
  }, [wanted, theme, setTheme]);
  return null;
}

export function Providers({ children, nonce }: { children: React.ReactNode; nonce?: string }) {
  const [client] = useState(
    () =>
      new QueryClient({
        defaultOptions: {
          queries: { refetchOnWindowFocus: false, staleTime: 5_000, retry: 1 },
        },
      }),
  );
  // The site's default theme, for anyone who has not picked one. /site.config.js has
  // loaded by the first render in the browser; the server does not have it and says
  // "system", which the browser's value then replaces.
  // What an administrator set, kept from the visit before, comes first.
  const [defaultTheme] = useState(() => (typeof window === "undefined" ? "system" : (siteAppearance.get().defaultTheme ?? normalizeBranding(window.OblethChatConfig).appearance.defaultTheme)));
  return (
    <QueryClientProvider client={client}>
      <ThemeProvider attribute="data-theme" storageKey={THEME_KEY} defaultTheme={defaultTheme} enableSystem disableTransitionOnChange nonce={nonce}>
        <BrandingProvider>
          <AccentStyle />
          <SiteDefaultTheme />
          <SessionProvider>
            <TooltipProvider delayDuration={300}>{children}</TooltipProvider>
            <Toaster position="bottom-right" />
          </SessionProvider>
        </BrandingProvider>
      </ThemeProvider>
    </QueryClientProvider>
  );
}
