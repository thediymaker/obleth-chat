"use client";

import dynamic from "next/dynamic";
import { Suspense } from "react";

const LibraryPage = dynamic(() => import("@/components/library/library-page").then((m) => m.LibraryPage));
const KnowledgeOnly = dynamic(() => import("@/components/library/library-shell").then((m) => m.KnowledgeOnly));

export default function Page() {
  return (
    <KnowledgeOnly page="library" title="Library">
      <Suspense>
        <LibraryPage tab="saved" />
      </Suspense>
    </KnowledgeOnly>
  );
}
