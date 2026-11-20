"use client";

import dynamic from "next/dynamic";
import { useParams } from "next/navigation";

const AdminPage = dynamic(() => import("@/components/admin/admin-page").then((m) => m.AdminPage));

export default function Page() {
  const { section } = useParams<{ section: string[] }>();
  return <AdminPage segments={section} />;
}
