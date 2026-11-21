"use client";

// Administration › Branding: everything the installation's branding file can say,
// set here instead. The site's name and logo, its look (accent, starting theme,
// corners), the words on the home and sign-in pages, the links and the icons. The
// form starts from what the site shows now; saving keeps only what differs from the
// file, so anything left alone keeps following the file. Images are uploaded on
// their own and show at once. Saving changes this page at once, and everyone else's
// the next time they load Chat.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2, Upload } from "lucide-react";
import { useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { ErrorBox, Loading } from "@/components/app-shell/page";
import { CheckField, Columns, Field, FormActions, FormError, Note, Pill, Section, SelectField, TextAreaField, TextField, useSubmit } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ACCENTS, DEFAULT_ACCENT, GROUND, accentHex, shadeFor } from "@/lib/accent";
import { api } from "@/lib/api";
import { type Branding, type Link, normalizeBranding } from "@/lib/branding";
import { when } from "@/lib/format";
import { BRAND_ICON_NAMES } from "@/lib/icons";
import { CORNER_NAMES, cornerProperties, siteAppearance, withAppearance, type Corners, type SiteAppearance } from "@/lib/site-appearance";
import { cn } from "@/lib/utils";

/** The settings as the server keeps them: null leaves one to the installation's file. */
export interface AppearanceSettings {
  siteName: string | null;
  accent: string | null;
  defaultTheme: "system" | "light" | "dark" | null;
  personalAccent: boolean | null;
  corners: Corners | null;
  /** What was changed of the words, links and icons, in the file's shape. */
  branding?: Record<string, unknown>;
  /** Uploaded images' addresses by slot: logo-light, logo-dark, favicon. */
  images?: Record<string, string>;
  updatedAt?: string;
}

const KEY = ["admin-appearance"] as const;
const THEME_NAMES = { system: "Follow the device", light: "Light", dark: "Dark" } as const;
const CORNER_LABELS: Record<Corners, string> = { sharp: "Sharp", soft: "Soft", round: "Round" };
const SLOTS = [
  ["logo-light", "Logo on the light theme", "light"],
  ["logo-dark", "Logo on the dark theme", "dark"],
  ["favicon", "Browser tab icon", "light"],
] as const;
type Slot = (typeof SLOTS)[number][0];
const IMAGE_TYPES = "image/png,image/svg+xml,image/jpeg,image/webp,image/gif,image/x-icon,.ico";
const MAX_LINKS = 8;

/** A typed color, with or without its #; "" when it is not one. */
export const typedHex = (text: string) => accentHex(text.trim().startsWith("#") ? text.trim() : `#${text.trim()}`);

/** The form: the words as the site shows them, and the look's choices ("" is as installed). */
export interface BrandingForm {
  siteName: string;
  siteSubtitle: string;
  assistantName: string;
  description: string;
  logoAlt: string;
  hideLogo: boolean;
  iconGreeting: string;
  iconAssistant: string;
  iconLoading: string;
  loginTitle: string;
  loginDescription: string;
  ssoLabel: string;
  ssoHint: string;
  loginHelp: string;
  helpLinkLabel: string;
  helpLinkUrl: string;
  links: Link[];
  homeEyebrow: string;
  homeTitle: string;
  homeDescription: string;
  homeFooter: string;
  accent: string;
  defaultTheme: string;
  personalAccent: string;
  corners: string;
}

/** What the server's answer means for the page: the settings that are set, images included. */
export function shownOf(saved: AppearanceSettings): SiteAppearance & { branding: Record<string, unknown> } {
  const look = Object.fromEntries(Object.entries(saved).filter(([key, value]) => value != null && ["siteName", "accent", "defaultTheme", "personalAccent", "corners"].includes(key)));
  const images = saved.images && Object.keys(saved.images).length ? { images: saved.images } : {};
  return { ...look, branding: { ...(saved.branding || {}), ...images } };
}

