"use client";

// A project's Log tab: the person's days on the left, the chosen day's entry in the
// middle as a timeline with references to its chats, runs and commits, and on the
// right the week's figures and requests that write from the log in a new chat.

import { useQueryClient } from "@tanstack/react-query";
import { Download, Pencil, RefreshCw, ScrollText, Trash2 } from "lucide-react";
import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { EmptyState, ErrorBox, Eyebrow, FeatureOff, isFeatureOff, Loading } from "@/components/app-shell/page";
import { useChat } from "@/components/chat/chat-provider";
import { FormActions, FormDialog, FormError, Note, TextField, messageOf, useConfirm, useSubmit } from "@/components/manage/kit";
import { useNewChatInProject } from "@/components/projects/project-frame";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import {
  addDays,
  addNote,
  browserZone,
  deleteNote,
  editLine,
  editNote,
  exportLog,
  isoDay,
  logKeys,
  logPrompts,
  longDay,
  mondayOf,
  monthOf,
  refHref,
  refreshLog,
  refText,
  saveLogChoice,
  shortDay,
  useLogDay,
  useLogDays,
  useLogWeek,
  writeAgain,
  type DaySummary,
  type LogDay,
  type LogLine,
  type LogNote,
  type LogRef,
} from "./api";

const PAGE = 30;

export function ProjectLog({ projectId }: { projectId: string }) {
  const params = useSearchParams();
  const client = useQueryClient();
  const [limit, setLimit] = useState(PAGE);
  const [exporting, setExporting] = useState(false);
  const list = useLogDays(projectId, limit);
  const synced = useRef(false);
  // The person's days are cut in the time zone their browser is in.
  const savedZone = list.data?.timeZone;
  const loaded = !!list.data;
  useEffect(() => {
    const zone = browserZone();
    if (!loaded || synced.current || !zone || savedZone === zone) return;
    synced.current = true;
    saveLogChoice({ timeZone: zone })
      .then(() => refreshLog(client, projectId))
      .catch(() => {});
  }, [loaded, savedZone, client, projectId]);

  if (list.isPending) return <Loading label="Loading the log…" />;
  if (list.error) return isFeatureOff(list.error) ? <FeatureOff what="The project log" icon={ScrollText} /> : <ErrorBox error={list.error} onRetry={() => list.refetch()} />;
  const days = list.data.items;
  const zone = list.data.timeZone || "UTC";
  const picked = params.get("day");
  const day = picked && /^\d{4}-\d{2}-\d{2}$/.test(picked) ? picked : days[0]?.day || null;

  return (
    <div className="grid gap-4">
      <div className="flex flex-wrap items-center justify-end gap-2">
        <Button variant="outline" size="sm" onClick={() => setExporting(true)} disabled={!days.length}>
          <Download /> Export the log
        </Button>
      </div>
      {days.length && day ? (
        <div className="grid grid-cols-[minmax(0,1fr)] gap-6 lg:grid-cols-[13rem_minmax(0,1fr)] xl:grid-cols-[13rem_minmax(0,1fr)_16rem]">
          <Days days={days} selected={day} more={list.data.more} onMore={() => setLimit((n) => n + PAGE)} zone={zone} />
          <DayEntry key={day} projectId={projectId} day={day} />
          <Aside projectId={projectId} day={day} />
        </div>
      ) : (
        <EmptyState
          icon={ScrollText}
          title="No days in the log yet"
          text="A day is written here once your work in this project goes quiet: what you asked in your chats, what the replies found, your research runs and commits. Days with no work are left out."
        />
      )}
      <ExportDialog projectId={projectId} open={exporting} onOpenChange={setExporting} last={days[0]?.day} />
    </div>
  );
}

