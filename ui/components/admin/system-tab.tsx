"use client";

// Administration › System: which Chat this is, the banner, data retention, knowledge
// search's embedding model, decisions, and guides for everything set where Chat runs.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { BarChart3, BookOpen, Box, ExternalLink, Link2, Lock, Settings, Shield, Terminal } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { ErrorBox, Loading } from "@/components/app-shell/page";
import { Button } from "@/components/ui/button";
import { CheckField, Columns, Field, FormActions, FormError, NativeSelect, Note, Pill, Section, SelectField, TextField, useConfirm, useSubmit } from "@/components/manage/kit";
import { Textarea } from "@/components/ui/textarea";
import { api } from "@/lib/api";
import { when } from "@/lib/format";
import { useFeature } from "@/lib/queries";
import { useSession, type Banner } from "@/lib/session";

// ---- About ----

export interface Health {
  workspaces?: { enabled: boolean; namespace?: string | null; browser?: boolean; off?: string | null };
  about?: {
    version: string;
    repository: string;
    migration?: { version: number; description: string } | null;
    buildMigration?: number | null;
    replica?: string;
    startedAt?: string;
    images?: { workspace?: string | null; browser?: string | null };
  };
}

/** Whether this replica runs workspaces, and if not, why. */
export function workspaceStatus(health: Health | null | undefined) {
  const w = health?.workspaces;
  if (!w) return "Linux workspaces and each person’s browser on Kubernetes";
  if (w.enabled) return `On, in namespace ${w.namespace}${w.browser ? ", with the browser" : ""}`;
  return `Off: ${w.off || "not set up"}`;
}

function Out({ href, children }: { href: string; children: React.ReactNode }) {
  return (
    <a href={href} target="_blank" rel="noopener" className="inline-flex items-center gap-1 underline-offset-2 hover:underline">
      {children}
      <ExternalLink className="size-3" />
    </a>
  );
}
function Image({ value }: { value: string }) {
  const [name, digest] = value.split("@");
  return (
    <>
      <code title={value} className="font-mono text-sm">
        {name!.split("/").pop()}
      </code>
      {digest && <small className="ml-1.5 font-mono text-xs text-faint">{digest.slice(7, 19)}</small>}
    </>
  );
}

/** Which Chat this is: its version and source, the database's migrations and this replica. */
export function AboutCard({ health }: { health: Health | null | undefined }) {
  const a = health?.about;
  if (!a) return null;
  const tag = `v${a.version}`;
  const m = a.migration;
  const ahead = m && a.buildMigration != null && m.version > a.buildMigration;
  const rows: [string, React.ReactNode][] = [
    [
      "Version",
      <>
        <strong className="font-semibold">{tag}</strong> · <Out href={`${a.repository}/releases/tag/${tag}`}>Release notes</Out>
      </>,
    ],
    ["Source", <Out href={a.repository}>{a.repository.replace(/^https:\/\//, "")}</Out>],
    [
      "Database",
      m ? (
        <>
          Migration {String(m.version).padStart(3, "0")}, {m.description}
          {ahead && <small className="text-xs text-faint"> · newer than this replica: another one runs a newer Chat</small>}
        </>
      ) : (
        "Not readable"
      ),
    ],
    [
      "This replica",
      <>
        {a.replica && (
          <>
            <code className="font-mono text-sm">{a.replica}</code> ·{" "}
          </>
        )}
        started {when(a.startedAt)}
      </>,
    ],
    ...(a.images?.workspace ? ([["Workspace image", <Image value={a.images.workspace} />]] as [string, React.ReactNode][]) : []),
    ...(a.images?.browser ? ([["Browser image", <Image value={a.images.browser} />]] as [string, React.ReactNode][]) : []),
  ];
  return (
    <Section title="About">
      <dl className="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-6 gap-y-2 text-md">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-muted-foreground">{k}</dt>
            <dd className="min-w-0 break-words">{v}</dd>
          </div>
        ))}
      </dl>
    </Section>
  );
}

// ---- Banner ----

