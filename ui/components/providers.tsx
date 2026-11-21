"use client";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ThemeProvider } from "next-themes";
import { useState } from "react";
import { AccentStyle } from "@/components/accent";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Toaster } from "@/components/ui/sonner";
import { normalizeBranding } from "@/lib/branding";
import { BrandingProvider } from "@/lib/branding-context";
import { SessionProvider } from "@/lib/session";

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
  const [defaultTheme] = useState(() => (typeof window === "undefined" ? "system" : normalizeBranding(window.OblethChatConfig).appearance.defaultTheme));
  return (
    <QueryClientProvider client={client}>
      <ThemeProvider attribute="data-theme" storageKey="obleth.theme" defaultTheme={defaultTheme} enableSystem disableTransitionOnChange nonce={nonce}>
        <BrandingProvider>
          <AccentStyle />
          <SessionProvider>
            <TooltipProvider delayDuration={300}>{children}</TooltipProvider>
            <Toaster position="bottom-right" />
          </SessionProvider>
        </BrandingProvider>
      </ThemeProvider>
    </QueryClientProvider>
  );
}
