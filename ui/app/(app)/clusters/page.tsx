"use client";

import dynamic from "next/dynamic";

const ClustersPage = dynamic(() => import("@/components/clusters/clusters-page").then((m) => m.ClustersPage));

export default function Page() {
  return <ClustersPage />;
}
