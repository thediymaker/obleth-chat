"use client";

import { AppShell } from "@/components/app-shell/app-shell";
import { AuthScreen } from "@/components/auth/auth-screen";
import { ChatProvider } from "@/components/chat/chat-provider";
import { HashRedirect } from "@/components/hash-redirect";
import { useSession } from "@/lib/session";

/** Every signed-in page. Until the session is known (or when there is none) this
 * shows the sign-in screens at the same URL, so the page opens after signing in. */
export default function SignedInLayout({ children }: { children: React.ReactNode }) {
  const { phase } = useSession();
  return (
    <>
      <HashRedirect />
      {phase === "ready" ? (
        <ChatProvider>
          <AppShell>{children}</AppShell>
        </ChatProvider>
      ) : (
        <AuthScreen />
      )}
    </>
  );
}
