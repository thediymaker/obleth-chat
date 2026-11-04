"use client";

import { AppTab } from "@/components/apps/app-tab";
import { ProjectTabGuard } from "@/components/projects/project-frame";

export default function Page() {
  return <ProjectTabGuard tab="app">{(projectId) => <AppTab key={projectId} projectId={projectId} />}</ProjectTabGuard>;
}
