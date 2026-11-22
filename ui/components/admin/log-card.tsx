"use client";

// Administration › System › Project log: whether Chat writes each person's daily log of
// their work in a project, and the model that writes it. Endpoint in
// api/src/project_log/mod.rs.

import { toast } from "sonner";
import { CheckField, FormActions, FormError, Note, Pill, Section, TextField, useSubmit } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/api";
import { useModels } from "@/lib/queries";

export interface LogSettings {
  enabled: boolean;
  model: string | null;
  people: number;
  days: number;
  gateway: boolean;
  error?: string;
}

export function LogCard({ log: l, onSaved }: { log: LogSettings; onSaved: (l: LogSettings) => void }) {
  const models = useModels();
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    onSaved(
      await api.request<LogSettings>("/api/admin/log", {
        method: "PUT",
        body: { enabled: form.get("log-enabled") === "on", model: String(form.get("log-model") || "").trim() || null },
      }),
    );
    toast("Project log saved");
  });
  return (
    <Section
      title="Project log"
      description="Each project gets a Log tab: one entry a day for each person, written from their own chats, research runs and commits in the project, with every line linked to where it came from. People see only their own log, also in a shared project, add their own notes, and can turn it off for themselves in Settings. Temporary chats are never read."
      actions={<Pill tone={l.enabled ? "accent" : ""}>{l.enabled ? "On" : "Off"}</Pill>}
    >
      <form onSubmit={onSubmit} className="grid gap-4">
        <CheckField id="log-enabled" label="Turn on the project log" defaultChecked={l.enabled} disabled={!l.gateway} description={l.gateway ? undefined : "The project log asks models through the gateway; connect one first."} />
        <TextField
          id="log-model"
          label="Model"
          maxLength={160}
          list="log-models"
          spellCheck={false}
          autoComplete="off"
          placeholder="gemma4-31b-it"
          defaultValue={l.model || ""}
          help="Writes a day once its work in a project has been quiet for 30 minutes, or when the day ends, and again when more work arrives (at most every 2 hours while the day is under way, and 8 times a day in all): one request to this model each time. It reads up to about 18,000 characters of a day's chats. Each day is charged to its person as Project log in Usage."
        />
        <datalist id="log-models">
          {(models.data || [])
            .map((x) => x.id)
            .filter((id) => id && id !== "auto")
            .map((id) => (
              <option key={id} value={id} />
            ))}
        </datalist>
        <Note>
          {l.days} {l.days === 1 ? "day" : "days"} written for {l.people} {l.people === 1 ? "person" : "people"}.
        </Note>
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            Save
          </Button>
        </FormActions>
      </form>
    </Section>
  );
}
