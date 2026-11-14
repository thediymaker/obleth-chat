"use client";

// Building blocks shared by Settings, Administration, MCP servers and Groups: cards,
// form fields, pills, filter chips, stat tiles, meters, and confirmations. Forms are
// uncontrolled and read with FormData.

import { AlertCircle, Copy } from "lucide-react";
import { useCallback, useRef, useState } from "react";
import { toast } from "sonner";
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
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Dropdown, type DropdownProps } from "@/components/ui/dropdown";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";

/** "Admin" → "Admin", "sign_in" → "Sign In", as titleCase did. */
export const titleCase = (value: string | null | undefined) => String(value || "").replace(/(^|[-_ ])\w/g, (c) => c.replace(/[-_]/g, " ").toUpperCase());

/** A person's name: display name, else username. */
export const displayName = (user: { displayName?: string | null; display_name?: string | null; username?: string | null } | null | undefined, fallback = "Unknown account") =>
  user?.displayName || user?.display_name || user?.username || fallback;

export type Tone = "" | "accent" | "amber" | "danger";

/** A small status label. */
export function Pill({ children, tone = "", className, title }: { children: React.ReactNode; tone?: Tone; className?: string; title?: string }) {
  return (
    <span
      title={title}
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

/** The server's message for a form that failed. */
export function FormError({ message }: { message: string }) {
  if (!message) return null;
  return (
    <div role="alert" className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive-foreground">
      <AlertCircle className="mt-0.5 size-4 shrink-0" />
      <span>{message}</span>
    </div>
  );
}

/** The buttons at the end of a form. */
export function FormActions({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={cn("flex flex-wrap items-center justify-end gap-2", className)}>{children}</div>;
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
        if ((e as Error).name !== "AbortError") setError((e as Error).message);
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
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: React.ReactNode;
  description?: React.ReactNode;
  children: React.ReactNode;
  wide?: boolean;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={cn("max-h-[90dvh] overflow-y-auto", wide && "sm:max-w-2xl")} {...(description ? {} : { "aria-describedby": undefined })}>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          {description && <DialogDescription>{description}</DialogDescription>}
        </DialogHeader>
        {children}
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

/** A confirmation before something that can't be undone. `ask` opens it; the element renders it. */
export function useConfirm() {
  const [pending, setPending] = useState<Pending | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const current = useRef<Pending | null>(null);
  current.current = pending;
  const ask = useCallback((p: Pending) => {
    setError("");
    setPending(p);
  }, []);
  const element = (
    <AlertDialog
      open={!!pending}
      onOpenChange={(open) => {
        if (!open && !busy) setPending(null);
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{pending?.title}</AlertDialogTitle>
          <AlertDialogDescription>{pending?.text}</AlertDialogDescription>
        </AlertDialogHeader>
        <FormError message={error} />
        <AlertDialogFooter>
          <AlertDialogCancel disabled={busy}>{pending?.cancel || "Cancel"}</AlertDialogCancel>
          <AlertDialogAction
            disabled={busy}
            variant={pending?.tone !== "default" ? "destructive" : "default"}
            onClick={async (event) => {
              event.preventDefault();
              const p = current.current;
              if (!p) return;
              setBusy(true);
              setError("");
              try {
                await p.action();
                setPending(null);
              } catch (e) {
                setError((e as Error).message);
              } finally {
                setBusy(false);
              }
            }}
          >
            {pending?.label}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
  return { ask, element, clear: () => setPending(null) };
}

/** Filter chips such as a period: one is on at a time. */
export function Chips<T extends string | number>({
  options,
  value,
  onChange,
  label,
  className,
}: {
  options: readonly (readonly [T, string])[];
  value: T;
  onChange: (value: T) => void;
  label: string;
  className?: string;
}) {
  return (
    <div role="group" aria-label={label} className={cn("inline-flex flex-wrap items-center gap-1", className)}>
      {options.map(([id, name]) => (
        <button
          key={String(id)}
          type="button"
          aria-pressed={id === value}
          onClick={() => onChange(id)}
          className={cn(
            "h-7 rounded-full border px-3 text-sm text-muted-foreground transition-colors hover:text-foreground",
            id === value && "border-line-strong bg-secondary text-foreground",
          )}
        >
          {name}
        </button>
      ))}
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

/** A quota: full at the limit, amber from 80%. */
export function Quota({ value, max, label, className }: { value: number; max: number; label: string; className?: string }) {
  const f = max > 0 ? value / max : 0;
  return <Meter value={Math.min(value, max)} max={Math.max(1, max)} label={label} tone={f >= 1 ? "danger" : f >= 0.8 ? "amber" : "blue"} className={className} level={f >= 1 ? "full" : f >= 0.8 ? "near" : ""} />;
}

/** Copies a value, or selects it when the clipboard is blocked. */
export async function copyText(value: string, input?: HTMLInputElement | HTMLTextAreaElement | null, done = "Copied") {
  try {
    await navigator.clipboard.writeText(value);
    toast(done);
  } catch {
    input?.select();
    toast("Press Ctrl+C or ⌘C to copy");
  }
}

/** A read-only value with a copy button. */
export function CopyField({ id, label, value, rows, help, copyLabel = "Copy", done }: { id: string; label: string; value: string; rows?: number; help?: React.ReactNode; copyLabel?: string; done?: string }) {
  const ref = useRef<HTMLInputElement & HTMLTextAreaElement>(null);
  return (
    <Field id={id} label={label} help={help}>
      <div className="flex items-start gap-2">
        {rows ? (
          <Textarea ref={ref} id={id} readOnly rows={rows} value={value} className="font-mono text-sm" onFocus={(e) => e.currentTarget.select()} />
        ) : (
          <Input ref={ref} id={id} readOnly value={value} className="font-mono" onFocus={(e) => e.currentTarget.select()} />
        )}
        <Button type="button" variant="outline" size="sm" className="mt-0.5" onClick={() => copyText(value, ref.current, done)}>
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
