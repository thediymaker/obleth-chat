// Icons by name (lib/icons), for code that picks one by name, such as branding.
import { Terminal, type LucideProps } from "lucide-react";
import { BRAND_ICONS } from "@/lib/icons";

export function NamedIcon({ name, ...props }: { name: string } & LucideProps) {
  const Icon = BRAND_ICONS[name] ?? Terminal;
  return <Icon aria-hidden="true" {...props} />;
}
