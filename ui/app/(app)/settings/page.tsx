"use client";

import dynamic from "next/dynamic";

const PersonalSettings = dynamic(() => import("@/components/settings/personal-settings").then((m) => m.PersonalSettings));

export default function Page() {
  return <PersonalSettings />;
}
