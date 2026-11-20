"use client";

import dynamic from "next/dynamic";

const ProjectCode = dynamic(() => import("@/components/code/project-code").then((m) => m.ProjectCode));
const ProjectTabGuard = dynamic(() => import("@/components/projects/project-frame").then((m) => m.ProjectTabGuard));

export default function Page() {
  return <ProjectTabGuard tab="code">{(projectId) => <ProjectCode key={projectId} projectId={projectId} />}</ProjectTabGuard>;
}
