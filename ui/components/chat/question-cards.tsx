"use client";

// Cards at the end of a reply that ask the person something: the
// model's questions with a few choices each (ask_user), its offers to turn on a
// workspace (request_workspace), to set the chat up for research (suggest_research),
// to make an app (suggest_app) and to start a project for the chat (suggest_project).
// Only the newest reply's card can be answered.

import { Box, Check, CircleHelp, FlaskConical, Folder, Terminal } from "lucide-react";
import Link from "next/link";
import { useEffect, useRef, useState } from "react";
import { researchAllowed, startResearch } from "@/components/research/start";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useChatProject, useChatProjectId } from "@/components/projects/chat-project";
import { NewProjectDialog } from "@/components/projects/project-dialog";
import type { ChatStore } from "@/lib/chat/store";
import type { ActivityEntry, Message } from "@/lib/chat/types";
import { cn } from "@/lib/utils";
import { useChatStore as useChat } from "./chat-provider";
import { APP_STEP_TYPES, appsAllowed, startApp } from "./offers";

interface Question {
  header: string;
  question: string;
  multiple?: boolean;
  options: { label: string; description?: string; recommended?: boolean }[];
}
interface Form {
  step: number;
  picks: Set<number>[];
  other: string[];
  skipped: boolean;
}
// The state of each card, per store (a session), by message.
const states = new WeakMap<ChatStore, { forms: Map<string, Form>; offers: Map<string, string>; made: Map<string, string> }>();
function stateOf(store: ChatStore) {
  let s = states.get(store);
  if (!s) states.set(store, (s = { forms: new Map(), offers: new Map(), made: new Map() }));
  return s;
}
const activityOf = (m?: Message) => (Array.isArray(m?.activity) ? m!.activity : []);
export const questionEntry = (m?: Message) => activityOf(m).find((e) => e.kind === "question" && Array.isArray(e.questions) && e.questions.length) as (ActivityEntry & { questions: Question[] }) | undefined;
const offerOf = (m?: Message) => activityOf(m).find((e) => e.kind === "workspace");
const suggestionOf = (m?: Message) => activityOf(m).find((e) => e.kind === "research");
const appOf = (m?: Message) => activityOf(m).find((e) => e.kind === "app");
const projectOf = (m?: Message) => activityOf(m).find((e) => e.kind === "project");
const keyOf = (m: Message, index: number) => m.id || `message-${index}`;
const answered = (form: Form, i: number) => form.picks[i]!.size > 0 || !!form.other[i]!.trim();

/** The answers as the person's next message. */
export function answersText(questions: Question[], form: Pick<Form, "picks" | "other">) {
  return questions
    .map((q, i) => {
      const picked = q.options.filter((_, n) => form.picks[i]!.has(n)).map((o) => o.label);
      const other = form.other[i]!.trim();
      const answer = [...picked, ...(other ? [other] : [])].join(", ") || "No preference";
      return `**${q.question}**\n${answer}`;
    })
    .join("\n\n");
}

function Closed({ icon: Icon, children, className }: { icon: typeof Terminal; children: React.ReactNode; className?: string }) {
  return (
    <div className={cn("mt-3 flex gap-2.5 rounded-lg border px-3 py-2.5 text-md", className)}>
      <Icon className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
      <div className="grid min-w-0 flex-1 gap-1">{children}</div>
    </div>
  );
}
function Offer({ icon: Icon, label, title, children, busy, busyLabel, acceptLabel, onAccept, onDismiss }: { icon: typeof Terminal; label: string; title: React.ReactNode; children: React.ReactNode; busy: boolean; busyLabel: string; acceptLabel: string; onAccept: () => void; onDismiss: () => void }) {
  return (
    <section className="mt-3 flex flex-wrap gap-3 rounded-lg border bg-card p-3" aria-label={label}>
      <span className="flex size-8 shrink-0 items-center justify-center rounded-md bg-secondary">
        <Icon className="size-4" />
      </span>
      <div className="min-w-0 flex-1 basis-56">
        <strong className="text-md font-semibold">{title}</strong>
        <div className="grid gap-1 text-md text-text-2">{children}</div>
      </div>
      <div className="flex items-start gap-1.5 self-end">
        <Button size="sm" variant="ghost" disabled={busy} onClick={onDismiss}>
          Not now
        </Button>
        <Button size="sm" disabled={busy} onClick={onAccept}>
          {busy ? busyLabel : acceptLabel}
        </Button>
      </div>
    </section>
  );
}

