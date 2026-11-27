"use client";

// Administration › System › Comparing answers: when Chat also asks the models Auto
// ranked next, so the person can read the answers side by side and keep one.
// Endpoint in api/src/compare.rs.

import { toast } from "sonner";
import { CheckField, FormActions, FormError, Note, Pill, Section, SelectField, TextField, useSubmit } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/api";

export interface CompareSettings {
  closeCall: boolean;
  closeGap: number;
  hard: boolean;
  sampleEvery: number;
  dailyLimit: number;
  updatedAt?: string;
  error?: string;
}

const GAPS: [string, string][] = [
  ["0.02", "Almost a tie"],
  ["0.05", "Close"],
  ["0.1", "Fairly close"],
  ["0.2", "Anywhere near"],
];
const SHARES: [string, string][] = [
  ["0", "Never"],
  ["50", "1 reply in 50"],
  ["20", "1 reply in 20"],
  ["10", "1 reply in 10"],
  ["5", "1 reply in 5"],
];
/** The choices of a list, with the saved value added when it is none of them. */
const withSaved = (options: [string, string][], saved: number, name: (n: number) => string) => (options.some(([value]) => Number(value) === saved) ? options : [...options, [String(saved), name(saved)] as [string, string]]);

/** What the form says, as the API takes it. */
export function compareBody(form: FormData): Omit<CompareSettings, "updatedAt" | "error"> {
  return {
    closeCall: form.get("compare-close") === "on",
    closeGap: Number(form.get("compare-gap")),
    hard: form.get("compare-hard") === "on",
    sampleEvery: Number(form.get("compare-sample")),
    dailyLimit: Number(form.get("compare-daily")),
  };
}

export const compareOn = (c: Pick<CompareSettings, "closeCall" | "hard" | "sampleEvery">) => c.closeCall || c.hard || c.sampleEvery > 0;

export function CompareCard({ compare: c, onSaved }: { compare: CompareSettings; onSaved: (c: CompareSettings) => void }) {
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    onSaved(await api.request<CompareSettings>("/api/admin/compare", { method: "PATCH", body: compareBody(form) }));
    toast("Comparing answers saved");
  });
  return (
    <Section
      title="Comparing answers"
      description="Now and then, Chat also puts a message Auto answered to the two models Auto ranked next. They answer in the background, and the person can read all three side by side and keep the one the chat goes on from."
      actions={<Pill tone={compareOn(c) ? "accent" : ""}>{compareOn(c) ? "On" : "Off"}</Pill>}
    >
      <form onSubmit={onSubmit} className="grid gap-4">
        <CheckField id="compare-close" label="When Auto’s pick was a close call" defaultChecked={c.closeCall} description="Its second choice scored nearly as well as its first." />
        <SelectField id="compare-gap" label="How close counts" defaultValue={String(c.closeGap)} options={withSaved(GAPS, c.closeGap, (n) => `Within ${n}`)} />
        <CheckField id="compare-hard" label="When the request is hard" defaultChecked={c.hard} description="The gateway read the message as hard, which is where models differ most." />
        <SelectField id="compare-sample" label="By chance" defaultValue={String(c.sampleEvery)} options={withSaved(SHARES, c.sampleEvery, (n) => `1 reply in ${n}`)} />
        <TextField
          id="compare-daily"
          label="Most in a day, for one person"
          type="number"
          min={1}
          max={100}
          required
          defaultValue={c.dailyLimit}
          help="Each comparison asks up to two more models, so it costs up to two extra answers. They count toward the person’s daily limits like any other."
        />
        <Note>Only replies Auto chose that needed no tools are compared, and never in a temporary chat. The gateway has to name the models it ranked next; older gateways do not, and then nothing is compared.</Note>
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
