"use client";

// The model and its thinking effort in one pill. It opens the model list, searchable,
// with the short list first and See all models, the effort slider, and a way to the
// other parameters.

import { Bookmark, Box, Check, ChevronDown, ChevronRight, Search, Settings } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Slider } from "@/components/ui/slider";
import { EFFORT_DESCRIPTIONS, EFFORTS, effortLabel } from "@/lib/chat/composer";
import { chooserModels } from "@/lib/chat/model-choices";
import { cn } from "@/lib/utils";
import { useChat } from "./chat-provider";

const TINT: Record<string, string> = { busy: "text-amber", full: "text-destructive-foreground", offline: "text-destructive-foreground" };

export function EffortControl() {
  const store = useChat();
  const value = EFFORTS.indexOf(store.reasoningEffort as never);
  const locked = store.isLocked("reasoningEffort");
  return (
    <div className="grid gap-2" data-effort={store.reasoningEffort}>
      <div className="flex items-center justify-between">
        <label id="reasoning-effort-label" className="text-sm font-medium">
          Thinking effort
        </label>
        <span className="text-xs font-medium text-blue" aria-hidden="true">
          {store.reasoningEffort === "high" ? "Warp speed" : ""}
        </span>
      </div>
      <Slider
        id="reasoning-effort"
        aria-labelledby="reasoning-effort-label"
        aria-valuetext={effortLabel(store.reasoningEffort)}
        aria-describedby="effort-description effort-help"
        min={0}
        max={3}
        step={1}
        value={[Math.max(0, value)]}
        disabled={store.streaming || locked}
        onValueChange={([v]) => store.setEffort(EFFORTS[v ?? 0] || "none")}
        onValueCommit={() => store.saveEffort()}
      />
      <div className="flex justify-between text-xs text-muted-foreground" aria-hidden="true">
        {EFFORTS.map((v, i) => (
          <span key={v} className={cn(i === value && "font-medium text-foreground")}>
            {effortLabel(v)}
          </span>
        ))}
      </div>
      <p id="effort-description" className="text-sm text-text-2">
        {EFFORT_DESCRIPTIONS[store.reasoningEffort]}
      </p>
      <p id="effort-help" className="text-xs text-muted-foreground">
        {locked ? "Thinking effort is locked by the model policy." : "Requested effort; model support varies."}
      </p>
    </div>
  );
}

