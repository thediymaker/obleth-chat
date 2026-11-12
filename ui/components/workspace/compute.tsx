"use client";

// Compute: the pages this server offers under it (Workspaces with the workspace
// feature, Clusters with clusters) and what a page shows when its feature is off.

import { Cpu, Server, Terminal } from "lucide-react";
import { EmptyState } from "@/components/app-shell/page";
import { SectionTabs, type SectionTab } from "@/components/app-shell/section-tabs";
import { useCapabilities } from "@/lib/queries";

export function useComputeTabs(): SectionTab[] {
  const features = useCapabilities().data?.features || {};
  return [
    ...(features.workspaces ? [{ href: "/workspaces", label: "Workspaces", icon: Terminal }] : []),
    ...(features.clusters ? [{ href: "/clusters", label: "Clusters", icon: Server }] : []),
    ...(features.devices ? [{ href: "/devices", label: "Devices", icon: Cpu }] : []),
  ];
}

export function ComputeTabs() {
  const tabs = useComputeTabs();
  return <SectionTabs label="Compute" tabs={tabs} />;
}

const OFF = {
  workspaces: { icon: Terminal, title: "Workspaces are not enabled.", text: "Private Linux workspaces with a live preview appear here once Kubernetes is configured for this server." },
  clusters: { icon: Server, title: "Clusters are not enabled.", text: "Slurm and Kubernetes clusters appear here once connections are configured for this server." },
  devices: { icon: Cpu, title: "Devices are not enabled.", text: "Boards plugged into your computer appear here once connections are configured for this server." },
};

export function ComputeOff({ page }: { page: keyof typeof OFF }) {
  const { icon, title, text } = OFF[page];
  return <EmptyState icon={icon} title={title} text={text} />;
}