export function QuestionCard({ m, index }: { m: Message; index: number }) {
  const store = useChat();
  const entry = questionEntry(m);
  const { forms } = stateOf(store);
  const [, redraw] = useState(0);
  const optionsRef = useRef<HTMLDivElement>(null);
  const focusAfter = useRef<string | null>(null);
  useEffect(() => {
    if (!focusAfter.current) return;
    const target = focusAfter.current === "submit" ? optionsRef.current?.parentElement?.querySelector<HTMLElement>("[data-submit]") : optionsRef.current?.querySelector<HTMLElement>(focusAfter.current);
    target?.focus();
    focusAfter.current = null;
  });
  if (!entry) return null;
  const key = keyOf(m, index);
  const isLast = index === (store.active?.messages?.length || 0) - 1;
  const open = isLast && m.status === "completed" && !store.streaming && !forms.get(key)?.skipped;
  if (!open)
    return (
      <Closed icon={CircleHelp}>
        {entry.questions.map((q, i) => (
          <p key={i}>
            <strong className="font-medium">{q.header}</strong> {q.question}
          </p>
        ))}
        {isLast && forms.get(key)?.skipped && <p className="text-faint">Skipped. Answer in your own words below.</p>}
      </Closed>
    );
  const questions = entry.questions;
  if (!forms.has(key)) forms.set(key, { step: 0, picks: questions.map(() => new Set()), other: questions.map(() => ""), skipped: false });
  const form = forms.get(key)!;
  const step = Math.min(form.step, questions.length - 1),
    q = questions[step]!;
  const update = (focus: string | null = '[aria-checked="true"], [role="radio"], [role="checkbox"]') => {
    focusAfter.current = focus;
    redraw((n) => n + 1);
  };
  const pick = (n: number) => {
    if (q.multiple) {
      if (form.picks[step]!.has(n)) form.picks[step]!.delete(n);
      else form.picks[step]!.add(n);
      update(`[data-option="${n}"]`);
      return;
    }
    form.picks[step] = new Set([n]);
    form.other[step] = "";
    // One choice made: go on to the next question.
    if (step < questions.length - 1) {
      form.step++;
      update();
    } else update("submit");
  };
  const last = step === questions.length - 1,
    any = questions.some((_, i) => answered(form, i));
  const next = () => {
    form.step = Math.min(form.step + 1, questions.length - 1);
    update();
  };
  const submit = () => store.send(answersText(questions, form));
  return (
    <section className="mt-3 grid gap-3 rounded-lg border bg-card p-3" aria-label="Questions from the model">
      <div className="flex items-center justify-between gap-2">
        <strong className="text-md font-semibold">{q.header}</strong>
        {questions.length > 1 && (
          <span className="flex gap-1" aria-label={`Question ${step + 1} of ${questions.length}`}>
            {questions.map((_, i) => (
              <span key={i} className={cn("h-1.5 w-4 rounded-full", i === step ? "bg-foreground" : answered(form, i) ? "bg-success" : "bg-line-strong")} />
            ))}
          </span>
        )}
      </div>
      <p className="text-md">{q.question}</p>
      <div ref={optionsRef} className="grid gap-1.5" role={q.multiple ? "group" : "radiogroup"} aria-label={q.question}>
        {q.options.map((o, i) => {
          const on = form.picks[step]!.has(i);
          return (
            <button
              key={i}
              type="button"
              role={q.multiple ? "checkbox" : "radio"}
              aria-checked={on}
              data-option={i}
              onClick={() => pick(i)}
              className={cn("flex items-start gap-2.5 rounded-md border px-3 py-2 text-left hover:bg-accent", on && "border-foreground bg-secondary")}
            >
              <span className={cn("mt-0.5 flex size-4 shrink-0 items-center justify-center border", q.multiple ? "rounded" : "rounded-full", on && "border-foreground bg-foreground text-background")}>
                {on && <Check className="size-3" />}
              </span>
              <span className="min-w-0 flex-1">
                <strong className="block text-md font-medium">{o.label}</strong>
                {o.description && <small className="block text-sm text-muted-foreground">{o.description}</small>}
              </span>
              {o.recommended && <Badge variant="secondary">Recommended</Badge>}
            </button>
          );
        })}
        <label className="flex items-center gap-2 rounded-md border px-3 py-1.5">
          <span className="text-sm text-muted-foreground">Other</span>
          <input
            type="text"
            className="h-7 min-w-0 flex-1 bg-transparent text-md outline-none"
            value={form.other[step]}
            placeholder="Type your answer"
            maxLength={500}
            aria-label="Another answer"
            onChange={(e) => {
              form.other[step] = e.target.value;
              // Typing another answer replaces a single choice.
              if (!q.multiple && e.target.value.trim() && form.picks[step]!.size) form.picks[step]!.clear();
              update(null);
            }}
            onKeyDown={(e) => {
              // Enter in the Other box moves on, like choosing an option.
              if (e.key !== "Enter") return;
              e.preventDefault();
              if (!last) next();
              else if (any) submit();
            }}
          />
        </label>
      </div>
      <div className="flex flex-wrap items-center gap-1.5">
        <Button
          size="sm"
          variant="ghost"
          onClick={() => {
            form.skipped = true;
            redraw((n) => n + 1);
            document.getElementById("prompt")?.focus();
          }}
        >
          Skip all
        </Button>
        {step > 0 && (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              form.step = Math.max(form.step - 1, 0);
              update();
            }}
          >
            Previous
          </Button>
        )}
        <span className="flex-1" />
        {last ? (
          <Button size="sm" data-submit disabled={!any} onClick={submit}>
            Submit answers
          </Button>
        ) : (
          <Button size="sm" variant="outline" onClick={next}>
            {answered(form, step) ? "Next" : "Skip question"}
          </Button>
        )}
      </div>
    </section>
  );
}