export function ModelPicker() {
  const store = useChat();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [all, setAll] = useState(false);
  const list = useRef<HTMLDivElement>(null);
  const name = store.modelName(store.model),
    level = EFFORTS.indexOf(store.reasoningEffort as never),
    busy = store.load.label(store.model);
  const favorites = store.preferences?.favorites || [];
  const { choices, hidden } = chooserModels(store.models, { query, all, favorites, current: store.model });
  useEffect(() => {
    if (!open) return;
    setQuery("");
    setAll(false);
    store.load.refresh();
  }, [open, store]);
  const options = () => [...(list.current?.querySelectorAll<HTMLElement>("[data-option]") || [])];
  const select = (id: string) => {
    setOpen(false);
    store.selectModel(id);
  };
  return (
    <Popover open={open} onOpenChange={(o) => setOpen(o && !store.streaming)}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-haspopup="dialog"
          aria-label={`Model: ${name}${busy ? `. ${busy}` : ""}. Thinking effort: ${effortLabel(store.reasoningEffort)}`}
          title={`${name}${busy ? ` · ${busy}` : ""} · thinking ${effortLabel(store.reasoningEffort).toLowerCase()}`}
          disabled={!store.available || store.streaming}
          className="inline-flex h-8 max-w-[min(320px,48vw)] min-w-0 shrink items-center gap-[7px] rounded-md border border-line-strong px-2.5 text-sm font-medium text-text-2 transition-colors hover:border-control-border hover:bg-secondary disabled:opacity-50"
        >
          <Box className={cn("size-4 shrink-0", TINT[store.load.tint(store.model)])} />
          <span className="min-w-0 truncate">{name}</span>
          <span className="flex items-end gap-px" aria-hidden="true">
            {[1, 2, 3].map((i) => (
              <i key={i} className={cn("block w-0.5 rounded-full", level >= i ? "bg-foreground" : "bg-line-strong")} style={{ height: 4 + i * 2 }} />
            ))}
          </span>
          <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-[min(22rem,calc(100vw-2rem))] p-0" aria-label="Choose a model" onOpenAutoFocus={(e) => e.preventDefault()}>
        <label className="flex items-center gap-2 border-b px-3 py-2">
          <Search className="size-4 text-muted-foreground" />
          <input
            autoFocus
            aria-label="Search models"
            placeholder="Search models"
            autoComplete="off"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                options()[0]?.click();
              } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                e.preventDefault();
                const all = options();
                all[e.key === "ArrowDown" ? 0 : all.length - 1]?.focus();
              }
            }}
            className="h-7 min-w-0 flex-1 bg-transparent text-md outline-none"
          />
        </label>
        <div
          ref={list}
          role="listbox"
          aria-label="Available models"
          className="max-h-72 overflow-y-auto p-1"
          onKeyDown={(e) => {
            if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
            e.preventDefault();
            const all = options(),
              index = all.indexOf(document.activeElement as HTMLElement);
            all[(index + (e.key === "ArrowDown" ? 1 : -1) + all.length) % all.length]?.focus();
          }}
        >
          {choices.map((m) => {
            const selected = m.id === store.model,
              label = store.load.label(m.id);
            const detail = m.id === "auto" ? "Picks a permitted model for each request" : m.description || m.detail || (m.name && m.name !== m.id ? m.id : "");
            const Icon = favorites.includes(m.id) ? Bookmark : Box;
            return (
              <button
                key={m.id}
                type="button"
                role="option"
                data-option
                aria-selected={selected}
                title={label || undefined}
                onClick={() => select(m.id)}
                className={cn("flex w-full items-start gap-2.5 rounded-md px-2 py-1.5 text-left outline-none hover:bg-accent focus-visible:bg-accent", selected && "bg-secondary")}
              >
                <Icon className={cn("mt-0.5 size-4 shrink-0 text-muted-foreground", TINT[store.load.tint(m.id)])} />
                <span className="min-w-0 flex-1">
                  <strong className="flex flex-wrap items-center gap-1.5 text-md font-medium">
                    {m.name || m.id}
                    {m.isDefault && <Badge variant="secondary">Default</Badge>}
                    {m.metadata?.supportsVision && <Badge variant="secondary">Vision</Badge>}
                    {label && <span className="sr-only">. {label}</span>}
                  </strong>
                  {detail && <small className="block truncate text-xs text-muted-foreground">{detail}</small>}
                </span>
                {selected && <Check className="mt-0.5 size-4 shrink-0" />}
              </button>
            );
          })}
          {hidden > 0 && (
            <button
              type="button"
              data-option
              onClick={() => {
                const at = options().length - 1;
                setAll(true);
                requestAnimationFrame(() => options()[at]?.focus({ preventScroll: true }));
              }}
              className="flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left outline-none hover:bg-accent focus-visible:bg-accent"
            >
              <ChevronDown className="size-4 text-muted-foreground" />
              <span className="flex-1">
                <strong className="block text-md font-medium">See all models</strong>
                <small className="text-xs text-muted-foreground">{hidden} more</small>
              </span>
            </button>
          )}
          {!choices.length && <p className="px-2 py-3 text-sm text-muted-foreground">No matching models.</p>}
        </div>
        <div className="grid gap-2 border-t p-3">
          <EffortControl />
          <button
            type="button"
            onClick={() => {
              setOpen(false);
              store.openDialog({ kind: "parameters" });
            }}
            className="flex items-center gap-2 rounded-md px-1 py-1 text-left text-sm text-muted-foreground hover:bg-accent hover:text-foreground"
          >
            <Settings className="size-4" />
            <span className="flex-1">Temperature, output limit, instructions</span>
            <ChevronRight className="size-4" />
          </button>
        </div>
      </PopoverContent>
    </Popover>
  );
}
