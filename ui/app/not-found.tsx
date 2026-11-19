"use client";

// An address nothing answers to: the same block missing items use, inside the
// signed-in shell (or behind the sign-in screens, like every page).

import { NotFound, Page } from "@/components/app-shell/page";
import SignedInLayout from "./(app)/layout";

export default function NotFoundPage() {
  return (
    <SignedInLayout>
      <Page>
        <NotFound what="page" to="/" back="Chat" />
      </Page>
    </SignedInLayout>
  );
}