export interface BannerSettings {
  text: string;
  link?: { label: string; url: string } | null;
  tone?: "info" | "warning";
  signedOut?: boolean;
  dismissible?: boolean;
  updatedAt?: string;
}
/** The banner form's body; a link needs both its text and its address. */
export function bannerBody(form: FormData) {
  const label = String(form.get("banner-link-label") || "").trim();
  const url = String(form.get("banner-link-url") || "").trim();
  if (!label !== !url) throw new Error("A link needs both its text and its address.");
  return {
    text: String(form.get("banner-text") ?? ""),
    link: url ? { label, url } : null,
    tone: String(form.get("banner-tone") || "info"),
    signedOut: form.get("banner-signed-out") === "on",
    dismissible: form.get("banner-dismissible") === "on",
  };
}

export function BannerCard({ banner: b, onSaved }: { banner: BannerSettings; onSaved: (b: BannerSettings) => void }) {
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    const saved = await api.request<BannerSettings>("/api/admin/banner", { method: "PATCH", body: bannerBody(form) });
    onSaved(saved);
    toast(saved.text ? "Banner saved" : "Banner turned off");
  });
  return (
    <Section
      title="Banner"
      description="One line at the top of every page, for an announcement or a note such as that Chat is in development. People see a change the next time they load Chat."
      actions={<Pill>{b.text ? "On" : "Off"}</Pill>}
    >
      <form onSubmit={onSubmit} className="grid gap-4">
        <TextField
          id="banner-text"
          label="Text"
          maxLength={200}
          placeholder="No banner"
          defaultValue={b.text}
          help="Leave blank to show no banner. Keep it short: the top bar shows one line, and the rest when someone points at it."
        />
        <Columns>
          <TextField id="banner-link-label" label="Link text" maxLength={40} placeholder="Optional" defaultValue={b.link?.label || ""} />
          <TextField id="banner-link-url" label="Link address" type="url" maxLength={2048} placeholder="https://" spellCheck={false} autoComplete="off" defaultValue={b.link?.url || ""} />
        </Columns>
        <SelectField
          id="banner-tone"
          label="Style"
          defaultValue={b.tone || "info"}
          help="A notice is quiet; a warning is amber."
          options={[
            ["info", "Notice"],
            ["warning", "Warning"],
          ]}
        />
        <CheckField id="banner-signed-out" label="Show it on the sign-in page" defaultChecked={!!b.signedOut} description="Anyone who opens Chat sees it there, signed in or not." />
        <CheckField id="banner-dismissible" label="Let people close it" defaultChecked={!!b.dismissible} description="A closed banner stays closed in that browser until you change its text or link." />
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            Save banner
          </Button>
        </FormActions>
      </form>
    </Section>
  );
}

// ---- Retention ----

export interface Retention {
  conversationDays: number | null;
  unusedFileDays: number | null;
  auditDays: number | null;
  lastRun?: string | { at?: string } | null;
}
export function retentionBody(form: FormData): Pick<Retention, "conversationDays" | "unusedFileDays" | "auditDays"> {
  const read = (id: string) => (String(form.get(id) ?? "").trim() ? Number(form.get(id)) : null);
  return { conversationDays: read("retention-conversations"), unusedFileDays: read("retention-files"), auditDays: read("retention-audit") };
}
/** "Inactive conversations: delete after 365 days. Unused files: keep forever. …" */
export const retentionSummary = (body: ReturnType<typeof retentionBody>) =>
  (
    [
      ["Inactive conversations", body.conversationDays],
      ["Unused files", body.unusedFileDays],
      ["Audit activity", body.auditDays],
    ] as const
  )
    .map(([label, days]) => `${label}: ${days == null ? "keep forever" : `delete after ${days} days`}`)
    .join(". ");

