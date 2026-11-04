"use client";

import { useParams } from "next/navigation";
import { WorkspacePage } from "@/components/workspace/workspace-page";

export default function Page() {
  const { id } = useParams<{ id: string }>();
  const conversationId = decodeURIComponent(id);
  return <WorkspacePage key={conversationId} conversationId={conversationId} />;
}
