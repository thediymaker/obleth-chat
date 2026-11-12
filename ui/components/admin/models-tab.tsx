"use client";

// Administration › Models: publish models, choose the default, who may see each one,
// generation defaults and which of them people may change, and the short list the
// model chooser shows first. Providers and credentials live in Obleth Gateway.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ExternalLink, RefreshCw } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { EmptyState, ErrorBox, Loading } from "@/components/app-shell/page";
import { Button } from "@/components/ui/button";
import { CheckField, Columns, FormActions, FormDialog, FormError, Note, Pill, Section, SelectField, TableWrap, TextAreaField, TextField, td, th, useSubmit } from "@/components/manage/kit";
import { GenerationFields, readGeneration, type Generation } from "@/components/settings/generation";
import { api } from "@/lib/api";
import { keys, useCapabilities } from "@/lib/queries";
import { useAdminGroups, type AdminGroup } from "./groups-tab";
import { SearchForm } from "./people-tab";
import { useKept } from "./state";

export interface AdminModel {
  id: string;
  name?: string;
  description?: string;
  detail?: string;
  tags?: string[];
  order?: number;
  enabled?: boolean;
  published?: boolean;
  isDefault?: boolean;
  worker?: boolean;
  /** How far the model is trusted with changes through connections, and what certification found. */
  actions?: "auto" | "certified" | "provisional" | "blocked";
  effectiveActions?: "certified" | "provisional" | "blocked";
  certification?: { score?: number; passed?: number; total?: number; suite?: string; at?: string } | null;
  available?: boolean;
  shortList?: boolean;
  visibility?: "all" | "admins" | "groups";
  groupIds?: string[];
  defaults?: Generation;
  allowedOverrides?: string[];
}
/** What certification found for a model, in a line. */
export function certificationNote(m: AdminModel) {
  const c = m.certification;
  if (!c || typeof c.score !== "number") return "Not certified yet. Run evals/certify and publish.py to record a score; 90% or more counts as certified.";
  const when = c.at ? new Date(c.at).toLocaleDateString() : "";
  return `Certified ${c.score}% (${c.passed ?? "?"} of ${c.total ?? "?"} cases${c.suite ? `, ${c.suite}` : ""}${when ? `, ${when}` : ""}).`;
}
export const GENERATION = [
  ["systemPrompt", "System instructions"],
  ["reasoningEffort", "Thinking effort"],
  ["temperature", "Temperature"],
  ["topP", "Top P"],
  ["maxTokens", "Output token limit"],
] as const;

const modelsKey = ["admin", "models"] as const;
const shown = (models: AdminModel[]) => models.filter((m) => m.enabled !== false && m.published !== false);
export const statusOf = (m: AdminModel) => (m.available === false ? "Unavailable" : m.enabled === false ? "Disabled" : m.published === false ? "Unpublished" : "Published");

/** What the short list card says. */
export function shortListNote(models: AdminModel[]) {
  const listed = models.filter((m) => m.shortList);
  const all = shown(models);
  return listed.length
    ? `The model chooser shows these ${listed.length} first, with the other ${Math.max(0, all.length - listed.length)} under See all models. Searching still finds every model.`
    : `The model chooser lists all ${all.length} models. Pick a few to show first, so people aren’t faced with the whole list.`;
}

/** The policy the Configure form saves. */
export function modelBody(form: FormData, groups: AdminGroup[]) {
  const on = (name: string) => form.get(name) === "on";
  const text = (name: string) => String(form.get(name) ?? "");
  return {
    name: text("model-name").trim(),
    description: text("model-description").trim(),
    tags: [
      ...new Set(
        text("model-tags")
          .split(",")
          .map((t) => t.trim())
          .filter(Boolean),
      ),
    ],
    order: Number(text("model-order")),
    enabled: on("model-enabled"),
    published: on("model-published"),
    isDefault: on("model-default"),
    worker: on("model-worker"),
    actions: text("model-actions") || "auto",
    visibility: text("model-visibility"),
    groupIds: groups.filter((g) => on(`model-group-${g.id}`)).map((g) => g.id),
    defaults: readGeneration(form, "model"),
    allowedOverrides: GENERATION.filter(([key]) => on(`allow-${key}`)).map(([key]) => key),
  };
}

