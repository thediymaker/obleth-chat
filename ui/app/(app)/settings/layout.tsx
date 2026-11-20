"use client";

import dynamic from "next/dynamic";

const SettingsFrame = dynamic(() => import("@/components/settings/settings-frame").then((m) => m.SettingsFrame));

export default function SettingsLayout({ children }: { children: React.ReactNode }) {
  return <SettingsFrame>{children}</SettingsFrame>;
}
