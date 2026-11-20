"use client";

import dynamic from "next/dynamic";

const DevicesPage = dynamic(() => import("@/components/devices/devices-page").then((m) => m.DevicesPage));

export default function Page() {
  return <DevicesPage />;
}
