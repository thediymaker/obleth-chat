"use client";

import { Suspense } from "react";
import { BatchesPage } from "@/components/batches/batches-page";

export default function Page() {
  return (
    <Suspense>
      <BatchesPage />
    </Suspense>
  );
}