/** The days, newest first, under their month. */
function Days({ days, selected, more, onMore, zone }: { days: DaySummary[]; selected: string; more: boolean; onMore: () => void; zone: string }) {
  const path = usePathname();
  return (
    <aside aria-label="Days" className="grid min-w-0 content-start gap-1">
      <nav aria-label="Days with work" className="-mx-1 flex gap-1 overflow-x-auto px-1 pb-1 lg:flex-col lg:overflow-visible">
        {days.map((d, i) => {
          const month = monthOf(d.day);
          const first = i === 0 || monthOf(days[i - 1]!.day) !== month;
          const on = d.day === selected;
          return (
            <div key={d.day} className="contents">
              {first && <Eyebrow className="hidden px-2 pt-2 pb-1 lg:block">{month}</Eyebrow>}
              <Link
                href={`${path}?day=${d.day}`}
                scroll={false}
                aria-current={on ? "page" : undefined}
                className={cn(
                  "flex min-h-10 max-w-60 shrink-0 items-center gap-2.5 rounded-md px-2 text-sm lg:max-w-none",
                  on ? "bg-raised text-foreground" : "text-muted-foreground hover:bg-accent hover:text-foreground",
                )}
              >
                <span className="w-12 shrink-0 font-mono text-xs">{shortDay(d.day)}</span>
                {d.state === "writing" && <Alive />}
                <span className="min-w-0 grow truncate">{d.headline || (d.state === "writing" ? "Being written" : "Not written yet")}</span>
              </Link>
            </div>
          );
        })}
      </nav>
      {more && (
        <Button variant="ghost" size="xs" className="justify-start" onClick={onMore}>
          Show older days
        </Button>
      )}
      <Note className="hidden px-2 pt-2 text-xs lg:block">A day is written when its work goes quiet. Days with no work are left out. Times are in {zone}.</Note>
    </aside>
  );
}

/** The mark of something working now: the only use of the accent. */
function Alive() {
  return (
    <span className="relative inline-flex size-1.5 shrink-0" aria-hidden="true">
      <span className="signal-pulse absolute inset-0 rounded-full bg-signal" />
      <span className="absolute inset-0 rounded-full bg-signal" />
    </span>
  );
}

type Row = { type: "line"; line: LogLine; time: string | null } | { type: "note"; note: LogNote; time: string | null };

/** The day's lines and the person's notes in the order of their times; notes added on a later day come last. */
function timeline(d: LogDay): Row[] {
  const rows: Row[] = [...d.lines.map((line) => ({ type: "line" as const, line, time: line.time })), ...d.notes.map((note) => ({ type: "note" as const, note, time: note.sameDay ? note.time : null }))];
  return rows.map((r, i) => [r, i] as const).sort(([a, i], [b, j]) => (a.time ?? "99:99").localeCompare(b.time ?? "99:99") || i - j).map(([r]) => r);
}

