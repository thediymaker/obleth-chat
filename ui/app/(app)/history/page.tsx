"use client";

import dynamic from "next/dynamic";
import { Suspense } from "react";

const HistoryPage = dynamic(() => import("@/components/chat/history-page").then((m) => m.HistoryPage));

// The page reads its search from the URL, which needs a Suspense boundary.
export default function Page() {
  return (
    <Suspense>
      <HistoryPage />
    </Suspense>
  );
}
