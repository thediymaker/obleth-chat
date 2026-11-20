"use client";

import dynamic from "next/dynamic";

const ProjectOverview = dynamic(() => import("@/components/projects/project-overview").then((m) => m.ProjectOverview));

export default function Page() {
  return <ProjectOverview />;
}