export function RetentionCard({ retention: r, onSaved, confirm }: { retention: Retention; onSaved: (r: Retention) => void; confirm: ReturnType<typeof useConfirm>["ask"] }) {
  const lastRun = typeof r.lastRun === "string" ? r.lastRun : r.lastRun?.at;
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    const body = retentionBody(form);
    const save = async () => {
      onSaved(await api.request<Retention>("/api/admin/retention", { method: "PATCH", body }));
      toast("Retention policy saved");
    };
    if (Object.values(body).some((v) => v != null))
      confirm({
        title: "Enable automatic deletion?",
        text: `${retentionSummary(body)}. This applies to all accounts and permanently deletes eligible data during scheduled cleanup. Backup retention is managed separately.`,
        label: "Apply deletion policy",
        action: save,
      });
    else await save();
  });
  const on = [r.conversationDays, r.unusedFileDays, r.auditDays].some((v) => v != null);
  return (
    <Section
      title="Data retention"
      description="Keep all data by default. Enter a number of days to automatically remove eligible data across this installation."
      actions={<Pill tone={on ? "amber" : ""}>{on ? "Automatic deletion enabled" : "Keep forever"}</Pill>}
    >
      <form onSubmit={onSubmit} className="grid gap-4">
        <TextField
          id="retention-conversations"
          label="Inactive conversations · days"
          type="number"
          min={1}
          max={3650}
          step={1}
          placeholder="Keep forever"
          defaultValue={r.conversationDays ?? ""}
          help="Delete conversations with no recent activity, including archived conversations. Active responses are excluded."
        />
        <TextField
          id="retention-files"
          label="Unused files · days"
          type="number"
          min={1}
          max={3650}
          step={1}
          placeholder="Keep forever"
          defaultValue={r.unusedFileDays ?? ""}
          help="Delete older uploads only when no saved conversation references them."
        />
        <TextField
          id="retention-audit"
          label="Audit activity · days"
          type="number"
          min={30}
          max={3650}
          step={1}
          placeholder="Keep forever"
          defaultValue={r.auditDays ?? ""}
          help="Keep at least 30 days when automatic audit cleanup is enabled."
        />
        <Note className="rounded-md border border-amber/40 bg-amber/10 px-3 py-2 text-amber">
          Automatic deletion is permanent and applies to all accounts. Leave fields blank to keep data forever. Backup retention is managed separately.
        </Note>
        {lastRun && <Note>Last cleanup: {when(lastRun)}</Note>}
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            Save retention
          </Button>
        </FormActions>
      </form>
    </Section>
  );
}

// ---- Knowledge search (embedding) ----

interface Prefixes {
  queryPrefix: string;
  documentPrefix: string;
}
export interface Embedding {
  error?: string;
  chosen?: { model: string | null; queryPrefix: string | null; documentPrefix: string | null };
  defaults: Prefixes & { model: string };
  effective: Prefixes & { model: string };
  models?: string[] | null;
  prefixes?: Record<string, Prefixes>;
  index: { documents: number; pendingDocuments: number; savedItems: number; pendingSavedItems: number };
  reindexed?: boolean;
}
const n = (count: number, one: string, many = `${one}s`) => `${count} ${count === 1 ? one : many}`;
const crlf = (s: string) => s.replace(/\r\n/g, "\n");

/** A new model brings its own prefixes, unless the ones shown were changed by hand. */
export function nextPrefixes(prefixes: Embedding["prefixes"], previous: string, next: string, current: Prefixes): Prefixes {
  const before = prefixes?.[previous] || ({} as Partial<Prefixes>);
  const after = prefixes?.[next] || { queryPrefix: "", documentPrefix: "" };
  return {
    queryPrefix: crlf(current.queryPrefix) === (before.queryPrefix ?? "") ? after.queryPrefix : current.queryPrefix,
    documentPrefix: crlf(current.documentPrefix) === (before.documentPrefix ?? "") ? after.documentPrefix : current.documentPrefix,
  };
}
/** What to save: a value equal to the default follows the default (the environment's model, the model's own prefixes). */
export function embeddingBody(e: Embedding, next: Prefixes & { model: string }) {
  const pick = (value: string, fallback: string) => (value === fallback ? null : value);
  const own = e.prefixes?.[next.model] || { queryPrefix: "", documentPrefix: "" };
  return { model: pick(next.model, e.defaults.model), queryPrefix: pick(crlf(next.queryPrefix), own.queryPrefix), documentPrefix: pick(crlf(next.documentPrefix), own.documentPrefix) };
}
export const embeddingStatus = (e: Embedding) => {
  const i = e.index;
  return i.pendingDocuments || i.pendingSavedItems
    ? `Indexing again: ${n(i.pendingDocuments, "document")} of ${i.documents} and ${n(i.pendingSavedItems, "saved item")} of ${i.savedItems} still to do. Until then, search finds only what is done.`
    : `${n(i.documents, "document")} and ${n(i.savedItems, "saved item")} are indexed with ${e.effective.model}.`;
};

