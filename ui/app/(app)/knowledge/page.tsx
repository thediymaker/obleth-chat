"use client";

import dynamic from "next/dynamic";

const KnowledgeHome = dynamic(() => import("@/components/knowledge/knowledge-home").then((m) => m.KnowledgeHome));
const KnowledgeOnly = dynamic(() => import("@/components/library/library-shell").then((m) => m.KnowledgeOnly));

export default function Page() {
  return (
    <KnowledgeOnly page="knowledge" title="Library › Collections">
      <KnowledgeHome />
    </KnowledgeOnly>
  );
}
