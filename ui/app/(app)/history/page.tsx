import { Suspense } from "react";
import { HistoryPage } from "@/components/chat/history-page";

// The page reads its search from the URL, which needs a Suspense boundary.
export default function Page() {
  return (
    <Suspense>
      <HistoryPage />
    </Suspense>
  );
}
