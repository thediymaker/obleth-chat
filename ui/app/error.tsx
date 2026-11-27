"use client";

// Anything else that failed to draw, the shell and the sign-in screens included.

import { PageError } from "@/components/app-shell/page-error";

export default function ErrorPage(props: { error: Error & { digest?: string }; reset: () => void }) {
  return <PageError {...props} />;
}
