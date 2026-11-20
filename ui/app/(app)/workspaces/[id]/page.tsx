"use client";

import dynamic from "next/dynamic";
import { useParams } from "next/navigation";

const WorkspacePage = dynamic(() => import("@/components/workspace/workspace-page").then((m) => m.WorkspacePage));

export default function Page() {
  const { id } = useParams<{ id: string }>();
  const conversationId = decodeURIComponent(id);
  return <WorkspacePage key={conversationId} conversationId={conversationId} />;
}
