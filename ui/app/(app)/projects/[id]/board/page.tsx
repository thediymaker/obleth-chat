"use client";

import dynamic from "next/dynamic";

const ProjectBoard = dynamic(() => import("@/components/board/board-page").then((m) => m.ProjectBoard));

export default function Page() {
  return <ProjectBoard />;
}