export function EmbeddingCard({ embedding: e, onSaved, confirm }: { embedding: Embedding; onSaved: (e: Embedding) => void; confirm: ReturnType<typeof useConfirm>["ask"] }) {
  const current = e.effective.model;
  const [model, setModel] = useState(current);
  const [prefixes, setPrefixes] = useState<Prefixes>({ queryPrefix: e.effective.queryPrefix, documentPrefix: e.effective.documentPrefix });
  const models = [...new Set([...(e.models || []), current, e.defaults.model])].sort();
  const indexing = !!(e.index.pendingDocuments || e.index.pendingSavedItems);
  /** Any change to what makes the vectors re-indexes everything, so it is confirmed first. */
  async function save(body: ReturnType<typeof embeddingBody>, next: Prefixes & { model: string }) {
    const run = async () => {
      const r = await api.request<Embedding>("/api/admin/knowledge/embedding", { method: "PATCH", body });
      onSaved(r);
      toast(r.reindexed ? "Saved. Knowledge is being indexed again." : "Saved");
    };
    const now = e.effective;
    if (next.model === now.model && crlf(next.queryPrefix) === now.queryPrefix && crlf(next.documentPrefix) === now.documentPrefix) return run();
    confirm({
      title: "Index everything again?",
      text: `Chat will index all ${n(e.index.documents, "document")} and ${n(e.index.savedItems, "saved item")} again with ${next.model}. Vectors from the old and new settings can't be compared, so until it finishes, people's searches find only what has been redone. This can't be undone by switching back: that indexes everything again too.`,
      label: "Change and re-index",
      action: run,
    });
  }
  const form = useSubmit(async () => {
    const next = { model, ...prefixes };
    await save(embeddingBody(e, next), next);
  });
  return (
    <Section
      title="Knowledge search"
      description="Chat turns documents and saved items into vectors with an embedding model, so search can find passages by meaning."
      actions={<Pill tone={indexing ? "amber" : "accent"}>{indexing ? "Indexing" : "Up to date"}</Pill>}
    >
      <Note>{embeddingStatus(e)}</Note>
      <form onSubmit={form.onSubmit} className="grid gap-4">
        <Field
          id="embedding-model"
          label="Embedding model"
          help={e.models ? "Embedding models your gateway offers." : "The gateway could not list its embedding models, so only the current and default models are shown."}
        >
          <NativeSelect
            id="embedding-model"
            name="embedding-model"
            value={model}
            onChange={(event) => {
              setPrefixes((old) => nextPrefixes(e.prefixes, model, event.target.value, old));
              setModel(event.target.value);
            }}
          >
            {models.map((id) => (
              <option key={id} value={id}>
                {id}
                {id === e.defaults.model ? " (default)" : ""}
              </option>
            ))}
          </NativeSelect>
        </Field>
        <Field id="embedding-query" label="Search prefix" help="Put before each search. Chat fills in the prefix the model’s makers publish; change it only if the model’s documentation says otherwise.">
          <Textarea
            id="embedding-query"
            name="embedding-query"
            rows={2}
            maxLength={500}
            spellCheck={false}
            className="font-mono text-sm"
            value={prefixes.queryPrefix}
            onChange={(ev) => setPrefixes((old) => ({ ...old, queryPrefix: ev.target.value }))}
          />
        </Field>
        <Field id="embedding-document" label="Passage prefix" help="Put before each passage and saved item. Many models use none.">
          <Textarea
            id="embedding-document"
            name="embedding-document"
            rows={2}
            maxLength={500}
            spellCheck={false}
            className="font-mono text-sm"
            value={prefixes.documentPrefix}
            onChange={(ev) => setPrefixes((old) => ({ ...old, documentPrefix: ev.target.value }))}
          />
        </Field>
        <Note className="rounded-md border border-amber/40 bg-amber/10 px-3 py-2 text-amber">
          Changing the model or a prefix makes Chat index every document and saved item again: vectors from different models can’t be compared. Until that finishes, search finds only what has been redone, and a large library
          takes a while and uses the gateway’s embedding capacity.
        </Note>
        <FormError message={form.error} />
        <FormActions>
          <Button
            type="button"
            variant="outline"
            onClick={() =>
              save({ model: null, queryPrefix: null, documentPrefix: null }, e.defaults).catch((err) => toast(err.message))
            }
          >
            Use defaults
          </Button>
          <Button type="submit" disabled={form.busy}>
            Save
          </Button>
        </FormActions>
      </form>
    </Section>
  );
}

