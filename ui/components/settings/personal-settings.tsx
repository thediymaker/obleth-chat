"use client";

// Settings › Personal: usage, profile, conversation defaults, the workspace agent,
// presets, the browser, security, API tokens, appearance, files and favorites.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { File, Plus } from "lucide-react";
import { useRouter } from "next/navigation";
import { useState } from "react";
import { toast } from "sonner";
import { usePageChrome } from "@/components/app-shell/chrome";
import { EmptyState } from "@/components/app-shell/page";
import { ThemeToggle } from "@/components/theme-toggle";
import { Button } from "@/components/ui/button";
import { CheckField, DetailRow, FormActions, FormDialog, FormError, Note, Pill, Section, SelectField, TextAreaField, TextField, displayName, useConfirm, useSubmit } from "@/components/manage/kit";
import { SectionHeading } from "@/components/settings/settings-frame";
import { SecurityCard, TokensCard, type ApiToken, type DeviceSession } from "@/components/settings/account-cards";
import { BrowserCard } from "@/components/settings/browser-card";
import { FilesDialog } from "@/components/settings/files-dialog";
import { GenerationFields, describeGeneration, generationKeys, generationValues, modelOptions, readGeneration, type Generation, type PolicyModel } from "@/components/settings/generation";
import { UsageCard, type OwnUsage } from "@/components/settings/usage-card";
import { api } from "@/lib/api";
import { keys, useFeature, useModels, usePreferences, usePresets, type Model } from "@/lib/queries";
import { useSession, useUser } from "@/lib/session";

export interface Preferences extends Generation {
  defaultModel?: string;
  density?: "comfortable" | "compact";
  focus?: boolean;
  context?: boolean;
  favorites?: string[];
  agentInstructions?: string | null;
}
export interface PreferencesResponse {
  preferences?: Preferences;
  overrides?: Record<string, unknown>;
  defaultAgentInstructions?: string;
}
export interface Preset {
  id: string;
  name: string;
  model: string;
  parameters?: Generation;
}

/** The preferences as Settings shows them: saved values over the default model's policy. */
export function shownPreferences(data: PreferencesResponse | undefined, models: Model[]): Preferences {
  const preferences = data?.preferences || {};
  const preferred = models.find((m) => m.id === preferences.defaultModel) as PolicyModel | undefined;
  return { ...preferences, ...generationValues(preferred, data?.overrides || {}) };
}

/** Saves part of the preferences and shares the answer with every page. */
export function useSavePreferences() {
  const queries = useQueryClient();
  return async (body: Record<string, unknown>) => {
    const r = await api.request<PreferencesResponse>("/api/preferences", { method: "PATCH", body });
    queries.setQueryData(keys.preferences, r);
    await queries.invalidateQueries({ queryKey: keys.preferences });
    toast("Preferences saved");
    return r;
  };
}

const accountKey = ["settings", "account"] as const;

