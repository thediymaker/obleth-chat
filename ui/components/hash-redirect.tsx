"use client";

import { useRouter } from "next/navigation";
import { useEffect } from "react";
import { hashPath } from "@/lib/hash-links";

/** Moves a #hash address (lib/hash-links) to its path,
 * on load and whenever the hash changes. */
export function HashRedirect() {
  const router = useRouter();
  useEffect(() => {
    const move = () => {
      const target = hashPath(window.location.hash);
      if (target) router.replace(target);
    };
    move();
    window.addEventListener("hashchange", move);
    return () => window.removeEventListener("hashchange", move);
  }, [router]);
  return null;
}
