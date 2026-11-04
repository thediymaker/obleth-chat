"use client";

import { KnowledgeHome } from "@/components/knowledge/knowledge-home";
import { KnowledgeOnly } from "@/components/library/library-shell";

export default function Page() {
  return (
    <KnowledgeOnly page="knowledge" title="Collections">
      <KnowledgeHome />
    </KnowledgeOnly>
  );
}