/** The form for what is saved: the words the site shows with it, over the file's. */
export function formOf(saved: AppearanceSettings, shown: Branding): BrandingForm {
  return {
    siteName: shown.siteName,
    siteSubtitle: shown.siteSubtitle,
    assistantName: shown.assistantName,
    description: shown.description,
    logoAlt: shown.logo.alt,
    hideLogo: saved.branding?.hideLogo === true,
    iconGreeting: shown.icons.greeting,
    iconAssistant: shown.icons.assistant,
    iconLoading: shown.icons.loading,
    loginTitle: shown.login.title,
    loginDescription: shown.login.description,
    ssoLabel: shown.login.ssoLabel,
    ssoHint: shown.login.ssoHint,
    loginHelp: shown.login.help,
    helpLinkLabel: shown.login.helpLink?.label ?? "",
    helpLinkUrl: shown.login.helpLink?.url ?? "",
    links: shown.links.map((link) => ({ ...link })),
    homeEyebrow: shown.home.eyebrow,
    homeTitle: shown.home.title,
    homeDescription: shown.home.description,
    homeFooter: shown.home.footer,
    accent: saved.accent ?? "",
    defaultTheme: saved.defaultTheme ?? "",
    personalAccent: saved.personalAccent == null ? "" : saved.personalAccent ? "yes" : "no",
    corners: saved.corners ?? "",
  };
}

/**
 * What the form sends: for the look, null where a choice is left as installed; for
 * the words, only those that differ from the file's, so the rest keeps following it.
 * A text cleared here that the file has is sent empty, which hides that line.
 */
