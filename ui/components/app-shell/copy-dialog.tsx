"use client";

// The text a copy could not put on the clipboard, selected to copy by hand.

import { useEffect, useState } from "react";
import { FormDialog } from "@/components/manage/kit";
import { Textarea } from "@/components/ui/textarea";
import { setCopyByHand } from "@/lib/clipboard";

export function CopyByHand() {
  const [text, setText] = useState<string | null>(null);
  useEffect(() => setCopyByHand(setText), []);
  return (
    <FormDialog open={text !== null} onOpenChange={(open) => !open && setText(null)} title="Copy text" description="Copying is blocked here. Press Ctrl+C or ⌘C to copy it.">
      <Textarea rows={8} readOnly value={text ?? ""} onFocus={(e) => e.currentTarget.select()} autoFocus aria-label="Text to copy" />
    </FormDialog>
  );
}
