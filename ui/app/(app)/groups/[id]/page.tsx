"use client";

import dynamic from "next/dynamic";
import { useParams } from "next/navigation";

const GroupsPage = dynamic(() => import("@/components/groups/groups-page").then((m) => m.GroupsPage));

export default function Page() {
  const { id } = useParams<{ id: string }>();
  return <GroupsPage id={decodeURIComponent(id)} />;
}
