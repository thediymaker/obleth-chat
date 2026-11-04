import "./globals.css";
import type { Metadata, Viewport } from "next";
import { headers } from "next/headers";
import Script from "next/script";
import { Providers } from "@/components/providers";

export const metadata: Metadata = {
  title: "Chat",
  description: "A space to ask questions, explore ideas, and create.",
};

export const viewport: Viewport = { width: "device-width", initialScale: 1, colorScheme: "light dark" };

export default async function RootLayout({ children }: { children: React.ReactNode }) {
  // Every document renders per request: its scripts carry this request's CSP nonce (proxy.ts).
  const nonce = (await headers()).get("x-nonce") ?? undefined;
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        {/* Public branding; the Rust server serves it from the install's branding folder. */}
        <Script src="/site.config.js" strategy="beforeInteractive" nonce={nonce} />
        <link rel="preload" href="/fonts/space-grotesk.woff2" as="font" type="font/woff2" crossOrigin="" />
      </head>
      <body className="min-h-dvh">
        <Providers nonce={nonce}>{children}</Providers>
      </body>
    </html>
  );
}
