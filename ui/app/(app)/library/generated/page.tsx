"use client";

import { Suspense } from "react";
import { LibraryPage } from "@/components/library/library-page";
import { KnowledgeOnly } from "@/components/library/library-shell";

export default function Page() {
  return (
    <KnowledgeOnly page="library" title="Generated">
      <Suspense>
        <LibraryPage tab="generated" />
      </Suspense>
    </KnowledgeOnly>
  );
}
