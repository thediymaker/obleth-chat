"use client";

import dynamic from "next/dynamic";

const AdminPage = dynamic(() => import("@/components/admin/admin-page").then((m) => m.AdminPage));

export default function Page() {
  return <AdminPage />;
}
