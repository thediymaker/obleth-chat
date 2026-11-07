"use client";

import { Suspense } from "react";
import { MemoryPage } from "@/components/memory/memory-page";

export default function Page() {
  return (
    <Suspense>
      <MemoryPage />
    </Suspense>
  );
}
