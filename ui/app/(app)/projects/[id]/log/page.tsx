"use client";

import dynamic from "next/dynamic";

const ProjectLog = dynamic(() => import("@/components/log/project-log").then((m) => m.ProjectLog));
const ProjectTabGuard = dynamic(() => import("@/components/projects/project-frame").then((m) => m.ProjectTabGuard));

export default function Page() {
  return <ProjectTabGuard tab="log">{(projectId) => <ProjectLog key={projectId} projectId={projectId} />}</ProjectTabGuard>;
}
