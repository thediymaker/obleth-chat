"use client";

// Settings › Administration: accounts, model access, and service activity, one tab
// per section. The URL keeps the earlier hashes' shape: /settings/admin/<section>,
// and /settings/admin/groups/<id> for one group. Only administrators see any of it,
// and nobody else's browser asks for administrator data.

import { BarChart3, Box, Clock, Gauge, LayoutGrid, Palette, Server, Shield, Terminal, ThumbsUp, Users } from "lucide-react";
import Link from "next/link";
import { usePageChrome } from "@/components/app-shell/chrome";
import { EmptyState } from "@/components/app-shell/page";
import { SectionTabs } from "@/components/app-shell/section-tabs";
import { Button } from "@/components/ui/button";
import { Pill } from "@/components/manage/kit";
import { SectionHeading } from "@/components/settings/settings-frame";
import { useSession } from "@/lib/session";
import { ActivityTab } from "./activity-tab";
import { AppearanceTab } from "./appearance-tab";
import { FeedbackTab } from "./feedback-tab";
import { GroupsTab } from "./groups-tab";
import { InsightsTab } from "./insights-tab";
import { ModelsTab } from "./models-tab";
import { PeopleTab } from "./people-tab";
import { SystemTab } from "./system-tab";
import { UsageTab } from "./usage-tab";
import { WorkspacesTab } from "./workspaces-tab";

export const ADMIN_TABS = [
  ["users", Users, "People"],
  ["groups", LayoutGrid, "Groups"],
  ["models", Box, "Models"],
  ["usage", BarChart3, "Usage & limits"],
  ["insights", Gauge, "Insights"],
  ["workspaces", Terminal, "Workspaces"],
  ["feedback", ThumbsUp, "Feedback"],
  ["audit", Clock, "Activity"],
  ["appearance", Palette, "Branding"],
  ["system", Server, "System"],
] as const;
export type AdminSection = (typeof ADMIN_TABS)[number][0];

/** Which tab and group a path names, as adminLocation read the hash. Unknown sections open People. */
export function adminLocation(segments: string[] = []): { section: AdminSection; groupId: string } {
  const [first = "users", second = ""] = segments.map((s) => decodeURIComponent(s));
  const section = ADMIN_TABS.some(([id]) => id === first) ? (first as AdminSection) : "users";
  return { section, groupId: section === "groups" ? second : "" };
}

export function AdminPage({ segments = [] }: { segments?: string[] }) {
  const { isAdmin } = useSession();
  const { section, groupId } = adminLocation(segments);
  const title = ADMIN_TABS.find(([id]) => id === section)?.[2] ?? "Administration";
  usePageChrome(isAdmin ? `${title} · Administration` : "Administration");
  if (!isAdmin)
    return (
      <EmptyState
        icon={Shield}
        title="Administrator access required"
        text="Your personal account settings are available below."
        action={
          <Button variant="outline" asChild>
            <Link href="/settings">Open Settings</Link>
          </Button>
        }
      />
    );
  return (
    <div className="min-w-0">
      <SectionHeading title="Administration" description="Accounts, model access, and service activity." actions={<Pill tone="accent">Administrator</Pill>} />
      <div className="mb-6">
        <SectionTabs label="Administration sections" current={`/settings/admin/${section}`} tabs={ADMIN_TABS.map(([id, Icon, label]) => ({ href: `/settings/admin/${id}`, label, icon: Icon }))} />
      </div>
      {section === "users" && <PeopleTab />}
      {section === "groups" && <GroupsTab groupId={groupId} />}
      {section === "models" && <ModelsTab />}
      {section === "usage" && <UsageTab />}
      {section === "insights" && <InsightsTab />}
      {section === "workspaces" && <WorkspacesTab />}
      {section === "feedback" && <FeedbackTab />}
      {section === "audit" && <ActivityTab />}
      {section === "appearance" && <AppearanceTab />}
      {section === "system" && <SystemTab />}
    </div>
  );
}
