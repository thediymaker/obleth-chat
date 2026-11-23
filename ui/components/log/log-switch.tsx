"use client";

// Settings › Personal › Project log: the person's own switch for their daily log of
// their work in projects, shown while the log is on for this server.

import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { toast } from "sonner";
import { Section, SwitchField, messageOf } from "@/components/manage/kit";
import { keys, useFeature } from "@/lib/queries";
import { saveLogChoice } from "./api";

export function LogSwitchCard() {
  const available = useFeature("log");
  const mine = useFeature("myLog");
  const client = useQueryClient();
  const [on, setOn] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);
  if (!available) return null;
  const checked = on ?? mine;
  async function change(next: boolean) {
    setOn(next);
    setSaving(true);
    try {
      await saveLogChoice({ enabled: next });
      await client.invalidateQueries({ queryKey: keys.capabilities });
    } catch (e) {
      setOn(!next);
      toast(messageOf(e));
    } finally {
      setSaving(false);
    }
  }
  return (
    <Section title="Project log" description="A model writes one entry a day of your own work in each project, from your chats, research runs and commits there. Only you see your log, also in a shared project.">
      <SwitchField
        id="log-enabled"
        label="Write my project log"
        description="Off, no more days are written and the Log tab is hidden; the days already written are kept."
        checked={checked}
        disabled={saving}
        onCheckedChange={change}
      />
    </Section>
  );
}