function useOffer(store: ChatStore, key: string): [string | undefined, (value: string | null) => void] {
  const { offers } = stateOf(store);
  const [, redraw] = useState(0);
  return [
    offers.get(key),
    (value) => {
      if (value === null) offers.delete(key);
      else offers.set(key, value);
      redraw((n) => n + 1);
    },
  ];
}

export function WorkspaceOffer({ m, index }: { m: Message; index: number }) {
  const store = useChat();
  const entry = offerOf(m);
  const key = keyOf(m, index);
  const [offer, setOffer] = useOffer(store, key);
  if (!entry) return null;
  const id = store.active?.id;
  const isLast = index === (store.active?.messages?.length || 0) - 1;
  if ((id && store.workspace.has(id)) || offer === "done")
    return (
      <Closed icon={Terminal}>
        <p>
          <strong className="font-medium">Workspace</strong> Turned on for this chat.
        </p>
      </Closed>
    );
  if (!isLast || m.status !== "completed" || store.streaming || offer === "dismissed")
    return (
      <Closed icon={Terminal}>
        <p>
          <strong className="font-medium">Workspace</strong> {entry.reason || "Asked to turn on a workspace."}
        </p>
        {offer === "dismissed" && <p className="text-faint">Not turned on. Use the Workspace button below whenever you want it.</p>}
      </Closed>
    );
  return (
    <Offer
      icon={Terminal}
      label="Turn on a workspace"
      title="This needs a workspace"
      busy={offer === "busy"}
      busyLabel="Starting…"
      acceptLabel="Turn on workspace"
      onDismiss={() => setOffer("dismissed")}
      onAccept={() =>
        store.run(async () => {
          if (!id) return;
          setOffer("busy");
          try {
            await store.workspace.create(id);
          } catch (error) {
            setOffer(null);
            throw error;
          }
          setOffer("done");
          store.send("The workspace is on. Go ahead.");
        })
      }
    >
      <p>{entry.reason || "A project folder for this chat in your own Linux container, where the model can write files, run code and show a live preview."}</p>
    </Offer>
  );
}

export function ResearchOffer({ m, index }: { m: Message; index: number }) {
  const store = useChat();
  const entry = suggestionOf(m);
  const [offer, setOffer] = useOffer(store, keyOf(m, index));
  if (!entry) return null;
  const isLast = index === (store.active?.messages?.length || 0) - 1;
  const goal = (
    <p>
      <strong className="font-medium">Research</strong> {entry.goal || "Suggested research."}
    </p>
  );
  if (offer === "done")
    return (
      <Closed icon={FlaskConical}>
        {goal}
        <p className="text-faint">Started. The chat is setting it up.</p>
      </Closed>
    );
  if (!isLast || m.status !== "completed" || store.streaming || offer === "dismissed" || !researchAllowed(store))
    return (
      <Closed icon={FlaskConical}>
        {goal}
        {offer === "dismissed" && <p className="text-faint">Not started. Ask whenever you want it set up.</p>}
      </Closed>
    );
  return (
    <Offer
      icon={FlaskConical}
      label="Start research"
      title="Worth researching"
      busy={offer === "busy"}
      busyLabel="Setting up…"
      acceptLabel="Start research"
      onDismiss={() => setOffer("dismissed")}
      onAccept={() =>
        store.run(async () => {
          const id = store.active?.id;
          if (!id) return;
          setOffer("busy");
          try {
            await startResearch(store, id, entry);
          } catch (error) {
            setOffer(null);
            throw error;
          }
          setOffer("done");
          store.send(`Let's set this up as research. The goal: ${entry.goal}${entry.metric ? ` Measured by ${entry.metric}.` : ""}`);
        })
      }
    >
      <p>{entry.goal || ""}</p>
      {entry.metric && <p className="text-sm">Measured by {entry.metric}</p>}
      <p className="text-sm text-faint">Tries alternatives on branches in a workspace, {entry.inProject ? "in this project" : "in a new project"}, and keeps the best. Tools run without asking.</p>
    </Offer>
  );
}

