"use client";

// Administration › Appearance: the site's name, its accent color, the theme people
// start in, whether they may pick their own accent, and how round corners are. Each
// can be left as the installation's branding file has it. The preview shows the
// choice on both themes before it is saved; saving changes the page at once here,
// and for everyone else the next time they load Chat.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { toast } from "sonner";
import { ErrorBox, Loading } from "@/components/app-shell/page";
import { CheckField, Field, FormActions, FormError, Note, Pill, Section, SelectField, TextField, useSubmit } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { ACCENTS, DEFAULT_ACCENT, GROUND, accentHex, shadeFor } from "@/lib/accent";
import { api } from "@/lib/api";
import { normalizeBranding } from "@/lib/branding";
import { when } from "@/lib/format";
import { BRAND_ICON_NAMES } from "@/lib/icons";
import { CORNER_NAMES, cornerProperties, siteAppearance, type Corners } from "@/lib/site-appearance";
import { cn } from "@/lib/utils";

/** The settings as the server keeps them: null leaves one to the installation's file. */
export interface AppearanceSettings {
  siteName: string | null;
  accent: string | null;
  defaultTheme: "system" | "light" | "dark" | null;
  personalAccent: boolean | null;
  corners: Corners | null;
  updatedAt?: string;
}

const KEY = ["admin-appearance"] as const;
const THEME_NAMES = { system: "Follow the device", light: "Light", dark: "Dark" } as const;
const CORNER_LABELS: Record<Corners, string> = { sharp: "Sharp", soft: "Soft", round: "Round" };

/** A typed color, with or without its #; "" when it is not one. */
export const typedHex = (text: string) => accentHex(text.trim().startsWith("#") ? text.trim() : `#${text.trim()}`);

/** What the form sends: a blank name and "as installed" choices go as null. */
export function appearanceBody(form: { siteName: string; accent: string; defaultTheme: string; personalAccent: string; corners: string }): Omit<AppearanceSettings, "updatedAt"> {
  const accent = form.accent.trim();
  if (accent && !typedHex(accent)) throw new Error("The accent is a color as six hex digits, such as #d7f25a.");
  return {
    siteName: form.siteName.trim() || null,
    accent: accent ? typedHex(accent) : null,
    defaultTheme: (["system", "light", "dark"].includes(form.defaultTheme) ? form.defaultTheme : null) as AppearanceSettings["defaultTheme"],
    personalAccent: form.personalAccent === "yes" ? true : form.personalAccent === "no" ? false : null,
    corners: (CORNER_NAMES.includes(form.corners as Corners) ? form.corners : null) as Corners | null,
  };
}

/** One theme's sample: a card, a step that is running, the status line and a button, in the chosen look. */
function Sample({ theme, accent, corners, name }: { theme: "dark" | "light"; accent: string; corners: Corners; name: string }) {
  const shade = shadeFor(accent, GROUND[theme]);
  const dark = theme === "dark";
  const radii = { "--corner-sm": "4px", "--corner-md": "6px", "--corner-lg": "8px", "--corner-xl": "12px", ...cornerProperties(corners) } as Record<string, string>;
  const ink = dark ? "#fafafa" : "#18181b",
    quiet = dark ? "#a1a1aa" : "#5b6472",
    line = dark ? "#2f2f34" : "#c4cad3",
    card = dark ? "#111113" : "#ffffff";
  return (
    <figure className="m-0 min-w-0" data-sample={theme}>
      <div className="overflow-hidden border" style={{ background: GROUND[theme], color: ink, borderColor: line, borderRadius: radii["--corner-xl"] }}>
        <div className="grid gap-2.5 p-3.5">
          <div className="text-sm font-medium">{name}</div>
          <div className="grid gap-2 border p-3" style={{ background: card, borderColor: line, borderRadius: radii["--corner-lg"] }}>
            <div className="flex items-center gap-2 text-sm">
              <span className="size-2 shrink-0 rounded-full" style={{ background: shade.color }} />
              <span style={{ color: shade.color }}>Running the sweep</span>
              <span className="ml-auto font-mono text-xs" style={{ color: quiet }}>
                1:20
              </span>
            </div>
            <div className="h-1.5 overflow-hidden rounded-full" style={{ background: line }}>
              <div className="h-full w-2/5 rounded-full" style={{ background: shade.color }} />
            </div>
            <div className="flex items-center gap-2">
              <span className="border px-2.5 py-1 text-xs" style={{ borderColor: line, borderRadius: radii["--corner-md"], color: quiet }}>
                Stop
              </span>
              <span className="px-2.5 py-1 text-xs font-medium" style={{ background: ink, color: GROUND[theme], borderRadius: radii["--corner-md"] }}>
                Send
              </span>
            </div>
          </div>
        </div>
        <div className="flex border-t font-mono text-xs" style={{ borderColor: line }}>
          <span className="px-3 py-1.5 font-medium" style={{ background: shade.color, color: shade.foreground }}>
            Working 1:20
          </span>
          <span className="px-3 py-1.5" style={{ color: quiet }}>
            context 38%
          </span>
        </div>
      </div>
      <figcaption className="mt-1.5 text-xs text-muted-foreground">
        {dark ? "Dark" : "Light"}
        {shade.adjusted && ` · shown as ${shade.color} to stay readable`}
      </figcaption>
    </figure>
  );
}

