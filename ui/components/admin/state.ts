"use client";

// A tab's filters (period, group, sort…) kept while the person moves between
// Administration tabs, until the page reloads.

import { useCallback, useState } from "react";

const kept = new Map<string, unknown>();

export function useKept<T>(key: string, initial: T): [T, (value: T | ((old: T) => T)) => void] {
  const [value, setValue] = useState<T>(() => (kept.has(key) ? (kept.get(key) as T) : initial));
  const set = useCallback(
    (next: T | ((old: T) => T)) =>
      setValue((old) => {
        const v = typeof next === "function" ? (next as (old: T) => T)(old) : next;
        kept.set(key, v);
        return v;
      }),
    [key],
  );
  return [value, set];
}

/** For tests: forget every kept filter. */
export const forgetKept = () => kept.clear();
