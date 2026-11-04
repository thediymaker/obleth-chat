"use client";

import { useParams } from "next/navigation";
import { GroupsPage } from "@/components/groups/groups-page";

export default function Page() {
  const { id } = useParams<{ id: string }>();
  return <GroupsPage id={decodeURIComponent(id)} />;
}
