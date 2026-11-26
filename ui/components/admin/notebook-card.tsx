"use client";

// Administration › System › Notebook: the model a project's notebook uses for its
// own work, such as each new source's guide. Endpoint in api/src/notebook/mod.rs.

import { toast } from "sonner";
import { FormActions, FormError, Note, Pill, Section, TextField, useSubmit } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/api";
import { useModels } from "@/lib/queries";

export interface NotebookSettings {
  model: string | null;
  gateway: boolean;
  knowledge: boolean;
  error?: string;
}

export function NotebookCard({ notebook: n, onSaved }: { notebook: NotebookSettings; onSaved: (n: NotebookSettings) => void }) {
  const models = useModels();
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    onSaved(await api.request<NotebookSettings>("/api/admin/notebook", { method: "PUT", body: { model: String(form.get("notebook-model") || "").trim() || null } }));
    toast("Notebook saved");
  });
  return (
    <Section
      title="Notebook"
      description="A project’s Notebook tab answers from the sources each person ticks. With a Notebook model named here, each new source in a project gets a short guide with suggested questions, and the notebook shows how much of the model’s context the ticked sources take."
      actions={<Pill tone={n.model ? "accent" : ""}>{n.model ? "On" : "Off"}</Pill>}
    >
      <form onSubmit={onSubmit} className="grid gap-4">
        <TextField
          id="notebook-model"
          label="Notebook model"
          maxLength={160}
          list="notebook-models"
          spellCheck={false}
          autoComplete="off"
          disabled={!n.gateway}
          defaultValue={n.model || ""}
          help="Writes a guide once for each document indexed in a project’s collections from now on, from its headings and opening text: about one short request per document, charged to the person who added it. Choose a model with a large context window that people may use. Leave it empty to turn this off."
        />
        <datalist id="notebook-models">
          {(models.data || [])
            .map((x) => x.id)
            .filter((id) => id && id !== "auto")
            .map((id) => (
              <option key={id} value={id} />
            ))}
        </datalist>
        {!n.gateway && <Note>The notebook asks models through the gateway; connect one first.</Note>}
        {n.gateway && !n.knowledge && <Note>Notebooks need knowledge search, which is not set up on this server.</Note>}
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy || !n.gateway}>
            Save
          </Button>
        </FormActions>
      </form>
    </Section>
  );
}