function Editor({ saved, onSaved }: { saved: AppearanceSettings; onSaved: (s: AppearanceSettings) => void }) {
  // What the installation's own file says, for "as installed" and the preview.
  const file = useMemo(() => normalizeBranding(typeof window === "undefined" ? undefined : window.OblethChatConfig, BRAND_ICON_NAMES), []);
  const installed = { accent: file.appearance.accent || DEFAULT_ACCENT, theme: file.appearance.defaultTheme, personal: file.appearance.personalAccent };
  const [form, setForm] = useState({
    siteName: saved.siteName ?? "",
    accent: saved.accent ?? "",
    defaultTheme: saved.defaultTheme ?? "",
    personalAccent: saved.personalAccent == null ? "" : saved.personalAccent ? "yes" : "no",
    corners: saved.corners ?? "",
  });
  const change = (patch: Partial<typeof form>) => setForm((f) => ({ ...f, ...patch }));
  const { busy, error, onSubmit } = useSubmit(async () => {
    const next = await api.request<AppearanceSettings>("/api/admin/appearance", { method: "PATCH", body: appearanceBody(form) });
    onSaved(next);
    // This page takes the look now; everyone else gets it with their next session answer.
    siteAppearance.set(Object.fromEntries(Object.entries(next).filter(([, value]) => value != null)));
    toast("Appearance saved");
  });
  const shownAccent = typedHex(form.accent) || installed.accent;
  const shownCorners = (form.corners || "soft") as Corners;
  const custom = !!form.accent && !ACCENTS.some((a) => a.value === typedHex(form.accent));
  const swatches = [{ name: "As installed", value: "", shown: installed.accent }, ...ACCENTS.map((a) => ({ ...a, shown: a.value }))];
  const untouched = !saved.siteName && !saved.accent && !saved.defaultTheme && saved.personalAccent == null && !saved.corners;
  return (
    <Section
      title="Appearance"
      description="How this site looks to everyone. Anything left as installed follows the branding file where Chat runs. People see a change the next time they load Chat."
      actions={<Pill>{untouched ? "As installed" : `Changed ${saved.updatedAt ? when(saved.updatedAt) : ""}`.trim()}</Pill>}
    >
      <form onSubmit={onSubmit} className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_minmax(0,22rem)]">
        <div className="grid content-start gap-4">
          <TextField id="appearance-name" label="Site name" maxLength={60} placeholder={file.siteName} value={form.siteName} onChange={(e) => change({ siteName: e.target.value })} help="Beside the logo, in the browser tab and on the sign-in page. Blank uses the installed name." />
          <Field id="appearance-accent" label="Accent" help="The one color that marks what is alive: a step that is running, a reply being written, a job at work. Chat adjusts it per theme so it stays readable.">
            <div className="flex flex-wrap items-center gap-1.5">
              <div role="radiogroup" aria-label="Accent color" className="flex flex-wrap gap-1.5">
                {swatches.map((swatch) => {
                  const on = (form.accent.trim() ? typedHex(form.accent) : "") === swatch.value;
                  return (
                    <button
                      key={swatch.name}
                      type="button"
                      role="radio"
                      aria-checked={on}
                      aria-label={swatch.name}
                      title={swatch.name}
                      onClick={() => change({ accent: swatch.value })}
                      className={cn("relative flex size-8 items-center justify-center rounded-full border-2 border-transparent p-0.5 transition-colors hover:border-line-strong", on && "border-foreground hover:border-foreground")}
                    >
                      <span className="size-full rounded-full" style={{ background: swatch.shown }} />
                      {!swatch.value && <span className="absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full border bg-background" aria-hidden="true" />}
                    </button>
                  );
                })}
              </div>
              <input
                id="appearance-accent"
                aria-label="Accent as a hex color"
                value={form.accent}
                onChange={(e) => change({ accent: e.target.value })}
                placeholder={installed.accent}
                spellCheck={false}
                autoComplete="off"
                maxLength={7}
                className={cn("h-8 w-28 rounded-md border bg-transparent px-2.5 font-mono text-sm outline-none focus-visible:border-focus-border", custom && "border-line-strong")}
              />
            </div>
          </Field>
          <SelectField
            id="appearance-theme"
            label="Theme people start in"
            value={form.defaultTheme}
            onChange={(e) => change({ defaultTheme: e.target.value })}
            help="For anyone who has not picked one. Their own choice in the header or Settings wins."
            options={[["", `As installed (${THEME_NAMES[installed.theme].toLowerCase()})`], ...(Object.entries(THEME_NAMES) as [string, string][])]}
          />
          <SelectField
            id="appearance-personal"
            label="People may pick their own accent"
            value={form.personalAccent}
            onChange={(e) => change({ personalAccent: e.target.value })}
            help="In Settings, kept in their browser. Off keeps the site's color for everyone."
            options={[
              ["", `As installed (${installed.personal ? "yes" : "no"})`],
              ["yes", "Yes"],
              ["no", "No"],
            ]}
          />
          <SelectField
            id="appearance-corners"
            label="Corners"
            value={form.corners}
            onChange={(e) => change({ corners: e.target.value })}
            help="How round cards, buttons and boxes are."
            options={[["", "As installed (soft)"], ...CORNER_NAMES.map((c) => [c, CORNER_LABELS[c]] as [string, string])]}
          />
          <FormError message={error} />
          <FormActions>
            <Button type="submit" disabled={busy}>
              Save appearance
            </Button>
            <Button type="button" variant="outline" disabled={busy} onClick={() => setForm({ siteName: "", accent: "", defaultTheme: "", personalAccent: "", corners: "" })}>
              Set everything as installed
            </Button>
          </FormActions>
        </div>
        <div className="grid content-start gap-3" aria-label="Preview">
          <div className="text-sm font-medium">Preview</div>
          <Sample theme="dark" accent={shownAccent} corners={shownCorners} name={form.siteName.trim() || file.siteName} />
          <Sample theme="light" accent={shownAccent} corners={shownCorners} name={form.siteName.trim() || file.siteName} />
          <Note>The logo and the sign-in page’s text are set in the branding file where Chat runs.</Note>
        </div>
      </form>
    </Section>
  );
}

export function AppearanceTab() {
  const queries = useQueryClient();
  const query = useQuery({ queryKey: KEY, queryFn: () => api.request<AppearanceSettings>("/api/admin/appearance") });
  if (query.isLoading) return <Loading />;
  if (query.error || !query.data) return <ErrorBox error={query.error || "The appearance could not be read."} onRetry={() => void query.refetch()} />;
  // The key remounts the form on what was saved, so its fields show the server's word.
  return <Editor key={query.data.updatedAt || "new"} saved={query.data} onSaved={(next) => queries.setQueryData(KEY, next)} />;
}
