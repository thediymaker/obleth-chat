"use client";

import dynamic from "next/dynamic";

const ProjectFrame = dynamic(() => import("@/components/projects/project-frame").then((m) => m.ProjectFrame));

export default function Layout({ children }: { children: React.ReactNode }) {
  return <ProjectFrame>{children}</ProjectFrame>;
}
