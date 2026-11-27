"use client";

// A signed-in page that failed to draw, inside the shell, which stays.

import { PageError } from "@/components/app-shell/page-error";

export default function ErrorPage(props: { error: Error & { digest?: string }; reset: () => void }) {
  return <PageError {...props} />;
}
