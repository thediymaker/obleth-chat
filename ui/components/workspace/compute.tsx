"use client";

// Compute: the pages this server offers under it (Workspaces with the workspace
// feature, Clusters with clusters) and what a page shows when its feature is off.

import { Cpu, Server, Terminal } from "lucide-react";
import { FeatureOff } from "@/components/app-shell/page";
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
  workspaces: { icon: Terminal, what: "Workspaces" },
  clusters: { icon: Server, what: "Clusters" },
  devices: { icon: Cpu, what: "Devices" },
};

export function ComputeOff({ page }: { page: keyof typeof OFF }) {
  const { icon, what } = OFF[page];
  return <FeatureOff icon={icon} what={what} plural />;
}
