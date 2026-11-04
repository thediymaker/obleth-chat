"use client";

import { SettingsFrame } from "@/components/settings/settings-frame";

export default function SettingsLayout({ children }: { children: React.ReactNode }) {
  return <SettingsFrame>{children}</SettingsFrame>;
}