export function PersonalSettings() {
  usePageChrome("Settings");
  const user = useUser();
  const { reload } = useSession();
  const router = useRouter();
  const queries = useQueryClient();
  const workspaces = useFeature("workspaces");
  const browser = useFeature("browser");
  const { data: models = [] } = useModels();
  const { data: preferenceData } = usePreferences() as { data: PreferencesResponse | undefined };
  const { data: presetData } = usePresets() as { data: { presets?: Preset[] } | undefined };
  const presets = presetData?.presets ?? [];
  const p = shownPreferences(preferenceData, models);
  const save = useSavePreferences();
  const account = useQuery({
    queryKey: accountKey,
    queryFn: async () => {
      const [s, u, t] = await Promise.all([
        api.request<{ sessions?: DeviceSession[] }>("/api/auth/sessions"),
        api.request<OwnUsage>("/api/usage"),
        api.request<{ tokens?: ApiToken[] }>("/api/auth/tokens"),
      ]);
      return { sessions: s.sessions ?? [], usage: u, tokens: t.tokens ?? [] };
    },
  });
  const refresh = () => queries.invalidateQueries({ queryKey: accountKey });
  const [files, setFiles] = useState(false);
  const [preset, setPreset] = useState<Preset | "new" | null>(null);
  const confirm = useConfirm();

  const profile = useSubmit(async (form) => {
    await api.request("/api/profile", { method: "PATCH", body: { displayName: String(form.get("profile-name") || "").trim() } });
    await reload();
    toast("Profile updated");
  });
  const defaults = useSubmit((form) => save({ defaultModel: String(form.get("default-model")), ...readGeneration(form, "default") }));
  const appearance = useSubmit((form) =>
    save({ density: form.get("appearance-compact") === "on" ? "compact" : "comfortable", focus: form.get("appearance-focus") === "on", context: form.get("appearance-context") === "on" }),
  );
  const favorites = useSubmit((form) => save({ favorites: models.filter((m) => m.id !== "auto" && form.get(`favorite-${m.id}`) === "on").map((m) => m.id) }));

  function applyPreset(x: Preset) {
    if (!models.some((m) => m.id === x.model)) {
      toast("This preset’s model is not available to your account. Edit the preset to choose another model.");
      return;
    }
    router.push(`/?preset=${encodeURIComponent(x.id)}`);
  }

  return (
    <div>
      <SectionHeading title="Personal settings" description="Your profile, defaults, and signed-in devices." />
      {account.data?.usage && <UsageCard usage={account.data.usage} models={models} />}
      {confirm.element}
      <FilesDialog open={files} onOpenChange={setFiles} />
      <PresetDialog key={preset === "new" ? "new" : preset?.id ?? "none"} preset={preset} onClose={() => setPreset(null)} models={models} values={p} />
      <div className="grid gap-6 lg:grid-cols-[minmax(0,1.35fr)_minmax(0,1fr)]">
        <div className="grid min-w-0 content-start gap-6">
          <Section title="Profile" description="Your name appears on your messages and account.">
            <form onSubmit={profile.onSubmit} className="grid gap-4">
              <TextField id="profile-name" label="Display name" maxLength={128} required defaultValue={displayName(user)} />
              <div>
                <DetailRow label="Username">{user.username}</DetailRow>
                <DetailRow label="Role">{user.role}</DetailRow>
              </div>
              <FormError message={profile.error} />
              <FormActions>
                <Button type="submit" disabled={profile.busy}>
                  Save changes
                </Button>
              </FormActions>
            </form>
          </Section>
          <Section title="Conversation defaults" description="Start new conversations with these settings. Model policies may lock individual controls.">
            {preferenceData ? (
              <form key={JSON.stringify(p)} onSubmit={defaults.onSubmit} className="grid gap-4">
                <SelectField id="default-model" label="Default model" options={modelOptions(models)} defaultValue={p.defaultModel || "auto"} help="Picking a model in a chat also makes it your default." />
                <GenerationFields values={p} prefix="default" />
                <FormError message={defaults.error} />
                <FormActions>
                  <Button type="submit" disabled={defaults.busy}>
                    Save defaults
                  </Button>
                </FormActions>
              </form>
            ) : (
              <Note role="status">Loading…</Note>
            )}
          </Section>
          {workspaces && preferenceData && <AgentCard data={preferenceData} />}
          <Section
            title="Saved presets"
            description="Keep useful combinations ready to use."
            actions={
              <Button size="sm" variant="outline" onClick={() => setPreset("new")}>
                <Plus /> New preset
              </Button>
            }
          >
            {presets.length ? (
              <ul className="grid gap-2">
                {presets.map((x) => (
                  <li key={x.id} className="flex flex-wrap items-start gap-3 rounded-md border px-3 py-2.5">
                    <div className="min-w-0 grow">
                      <h3 className="text-md font-medium">{x.name}</h3>
                      <p className="text-sm text-muted-foreground">{models.find((m) => m.id === x.model)?.name || x.model}</p>
                      <small className="text-xs text-muted-foreground">{describeGeneration(x.parameters || {})}</small>
                    </div>
                    <div className="flex gap-1.5">
                      <Button size="xs" variant="outline" onClick={() => applyPreset(x)}>
                        Use
                      </Button>
                      <Button size="xs" variant="outline" onClick={() => setPreset(x)}>
                        Edit
                      </Button>
                      <Button
                        size="xs"
                        variant="outline"
                        onClick={() =>
                          confirm.ask({
                            title: "Delete this preset?",
                            text: "Your conversations will remain available.",
                            label: "Delete preset",
                            cancel: "Keep preset",
                            action: async () => {
                              await api.request(`/api/presets/${encodeURIComponent(x.id)}`, { method: "DELETE" });
                              await queries.invalidateQueries({ queryKey: keys.presets });
                              toast("Preset deleted");
                            },
                          })
                        }
                      >
                        Delete
                      </Button>
                    </div>
                  </li>
                ))}
              </ul>
            ) : (
              <EmptyState title="No presets yet" text="Save instructions and generation settings you often use together." />
            )}
          </Section>
        </div>
        <div className="grid min-w-0 content-start gap-6">
          {browser && <BrowserCard />}
          <SecurityCard sessions={account.data?.sessions ?? []} loading={account.isFetching && !account.data} error={account.error} onRefresh={refresh} />
          <TokensCard tokens={account.data?.tokens ?? []} onChange={refresh} />
          <Section title="Appearance">
            <div className="grid gap-1.5">
              <h3 className="text-md font-medium">Theme</h3>
              <ThemeToggle labels className="w-fit" />
              <Note>Saved in this browser. System follows your device.</Note>
            </div>
            <Note>Layout preferences are saved to your account across devices.</Note>
            <form key={`${p.density}-${p.focus}-${p.context}`} onSubmit={appearance.onSubmit} className="grid gap-3">
              <CheckField id="appearance-compact" label="Compact spacing" defaultChecked={p.density === "compact"} description="Fit more messages on screen." />
              <CheckField id="appearance-focus" label="Focus mode" defaultChecked={!!p.focus} description="Collapse the main navigation." />
              <CheckField id="appearance-context" label="Conversation context" defaultChecked={p.context !== false} description="Show model and generation details." />
              <FormError message={appearance.error} />
              <FormActions>
                <Button type="submit" disabled={appearance.busy}>
                  Save appearance
                </Button>
              </FormActions>
            </form>
          </Section>
          <Section title="Your files" description="Private text, code, document and image attachments stored in your account.">
            <FormActions>
              <Button size="sm" variant="outline" onClick={() => setFiles(true)}>
                <File /> Manage files
              </Button>
            </FormActions>
          </Section>
          <Section title="Favorite models" description="Pin your usual models to the top of the chooser.">
            <form key={(p.favorites || []).join()} onSubmit={favorites.onSubmit} className="grid gap-3">
              <div className="grid max-h-80 gap-3 overflow-y-auto">
                {models.filter((m) => m.id !== "auto").length ? (
                  models
                    .filter((m) => m.id !== "auto")
                    .map((m) => (
                      <CheckField
                        key={m.id}
                        id={`favorite-${m.id}`}
                        label={m.name || m.id}
                        defaultChecked={(p.favorites || []).includes(m.id)}
                        description={String(m.description || m.detail || "") || undefined}
                      />
                    ))
                ) : (
                  <Note>No models available to this account.</Note>
                )}
              </div>
              <FormError message={favorites.error} />
              <FormActions>
                <Button type="submit" disabled={favorites.busy}>
                  Save favorites
                </Button>
              </FormActions>
            </form>
          </Section>
        </div>
      </div>
    </div>
  );
}

