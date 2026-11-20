"use client";

import dynamic from "next/dynamic";
import { Suspense } from "react";

const MemoryPage = dynamic(() => import("@/components/memory/memory-page").then((m) => m.MemoryPage));

export default function Page() {
  return (
    <Suspense>
      <MemoryPage />
    </Suspense>
  );
}
