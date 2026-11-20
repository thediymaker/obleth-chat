"use client";

import dynamic from "next/dynamic";

const ProjectResearch = dynamic(() => import("@/components/research/project-research").then((m) => m.ProjectResearch));
const ProjectTabGuard = dynamic(() => import("@/components/projects/project-frame").then((m) => m.ProjectTabGuard));

export default function Page() {
  return <ProjectTabGuard tab="research">{(projectId) => <ProjectResearch key={projectId} projectId={projectId} />}</ProjectTabGuard>;
}