export function ModelsTab() {
  const queries = useQueryClient();
  const capabilities = useCapabilities().data;
  const gatewayUrl = String(capabilities?.adminGatewayUrl || capabilities?.gatewayAdminUrl || "");
  const models = useQuery({ queryKey: modelsKey, queryFn: async () => (await api.request<{ models?: AdminModel[] }>("/api/admin/models")).models ?? [] });
  const groups = useAdminGroups();
  const [query, setQuery] = useKept("models.query", "");
  const [editing, setEditing] = useState<AdminModel | null>(null);
  const [shortList, setShortList] = useState(false);
  const all = models.data ?? [];
  const q = query.toLowerCase();
  const list = all.filter((m) => [m.name, m.id, m.description, ...(m.tags || [])].join(" ").toLowerCase().includes(q));
  const listed = all.filter((m) => m.shortList);
  /** The catalog changed: everyone's model list and this tab load again. */
  const changed = async () => {
    await Promise.all([queries.invalidateQueries({ queryKey: keys.models }), queries.invalidateQueries({ queryKey: modelsKey })]);
  };
  const reload = () => {
    models.refetch();
    groups.refetch();
  };
  return (
    <div className="grid gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <SearchForm id="admin-model-query" label="Search models" placeholder="Search model name, ID, or tags…" value={query} onSearch={setQuery} />
        {gatewayUrl && (
          <Button size="sm" variant="outline" asChild>
            <a href={gatewayUrl} target="_blank" rel="noopener noreferrer">
              Manage providers in Gateway <ExternalLink />
            </a>
          </Button>
        )}
      </div>
      <Note>Publish models, choose the default, and control generation settings. Provider endpoints and credentials are managed in Obleth Gateway.</Note>
      <ErrorBox error={models.error || groups.error} onRetry={reload} />
      {shown(all).length > 0 && (
        <Section
          title="Short list"
          headingLevel={3}
          description={shortListNote(all)}
          actions={
            <Button size="sm" variant="outline" onClick={() => setShortList(true)}>
              {listed.length ? "Change" : "Choose models"}
            </Button>
          }
        >
          {listed.length > 0 && (
            <div className="flex flex-wrap gap-1.5">
              {listed.map((m) => (
                <Pill key={m.id}>{m.name || m.id}</Pill>
              ))}
            </div>
          )}
        </Section>
      )}
      {models.isLoading ? (
        <Loading label="Loading administration…" />
      ) : list.length ? (
        <TableWrap>
          <table className="w-full min-w-[860px]">
            <thead className="border-b bg-muted/40">
              <tr>
                <th className={th}>Model</th>
                <th className={th}>Tags</th>
                <th className={th}>Visible to</th>
                <th className={th}>User controls</th>
                <th className={th}>Status</th>
                <th className={th}>
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {list.map((m) => (
                <tr key={m.id} className="border-b border-line-soft align-top last:border-0">
                  <td className={`${td} max-w-80`}>
                    <strong className="block font-medium">{m.name || m.id}</strong>
                    {m.name && m.name !== m.id && <code className="block font-mono text-xs text-faint">{m.id}</code>}
                    {(m.description || m.detail) && <small className="line-clamp-2 text-xs text-muted-foreground">{m.description || m.detail}</small>}
                  </td>
                  <td className={td}>
                    <div className="flex flex-wrap gap-1">{m.tags?.length ? m.tags.map((t) => <Pill key={t}>{t}</Pill>) : <span className="text-faint">—</span>}</div>
                  </td>
                  <td className={td}>{m.visibility === "groups" ? `${m.groupIds?.length || 0} groups` : m.visibility === "admins" ? "Administrators" : "Everyone"}</td>
                  <td className={td}>{m.allowedOverrides?.length || 0} adjustable</td>
                  <td className={td}>
                    <div className="flex flex-wrap gap-1" data-testid="model-status">
                      {m.isDefault && <Pill tone="accent">Default</Pill>}
                      {m.shortList && <Pill>Short list</Pill>}
                      <Pill tone={statusOf(m) === "Published" ? "accent" : ""}>{statusOf(m)}</Pill>
                    </div>
                  </td>
                  <td className={`${td} text-right`}>
                    <Button size="xs" variant="outline" onClick={() => setEditing(m)}>
                      Configure
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </TableWrap>
      ) : (
        <EmptyState
          title="No models found"
          text={all.length ? "Try another search." : "Register models in Obleth Gateway, then refresh this page."}
          action={
            <Button size="sm" variant="outline" onClick={reload}>
              <RefreshCw /> Refresh models
            </Button>
          }
        />
      )}
      {shortList && <ShortListDialog models={all} onClose={() => setShortList(false)} onSaved={changed} />}
      {editing && <ModelDialog model={editing} groups={groups.data ?? []} onClose={() => setEditing(null)} onSaved={changed} />}
    </div>
  );
}

function ShortListDialog({ models, onClose, onSaved }: { models: AdminModel[]; onClose: () => void; onSaved: () => Promise<void> }) {
  const list = shown(models);
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    const picked = list.filter((m) => form.get(`short-${m.id}`) === "on").map((m) => m.id);
    await api.request("/api/admin/model-short-list", { method: "PUT", body: { models: picked } });
    await onSaved();
    onClose();
    toast(picked.length ? "Short list saved" : "The chooser lists every model");
  });
  return (
    <FormDialog open onOpenChange={(o) => !o && onClose()} title="Short list">
      <form onSubmit={onSubmit} className="grid gap-4">
        <Note>The model chooser shows these first, then See all models. Each person also sees Auto, their favorites, the default model and the one they’re using. Pick none to list every model.</Note>
        <div className="grid max-h-[50dvh] gap-3 overflow-y-auto">
          {list.map((m) => (
            <CheckField
              key={m.id}
              id={`short-${m.id}`}
              label={m.name || m.id}
              defaultChecked={!!m.shortList}
              description={m.available === false ? "Not offered by the gateway right now" : m.description || m.detail || undefined}
            />
          ))}
        </div>
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            Save short list
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

function ModelDialog({ model: m, groups, onClose, onSaved }: { model: AdminModel; groups: AdminGroup[]; onClose: () => void; onSaved: () => Promise<void> }) {
  const [visibility, setVisibility] = useState(m.visibility || "all");
  const [actions, setActions] = useState(m.actions || "auto");
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    await api.request(`/api/admin/models/${encodeURIComponent(m.id)}`, { method: "PUT", body: modelBody(form, groups) });
    await onSaved();
    onClose();
    toast("Model policy saved");
  });
  const d = m.defaults || {};
  return (
    <FormDialog open onOpenChange={(o) => !o && onClose()} title={`Configure ${m.name || m.id}`} description={m.id} wide>
      <form onSubmit={onSubmit} className="grid gap-4">
        <TextField id="model-name" label="Display name" maxLength={128} required defaultValue={m.name || m.id} />
        <TextAreaField id="model-description" label="Description" maxLength={2048} rows={2} defaultValue={m.description || ""} />
        <Columns>
          <TextField id="model-tags" label="Tags" maxLength={512} defaultValue={(m.tags || []).join(", ")} help="Separate tags with commas." />
          <TextField id="model-order" label="Sort order" type="number" step={1} min={-10000} max={10000} required defaultValue={m.order || 0} />
        </Columns>
        <div className="grid gap-3">
          <CheckField id="model-enabled" label="Enabled" defaultChecked={m.enabled !== false} description="Allow requests to this model." />
          <CheckField id="model-published" label="Published" defaultChecked={m.published !== false} description="Show this model in the account catalog." />
          <CheckField id="model-default" label="Default model" defaultChecked={!!m.isDefault} description="Auto routes to this model when the account can access it." />
          <CheckField
            id="model-worker"
            label="Worker"
            defaultChecked={!!m.worker}
            description="Other models can hand tasks to this one, several at once, to explore a workspace and report back. Choose fast, efficient models."
          />
        </div>
        <SelectField
          id="model-actions"
          label="Changes through connections"
          value={actions}
          onChange={(e) => setActions(e.target.value as typeof actions)}
          options={[
            ["auto", `Decide from certification${m.certification ? ` (now ${m.effectiveActions})` : " (none yet, so provisional)"}`],
            ["certified", "Certified: a change may run on a decision’s say-so"],
            ["provisional", "Provisional: every change asks"],
            ["blocked", "Blocked: reads only"],
          ]}
        />
        <Note>{certificationNote(m)}</Note>
        <SelectField
          id="model-visibility"
          label="Visible to"
          value={visibility}
          onChange={(e) => setVisibility(e.target.value as typeof visibility)}
          options={[
            ["all", "Everyone"],
            ["admins", "Administrators"],
            ["groups", "Selected groups"],
          ]}
        />
        <div className="grid gap-3" hidden={visibility !== "groups"}>
          <Note>Choose the groups that can use this model.</Note>
          {groups.length ? (
            groups.map((g) => <CheckField key={g.id} id={`model-group-${g.id}`} label={g.name} defaultChecked={(m.groupIds || []).includes(g.id)} />)
          ) : (
            <Note>Create a group before selecting group-only access.</Note>
          )}
        </div>
        <div className="border-t pt-4">
          <h3 className="text-md font-medium">Generation defaults</h3>
          <Note>Uncheck a control to lock it to the model default.</Note>
        </div>
        <GenerationFields values={d} prefix="model" />
        <div className="grid gap-3">
          {GENERATION.map(([key, label]) => (
            <CheckField key={key} id={`allow-${key}`} label={`Users may change ${label.toLowerCase()}`} defaultChecked={(m.allowedOverrides || []).includes(key)} />
          ))}
        </div>
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            Save model policy
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}
