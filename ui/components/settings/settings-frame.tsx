"use client";

// Settings: the heading and the Personal / MCP servers / Administration tabs over
// each settings page. Administration is shown only to administrators, and MCP
// servers only where connections are on.

import { KeyRound, Link2, Settings, Shield } from "lucide-react";
import { Page, PageHeader } from "@/components/app-shell/page";
import { SectionTabs, type SectionTab } from "@/components/app-shell/section-tabs";
import { useFeature } from "@/lib/queries";
import { useSession } from "@/lib/session";

/** The tabs over Settings, as settingsTabs built them. */
export function settingsTabs(admin: boolean, connections: boolean): SectionTab[] {
  return [
    { href: "/settings", label: "Personal", icon: Settings },
    ...(connections ? [{ href: "/settings/mcp", label: "MCP servers", icon: Link2 }] : []),
    ...(connections ? [{ href: "/settings/secrets", label: "Secrets", icon: KeyRound }] : []),
    ...(admin ? [{ href: "/settings/admin", label: "Administration", icon: Shield, prefix: true }] : []),
  ];
}

export function SettingsFrame({ children }: { children: React.ReactNode }) {
  const { isAdmin } = useSession();
  const connections = useFeature("connections");
  return (
    <Page className="max-w-[1400px]">
      <PageHeader title="Settings" description={`Your account and preferences${isAdmin ? ", and how this installation runs" : ""}.`}>
        <SectionTabs label="Settings sections" tabs={settingsTabs(isAdmin, connections)} />
      </PageHeader>
      {children}
    </Page>
  );
}

/** The heading of one settings section, under the tabs. */
export function SectionHeading({ title, description, actions }: { title: string; description?: React.ReactNode; actions?: React.ReactNode }) {
  return (
    <div className="mb-5 flex flex-wrap items-start justify-between gap-3">
      <div className="min-w-0">
        <h2 className="text-lg font-semibold">{title}</h2>
        {description && <p className="mt-1 max-w-3xl text-sm text-muted-foreground">{description}</p>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </div>
  );
}