function DayEntry({ projectId, day }: { projectId: string; day: string }) {
  const client = useQueryClient();
  const q = useLogDay(projectId, day);
  const confirm = useConfirm();
  const [asking, setAsking] = useState(false);
  const show = (d: LogDay) => {
    client.setQueryData(logKeys.day(projectId, day), d);
    client.invalidateQueries({ queryKey: [...logKeys.all(projectId), "days"] });
    client.invalidateQueries({ queryKey: [...logKeys.all(projectId), "week"] });
  };
  if (q.isPending) return <Loading label="Loading the day…" />;
  if (q.error) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />;
  const d = q.data;
  const writing = d.state === "writing";
  const rows = timeline(d);

  async function again() {
    setAsking(true);
    try {
      show((await writeAgain(projectId, day)).day);
      toast("Writing this day again");
    } catch (e) {
      toast(messageOf(e));
    } finally {
      setAsking(false);
    }
  }

  return (
    <section aria-label={`${longDay(day)}’s entry`} className="grid min-w-0 content-start gap-6">
      {confirm.element}
      <header className="grid gap-1.5">
        <span className="font-mono text-xs text-muted-foreground">{longDay(day)}</span>
        <h2 className="text-2xl leading-tight font-semibold tracking-[-0.4px] text-pretty">{d.headline || (writing ? "Writing this day…" : "Not written yet")}</h2>
        <p className="text-sm text-muted-foreground">Written from your chats, runs and commits in this project. Every line links to where it came from, and you can edit any of it.</p>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2 pt-1">
          {writing ? (
            <span className="flex items-center gap-2 text-sm" role="status">
              <Alive /> Writing this day…
            </span>
          ) : (
            <Button variant="outline" size="sm" onClick={again} disabled={asking}>
              <RefreshCw /> Write this day again
            </Button>
          )}
          {!writing && d.state === "waiting" && <span className="text-sm text-muted-foreground">Written once its work has been quiet for half an hour, or when the day ends.</span>}
          {!writing && d.newer && d.state === "written" && <span className="text-sm text-muted-foreground">Newer work goes in when it is quiet.</span>}
          {!writing && d.failed && <span className="text-sm text-muted-foreground">The last try to write it did not work; it is tried again soon.</span>}
        </div>
      </header>

      {rows.length > 0 && (
        <ol className="grid">
          {rows.map((r, i) =>
            r.type === "line" ? (
              <LineRow key={r.line.id} projectId={projectId} line={r.line} last={i === rows.length - 1} onSaved={(text) => editLine(projectId, day, r.line.id, text).then((x) => show(x.day))} />
            ) : (
              <NoteRow
                key={r.note.id}
                note={r.note}
                last={i === rows.length - 1}
                onSaved={(text) => editNote(projectId, day, r.note.id, text).then((x) => show(x.day))}
                onDelete={() =>
                  confirm.ask({
                    title: "Delete this note?",
                    text: "Your note is removed from the day. The rest of the entry stays.",
                    label: "Delete note",
                    action: async () => {
                      await deleteNote(projectId, day, r.note.id);
                      await refreshLog(client, projectId);
                      toast("Note deleted");
                    },
                  })
                }
              />
            ),
          )}
        </ol>
      )}

      {d.open.length > 0 && (
        <section className="grid gap-2 rounded-lg border bg-card p-4">
          <h3>
            <Eyebrow>Still open at the end of the day</Eyebrow>
          </h3>
          <ul className="grid gap-2 pl-4 text-md text-text-2 list-disc">
            {d.open.map((l) => (
              <li key={l.id}>
                <span>{l.text}</span> <Refs projectId={projectId} refs={l.refs} inline />
              </li>
            ))}
          </ul>
        </section>
      )}

      <NoteForm onAdd={(text) => addNote(projectId, day, text).then((x) => show(x.day))} />
    </section>
  );
}

function Rail({ time, strong, last, children }: { time: string | null; strong: boolean; last: boolean; children: React.ReactNode }) {
  return (
    <li className="flex gap-3 sm:gap-4">
      <span className="w-11 shrink-0 pt-0.5 font-mono text-xs text-muted-foreground">{time ?? ""}</span>
      <span className="flex w-3 shrink-0 flex-col items-center" aria-hidden="true">
        <span className={cn("mt-1.5 size-2 rounded-full border", strong ? "border-foreground bg-foreground" : "border-line-strong")} />
        {!last && <span className="w-px grow bg-border" />}
      </span>
      <div className="grid min-w-0 grow content-start gap-2 pb-5">{children}</div>
    </li>
  );
}

function Label({ children }: { children: React.ReactNode }) {
  return <span className="text-2xs font-medium tracking-wider text-muted-foreground uppercase">{children}</span>;
}

function LineRow({ projectId, line, last, onSaved }: { projectId: string; line: LogLine; last: boolean; onSaved: (text: string) => Promise<unknown> }) {
  const [editing, setEditing] = useState(false);
  return (
    <Rail time={line.time} strong={line.kind === "decision"} last={last}>
      {(line.kind === "decision" || line.edited) && (
        <span className="flex gap-3">
          {line.kind === "decision" && <Label>Decision</Label>}
          {line.edited && <Label>Edited by you</Label>}
        </span>
      )}
      {editing ? (
        <EditText label="Line" initial={line.text} max={600} onCancel={() => setEditing(false)} onSave={(text) => onSaved(text).then(() => setEditing(false))} />
      ) : (
        <div className="group flex items-start gap-2">
          <p className="grow text-base leading-relaxed text-pretty">{line.text}</p>
          <Button variant="ghost" size="icon-xs" className="shrink-0 text-faint group-hover:text-muted-foreground focus-visible:text-foreground" aria-label="Edit this line" title="Edit this line" onClick={() => setEditing(true)}>
            <Pencil />
          </Button>
        </div>
      )}
      <Refs projectId={projectId} refs={line.refs} />
    </Rail>
  );
}

