"use client";

import dynamic from "next/dynamic";

const AppTab = dynamic(() => import("@/components/apps/app-tab").then((m) => m.AppTab));
const ProjectTabGuard = dynamic(() => import("@/components/projects/project-frame").then((m) => m.ProjectTabGuard));

export default function Page() {
  return <ProjectTabGuard tab="app">{(projectId) => <AppTab key={projectId} projectId={projectId} />}</ProjectTabGuard>;
}
