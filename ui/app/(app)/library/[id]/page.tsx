"use client";

import dynamic from "next/dynamic";

const LibraryPage = dynamic(() => import("@/components/library/library-page").then((m) => m.LibraryPage));
const KnowledgeOnly = dynamic(() => import("@/components/library/library-shell").then((m) => m.KnowledgeOnly));

// A saved item's link (#library/<id> before): Library with that item open.

import { useParams } from "next/navigation";
import { Suspense } from "react";

export default function Page() {
  const { id } = useParams<{ id: string }>();
  return (
    <KnowledgeOnly page="library" title="Library">
      <Suspense>
        <LibraryPage tab="saved" openId={decodeURIComponent(id)} />
      </Suspense>
    </KnowledgeOnly>
  );
}
