"use client";

import dynamic from "next/dynamic";

const GroupsPage = dynamic(() => import("@/components/groups/groups-page").then((m) => m.GroupsPage));

export default function Page() {
  return <GroupsPage />;
}
