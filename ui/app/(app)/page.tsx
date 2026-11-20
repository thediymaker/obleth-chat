"use client";

import dynamic from "next/dynamic";
import { Suspense } from "react";

const ChatHome = dynamic(() => import("@/components/chat/chat-view").then((m) => m.ChatHome));

// The home screen reads ?attach= and ?preset= from Settings' links, which needs a Suspense boundary.
export default function Page() {
  return (
    <Suspense>
      <ChatHome />
    </Suspense>
  );
}
