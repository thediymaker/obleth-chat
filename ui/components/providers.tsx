"use client";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ThemeProvider } from "next-themes";
import { useState } from "react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Toaster } from "@/components/ui/sonner";
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
  return (
    <QueryClientProvider client={client}>
      <ThemeProvider attribute="data-theme" storageKey="obleth.theme" defaultTheme="system" enableSystem disableTransitionOnChange nonce={nonce}>
        <BrandingProvider>
          <SessionProvider>
            <TooltipProvider delayDuration={300}>{children}</TooltipProvider>
            <Toaster position="bottom-right" />
          </SessionProvider>
        </BrandingProvider>
      </ThemeProvider>
    </QueryClientProvider>
  );
}
