"use client";

import { ProjectResearch } from "@/components/research/project-research";
import { ProjectTabGuard } from "@/components/projects/project-frame";

export default function Page() {
  return <ProjectTabGuard tab="research">{(projectId) => <ProjectResearch key={projectId} projectId={projectId} />}</ProjectTabGuard>;
}
