"use client";

// Administration › System › Memory: whether Chat learns about people across chats,
// whether it starts on for people who have not chosen, and the model that keeps the
// memories. Endpoint in api/src/memory.rs.

import { toast } from "sonner";
import { CheckField, FormActions, FormError, Note, Pill, Section, TextField, useSubmit } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/api";
import { useModels } from "@/lib/queries";

export interface MemorySettings {
  enabled: boolean;
  defaultOn: boolean;
  model: string | null;
  people: number;
  memories: number;
  gateway: boolean;
  embeddings: boolean;
  error?: string;
}

export function MemoryCard({ memory: m, onSaved }: { memory: MemorySettings; onSaved: (m: MemorySettings) => void }) {
  const models = useModels();
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    onSaved(
      await api.request<MemorySettings>("/api/admin/memory", {
        method: "PUT",
        body: {
          enabled: form.get("memory-enabled") === "on",
          defaultOn: form.get("memory-default-on") === "on",
          model: String(form.get("memory-model") || "").trim() || null,
        },
      }),
    );
    toast("Memory saved");
  });
  return (
    <Section
      title="Memory"
      description="Chat learns lasting things about each person from their chats, such as their role, projects, tools and how they like answers, and gives later chats the ones that fit. Each person sees, changes and deletes their own memories on the Memory page, and can turn memory off. Temporary chats are never read."
      actions={<Pill tone={m.enabled ? "accent" : ""}>{m.enabled ? "On" : "Off"}</Pill>}
    >
      <form onSubmit={onSubmit} className="grid gap-4">
        <CheckField id="memory-enabled" label="Turn on memory" defaultChecked={m.enabled} disabled={!m.gateway} description={m.gateway ? undefined : "Memory asks models through the gateway; connect one first."} />
        <CheckField
          id="memory-default-on"
          label="On for people who haven't chosen"
          defaultChecked={m.defaultOn}
          description="Off means each person turns memory on for themselves on the Memory page."
        />
        <TextField
          id="memory-model"
          label="Model"
          maxLength={160}
          list="memory-models"
          spellCheck={false}
          autoComplete="off"
          placeholder="gemma4-31b-it"
          defaultValue={m.model || ""}
          help="Reads each person's new messages after a reply and keeps their memories up to date. A fast model that follows instructions well is enough. Chat asks as each person, so the gateway counts the tokens as theirs."
        />
        <datalist id="memory-models">
          {(models.data || [])
            .map((x) => x.id)
            .filter((id) => id && id !== "auto")
            .map((id) => (
              <option key={id} value={id} />
            ))}
        </datalist>
        {!m.embeddings && <Note>Without knowledge search, memories are matched to messages by their words only, not by meaning.</Note>}
        <Note>
          {m.memories} {m.memories === 1 ? "memory" : "memories"} kept for {m.people} {m.people === 1 ? "person" : "people"}.
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
