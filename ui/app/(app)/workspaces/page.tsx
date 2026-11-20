"use client";

import dynamic from "next/dynamic";

const WorkspacesPage = dynamic(() => import("@/components/workspace/workspaces-page").then((m) => m.WorkspacesPage));

export default function Page() {
  return <WorkspacesPage />;
}