// ---- Decisions ----

export interface Decisions {
  error?: string;
  endpoint: "gateway" | "custom" | "off";
  url?: string | null;
  hasKey?: boolean;
  keyUnreadable?: boolean;
  canStoreKey?: boolean;
  model?: string | null;
  models?: string[];
  checkRuns?: boolean;
  checkTree?: boolean;
  checkReports?: boolean;
  pickSections?: boolean;
  gateway?: boolean;
}
export const decisionsUsage = (endpoint: string) =>
  ({
    gateway: "Questions go to the gateway as the person they are for, so they count toward that person’s usage and limits.",
    custom: "Questions go with this endpoint’s key, so their usage counts against the key, not against people.",
    off: "Nothing is asked. Features that use decisions run without them.",
  })[endpoint] || "";

/** The form as a settings change. The URL and key count only for another endpoint, and a blank key keeps the saved one. */
export function decisionsBody(form: FormData) {
  const on = (name: string) => form.get(name) === "on";
  const body: Record<string, unknown> = {
    endpoint: String(form.get("decisions-endpoint") || ""),
    model: String(form.get("decisions-model") || "").trim() || null,
    checkRuns: on("decisions-check-runs"),
    checkTree: on("decisions-check-tree"),
    checkReports: on("decisions-check-reports"),
    pickSections: on("decisions-pick-sections"),
  };
  if (body.endpoint !== "custom") return body;
  const url = String(form.get("decisions-url") || "").trim();
  const key = String(form.get("decisions-key") || "").trim();
  if (url) body.url = url;
  if (key) body.key = key;
  else if (on("decisions-remove-key")) body.key = null;
  return body;
}
/** What Check found, in one sentence. */
export function checkResult(r: { ok: boolean; model?: string; yes?: number; ms?: number; error?: string }) {
  const who = r.model || "The endpoint";
  const yes = Math.round((r.yes || 0) * 100);
  return !r.ok
    ? `No answer: ${r.error}`
    : (r.yes || 0) >= 0.5
      ? `Working. ${who} answered a sample true statement correctly (${yes}% yes) in ${r.ms} ms.`
      : `${who} answered a sample true statement wrongly (${yes}% yes). Try another model.`;
}

