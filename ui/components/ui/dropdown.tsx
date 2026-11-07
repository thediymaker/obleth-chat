"use client";

// A dropdown drawn by the app, in the theme, in place of the browser's own <select>
// list. It takes the same props and <option> children as a <select>, calls onChange
// with { target: { value } }, and with a name it sends its value with the form, so
// forms read with FormData keep working.

import * as React from "react";
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue } from "@/components/ui/select";
import { cn } from "@/lib/utils";

/** Radix reserves "" for "no value", so an option whose value is "" goes by this. */
const EMPTY = "\u0000";
const toItem = (value: string) => (value === "" ? EMPTY : value);
const fromItem = (value: string) => (value === EMPTY ? "" : value);

interface Choice {
  value: string;
  label: React.ReactNode;
  disabled?: boolean;
}
interface Group {
  label: React.ReactNode;
  choices: Choice[];
}

const text = (node: React.ReactNode): string =>
  typeof node === "string" || typeof node === "number" ? String(node) : Array.isArray(node) ? node.map(text).join("") : React.isValidElement(node) ? text((node.props as { children?: React.ReactNode }).children) : "";

/** The <option> and <optgroup> children, through fragments and arrays. */
function read(children: React.ReactNode): (Choice | Group)[] {
  const out: (Choice | Group)[] = [];
  React.Children.forEach(children, (child) => {
    if (!React.isValidElement(child)) return;
    const props = child.props as { value?: unknown; children?: React.ReactNode; disabled?: boolean; label?: React.ReactNode };
    if (child.type === React.Fragment) out.push(...read(props.children));
    else if (child.type === "optgroup") out.push({ label: props.label, choices: read(props.children).filter((c): c is Choice => !("choices" in c)) });
    else if (child.type === "option") out.push({ value: props.value === undefined ? text(props.children) : String(props.value), label: props.children, disabled: props.disabled });
  });
  return out;
}

export interface DropdownChange {
  target: { value: string; name?: string };
  currentTarget: { value: string; name?: string };
}
export type DropdownProps = Omit<React.ComponentProps<"select">, "onChange" | "value" | "defaultValue" | "children"> & {
  value?: string;
  defaultValue?: string;
  onChange?: (event: DropdownChange) => void;
  children?: React.ReactNode;
  placeholder?: string;
};

export function Dropdown({ id, name, value, defaultValue, onChange, children, className, disabled, placeholder, form, ...rest }: DropdownProps) {
  const items = React.useMemo(() => read(children), [children]);
  const flat = items.flatMap((i) => ("choices" in i ? i.choices : [i]));
  const [own, setOwn] = React.useState(() => defaultValue ?? flat[0]?.value ?? "");
  const current = value ?? own;
  const known = flat.some((c) => c.value === current);
  const aria = Object.fromEntries(Object.entries(rest).filter(([k]) => k.startsWith("aria-") || k === "title" || k.startsWith("data-")));
  const item = (c: Choice) => (
    <SelectItem key={c.value} value={toItem(c.value)} disabled={c.disabled}>
      {c.label}
    </SelectItem>
  );
  return (
    <>
      <Select
        value={known ? toItem(current) : undefined}
        disabled={disabled}
        onValueChange={(v) => {
          const next = fromItem(v);
          if (value === undefined) setOwn(next);
          onChange?.({ target: { value: next, name }, currentTarget: { value: next, name } });
        }}
      >
        <SelectTrigger id={id} className={cn("h-9 w-full min-w-0 bg-background text-md data-[size=default]:h-9 dark:bg-input/30", className)} {...aria}>
          <SelectValue placeholder={placeholder ?? (current ? current : "Choose…")} />
        </SelectTrigger>
        <SelectContent position="popper" className="max-h-80">
          {items.map((i, n) =>
            "choices" in i ? (
              <SelectGroup key={n}>
                <SelectLabel>{i.label}</SelectLabel>
                {i.choices.map(item)}
              </SelectGroup>
            ) : (
              item(i)
            ),
          )}
        </SelectContent>
      </Select>
      {name && !disabled && <input type="hidden" name={name} value={current} form={form} />}
    </>
  );
}