function NoteRow({ note, last, onSaved, onDelete }: { note: LogNote; last: boolean; onSaved: (text: string) => Promise<unknown>; onDelete: () => void }) {
  const [editing, setEditing] = useState(false);
  return (
    <Rail time={note.sameDay ? note.time : null} strong last={last}>
      <Label>{note.sameDay ? "Your note" : "Your note, added later"}</Label>
      {editing ? (
        <EditText label="Note" initial={note.text} max={2000} onCancel={() => setEditing(false)} onSave={(text) => onSaved(text).then(() => setEditing(false))} />
      ) : (
        <div className="group flex items-start gap-2">
          <p className="grow text-base leading-relaxed whitespace-pre-wrap text-pretty italic">{note.text}</p>
          <Button variant="ghost" size="icon-xs" className="shrink-0 text-faint group-hover:text-muted-foreground" aria-label="Edit this note" title="Edit this note" onClick={() => setEditing(true)}>
            <Pencil />
          </Button>
          <Button variant="ghost" size="icon-xs" className="shrink-0 text-faint group-hover:text-muted-foreground" aria-label="Delete this note" title="Delete this note" onClick={onDelete}>
            <Trash2 />
          </Button>
        </div>
      )}
    </Rail>
  );
}

function EditText({ label, initial, max, onSave, onCancel }: { label: string; initial: string; max: number; onSave: (text: string) => Promise<unknown>; onCancel: () => void }) {
  const [text, setText] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return (
    <form
      className="grid gap-2"
      onSubmit={async (e) => {
        e.preventDefault();
        setBusy(true);
        setError("");
        try {
          await onSave(text.trim());
        } catch (err) {
          setError(messageOf(err));
        } finally {
          setBusy(false);
        }
      }}
    >
      <Textarea aria-label={label} value={text} maxLength={max} rows={3} onChange={(e) => setText(e.target.value)} autoFocus />
      <FormError message={error} />
      <div className="flex gap-2">
        <Button type="submit" size="sm" disabled={busy || !text.trim()}>
          Save
        </Button>
        <Button type="button" size="sm" variant="ghost" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

/** Chips that open what a line came from. */
function Refs({ projectId, refs, inline = false }: { projectId: string; refs: LogRef[]; inline?: boolean }) {
  if (!refs.length) return null;
  const chip = "rounded-[5px] border px-1.5 py-0.5 font-mono text-[10.5px] leading-none text-muted-foreground";
  return (
    <span className={cn("flex flex-wrap gap-1.5", inline && "mt-1")}>
      {refs.map((r) => {
        const href = refHref(projectId, r);
        const text = refText(r);
        return href ? (
          <Link key={`${r.kind}-${r.id}`} href={href} className={cn(chip, "hover:border-line-strong hover:text-foreground")}>
            {text}
          </Link>
        ) : (
          <span key={`${r.kind}-${r.id}`} className={cn(chip, "line-through")} title="This chat was deleted">
            {text}
          </span>
        );
      })}
    </span>
  );
}

function NoteForm({ onAdd }: { onAdd: (text: string) => Promise<unknown> }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  return (
    <form
      className="flex items-center gap-2 rounded-lg border bg-card p-1.5 pl-3"
      onSubmit={async (e) => {
        e.preventDefault();
        if (!text.trim()) return;
        setBusy(true);
        try {
          await onAdd(text.trim());
          setText("");
        } catch (err) {
          toast(messageOf(err));
        } finally {
          setBusy(false);
        }
      }}
    >
      <Input
        aria-label="Add your own note"
        placeholder="Add your own note to this day"
        value={text}
        maxLength={2000}
        onChange={(e) => setText(e.target.value)}
        className="h-10 min-w-0 grow border-0 bg-transparent px-0 shadow-none focus-visible:ring-0 dark:bg-transparent"
      />
      <Button type="submit" disabled={busy || !text.trim()} className="h-10">
        Add
      </Button>
    </form>
  );
}

/** The week's figures, and requests that write from the log in a new chat. */
function Aside({ projectId, day }: { projectId: string; day: string }) {
  const week = useLogWeek(projectId, day);
  const store = useChat();
  const newChat = useNewChatInProject();
  const today = isoDay(new Date());
  const w = week.data;
  const thisWeek = !!w && w.from <= today && today <= w.to;
  const prompts = logPrompts(addDays(today, -30), today, mondayOf(today));
  const makes = [
    { title: "Weekly update", text: "A short note for your advisor, from Monday to today.", prompt: prompts.weekly },
    { title: "Methods section", text: "What was run, on what, with which settings.", prompt: prompts.methods },
    { title: "AI-use statement", text: "What the models did in this project, for the journal.", prompt: prompts.aiUse },
  ];
  const figures: [number, string, string][] = w
    ? [
        [w.daysWithWork, "day with work", "days with work"],
        [w.chats, "chat", "chats"],
        [w.runs, "run", "runs"],
        [w.decisions, "decision", "decisions"],
      ]
    : [];
  return (
    <aside aria-label="Made from the log" className="grid min-w-0 content-start gap-6 lg:col-start-2 xl:col-start-auto">
      <section className="grid gap-2">
        <h2>
          <Eyebrow>{!w || thisWeek ? "This week" : `Week of ${longDay(w.from)}`}</Eyebrow>
        </h2>
        {week.error ? (
          <ErrorBox error={week.error} compact />
        ) : (
          <div className="grid grid-cols-2 gap-2">
            {figures.map(([n, one, many]) => (
              <div key={many} className="grid gap-0.5 rounded-lg border p-3">
                <span className="font-mono text-lg tabular-nums">{n}</span>
                <span className="text-xs text-muted-foreground">{n === 1 ? one : many}</span>
              </div>
            ))}
          </div>
        )}
      </section>
      <section className="grid gap-2">
        <h2>
          <Eyebrow>Write from the log</Eyebrow>
        </h2>
        {makes.map((m) => (
          <button
            key={m.title}
            type="button"
            className="grid gap-1 rounded-lg border bg-card px-3.5 py-3 text-left transition-colors hover:border-line-strong hover:bg-raised"
            onClick={() => {
              newChat(projectId);
              store.starter(m.prompt, false);
            }}
          >
            <span className="text-md font-medium">{m.title}</span>
            <span className="text-xs leading-snug text-muted-foreground">{m.text}</span>
          </button>
        ))}
        <Note className="text-xs">Each starts a new chat in this project that reads the log.</Note>
      </section>
    </aside>
  );
}

/** Downloads a range of days as one Markdown file. */
function ExportDialog({ projectId, open, onOpenChange, last }: { projectId: string; open: boolean; onOpenChange: (open: boolean) => void; last?: string }) {
  const to = last || isoDay(new Date());
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    const r = await exportLog(projectId, String(form.get("log-from")), String(form.get("log-to")));
    const url = URL.createObjectURL(new Blob([r.markdown], { type: "text/markdown" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = r.filename;
    document.body.append(link);
    link.click();
    link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    toast(r.days === 1 ? "1 day exported" : `${r.days} days exported`);
    onOpenChange(false);
  });
  return (
    <FormDialog open={open} onOpenChange={onOpenChange} title="Export the log" description="The days in a range as one Markdown file, with their lines, what was still open and your notes. Up to 92 days.">
      <form key={String(open)} onSubmit={onSubmit} className="grid gap-4">
        <div className="grid gap-4 sm:grid-cols-2">
          <TextField id="log-from" label="From" type="date" required defaultValue={mondayOf(to)} max={to} />
          <TextField id="log-to" label="To" type="date" required defaultValue={to} />
        </div>
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            <Download /> Export
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}
