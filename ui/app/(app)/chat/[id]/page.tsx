"use client";

import dynamic from "next/dynamic";

const ChatRoute = dynamic(() => import("@/components/chat/chat-view").then((m) => m.ChatRoute));

export default function Page() {
  return <ChatRoute />;
}
