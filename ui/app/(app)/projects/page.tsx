"use client";

import dynamic from "next/dynamic";

const ProjectsHome = dynamic(() => import("@/components/projects/projects-home").then((m) => m.ProjectsHome));

export default function Page() {
  return <ProjectsHome />;
}