export function DecisionsCard({ decisions: d, onSaved }: { decisions: Decisions; onSaved: (d: Decisions) => void }) {
  const [endpoint, setEndpoint] = useState(d.endpoint);
  const [result, setResult] = useState("");
  const form = useSubmit(async (values) => {
    onSaved(await api.request<Decisions>("/api/admin/decisions", { method: "PATCH", body: decisionsBody(values) }));
    toast("Decisions saved");
  });
  const keyHelp = d.keyUnreadable
    ? "The saved key can’t be read because the installation’s secret key changed. Enter it again."
    : d.hasKey
      ? "A key is saved. Leave this blank to keep it."
      : d.canStoreKey
        ? "Sent as a bearer token. Leave blank if the endpoint needs none."
        : "Saving a key needs OBLETH_CHAT_SECRET_KEY_FILE, which encrypts it.";
  async function check(button: HTMLButtonElement) {
    const f = button.form;
    if (!f) return;
    setResult("Asking…");
    try {
      setResult(checkResult(await api.request("/api/admin/decisions/check", { method: "POST", body: decisionsBody(new FormData(f)) })));
    } catch (e) {
      setResult(`No answer: ${(e as Error).message}`);
    }
  }
  return (
    <Section
      title="Decisions"
      description="Short yes/no and pick-one questions Chat asks a model, such as whether a research run really tested its idea or a report matches its runs. Chat asks them in the verdicts format, so your gateway or any service that speaks it can answer. When decisions are off or fail, work goes on without them."
      actions={<Pill tone={d.endpoint === "off" ? "" : "accent"}>{d.endpoint === "off" ? "Off" : "On"}</Pill>}
    >
      <form onSubmit={form.onSubmit} className="grid gap-4">
        <SelectField
          id="decisions-endpoint"
          label="Ask"
          value={endpoint}
          onChange={(e) => setEndpoint(e.target.value as Decisions["endpoint"])}
          help={decisionsUsage(endpoint)}
          options={[
            ["gateway", "This gateway"],
            ["custom", "Another endpoint"],
            ["off", "Off"],
          ]}
        />
        <div className="grid gap-4" hidden={endpoint !== "custom"} data-testid="decisions-custom">
          <TextField
            id="decisions-url"
            label="Endpoint URL"
            type="url"
            maxLength={2048}
            placeholder="https://decisions.example.com"
            spellCheck={false}
            autoComplete="off"
            defaultValue={d.url || ""}
            help="An address that takes POST requests in the verdicts format. An address with no path, or one ending in /v1, gets /v1/verdicts added."
          />
          <TextField
            id="decisions-key"
            label="Key"
            type="password"
            maxLength={4096}
            autoComplete="off"
            disabled={!d.canStoreKey}
            placeholder={d.hasKey && !d.keyUnreadable ? "Saved key kept" : ""}
            help={keyHelp}
          />
          {d.hasKey && <CheckField id="decisions-remove-key" label="Remove the saved key" />}
        </div>
        <TextField
          id="decisions-model"
          label="Model"
          maxLength={160}
          list="decisions-models"
          placeholder="The endpoint’s default"
          spellCheck={false}
          autoComplete="off"
          defaultValue={d.model || ""}
          help="Leave blank to let the endpoint choose. The model must return token probabilities (logprobs): most self-hosted servers do, and many hosted APIs don’t."
        />
        <datalist id="decisions-models">
          {(d.models || []).map((id) => (
            <option key={id} value={id} />
          ))}
        </datalist>
        <CheckField
          id="decisions-check-runs"
          label="Check research runs"
          defaultChecked={!!d.checkRuns}
          description="When a run finishes, ask whether it really tested its idea, and tell the chat when it may not have. The result shows on the project’s Research tab."
        />
        <CheckField
          id="decisions-check-tree"
          label="Check where new experiments go"
          defaultChecked={!!d.checkTree}
          description="Before a new experiment is made, ask whether it builds on its parent, and refuse it once when it may belong somewhere else."
        />
        <CheckField
          id="decisions-check-reports"
          label="Check research reports"
          defaultChecked={!!d.checkReports}
          description="Ask whether comparisons and causes in a research reply follow from its runs. Values a reply quotes from runs are checked either way."
        />
        <CheckField
          id="decisions-pick-sections"
          label="Pick sections of papers"
          defaultChecked={!!d.pickSections}
          description="When research reads a paper, ask which of its sections answer what the chat is looking for, and return those. Off, the chat gets the outline and chooses itself."
        />
        <p className="text-sm text-muted-foreground" aria-live="polite">
          {result}
        </p>
        <FormError message={form.error} />
        <FormActions>
          <Button type="button" variant="outline" size="sm" onClick={(e) => check(e.currentTarget)}>
            Check
          </Button>
          <Button type="submit" disabled={form.busy}>
            Save
          </Button>
        </FormActions>
      </form>
    </Section>
  );
}

// ---- The tab ----

const DOCS = "https://www.obleth.com/docs/run-chat";

/** Asks for one part of the System tab; a failure becomes that card's error instead of the tab's. */
const orError = <T,>(path: string) => api.request<T>(path).catch((e: Error) => ({ error: e.message }) as T & { error: string });

