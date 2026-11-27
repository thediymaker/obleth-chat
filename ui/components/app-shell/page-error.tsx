"use client";

// A page that failed to draw: one block in place of a blank screen, with a way to try
// again. Next.js shows it for an error thrown while rendering (app/error.tsx and
// app/(app)/error.tsx); errors from requests are shown by the page that made them.

import { RefreshCw, TriangleAlert } from "lucide-react";
import { useEffect } from "react";
import { EmptyState, Page } from "@/components/app-shell/page";
import { Button } from "@/components/ui/button";

export function PageError({ error, reset }: { error: Error & { digest?: string }; reset: () => void }) {
  useEffect(() => {
    console.error(error);
  }, [error]);
  return (
    <Page>
      <EmptyState
        icon={TriangleAlert}
        title="This page could not be shown"
        text="Something went wrong while showing it. Your chats and files are not affected."
        action={
          <>
            <Button size="sm" variant="outline" onClick={reset}>
              <RefreshCw /> Try again
            </Button>
            {/* A full load, not a route change: it also clears whatever state failed. */}
            <Button asChild size="sm" variant="outline">
              <a href="/">Back to Chat</a>
            </Button>
          </>
        }
      />
    </Page>
  );
}
