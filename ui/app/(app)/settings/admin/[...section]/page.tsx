"use client";

import { useParams } from "next/navigation";
import { AdminPage } from "@/components/admin/admin-page";

export default function Page() {
  const { section } = useParams<{ section: string[] }>();
  return <AdminPage segments={section} />;
}
