import { BookOpen, Box, Code, CornerDownLeft, GitBranch, Globe, type LucideIcon, MessageSquare, Repeat, Split } from "lucide-react";
import { cn } from "@/lib/utils";

/** Each step type's icon and accent (one of Chat's project colors). */
export const STEP_LOOK: Record<string, { icon: LucideIcon; color: string }> = {
  agent: { icon: MessageSquare, color: "blue" },
  search: { icon: BookOpen, color: "green" },
  code: { icon: Code, color: "violet" },
  decide: { icon: Split, color: "gold" },
  if: { icon: GitBranch, color: "gold" },
  for_each: { icon: Repeat, color: "rose" },
  http: { icon: Globe, color: "blue" },
  response: { icon: CornerDownLeft, color: "green" },
};
const FALLBACK = { icon: Box, color: "gray" };
export const stepLook = (type: string) => STEP_LOOK[type] ?? FALLBACK;

/** The type's icon in a tinted tile. */
export function StepIcon({ type, className, size = "md" }: { type: string; className?: string; size?: "sm" | "md" }) {
  const { icon: Icon, color } = stepLook(type);
  return (
    <span
      className={cn("relative flex shrink-0 items-center justify-center overflow-hidden rounded-md", size === "sm" ? "size-6" : "size-8", className)}
      style={{ color: `var(--project-${color})` }}
    >
      <span className="absolute inset-0 bg-current opacity-[0.14]" />
      <Icon className={cn("relative", size === "sm" ? "size-3.5" : "size-4")} />
    </span>
  );
}
