"use client";

// Building blocks every page's cards, forms and dialogs share: cards, form fields,
// pills, filter chips, stat tiles, meters, and confirmations. Forms are uncontrolled
// and read with FormData.
//
// The save rule: an on/off that takes effect at once is a Switch (SwitchField). A card
// with one or two independent controls saves each change as it is made and says so in
// a toast; a card with several fields that belong together has a Save button.
//
// The dialog rule: a FormDialog's FormActions start with Cancel, which closes it; pass
// `cancel={false}` only where the dialog's own button already closes it (Done, Close).

import { Copy, Search } from "lucide-react";
import { createContext, useCallback, useContext, useState } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Dropdown, type DropdownProps } from "@/components/ui/dropdown";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { copyText } from "@/lib/clipboard";
import { initials } from "@/lib/format";
import { cn } from "@/lib/utils";
import { FormError } from "./form-error";

/** The message of whatever a request threw. */
export const messageOf = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** "Admin" → "Admin", "sign_in" → "Sign In", as titleCase did. */
export const titleCase = (value: string | null | undefined) => String(value || "").replace(/(^|[-_ ])\w/g, (c) => c.replace(/[-_]/g, " ").toUpperCase());

/** A person's name: display name, else username. */
export const displayName = (user: { displayName?: string | null; display_name?: string | null; username?: string | null } | null | undefined, fallback = "Unknown account") =>
  user?.displayName || user?.display_name || user?.username || fallback;

export type Tone = "" | "accent" | "amber" | "danger";

/** A small label for a status or a kind: the one such component, in four tones. */
export function Pill({ children, tone = "", className, ...rest }: React.ComponentProps<"span"> & { tone?: Tone }) {
  return (
    <span
      {...rest}
      data-tone={tone || undefined}
      className={cn(
        "inline-flex h-5 shrink-0 items-center rounded-full border px-2 text-2xs font-medium whitespace-nowrap text-muted-foreground",
        tone === "accent" && "border-success/30 bg-success/10 text-success",
        tone === "amber" && "border-amber/40 bg-amber/10 text-amber",
        tone === "danger" && "border-destructive/40 bg-destructive/10 text-destructive-foreground",
        className,
      )}
    >
      {children}
    </span>
  );
}

/** A person's initials in a circle, such as "JD" for Jane Doe. */
export function PersonAvatar({ name, size = "default", className }: { name: string | null | undefined; size?: "sm" | "default"; className?: string }) {
  return (
    <Avatar aria-hidden size={size} className={className}>
      <AvatarFallback className="bg-secondary text-2xs font-semibold text-text-2">{initials(name || "?")}</AvatarFallback>
    </Avatar>
  );
}

/** The earlier name for a Pill. */
export const Badge = Pill;

