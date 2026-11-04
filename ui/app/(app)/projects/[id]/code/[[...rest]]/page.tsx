"use client";

import { ProjectCode } from "@/components/code/project-code";
import { ProjectTabGuard } from "@/components/projects/project-frame";

export default function Page() {
  return <ProjectTabGuard tab="code">{(projectId) => <ProjectCode key={projectId} projectId={projectId} />}</ProjectTabGuard>;
}