export function brandingBody(form: BrandingForm, file: Branding): Omit<AppearanceSettings, "updatedAt" | "images"> & { branding: Record<string, unknown> } {
  const accent = form.accent.trim();
  if (accent && !typedHex(accent)) throw new Error("The accent is a color as six hex digits, such as #d7f25a.");
  const branding: Record<string, unknown> = {};
  const group = (name: string, pairs: [string, string, string][]) => {
    const changed = Object.fromEntries(pairs.filter(([, value, installed]) => value.trim() !== installed.trim()).map(([key, value]) => [key, value.trim()]));
    if (Object.keys(changed).length) branding[name] = { ...(branding[name] as object), ...changed };
  };
  for (const [key, value, installed] of [
    ["siteSubtitle", form.siteSubtitle, file.siteSubtitle],
    ["assistantName", form.assistantName, file.assistantName],
    ["description", form.description, file.description],
    ["logoAlt", form.logoAlt, file.logo.alt],
  ] as const)
    if (value.trim() !== installed.trim()) branding[key] = value.trim();
  if (form.hideLogo) branding.hideLogo = true;
  group("icons", [
    ["greeting", form.iconGreeting, file.icons.greeting],
    ["assistant", form.iconAssistant, file.icons.assistant],
    ["loading", form.iconLoading, file.icons.loading],
  ]);
  group("login", [
    ["title", form.loginTitle, file.login.title],
    ["description", form.loginDescription, file.login.description],
    ["ssoLabel", form.ssoLabel, file.login.ssoLabel],
    ["ssoHint", form.ssoHint, file.login.ssoHint],
    ["help", form.loginHelp, file.login.help],
  ]);
  const helpLink = { label: form.helpLinkLabel.trim(), url: form.helpLinkUrl.trim() };
  if (!helpLink.label !== !helpLink.url) throw new Error("The sign-in help link needs both its text and its address.");
  if (helpLink.label !== (file.login.helpLink?.label ?? "") || helpLink.url !== (file.login.helpLink?.url ?? "")) branding.login = { ...(branding.login as object), helpLink };
  const links = form.links.map((link) => ({ label: link.label.trim(), url: link.url.trim() })).filter((link) => link.label || link.url);
  if (links.some((link) => !link.label || !link.url)) throw new Error("Each link needs both its text and its address.");
  if (JSON.stringify(links) !== JSON.stringify(file.links)) branding.links = links;
  group("home", [
    ["eyebrow", form.homeEyebrow, file.home.eyebrow],
    ["title", form.homeTitle, file.home.title],
    ["description", form.homeDescription, file.home.description],
    ["footer", form.homeFooter, file.home.footer],
  ]);
  const name = form.siteName.trim();
  return {
    siteName: name && name !== file.siteName ? name : null,
    accent: accent ? typedHex(accent) : null,
    defaultTheme: (["system", "light", "dark"].includes(form.defaultTheme) ? form.defaultTheme : null) as AppearanceSettings["defaultTheme"],
    personalAccent: form.personalAccent === "yes" ? true : form.personalAccent === "no" ? false : null,
    corners: (CORNER_NAMES.includes(form.corners as Corners) ? form.corners : null) as Corners | null,
    branding,
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

/** One uploaded image: what shows now, a way to replace it, and a way back to the installed one. */
function ImageSlot({ slot, label, ground, shown, uploaded, onChanged }: { slot: Slot; label: string; ground: "light" | "dark"; shown: string; uploaded: boolean; onChanged: (images: Record<string, string>) => void }) {
  const input = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const send = async (work: () => Promise<{ images?: Record<string, string> }>, done: string) => {
    setBusy(true);
    try {
      onChanged((await work()).images ?? {});
      toast(done);
    } catch (e) {
      toast((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const path = `/api/admin/branding/images/${slot}`;
  return (
    <div className="flex min-w-0 items-center gap-3 rounded-md border p-3" data-image-slot={slot}>
      <span className="flex size-14 shrink-0 items-center justify-center overflow-hidden rounded-md border" style={{ background: GROUND[ground] }}>
        {/* eslint-disable-next-line @next/next/no-img-element -- the site's own logo, by address */}
        {shown ? <img src={shown} alt="" className="max-h-10 max-w-10 object-contain" /> : <span className="text-2xs text-muted-foreground">None</span>}
      </span>
      <div className="min-w-0 flex-1">
        <div className="text-md font-medium">{label}</div>
        <div className="text-xs text-muted-foreground">{uploaded ? "Uploaded here" : shown ? "As installed" : "None"}</div>
      </div>
      <input
        ref={input}
        type="file"
        accept={IMAGE_TYPES}
        className="sr-only"
        tabIndex={-1}
        aria-label={`${label}: choose an image`}
        onChange={(e) => {
          const file = e.target.files?.[0];
          e.target.value = "";
          if (!file) return;
          if (file.size > 512 * 1024) return void toast("The image must be at most 512 KB.");
          void send(() => api.request(path, { method: "PUT", raw: file, contentType: "application/octet-stream" }), "Image uploaded");
        }}
      />
      <Button type="button" variant="outline" size="sm" disabled={busy} onClick={() => input.current?.click()}>
        <Upload /> {uploaded ? "Replace" : "Upload"}
      </Button>
      {uploaded && (
        <Button type="button" variant="ghost" size="icon-sm" disabled={busy} aria-label={`Remove the uploaded ${label.toLowerCase()}`} title="Back to the installed image" onClick={() => void send(() => api.request(path, { method: "DELETE" }), "Image removed")}>
          <Trash2 />
        </Button>
      )}
    </div>
  );
}

function Editor({ saved, file, onSaved }: { saved: AppearanceSettings; file: { config: unknown; branding: Branding }; onSaved: (s: AppearanceSettings) => void }) {
  const installed = { accent: file.branding.appearance.accent || DEFAULT_ACCENT, theme: file.branding.appearance.defaultTheme, personal: file.branding.appearance.personalAccent };
  // What the site shows with what is saved: the starting point of the form, and where the images come from.
  const shown = useMemo(() => withAppearance(file.config, shownOf(saved), BRAND_ICON_NAMES), [file, saved]);
  const [form, setForm] = useState(() => formOf(saved, shown));
  const change = (patch: Partial<BrandingForm>) => setForm((f) => ({ ...f, ...patch }));
  const text = (key: keyof BrandingForm) => ({ value: form[key] as string, onChange: (e: { target: { value: string } }) => change({ [key]: e.target.value } as Partial<BrandingForm>) });
  const { busy, error, onSubmit } = useSubmit(async () => {
    const next = await api.request<AppearanceSettings>("/api/admin/appearance", { method: "PATCH", body: brandingBody(form, file.branding) });
    onSaved(next);
    toast("Branding saved");
  });
  const shownAccent = typedHex(form.accent) || installed.accent;
  const shownCorners = (form.corners || "soft") as Corners;
  const custom = !!form.accent && !ACCENTS.some((a) => a.value === typedHex(form.accent));
  const swatches = [{ name: "As installed", value: "", shown: installed.accent }, ...ACCENTS.map((a) => ({ ...a, shown: a.value }))];
  const untouched = !saved.siteName && !saved.accent && !saved.defaultTheme && saved.personalAccent == null && !saved.corners && !Object.keys(saved.branding || {}).length && !Object.keys(saved.images || {}).length;
  const icons: [string, string][] = [["", "None"], ...[...BRAND_ICON_NAMES].sort().map((name) => [name, name] as [string, string])];
  const setLink = (index: number, patch: Partial<Link>) => change({ links: form.links.map((link, i) => (i === index ? { ...link, ...patch } : link)) });
  const images = saved.images || {};
  const slotShown: Record<Slot, string> = { "logo-light": shown.logo.light, "logo-dark": shown.logo.dark, favicon: shown.favicon.light };
  const setImages = (next: Record<string, string>) => onSaved({ ...saved, images: next });
  return (
    <form onSubmit={onSubmit} className="grid gap-4">
      <Section
        title="Name and logo"
        description="What this site is called and the mark beside its name. The form starts from what the site shows now; anything you leave alone keeps following the branding file where Chat runs."
        actions={<Pill>{untouched ? "As installed" : `Changed ${saved.updatedAt ? when(saved.updatedAt) : ""}`.trim()}</Pill>}
      >
        <Columns>
          <TextField id="branding-name" label="Site name" maxLength={60} {...text("siteName")} help="Beside the logo, in the browser tab and on the sign-in page." />
          <TextField id="branding-subtitle" label="Line under the name" maxLength={120} placeholder="None" {...text("siteSubtitle")} />
          <TextField id="branding-assistant" label="The assistant’s name" maxLength={40} {...text("assistantName")} help="On the model’s replies." />
          <TextField id="branding-description" label="Description" maxLength={300} {...text("description")} help="For search engines and link previews." />
        </Columns>
        <div className="grid gap-2 lg:grid-cols-3">
          {SLOTS.map(([slot, label, ground]) => (
            <ImageSlot key={slot} slot={slot} label={label} ground={ground} shown={form.hideLogo && slot !== "favicon" ? "" : slotShown[slot]} uploaded={!!images[slot]} onChanged={setImages} />
          ))}
        </div>
        <Note>PNG, SVG, JPEG, WebP, GIF or ICO, up to 512 KB. One logo covers both themes. An upload shows at once, without Save.</Note>
        <Columns>
          <TextField id="branding-logo-alt" label="The logo, in words" maxLength={120} placeholder="Nothing: the name beside it says it" {...text("logoAlt")} help="Read aloud by screen readers." />
          <CheckField id="branding-hide-logo" label="No logo" description="Show the name alone." checked={form.hideLogo} onCheckedChange={(hideLogo) => change({ hideLogo })} />
        </Columns>
      </Section>

      <Section title="Look" description="The accent is the one color that marks what is alive: a step that is running, a reply being written, a job at work. Chat adjusts it per theme so it stays readable.">
        <div className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_minmax(0,22rem)]">
          <div className="grid content-start gap-4">
            <Field id="appearance-accent" label="Accent">
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
              help="In Settings, kept in their browser. Off keeps the site’s color for everyone."
              options={[
                ["", `As installed (${installed.personal ? "yes" : "no"})`],
                ["yes", "Yes"],
                ["no", "No"],
              ]}
            />
            <SelectField id="appearance-corners" label="Corners" value={form.corners} onChange={(e) => change({ corners: e.target.value })} help="How round cards, buttons and boxes are." options={[["", "As installed (soft)"], ...CORNER_NAMES.map((c) => [c, CORNER_LABELS[c]] as [string, string])]} />
          </div>
          <div className="grid content-start gap-3" aria-label="Preview">
            <div className="text-sm font-medium">Preview</div>
            <Sample theme="dark" accent={shownAccent} corners={shownCorners} name={form.siteName.trim() || file.branding.siteName} />
            <Sample theme="light" accent={shownAccent} corners={shownCorners} name={form.siteName.trim() || file.branding.siteName} />
          </div>
        </div>
      </Section>

      <Section title="Home page" description="The words over the message box of a new chat. Clear a line to hide it.">
        <Columns>
          <TextField id="branding-home-eyebrow" label="Small line over the title" maxLength={120} placeholder="None" {...text("homeEyebrow")} />
          <TextField id="branding-home-title" label="Title" maxLength={120} placeholder="None" {...text("homeTitle")} />
        </Columns>
        <TextAreaField id="branding-home-description" label="Line under the title" maxLength={300} rows={2} placeholder="None" {...text("homeDescription")} />
        <TextAreaField id="branding-home-footer" label="Footer" maxLength={300} rows={2} placeholder="None" {...text("homeFooter")} />
      </Section>

      <Section title="Sign-in page" description="What people see before they sign in. A blank title reads “Sign in to” and the site’s name.">
        <Columns>
          <TextField id="branding-login-title" label="Title" maxLength={120} placeholder={`Sign in to ${form.siteName.trim() || file.branding.siteName}`} {...text("loginTitle")} />
          <TextField id="branding-login-description" label="Line under the title" maxLength={300} placeholder="None" {...text("loginDescription")} />
          <TextField id="branding-sso-label" label="Single sign-on button" maxLength={60} {...text("ssoLabel")} help="Shown only when single sign-on is set up." />
          <TextField id="branding-sso-hint" label="Line under that button" maxLength={120} placeholder="None" {...text("ssoHint")} />
        </Columns>
        <TextAreaField id="branding-login-help" label="Help line under the form" maxLength={300} rows={2} placeholder="None" {...text("loginHelp")} />
        <Columns>
          <TextField id="branding-help-link-label" label="Link after the help line" maxLength={40} placeholder="None" {...text("helpLinkLabel")} />
          <TextField id="branding-help-link-url" label="Its address" type="url" maxLength={2048} placeholder="https://" spellCheck={false} autoComplete="off" {...text("helpLinkUrl")} />
        </Columns>
        <fieldset className="grid gap-2">
          <legend className="mb-1.5 text-sm font-medium">Links in the footer</legend>
          {form.links.map((link, index) => (
            <div key={index} className="flex items-center gap-2">
              <Input aria-label={`Link ${index + 1} text`} maxLength={40} placeholder="Text" value={link.label} onChange={(e) => setLink(index, { label: e.target.value })} className="w-48 shrink-0 max-[560px]:w-28" />
              <Input aria-label={`Link ${index + 1} address`} type="url" maxLength={2048} placeholder="https://" spellCheck={false} autoComplete="off" value={link.url} onChange={(e) => setLink(index, { url: e.target.value })} className="min-w-0 flex-1" />
              <Button type="button" variant="ghost" size="icon-sm" aria-label={`Remove link ${index + 1}`} onClick={() => change({ links: form.links.filter((_, i) => i !== index) })}>
                <Trash2 />
              </Button>
            </div>
          ))}
          {!form.links.length && <Note>No links.</Note>}
          {form.links.length < MAX_LINKS && (
            <div>
              <Button type="button" variant="outline" size="sm" onClick={() => change({ links: [...form.links, { label: "", url: "" }] })}>
                <Plus /> Add a link
              </Button>
            </div>
          )}
        </fieldset>
      </Section>

      <Section title="Icons" description="Small built-in icons beside the greeting, on the model’s replies and while a reply loads.">
        <div className="grid gap-4 sm:grid-cols-3">
          <SelectField id="branding-icon-greeting" label="Greeting" value={form.iconGreeting} onChange={(e) => change({ iconGreeting: e.target.value })} options={icons} />
          <SelectField id="branding-icon-assistant" label="Replies" value={form.iconAssistant} onChange={(e) => change({ iconAssistant: e.target.value })} options={icons} />
          <SelectField id="branding-icon-loading" label="Loading" value={form.iconLoading} onChange={(e) => change({ iconLoading: e.target.value })} options={icons} />
        </div>
      </Section>

      <div className="sticky bottom-0 z-10 -mx-1 flex flex-wrap items-center gap-3 border-t bg-background/95 px-1 py-3 backdrop-blur">
        <FormError message={error} />
        <FormActions className="ml-auto">
          <Button type="button" variant="outline" disabled={busy} title="Fills the form with what the branding file says. Nothing changes until you save; uploaded images stay until removed." onClick={() => setForm(formOf({ siteName: null, accent: null, defaultTheme: null, personalAccent: null, corners: null }, file.branding))}>
            Set the form as installed
          </Button>
          <Button type="submit" disabled={busy}>
            Save branding
          </Button>
        </FormActions>
      </div>
    </form>
  );
}

export function AppearanceTab() {
  const queries = useQueryClient();
  const query = useQuery({ queryKey: KEY, queryFn: () => api.request<AppearanceSettings>("/api/admin/appearance") });
  // What the installation's own file says: what "as installed" means, and what the words are compared with.
  const file = useMemo(() => {
    const config = typeof window === "undefined" ? undefined : window.OblethChatConfig;
    return { config, branding: normalizeBranding(config, BRAND_ICON_NAMES) };
  }, []);
  if (query.isLoading) return <Loading />;
  if (query.error || !query.data) return <ErrorBox error={query.error || "The branding could not be read."} onRetry={() => void query.refetch()} />;
  const onSaved = (next: AppearanceSettings) => {
    queries.setQueryData(KEY, next);
    // This page takes it now; everyone else gets it with their next session answer.
    siteAppearance.set(shownOf(next));
  };
  // The key remounts the form on what was saved, so its fields show the server's word.
  return <Editor key={query.data.updatedAt || "new"} saved={query.data} file={file} onSaved={onSaved} />;
}
