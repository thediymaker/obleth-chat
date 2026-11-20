"use client";

import dynamic from "next/dynamic";
import { useParams } from "next/navigation";

const CollectionPage = dynamic(() => import("@/components/knowledge/collection-page").then((m) => m.CollectionPage));
const KnowledgeOnly = dynamic(() => import("@/components/library/library-shell").then((m) => m.KnowledgeOnly));

export default function Page() {
  const { id } = useParams<{ id: string }>();
  return (
    <KnowledgeOnly page="knowledge" title="Collection">
      <CollectionPage id={decodeURIComponent(id)} />
    </KnowledgeOnly>
  );
}
