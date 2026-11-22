"use client";

import dynamic from "next/dynamic";
import { useParams } from "next/navigation";

const ProjectNotebook = dynamic(() => import("@/components/notebook/notebook-page").then((m) => m.ProjectNotebook));
const ProjectTabGuard = dynamic(() => import("@/components/projects/project-frame").then((m) => m.ProjectTabGuard));

/** /projects/<id>/notebook, and /projects/<id>/notebook/<chat> once its chat is open. */
export default function Page() {
  const { chat } = useParams<{ chat?: string[] }>();
  const chatId = chat?.[0] ? decodeURIComponent(chat[0]) : null;
  return <ProjectTabGuard tab="notebook">{(projectId) => <ProjectNotebook key={projectId} projectId={projectId} chatId={chatId} />}</ProjectTabGuard>;
}
