"use client";

// A saved item's link (#library/<id> before): Library with that item open.

import { useParams } from "next/navigation";
import { Suspense } from "react";
import { LibraryPage } from "@/components/library/library-page";
import { KnowledgeOnly } from "@/components/library/library-shell";

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
