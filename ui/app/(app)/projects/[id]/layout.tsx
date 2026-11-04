import { ProjectFrame } from "@/components/projects/project-frame";

export default function Layout({ children }: { children: React.ReactNode }) {
  return <ProjectFrame>{children}</ProjectFrame>;
}
