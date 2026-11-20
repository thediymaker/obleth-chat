"use client";

import dynamic from "next/dynamic";
import { Suspense } from "react";

const BatchesPage = dynamic(() => import("@/components/batches/batches-page").then((m) => m.BatchesPage));

export default function Page() {
  return (
    <Suspense>
      <BatchesPage />
    </Suspense>
  );
}
