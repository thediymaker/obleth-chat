"use client";

// Generation settings as personal defaults, presets and model policies show them.

import { Columns, SelectField, TextAreaField, TextField } from "@/components/manage/kit";
import type { Model } from "@/lib/queries";

export const generationKeys = ["systemPrompt", "temperature", "topP", "maxTokens", "reasoningEffort"] as const;
export type GenerationKey = (typeof generationKeys)[number];
export interface Generation {
  systemPrompt?: string;
  temperature?: number;
  topP?: number;
  maxTokens?: number;
  reasoningEffort?: string;
}
export const EFFORTS = ["none", "low", "medium", "high"] as const;
const fallback: Required<Generation> = { systemPrompt: "", temperature: 0.7, topP: 1, maxTokens: 0, reasoningEffort: "medium" };

export interface PolicyModel extends Model {
  defaults?: Generation;
  lockedParameters?: string[];
  allowedOverrides?: string[];
}

/** Missing personal keys inherit the published model policy, and locked ones always do (generation.js). */
export function generationValues(model: PolicyModel | undefined, personal: Record<string, unknown> = {}, conversation: Record<string, unknown> = {}): Required<Generation> {
  const defaults = { ...fallback, ...(model?.defaults || {}) } as Record<string, unknown>;
  return Object.fromEntries(
    generationKeys.map((key) => {
      const locked = model?.lockedParameters?.includes(key) || (Array.isArray(model?.allowedOverrides) && !model.allowedOverrides.includes(key));
      return [key, locked ? defaults[key] : (conversation[key] ?? personal[key] ?? defaults[key])];
    }),
  ) as Required<Generation>;
}

/** "medium effort · temperature 0.7 · 4,096 output tokens". */
export const describeGeneration = (p: Generation) =>
  [p.reasoningEffort && `${p.reasoningEffort} effort`, p.temperature != null && `temperature ${p.temperature}`, p.maxTokens && `${p.maxTokens.toLocaleString()} output tokens`].filter(Boolean).join(" · ");

/** The form's generation fields under `prefix`, read back by readGeneration. */
export function GenerationFields({ values, prefix }: { values: Generation; prefix: string }) {
  return (
    <>
      <TextAreaField id={`${prefix}-systemPrompt`} label="System instructions" rows={3} maxLength={8192} defaultValue={values.systemPrompt || ""} />
      <Columns>
        <SelectField id={`${prefix}-reasoningEffort`} label="Thinking effort" options={EFFORTS.map((v) => [v, v] as const)} defaultValue={values.reasoningEffort || "medium"} />
        <TextField id={`${prefix}-temperature`} label="Temperature" type="number" min={0} max={2} step={0.1} required defaultValue={values.temperature ?? 0.7} />
        <TextField id={`${prefix}-topP`} label="Top P" type="number" min={0.01} max={1} step={0.01} required defaultValue={values.topP ?? 1} />
        <TextField id={`${prefix}-maxTokens`} label="Output token limit" type="number" min={0} max={262144} step={1} placeholder="The model’s own limit" defaultValue={values.maxTokens || ""} />
      </Columns>
    </>
  );
}

export function readGeneration(form: FormData, prefix: string): Required<Generation> {
  const get = (key: string) => String(form.get(`${prefix}-${key}`) ?? "");
  return {
    systemPrompt: get("systemPrompt"),
    reasoningEffort: get("reasoningEffort"),
    temperature: Number(get("temperature")),
    topP: Number(get("topP")),
    maxTokens: Number(get("maxTokens")),
  };
}

/** Model options for a select, Auto included unless left out. */
export const modelOptions = (models: Model[], includeAuto = true) =>
  models.filter((m) => includeAuto || m.id !== "auto").map((m) => [m.id, m.name || m.id] as const);