/** The workspace agent's working instructions: Chat's default until the person saves their own. */
function AgentCard({ data }: { data: PreferencesResponse }) {
  const save = useSavePreferences();
  const own = data.preferences?.agentInstructions;
  const fallback = data.defaultAgentInstructions || "";
  const text = own || fallback;
  const form = useSubmit(async (values) => {
    const value = String(values.get("agent-instructions") || "").trim();
    await save({ agentInstructions: !value || value === fallback.trim() ? null : value });
  });
  return (
    <Section
      title="Workspace agent"
      description="How the model works in a workspace: how it explores, edits, checks its work and reports back. Chat always adds the facts about the workspace, such as the project folder and preview, and an AGENTS.md file in a project adds notes for that project."
      actions={<Pill tone={own ? "accent" : ""}>{own ? "Customized" : "Default"}</Pill>}
    >
      <form key={text} onSubmit={form.onSubmit} className="grid gap-4">
        <TextAreaField
          id="agent-instructions"
          label="Instructions"
          rows={16}
          maxLength={16384}
          spellCheck={false}
          className="[&_textarea]:field-sizing-fixed [&_textarea]:font-mono [&_textarea]:text-sm"
          defaultValue={text}
          help={own ? "Your instructions replace the default. Reset to get the latest default." : "Edit to make them your own. Changes apply from your next message."}
        />
        <FormError message={form.error} />
        <FormActions>
          {own && (
            <Button type="button" variant="ghost" onClick={() => save({ agentInstructions: null }).catch((e) => toast(e.message))}>
              Reset to default
            </Button>
          )}
          <Button type="submit" disabled={form.busy}>
            Save instructions
          </Button>
        </FormActions>
      </form>
    </Section>
  );
}

function PresetDialog({ preset, onClose, models, values }: { preset: Preset | "new" | null; onClose: () => void; models: Model[]; values: Preferences }) {
  const queries = useQueryClient();
  const existing = preset && preset !== "new" ? preset : null;
  const shown: Generation = existing?.parameters || Object.fromEntries(generationKeys.map((k) => [k, values[k]]));
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    const id = existing?.id;
    await api.request(`/api/presets${id ? `/${encodeURIComponent(id)}` : ""}`, {
      method: id ? "PATCH" : "POST",
      body: { name: String(form.get("preset-name") || "").trim(), model: String(form.get("preset-model")), parameters: readGeneration(form, "preset") },
    });
    await queries.invalidateQueries({ queryKey: keys.presets });
    onClose();
    toast(id ? "Preset updated" : "Preset created");
  });
  return (
    <FormDialog open={!!preset} onOpenChange={(o) => !o && onClose()} title={existing ? "Edit preset" : "Create preset"} wide>
      <form onSubmit={onSubmit} className="grid gap-4">
        <TextField id="preset-name" label="Preset name" maxLength={80} required defaultValue={existing?.name || ""} />
        <SelectField id="preset-model" label="Model" options={modelOptions(models)} defaultValue={existing?.model || values.defaultModel || "auto"} />
        <GenerationFields values={shown} prefix="preset" />
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            {existing ? "Save preset" : "Create preset"}
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}
