"use client";

// Everything around a signed-in page: the chat store and the app shell. The
// signed-in layout loads this only once someone is signed in, so the sign-in
// screen does not fetch it.

import { AppShell } from "@/components/app-shell/app-shell";
import { CopyByHand } from "@/components/app-shell/copy-dialog";
import { ChatProvider } from "@/components/chat/chat-provider";

export function SignedInApp({ children }: { children: React.ReactNode }) {
  return (
    <ChatProvider>
      <AppShell>{children}</AppShell>
      <CopyByHand />
    </ChatProvider>
  );
}
