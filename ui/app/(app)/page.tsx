import { Suspense } from "react";
import { ChatHome } from "@/components/chat/chat-view";

// The home screen reads ?attach= and ?preset= from Settings' links, which needs a Suspense boundary.
export default function Page() {
  return (
    <Suspense>
      <ChatHome />
    </Suspense>
  );
}