/** A card with a heading, an optional line under it and actions on the right. */
export function Section({
  title,
  description,
  actions,
  children,
  className,
  headingLevel = 2,
  ...rest
}: {
  title?: React.ReactNode;
  description?: React.ReactNode;
  actions?: React.ReactNode;
  children?: React.ReactNode;
  className?: string;
  headingLevel?: 2 | 3;
} & Omit<React.ComponentProps<"section">, "title">) {
  const Heading = headingLevel === 2 ? "h2" : "h3";
  return (
    <section className={cn("grid min-w-0 content-start gap-4 rounded-lg border bg-card p-4 sm:p-5 [&>*]:min-w-0", className)} {...rest}>
      {(title || actions) && (
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0 flex-1 basis-64">
            {title && <Heading className="text-base font-semibold">{title}</Heading>}
            {description && <div className="mt-1 text-sm text-muted-foreground">{description}</div>}
          </div>
          {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
        </div>
      )}
      {children}
    </section>
  );
}

/** Gray help text under a section or form. */
export function Note({ children, className, ...rest }: React.ComponentProps<"p">) {
  return (
    <p className={cn("text-sm text-muted-foreground", className)} {...rest}>
      {children}
    </p>
  );
}

/** A label, its control and help under it. */
export function Field({ id, label, help, children, className }: { id: string; label: React.ReactNode; help?: React.ReactNode; children: React.ReactNode; className?: string }) {
  return (
    <div className={cn("grid min-w-0 content-start gap-1.5", className)}>
      <Label htmlFor={id}>{label}</Label>
      {children}
      {help && (
        <p id={`${id}-help`} className="text-xs text-muted-foreground">
          {help}
        </p>
      )}
    </div>
  );
}

type InputProps = Omit<React.ComponentProps<typeof Input>, "id" | "name">;
export function TextField({ id, label, help, className, ...input }: InputProps & { id: string; label: React.ReactNode; help?: React.ReactNode }) {
  return (
    <Field id={id} label={label} help={help} className={className}>
      <Input id={id} name={id} aria-describedby={help ? `${id}-help` : undefined} {...input} />
    </Field>
  );
}

type AreaProps = Omit<React.ComponentProps<typeof Textarea>, "id" | "name">;
export function TextAreaField({ id, label, help, className, ...input }: AreaProps & { id: string; label: React.ReactNode; help?: React.ReactNode }) {
  return (
    <Field id={id} label={label} help={help} className={className}>
      <Textarea id={id} name={id} aria-describedby={help ? `${id}-help` : undefined} {...input} />
    </Field>
  );
}

/** A dropdown styled like the inputs, drawn in the theme; it takes <option> children
 * like a <select>, and with a name it sends its value with the form. */
export { Dropdown } from "@/components/ui/dropdown";

export function SelectField({
  id,
  label,
  help,
  options,
  className,
  ...select
}: Omit<DropdownProps, "id" | "name"> & { id: string; label: React.ReactNode; help?: React.ReactNode; options: readonly (readonly [string, string])[] }) {
  return (
    <Field id={id} label={label} help={help} className={className}>
      <Dropdown id={id} name={id} aria-describedby={help ? `${id}-help` : undefined} {...select}>
        {options.map(([value, name]) => (
          <option key={value} value={value}>
            {name}
          </option>
        ))}
      </Dropdown>
    </Field>
  );
}

/** A checkbox with a bold label and a line under it. */
export function CheckField({
  id,
  label,
  description,
  defaultChecked,
  checked,
  onCheckedChange,
  disabled,
  className,
}: {
  id: string;
  label: React.ReactNode;
  description?: React.ReactNode;
  defaultChecked?: boolean;
  checked?: boolean;
  onCheckedChange?: (checked: boolean) => void;
  disabled?: boolean;
  className?: string;
}) {
  return (
    // relative keeps the checkbox's hidden form input with its row, so a scrolling list
    // of these clips it instead of growing the page.
    <div className={cn("relative flex items-start gap-2.5", className)}>
      <Checkbox
        id={id}
        name={id}
        className="mt-0.5"
        defaultChecked={checked === undefined ? defaultChecked : undefined}
        checked={checked}
        onCheckedChange={onCheckedChange ? (value) => onCheckedChange(value === true) : undefined}
        disabled={disabled}
        aria-describedby={description ? `${id}-help` : undefined}
      />
      <div className="grid gap-0.5">
        <Label htmlFor={id} className="leading-snug">
          {label}
        </Label>
        {description && (
          <p id={`${id}-help`} className="text-xs text-muted-foreground">
            {description}
          </p>
        )}
      </div>
    </div>
  );
}

/** An on/off that takes effect at once: a switch with a bold label and a line under it. */
export function SwitchField({
  id,
  label,
  description,
  checked,
  onCheckedChange,
  disabled,
  className,
}: {
  id: string;
  label: React.ReactNode;
  description?: React.ReactNode;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <div className={cn("flex items-start gap-3", className)}>
      <Switch id={id} checked={checked} onCheckedChange={onCheckedChange} disabled={disabled} className="mt-0.5" aria-describedby={description ? `${id}-help` : undefined} />
      <div className="grid gap-0.5">
        <Label htmlFor={id} className="leading-snug">
          {label}
        </Label>
        {description && (
          <p id={`${id}-help`} className="text-xs text-muted-foreground">
            {description}
          </p>
        )}
      </div>
    </div>
  );
}

/** A search box that applies on submit, sending `query`; `children` sit between the
 *  box and the button, such as one more filter or Clear. */
export function SearchForm({
  id,
  label,
  placeholder,
  value,
  onSearch,
  maxLength = 128,
  className,
  children,
}: {
  id: string;
  label: string;
  placeholder: string;
  value: string;
  onSearch: (query: string) => void;
  maxLength?: number;
  className?: string;
  children?: React.ReactNode;
}) {
  return (
    <form
      role="search"
      className={cn("flex min-w-0 grow flex-wrap items-center gap-2 sm:max-w-md", className)}
      onSubmit={(e) => {
        e.preventDefault();
        onSearch(String(new FormData(e.currentTarget).get("query") || "").trim());
      }}
    >
      <label className="sr-only" htmlFor={id}>
        {label}
      </label>
      <div className="relative min-w-0 flex-1 basis-56">
        <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input key={value} id={id} name="query" placeholder={placeholder} maxLength={maxLength} defaultValue={value} className="pl-8" />
      </div>
      {children}
      <Button type="submit" variant="outline">
        Search
      </Button>
    </form>
  );
}

/** One of a few choices, each a card with a bold name and a line under it; picking one
 *  acts at once. `[value, name, note, off]` per choice. */
export function RadioCards({
  label,
  name,
  value,
  onChange,
  options,
  disabled,
}: {
  label: string;
  name: string;
  value: string;
  onChange: (value: string) => void;
  options: readonly (readonly [string, React.ReactNode, React.ReactNode, boolean?])[];
  disabled?: boolean;
}) {
  return (
    <RadioGroup aria-label={label} name={name} value={value} onValueChange={onChange} disabled={disabled} className="gap-2">
      {options.map(([id, title, note, off]) => (
        <Label
          key={id}
          htmlFor={`${name}-${id}`}
          className={cn(
            "flex cursor-pointer items-start gap-2.5 rounded-md border p-2.5 font-normal hover:bg-accent",
            value === id && "border-line-strong bg-secondary",
            (off || disabled) && "cursor-not-allowed opacity-60 hover:bg-transparent",
          )}
        >
          <RadioGroupItem id={`${name}-${id}`} value={id} disabled={off} className="mt-0.5" />
          <span className="grid gap-0.5">
            <strong className="text-md font-medium">{title}</strong>
            <small className="text-xs text-muted-foreground">{note}</small>
          </span>
        </Label>
      ))}
    </RadioGroup>
  );
}

/** Two or more fields side by side when there is room. */
export function Columns({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={cn("grid gap-4 sm:grid-cols-2", className)}>{children}</div>;
}

/** A label and its value on one line. */
export function DetailRow({ label, children, className }: { label: React.ReactNode; children: React.ReactNode; className?: string }) {
  return (
    <div className={cn("flex items-baseline justify-between gap-4 border-b border-line-soft py-2 text-md last:border-b-0", className)}>
      <span className="text-muted-foreground">{label}</span>
      <span className="min-w-0 truncate text-right">{children}</span>
    </div>
  );
}

export { FormError } from "./form-error";

/** Closes the FormDialog around a form, for its Cancel button. */
const DialogClose = createContext<(() => void) | null>(null);

/** The buttons at the end of a form. In a FormDialog they start with Cancel, named by
 *  `cancel`, or left out with `cancel={false}` when another button closes the dialog. */
export function FormActions({ children, className, cancel = "Cancel" }: { children: React.ReactNode; className?: string; cancel?: string | false }) {
  const close = useContext(DialogClose);
  return (
    <div className={cn("flex flex-wrap items-center justify-end gap-2", className)}>
      {close && cancel !== false && (
        <Button type="button" variant="ghost" onClick={close}>
          {cancel}
        </Button>
      )}
      {children}
    </div>
  );
}

/** Busy and error state around a form's submit, with the server's message shown inline. */
export function useSubmit(handler: (data: FormData, form: HTMLFormElement) => Promise<unknown> | unknown) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const onSubmit = useCallback(
    async (event: React.FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const form = event.currentTarget;
      setBusy(true);
      setError("");
      try {
        await handler(new FormData(form), form);
      } catch (e) {
        if ((e as Error).name !== "AbortError") setError(messageOf(e));
      } finally {
        setBusy(false);
      }
    },
    [handler],
  );
  return { busy, error, setError, onSubmit };
}

/** A plain dialog with a title and optional line under it; long ones scroll. */
export function FormDialog({
  open,
  onOpenChange,
  title,
  description,
  children,
  wide = false,
  className,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: React.ReactNode;
  description?: React.ReactNode;
  children: React.ReactNode;
  wide?: boolean;
  className?: string;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={cn("max-h-[90dvh] overflow-y-auto", wide && "sm:max-w-2xl", className)} {...(description ? {} : { "aria-describedby": undefined })}>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          {description && <DialogDescription>{description}</DialogDescription>}
        </DialogHeader>
        <DialogClose.Provider value={() => onOpenChange(false)}>{children}</DialogClose.Provider>
      </DialogContent>
    </Dialog>
  );
}

interface Pending {
  title: string;
  text: string;
  label: string;
  /** The button that backs out, "Cancel" unless the old wording said otherwise. */
  cancel?: string;
  tone?: "danger" | "default";
  action: () => Promise<unknown> | unknown;
}

/**
 * The one rule for destructive actions:
 * - Irreversible (delete, revoke, end a session, forget data): the trigger is a
 *   `DangerButton`, it asks first through `useConfirm`, and a success is toasted.
 * - Reversible (archive, turn off, detach): it acts at once and offers Undo in the toast.
 * - Either way a failure is toasted (or shown in the open dialog), never left as an
 *   unhandled rejection.
 */
export function DangerButton({ className, variant = "outline", size = "sm", ...props }: React.ComponentProps<typeof Button>) {
  return <Button variant={variant} size={size} className={cn("border-destructive/40 text-destructive-foreground hover:bg-destructive/10 hover:text-destructive-foreground", className)} {...props} />;
}

/** A yes/no question before something that can't be undone, open while `open`. The
 *  action may throw: its message shows in the dialog, which stays open. */
export function Confirm({
  open,
  onOpenChange,
  title,
  text,
  label,
  cancel = "Cancel",
  tone = "danger",
  action,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: React.ReactNode;
  text: React.ReactNode;
  label: React.ReactNode;
  cancel?: string;
  tone?: "danger" | "default";
  action: () => Promise<unknown> | unknown;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return (
    <AlertDialog
      open={open}
      onOpenChange={(next) => {
        if (busy) return;
        if (!next) setError("");
        onOpenChange(next);
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{text}</AlertDialogDescription>
        </AlertDialogHeader>
        <FormError message={error} />
        <AlertDialogFooter>
          <AlertDialogCancel disabled={busy}>{cancel}</AlertDialogCancel>
          <AlertDialogAction
            disabled={busy}
            variant={tone === "danger" ? "destructive" : "default"}
            onClick={async (event) => {
              event.preventDefault();
              setBusy(true);
              setError("");
              try {
                await action();
                onOpenChange(false);
              } catch (e) {
                setError(messageOf(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            {label}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

/** A Confirm asked for in code: `ask` opens it; the element renders it. */
export function useConfirm() {
  const [pending, setPending] = useState<Pending | null>(null);
  const ask = useCallback((p: Pending) => setPending(p), []);
  const element = (
    <Confirm
      open={!!pending}
      onOpenChange={(open) => !open && setPending(null)}
      title={pending?.title}
      text={pending?.text}
      label={pending?.label}
      cancel={pending?.cancel}
      tone={pending?.tone}
      action={() => pending?.action()}
    />
  );
  return { ask, element, clear: () => setPending(null) };
}

/** What a chip can show beside its name: an icon before it and a count after it. */
export interface ChipExtra {
  icon?: React.ComponentType<{ className?: string }>;
  count?: number;
}

/** Filter chips such as a period: one is on at a time. */
export function Chips<T extends string | number>({
  options,
  value,
  onChange,
  label,
  className,
}: {
  options: readonly (readonly [T, string] | readonly [T, string, ChipExtra])[];
  value: T;
  onChange: (value: T) => void;
  label: string;
  className?: string;
}) {
  return (
    <div role="group" aria-label={label} className={cn("inline-flex flex-wrap items-center gap-1", className)}>
      {options.map(([id, name, extra]) => {
        const Icon = extra?.icon;
        return (
          <button
            key={String(id)}
            type="button"
            aria-pressed={id === value}
            onClick={() => onChange(id)}
            className={cn(
              "inline-flex h-7 items-center gap-1.5 rounded-full border px-3 text-sm text-muted-foreground transition-colors hover:text-foreground",
              id === value && "border-line-strong bg-secondary text-foreground",
            )}
          >
            {Icon && <Icon className="size-3.5" />}
            {name}
            {extra?.count != null && <span className="text-2xs text-faint tabular-nums">{extra.count}</span>}
          </button>
        );
      })}
    </div>
  );
}

export interface Tile {
  label: string;
  value: React.ReactNode;
  note?: React.ReactNode;
}
/** A row of figures: a label, the number, and a line under it. */
export function Tiles({ tiles, className }: { tiles: Tile[]; className?: string }) {
  return (
    <div className={cn("grid grid-cols-2 gap-px overflow-hidden rounded-lg border bg-border sm:grid-cols-3 lg:grid-cols-6", tiles.length === 4 && "lg:grid-cols-4", tiles.length === 5 && "lg:grid-cols-5", className)}>
      {tiles.map((t) => (
        <div key={t.label} className="grid content-start gap-1 bg-card p-3">
          <span className="text-xs text-muted-foreground">{t.label}</span>
          <strong className="text-xl leading-tight font-semibold tabular-nums">{t.value}</strong>
          <small className="min-h-4 text-xs text-muted-foreground">{t.note || " "}</small>
        </div>
      ))}
    </div>
  );
}

/** A thin bar showing a share of the largest value. */
export function Meter({ value, max, tone = "blue", className, label, level }: { value: number; max: number; tone?: "blue" | "gold" | "green" | "amber" | "danger"; className?: string; label?: string; level?: string }) {
  const width = max > 0 ? Math.max(0, Math.min(100, (100 * value) / max)) : 0;
  return (
    <span
      className={cn("block h-1.5 w-full overflow-hidden rounded-full bg-secondary", className)}
      data-level={level || undefined}
      role={label ? "meter" : undefined}
      aria-label={label}
      aria-valuenow={label ? value : undefined}
      aria-valuemin={label ? 0 : undefined}
      aria-valuemax={label ? max : undefined}
    >
      <span
        className={cn(
          "block h-full rounded-full",
          tone === "blue" && "bg-chart-1",
          tone === "gold" && "bg-chart-3",
          tone === "green" && "bg-chart-2",
          tone === "amber" && "bg-amber",
          tone === "danger" && "bg-destructive",
        )}
        style={{ width: `${width.toFixed(1)}%` }}
      />
    </span>
  );
}

/** A quota: full at the limit, amber from 80%. With a `caption` (such as "2 MB of
 *  10 MB") the label and caption sit on a line above the bar. */
export function Quota({ value, max, label, caption, className }: { value: number; max: number; label: string; caption?: React.ReactNode; className?: string }) {
  const f = max > 0 ? value / max : 0;
  const meter = <Meter value={Math.min(value, max)} max={Math.max(1, max)} label={label} tone={f >= 1 ? "danger" : f >= 0.8 ? "amber" : "blue"} className={caption ? undefined : className} level={f >= 1 ? "full" : f >= 0.8 ? "near" : ""} />;
  if (!caption) return meter;
  return (
    <div className={cn("grid min-w-52 gap-1.5", className)}>
      <div className="flex justify-between gap-4 text-xs text-muted-foreground">
        <span>{label}</span>
        <span>{caption}</span>
      </div>
      {meter}
    </div>
  );
}

/** A read-only value with a copy button. */
export function CopyField({ id, label, value, rows, help, copyLabel = "Copy", done }: { id: string; label: string; value: string; rows?: number; help?: React.ReactNode; copyLabel?: string; done?: string }) {
  return (
    <Field id={id} label={label} help={help}>
      <div className="flex items-start gap-2">
        {rows ? (
          <Textarea id={id} readOnly rows={rows} value={value} className="font-mono text-sm" onFocus={(e) => e.currentTarget.select()} />
        ) : (
          <Input id={id} readOnly value={value} className="font-mono" onFocus={(e) => e.currentTarget.select()} />
        )}
        <Button type="button" variant="outline" size="sm" className="mt-0.5" onClick={() => copyText(value, done)}>
          <Copy /> {copyLabel}
        </Button>
      </div>
    </Field>
  );
}

/** A table that scrolls sideways on a phone instead of the page. */
export function TableWrap({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={cn("relative w-full min-w-0 overflow-x-auto rounded-lg border", className)}>{children}</div>;
}

/** Shared table cell classes, so every admin table reads alike. */
export const th = "h-9 px-3 text-left align-middle text-xs font-medium whitespace-nowrap text-muted-foreground";
export const td = "px-3 py-2.5 align-middle text-md";

/** A column of a DataTable: its heading, and classes for its cells such as
 *  "hidden sm:table-cell" to leave it out on a phone. */
export interface Column {
  head: React.ReactNode;
  className?: string;
}
export interface Row {
  key: React.Key;
  cells: React.ReactNode[];
}

/** A table of rows under headings, scrolling sideways on a phone; with no rows it says
 *  `empty` in one row across the columns. `minWidth` such as "min-w-[640px]" keeps
 *  wide tables readable. */
export function DataTable({ columns, rows, empty = "Nothing here yet.", minWidth, className, label }: { columns: (Column | string)[]; rows: Row[]; empty?: React.ReactNode; minWidth?: string; className?: string; label?: string }) {
  const cols = columns.map((c) => (typeof c === "string" ? { head: c } : c));
  return (
    <TableWrap className={className}>
      <table aria-label={label} className={cn("w-full text-left", minWidth)}>
        <thead className="border-b bg-muted/40">
          <tr>
            {cols.map((c, i) => (
              <th key={i} className={cn(th, c.className)}>
                {c.head}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.length ? (
            rows.map((r) => (
              <tr key={r.key} className="border-b border-line-soft last:border-0">
                {r.cells.map((cell, i) => (
                  <td key={i} className={cn(td, cols[i]?.className)}>
                    {cell}
                  </td>
                ))}
              </tr>
            ))
          ) : (
            <tr>
              <td colSpan={cols.length} className={cn(td, "py-6 text-center text-sm text-muted-foreground")}>
                {empty}
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </TableWrap>
  );
}
