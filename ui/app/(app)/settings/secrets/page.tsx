"use client";

import dynamic from "next/dynamic";

const SecretsPage = dynamic(() => import("@/components/settings/secrets-page").then((m) => m.SecretsPage));

export default function Page() {
  return <SecretsPage />;
}
