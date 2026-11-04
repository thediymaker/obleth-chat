"use client";

import { useParams } from "next/navigation";
import { CollectionPage } from "@/components/knowledge/collection-page";
import { KnowledgeOnly } from "@/components/library/library-shell";

export default function Page() {
  const { id } = useParams<{ id: string }>();
  return (
    <KnowledgeOnly page="knowledge" title="Collection">
      <CollectionPage id={decodeURIComponent(id)} />
    </KnowledgeOnly>
  );
}
