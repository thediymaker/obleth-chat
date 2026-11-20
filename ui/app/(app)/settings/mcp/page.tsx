"use client";

import dynamic from "next/dynamic";

const McpServersPage = dynamic(() => import("@/components/connections/mcp-page").then((m) => m.McpServersPage));

export default function Page() {
  return <McpServersPage />;
}