export function AppOffer({ m, index }: { m: Message; index: number }) {
  const store = useChat();
  const chatProject = useChatProjectId(store.active?.id ?? null);
  const entry = appOf(m);
  const key = keyOf(m, index);
  const [offer, setOffer] = useOffer(store, key);
  if (!entry) return null;
  const { made } = stateOf(store);
  const project = made.get(key);
  const isLast = index === (store.active?.messages?.length || 0) - 1;
  const name = (
    <p>
      <strong className="font-medium">App</strong> {entry.name || "Suggested app."}
    </p>
  );
  if (project)
    return (
      <Closed icon={Box}>
        {name}
        <p className="text-faint">
          Created.{" "}
          <Link className="underline underline-offset-2" href={`/projects/${encodeURIComponent(project)}/app`}>
            Open its App tab
          </Link>
        </p>
      </Closed>
    );
  if (!isLast || m.status !== "completed" || store.streaming || offer === "dismissed" || !appsAllowed(store))
    return (
      <Closed icon={Box}>
        {name}
        {offer === "dismissed" && <p className="text-faint">Not created. Ask whenever you want it.</p>}
      </Closed>
    );
  const steps: { type: string; title?: string }[] = entry.definition?.steps || [];
  return (
    <Offer
      icon={Box}
      label="Create an app"
      title={entry.name}
      busy={offer === "busy"}
      busyLabel="Creating…"
      acceptLabel="Create app"
      onDismiss={() => setOffer("dismissed")}
      onAccept={() =>
        store.run(async () => {
          const id = store.active?.id;
          if (!id) return;
          setOffer("busy");
          let result: { project: string; collection: string };
          try {
            result = await startApp(store, id, entry, chatProject);
          } catch (error) {
            setOffer(null);
            throw error;
          }
          made.set(key, result.project);
          setOffer("done");
          store.send(`I created the app “${entry.name}”${result.collection ? `, with the collection “${result.collection}” for its documents` : ""}. What should I do next?`);
        })
      }
    >
      {entry.description && <p>{entry.description}</p>}
      {steps.length > 0 && (
        <ol className="list-decimal pl-5 text-sm">
          {steps.map((s, i) => (
            <li key={i}>{s.title || APP_STEP_TYPES[s.type] || s.type}</li>
          ))}
        </ol>
      )}
      <p className="text-sm text-faint">
        Creates the app {entry.inProject ? "in this project" : "in a new project"}
        {entry.documents ? ", with a collection for its documents" : ""}. Try it, change it and publish it from the project’s App tab.
      </p>
    </Offer>
  );
}

export function ProjectOffer({ m, index }: { m: Message; index: number }) {
  const store = useChat();
  const id = store.active?.id ?? null;
  const current = useChatProject(id);
  const entry = projectOf(m);
  const key = keyOf(m, index);
  const [offer, setOffer] = useOffer(store, key);
  const [editing, setEditing] = useState(false);
  if (!entry) return null;
  const { made } = stateOf(store);
  const project = made.get(key);
  const isLast = index === (store.active?.messages?.length || 0) - 1;
  const name = (
    <p>
      <strong className="font-medium">Project</strong> {entry.name || "Suggested project."}
    </p>
  );
  if (project || current)
    return (
      <Closed icon={Folder}>
        {name}
        <p className="text-faint">
          {project ? "Started. " : "This chat is in a project. "}
          <Link className="underline underline-offset-2" href={`/projects/${encodeURIComponent(project || current!.id)}`}>
            Open {project ? "it" : current!.name}
          </Link>
        </p>
      </Closed>
    );
  if (!isLast || m.status !== "completed" || store.streaming || offer === "dismissed" || !id)
    return (
      <Closed icon={Folder}>
        {name}
        {offer === "dismissed" && <p className="text-faint">Not started. Use the project menu above the chat whenever you want it.</p>}
      </Closed>
    );
  return (
    <>
      <Offer
        icon={Folder}
        label="Start a project"
        title={entry.name}
        busy={editing}
        busyLabel="Start project"
        acceptLabel="Start project"
        onDismiss={() => setOffer("dismissed")}
        onAccept={() => setEditing(true)}
      >
        {entry.description && <p>{entry.description}</p>}
        {entry.instructions && <p className="line-clamp-4 whitespace-pre-line text-sm">{entry.instructions}</p>}
        <p className="text-sm text-faint">Moves this chat into a new project. Its chats share the instructions, collections, saved items and code, so a new chat there starts with what this one set up.</p>
      </Offer>
      <NewProjectDialog
        open={editing}
        onOpenChange={setEditing}
        fromChat={id}
        initial={{ name: entry.name, description: entry.description, instructions: entry.instructions }}
        onMoved={(p) => {
          made.set(key, p.id);
          setOffer("done");
          store.send(`I started the project “${p.name}” for this chat. Go ahead.`);
        }}
      />
    </>
  );
}