export function SystemTab() {
  const queries = useQueryClient();
  const session = useSession();
  const knowledge = useFeature("knowledge");
  const confirm = useConfirm();
  const retention = useQuery({ queryKey: ["admin", "retention"], queryFn: () => api.request<Retention>("/api/admin/retention") });
  const embedding = useQuery({ queryKey: ["admin", "embedding"], queryFn: () => orError<Embedding>("/api/admin/knowledge/embedding"), enabled: knowledge });
  const health = useQuery({ queryKey: ["admin", "health"], queryFn: () => api.request<Health>("/api/admin/health").catch(() => null) });
  const decisions = useQuery({ queryKey: ["admin", "decisions"], queryFn: () => orError<Decisions>("/api/admin/decisions") });
  const banner = useQuery({ queryKey: ["admin", "banner"], queryFn: () => orError<BannerSettings & { error?: string }>("/api/admin/banner") });
  const set = <T,>(key: string) => (value: T) => queries.setQueryData(["admin", key], value);
  const pages: [typeof Box, string, string, string][] = [
    [Box, "Connect the gateway", "/connect-gateway", "Model access and inference through Obleth"],
    [Lock, "Accounts and sign-in", "/accounts", "Local accounts and single sign-on"],
    [BookOpen, "Knowledge", "/enable-knowledge", "Collections and saved items in S3"],
    [Terminal, "Workspaces and the browser", "/enable-workspaces", workspaceStatus(health.data)],
    [Link2, "Connections and clusters", "/enable-connections", "MCP servers, Slurm and Kubernetes"],
    [Shield, "Reverse proxies", "/reverse-proxies", "Client addresses and sign-in limits"],
    [BarChart3, "Operate Chat", "/operations", "Health checks, metrics and backups"],
    [Settings, "Environment variables", "/environment-variables", "Every setting in one list"],
  ];
  const b = banner.data;
  const e = embedding.data;
  const d = decisions.data;
  return (
    <div className="grid gap-4">
      {confirm.element}
      <ErrorBox error={retention.error} onRetry={() => retention.refetch()} />
      {retention.isLoading && <Loading label="Loading administration…" />}
      <div className="grid items-start gap-4 lg:grid-cols-2">
        <div className="grid min-w-0 gap-4">
          <AboutCard health={health.data} />
          {b &&
            (b.error ? (
              <Section title="Banner">
                <ErrorBox error={b.error} />
              </Section>
            ) : (
              <BannerCard
                key={b.updatedAt ?? ""}
                banner={b}
                onSaved={(saved) => {
                  set<BannerSettings>("banner")(saved);
                  session.setBanner(saved.text ? (saved as Banner) : null);
                }}
              />
            ))}
          {retention.data && <RetentionCard key={JSON.stringify(retention.data)} retention={retention.data} onSaved={set<Retention>("retention")} confirm={confirm.ask} />}
        </div>
        <div className="grid min-w-0 gap-4">
          {knowledge &&
            e &&
            (e.error ? (
              <Section title="Knowledge search">
                <ErrorBox error={e.error} />
              </Section>
            ) : (
              <EmbeddingCard key={JSON.stringify(e.effective)} embedding={e} onSaved={set<Embedding>("embedding")} confirm={confirm.ask} />
            ))}
          {d &&
            (d.error ? (
              <Section title="Decisions">
                <ErrorBox error={d.error} />
              </Section>
            ) : (
              <DecisionsCard key={JSON.stringify(d)} decisions={d} onSaved={set<Decisions>("decisions")} />
            ))}
          <Section title="Set where Chat runs" description="The rest is configured with environment variables when Chat is deployed. These guides show how.">
            <div className="grid gap-1">
              {pages.map(([Icon, label, path, note]) => (
                <a key={path} href={`${DOCS}${path}`} target="_blank" rel="noopener" className="flex items-center gap-3 rounded-md px-2 py-2 hover:bg-accent">
                  <Icon className="size-4 shrink-0 text-muted-foreground" />
                  <span className="grid min-w-0 grow">
                    <span className="text-md font-medium">{label}</span>
                    <span className="truncate text-sm text-muted-foreground">{note}</span>
                  </span>
                  <ExternalLink className="size-3.5 shrink-0 text-muted-foreground" />
                </a>
              ))}
            </div>
          </Section>
        </div>
      </div>
    </div>
  );
}
