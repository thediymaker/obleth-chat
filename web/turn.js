import { icon, esc } from './manage-ui.js';

// How an assistant reply that used tools shows its work. While it runs: one live
// card (what it is doing now, its latest thought, the files it plans, a Steer box).
// When it is done: what changed and whether it works, checkpoints, and a "Show
// work" trace in which each thought sits before the step it led to.

const TOOL_ICONS = { search_knowledge: 'book', write_file: 'edit', edit_file: 'edit', read_file: 'file', list_files: 'grid', run_command: 'terminal', run_in_terminal: 'terminal', start_server: 'play', server_logs: 'file', plan: 'grid', research_setup: 'flask', research_status: 'grid', experiment_create: 'branch', experiment_checkout: 'branch', experiment_run: 'play', experiment_wait: 'clock', run_log: 'file', delegate: 'users', output_limit: 'info', browser_search: 'search', browser_open: 'globe', browser_click: 'cursor', browser_type: 'edit', browser_read: 'file', browser_scroll: 'down', browser_back: 'globe' };
const RESEARCH = new Set(['research_setup', 'research_status', 'experiment_create', 'experiment_checkout', 'experiment_run', 'experiment_wait', 'run_log']);
const kindOf = e => e.kind || 'tool';
const BROWSING = e => String(e.tool || '').startsWith('browser_');
const CARDS = new Set(['sources', 'saved', 'question', 'workspace', 'research', 'browser-handoff', 'browser-request']);
const FILE_TOOLS = new Set(['write_file', 'edit_file']);
// Searching knowledge, saving and asking are not work to show: their cards sit below
// the answer.
const KNOWLEDGE = e => CARDS.has(e.kind) || e.tool === 'search_knowledge' || e.tool === 'save_item';

export function duration(ms) {
  if (ms == null || !Number.isFinite(ms)) return '';
  if (ms < 10000) return `${(ms / 1000).toFixed(1)}s`;
  const s = Math.round(ms / 1000);
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, '0')}s`;
}
const clock = ms => { const s = Math.max(0, Math.round(ms / 1000)); return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`; };

/** The first sentence of a thought, without markdown, for its one-line row. */
export function gist(text, max = 160) {
  const plain = String(text || '').replace(/```[\s\S]*?```/g, ' ').replace(/[#*_`>]+/g, '').replace(/\s+/g, ' ').trim();
  const sentence = plain.match(/^(.{12,}?[.!?])(?=\s|$)/)?.[1] || plain;
  return sentence.length > max ? `${sentence.slice(0, max - 1).trimEnd()}…` : sentence;
}
/** The last sentences of a thought still being written. */
export function latest(text, max = 260) {
  const plain = String(text || '').replace(/```[\s\S]*?```/g, ' ').replace(/[#*_`>]+/g, '').replace(/\s+/g, ' ').trim();
  if (plain.length <= max) return plain;
  const tail = plain.slice(-max), cut = tail.search(/[.!?]\s+\S/);
  return cut >= 0 && cut < max * 0.75 ? tail.slice(cut + 1).trim() : `…${tail.trim()}`;
}

/**
 * The reply's work in order: thoughts (sliced from the reasoning by their offsets),
 * tool steps with a phase, notes and checkpoints. An older reply with one block of
 * reasoning gets it as a single first thought.
 */
export function workOf(m) {
  const entries = Array.isArray(m.activity) ? m.activity : [];
  const reasoning = m.reasoningContent || '';
  const thoughtsRecorded = entries.some(e => kindOf(e) === 'thought');
  const list = !thoughtsRecorded && reasoning.trim() ? [{ id: 'thought-all', kind: 'thought', from: 0, to: reasoning.length }, ...entries] : entries;
  const rows = [], pending = [];
  let wrote = false, phase = null;
  const phaseOf = e => {
    if (e.connection) return e.connection;
    if (e.tool === 'plan') return 'Plan';
    if (RESEARCH.has(e.tool)) return 'Research';
    if (BROWSING(e)) return 'Browse';
    if (FILE_TOOLS.has(e.tool)) return 'Build';
    if (e.tool === 'start_server' || e.tool === 'server_logs' || e.tool === 'run_in_terminal') return 'Run';
    if (e.tool === 'run_command') return wrote ? 'Run' : 'Explore';
    return wrote ? 'Build' : 'Explore';
  };
  for (const e of list) {
    const kind = kindOf(e);
    if (CARDS.has(kind)) continue;
    if (kind === 'thought') {
      const text = reasoning.slice(e.from || 0, e.to ?? reasoning.length).trim();
      if (text) pending.push({ type: 'thought', e, text });
      continue;
    }
    if (kind === 'note' || kind === 'checkpoint') {
      if (phase) { pending.splice(0).forEach(t => rows.push({ ...t, phase })); rows.push({ type: kind, e, phase }); }
      else pending.push({ type: kind, e });
      continue;
    }
    phase = phaseOf(e);
    if (FILE_TOOLS.has(e.tool) && e.status === 'completed') wrote = true;
    pending.splice(0).forEach(t => rows.push({ ...t, phase }));
    rows.push({ type: 'step', e, phase });
  }
  // Thinking after the last step led to the answer.
  pending.forEach(t => rows.push({ ...t, phase: phase ? 'Answer' : 'Thinking' }));
  return { rows: fold(rows), steps: list.filter(e => kindOf(e) === 'tool'), entries: list, reasoning, thoughtsRecorded };
}

const failed = r => r.type === 'step' && (r.e.status === 'error' || r.e.check === 'error');
/**
 * Folds failed exploration into one dead-end row, and a problem (a failed step or a
 * broken preview) with what fixed it into one problem row.
 */
function fold(rows) {
  const out = [];
  for (let i = 0; i < rows.length; i++) {
    const r = rows[i];
    if (failed(r) && r.phase === 'Explore' && r.e.status === 'error') {
      const tries = [r];
      while (i + 1 < rows.length && failed(rows[i + 1]) && rows[i + 1].phase === 'Explore') tries.push(rows[++i]);
      out.push({ type: 'deadend', phase: r.phase, tries });
      continue;
    }
    if (failed(r) && r.e.status !== 'declined') {
      // What follows until a step shows it fixed: the preview compiles again, or the
      // same tool succeeds. Unresolved within a few rows: leave the rows as they are.
      let end = -1;
      for (let j = i + 1; j < Math.min(rows.length, i + 10); j++) {
        const n = rows[j];
        if (n.type !== 'step') continue;
        if (failed(n)) continue;
        if (n.e.check === 'ok' || (n.e.tool === r.e.tool && !n.e.connection) || (n.e.connection && n.e.tool === r.e.tool)) { end = j; break; }
      }
      if (end > 0) {
        out.push({ type: 'problem', phase: r.phase, start: r, middle: rows.slice(i + 1, end), fix: rows[end] });
        i = end;
        continue;
      }
    }
    out.push(r);
  }
  return out;
}

/** Totals for the "Show work" line and the result card. */
export function stats(work) {
  const steps = work.steps, files = new Map();
  for (const s of steps) {
    if (!FILE_TOOLS.has(s.tool) || s.status !== 'completed' || !s.path) continue;
    const f = files.get(s.path) || { path: s.path, added: 0, removed: 0 };
    f.added += s.added || 0; f.removed += s.removed || 0; files.set(s.path, f);
  }
  const thoughtMs = work.entries.filter(e => kindOf(e) === 'thought').reduce((t, e) => t + (e.durationMs || 0), 0);
  const starts = work.entries.map(e => Date.parse(e.startedAt)).filter(Number.isFinite);
  const ends = work.entries.map(e => Date.parse(e.startedAt) + (e.durationMs || 0)).filter(Number.isFinite);
  const lastCheck = [...steps].reverse().find(s => s.check);
  return {
    steps: steps.length, thoughtMs, files: [...files.values()],
    added: [...files.values()].reduce((t, f) => t + f.added, 0), removed: [...files.values()].reduce((t, f) => t + f.removed, 0),
    deadEnds: work.rows.filter(r => r.type === 'deadend').length, fixes: work.rows.filter(r => r.type === 'problem').length,
    span: starts.length ? Math.max(...ends) - Math.min(...starts) : 0, began: starts.length ? Math.min(...starts) : null,
    check: lastCheck?.check || null, served: steps.some(s => (s.tool === 'start_server' || s.check) && s.status === 'completed'),
    checkpoints: work.entries.filter(e => kindOf(e) === 'checkpoint'),
  };
}

export function createTurn({ state, markdown, chatPath, route }) {
  const open = new Set(), filters = new Map(), queued = new Map();
  let steerDraft = '';
  const live = m => m.status === 'pending' || m.status === 'streaming';
  const conversation = () => route().id;

  /** Whether this reply is shown with the work view (it used a tool). */
  function applies(m) {
    return m.role === 'assistant' && Array.isArray(m.activity) && m.activity.some(e => kindOf(e) !== 'thought' && !KNOWLEDGE(e));
  }

  const filterOf = r => r.type === 'thought' ? 'thinking' : r.type === 'deadend' || r.type === 'problem' ? 'problems' : r.type === 'checkpoint' || (r.type === 'step' && (FILE_TOOLS.has(r.e.tool) || r.e.tool === 'plan')) ? 'files' : r.type === 'step' ? 'commands' : 'all';
  const fileButton = (path, extra = '') => `<button type="button" class="tr-file" data-action="turn-open-file" data-path="${esc(path)}">${esc(path)}</button>${extra}`;
  const lines = s => `${s.added ? ` <span class="tr-add">+${s.added}</span>` : ''}${s.removed ? ` <span class="tr-del">−${s.removed}</span>` : ''}`;
  function approval(s) {
    return `<div class="approval"><span>${s.question ? esc(s.question) : `Allow ${esc(s.connection || 'this tool')} to run this?`}</span><div class="approval-actions">${[['allow', s.question ? 'Allow once' : 'Allow', 'primary'], ['chat', s.remember || 'Allow for this chat', ''], ['deny', 'Deny', '']].map(([d, label, tone]) => `<button type="button" class="btn small ${tone}" data-action="tool-decision" data-decision="${d}" data-call="${esc(s.id)}">${label}</button>`).join('')}</div></div>`;
  }
  /** The workers a delegate step sent: each one's progress, then its report. */
  function agents(s) {
    const list = Array.isArray(s.agents) ? s.agents : [];
    if (!list.length) return '';
    const count = n => n >= 1000 ? `${(n / 1000).toFixed(n >= 10000 ? 0 : 1)}k tokens` : `${n} tokens`;
    return `<ol class="tr-agents">${list.map(a => {
      const mark = a.status === 'error' ? 'bad' : a.status === 'completed' ? 'ok' : a.status === 'running' ? 'now' : 'next';
      const facts = [a.status === 'completed' ? `${a.steps} step${a.steps === 1 ? '' : 's'}` : a.doing, a.status !== 'completed' && a.steps ? `step ${a.steps}` : '', a.tokens ? count(a.tokens) : ''].filter(Boolean).join(' · ');
      const head = `<span class="tr-agent-mark is-${mark}" aria-label="${esc(a.status || '')}"></span><span class="tr-agent-text"><strong>${esc(a.title)}</strong><small>${esc(facts)}</small></span>`;
      return a.report ? `<li><details class="tr-more"><summary class="tr-agent-line">${head}</summary><div class="tr-thinking">${markdown(a.report)}</div></details></li>` : `<li><div class="tr-agent-line">${head}</div></li>`;
    }).join('')}</ol>`;
  }
  function stepText(s) {
    if (s.tool === 'delegate') { const model = s.workerModel || s.agents?.[0]?.model, getting = s.status === 'running' && !s.agents && s.detail; return `${esc(s.summary || 'Sent agents')}${model ? ` <span class="tr-quiet">· ${esc(model)}</span>` : ''}${getting ? ` <span class="tr-quiet">· ${esc(s.detail)}</span>` : ''}`; }
    if (s.path && FILE_TOOLS.has(s.tool)) return `${s.tool === 'write_file' ? 'Wrote' : 'Edited'} ${fileButton(s.path, lines(s))}`;
    if (s.tool === 'run_in_terminal' && s.command) return `Opened <code>${esc(s.command)}</code> in the terminal <button type="button" class="tr-link" data-action="turn-run-terminal" data-command="${esc(s.command)}">Run again</button>`;
    if (s.tool === 'plan' && Array.isArray(s.files)) return `Planned ${s.files.length} file${s.files.length === 1 ? '' : 's'}`;
    if (BROWSING(s) && s.url) { let host = ''; try { host = new URL(s.url).host; } catch { /* not a URL */ } return `${esc(s.summary || s.tool)}${host && s.tool !== 'browser_open' ? ` <span class="tr-quiet">· ${esc(host)}</span>` : ''}`; }
    return esc(s.summary || s.tool);
  }
  const stepTime = s => ({ running: 'Running…', awaiting: 'Needs your answer', error: 'Failed', declined: 'Declined' }[s.status] || duration(s.durationMs));
  function step(s, extraClass = '') {
    const mark = s.status === 'error' || s.check === 'error' ? 'bad' : s.status === 'running' || s.status === 'awaiting' ? 'now' : s.status === 'declined' ? 'off' : 'ok';
    const glyph = icon(s.connection ? 'link' : TOOL_ICONS[s.tool] || 'terminal');
    const thumb = s.thumb && String(s.thumb).startsWith('data:image/jpeg;base64,') ? `<img class="tr-thumb" src="${esc(s.thumb)}" alt="" loading="lazy">` : '';
    const head = `${thumb}<span class="tr-text">${stepText(s)}${s.check === 'error' ? ' <span class="tr-del">· preview broke</span>' : ''}</span><small>${esc(stepTime(s))}</small>`;
    const detail = s.detail && s.status !== 'running' && s.status !== 'awaiting' ? String(s.detail) : '';
    const body = s.tool === 'delegate'
      ? `<div class="tr-line">${head}</div>${agents(s) || (s.status === 'error' && s.detail ? `<pre>${esc(s.detail)}</pre>` : '')}`
      : s.status === 'awaiting'
      ? `<div class="tr-line">${head}</div>${s.detail ? `<pre>${esc(s.detail)}</pre>` : ''}${approval(s)}`
      : detail && !(FILE_TOOLS.has(s.tool) && s.status === 'completed')
        ? `<details class="tr-more" ${s.status === 'error' ? 'open' : ''}><summary class="tr-line">${head}</summary><pre>${esc(detail)}</pre></details>`
        : `<div class="tr-line">${head}</div>`;
    return `<li class="tr-row tr-step is-${mark} ${extraClass}"><span class="tr-mark" aria-hidden="true">${glyph}</span><div class="tr-body">${body}</div></li>`;
  }
  function thought(r) {
    return `<li class="tr-row tr-thought" data-f="thinking"><span class="tr-mark" aria-hidden="true">${icon('orbit')}</span><div class="tr-body"><details class="tr-more"><summary class="tr-line"><span class="tr-text">${esc(gist(r.text))}</span><small>${esc(duration(r.e.durationMs))}</small></summary><div class="tr-thinking">${markdown(r.text)}</div></details></div></li>`;
  }
  function row(r) {
    const f = filterOf(r);
    if (r.type === 'thought') return thought(r);
    if (r.type === 'note') return `<li class="tr-row tr-note" data-f="all"><span class="tr-mark" aria-hidden="true">${icon('chat')}</span><div class="tr-body"><div class="tr-line"><span class="tr-text"><span class="tr-who">You, while it worked:</span> ${esc(r.e.summary)}</span></div></div></li>`;
    if (r.type === 'checkpoint') return `<li class="tr-row tr-checkpoint" data-f="files"><span class="tr-mark" aria-hidden="true">${icon('bookmark')}</span><div class="tr-body"><div class="tr-line"><span class="tr-text">${esc(r.e.summary)} saved <span class="tr-quiet">· ${esc(r.e.stat || 'project saved')}</span></span><button type="button" class="tr-link" data-action="checkpoint-restore" data-sha="${esc(r.e.sha)}" data-label="${esc(r.e.summary)}">Restore</button></div></div></li>`;
    if (r.type === 'deadend') return `<li class="tr-row tr-deadend" data-f="problems"><span class="tr-mark" aria-hidden="true"><span class="tr-dot"></span></span><div class="tr-body"><details class="tr-more tr-boxed"><summary class="tr-line"><span class="tr-text">Dead end: ${esc(r.tries[0].e.summary)}</span><small>${r.tries.length} tr${r.tries.length === 1 ? 'y' : 'ies'}</small></summary><ol class="tr-nested">${r.tries.map(t => step(t.e)).join('')}</ol></details></div></li>`;
    if (r.type === 'problem') {
      const saw = r.start.e.check === 'error' ? `${stepText(r.start.e)}: the preview stopped compiling` : `${esc(r.start.e.summary)}: ${esc(String(r.start.e.detail || 'failed').split('\n').find(l => l.trim()) || 'failed')}`;
      const why = r.middle.find(x => x.type === 'thought');
      const fixes = r.middle.filter(x => x.type === 'step');
      // The step that fixed it is often the edit itself, which the check then confirms.
      const fixedBy = fixes.length ? fixes.map(x => stepText(x.e)).join('<br>') : FILE_TOOLS.has(r.fix.e.tool) ? stepText(r.fix.e) : '';
      const check = FILE_TOOLS.has(r.fix.e.tool) && !fixes.length ? (r.fix.e.check === 'ok' ? 'The preview compiles again' : 'The change went through') : `${stepText(r.fix.e)}${r.fix.e.check === 'ok' ? ' · the preview compiles' : ''}`;
      return `<li class="tr-row tr-problem" data-f="problems"><span class="tr-mark" aria-hidden="true"><span class="tr-dot"></span></span><div class="tr-body"><div class="tr-problem-card"><div class="tr-problem-head">Problem fixed</div><dl><div><dt>Saw</dt><dd>${saw}</dd></div>${why ? `<div><dt>Why</dt><dd>${esc(gist(why.text, 220))}</dd></div>` : ''}${fixedBy ? `<div><dt>Fix</dt><dd>${fixedBy}</dd></div>` : ''}<div><dt>Check</dt><dd><span class="tr-ok">✓</span> ${check}</dd></div></dl></div></div></li>`;
    }
    return step(r.e).replace('<li class="tr-row', `<li data-f="${f}" class="tr-row`);
  }
  function trace(m, work) {
    const id = m.id || 'live', filter = filters.get(id) || 'all';
    const t0 = stats(work).began;
    const groups = [];
    for (const r of work.rows) {
      if (!groups.length || groups.at(-1).phase !== r.phase) groups.push({ phase: r.phase, rows: [] });
      groups.at(-1).rows.push(r);
    }
    const range = g => {
      const es = g.rows.flatMap(r => r.type === 'step' ? [r.e] : r.type === 'problem' ? [r.start.e, r.fix.e] : r.type === 'deadend' ? r.tries.map(t => t.e) : []).filter(e => Number.isFinite(Date.parse(e.startedAt)));
      if (!es.length || t0 == null) return '';
      const a = Math.min(...es.map(e => Date.parse(e.startedAt))), b = Math.max(...es.map(e => Date.parse(e.startedAt) + (e.durationMs || 0)));
      return `${clock(a - t0)} – ${clock(b - t0)}`;
    };
    const problems = work.rows.filter(r => r.type === 'problem' || r.type === 'deadend').length;
    const chips = [['all', 'All'], ['thinking', 'Thinking'], ['files', 'Files'], ['commands', 'Commands'], ['problems', `Problems${problems ? ` · ${problems}` : ''}`]];
    return `<section class="trace" data-filter="${filter}" aria-label="The work in this reply"><div class="trace-filters" role="group" aria-label="Show">${chips.map(([k, label]) => `<button type="button" class="trace-chip ${k === 'problems' && problems ? 'warn' : ''}" data-action="trace-filter" data-id="${esc(id)}" data-filter="${k}" aria-pressed="${filter === k}">${esc(label)}</button>`).join('')}</div><ol class="trace-list">${groups.map(g => `<li class="tr-phase"><span>${esc(g.phase)}</span><small>${esc(range(g))}</small></li>${g.rows.map(row).join('')}`).join('')}</ol></section>`;
  }

  // ---- While it works ----
  const PHASES = ['Plan', 'Explore', 'Build', 'Run'];
  function runCard(m, work) {
    const s = stats(work);
    const waiting = work.steps.filter(x => x.status === 'awaiting');
    const running = [...work.steps].reverse().find(x => x.status === 'running');
    const lastThought = [...work.entries].reverse().find(e => kindOf(e) === 'thought');
    const fresh = work.reasoning.slice(lastThought ? lastThought.to : 0).trim();
    const draft = m._draft && !work.steps.some(x => x.id === m._draft.id) ? m._draft : null;
    const now = waiting[0] ? `Waiting for you: ${waiting[0].summary}` : draft ? `Writing ${draft.path || 'a file'}` : running ? running.summary : fresh ? 'Thinking' : 'Working';
    const began = s.began || Date.parse(m.createdAt || m._began || '') || Date.now();
    const seen = work.rows.filter(r => r.type === 'step').map(r => r.phase);
    const current = draft ? 'Build' : seen.at(-1);
    // Workspace work goes through the usual phases; a cluster or server is its own.
    const usual = draft || work.steps.some(x => !x.connection && !BROWSING(x)) ? PHASES.filter(p => p !== 'Plan' || seen.includes('Plan')) : [];
    const phases = [...new Set([...usual, ...seen])];
    const rail = phases.map(p => {
      const at = phases.indexOf(p), cur = phases.indexOf(current);
      const status = p === current ? 'now' : seen.includes(p) || (cur > at) ? 'done' : 'next';
      return `<li class="run-phase is-${status}"><span></span>${esc(p)}</li>`;
    }).join('');
    const nowBlock = draft
      ? `<div class="run-now"><div class="run-now-head">${icon('edit')}<span class="tr-file-name">${esc(draft.path || 'file')}</span><span>${draft.lines} line${draft.lines === 1 ? '' : 's'} so far</span></div><pre class="run-code">${esc(draft.tail)}<span class="run-caret">▍</span></pre></div>`
      : '';
    const thoughtText = fresh || (lastThought ? work.reasoning.slice(lastThought.from, lastThought.to) : '');
    const plan = [...work.steps].reverse().find(x => x.tool === 'plan' && Array.isArray(x.files));
    const written = new Set(work.steps.filter(x => FILE_TOOLS.has(x.tool) && x.status === 'completed' && x.path).map(x => x.path));
    const fileList = [...new Set([...(plan?.files || []), ...written, ...(draft?.path ? [draft.path] : [])])];
    const files = fileList.length ? `<div class="run-section"><span class="eyebrow">${plan ? 'Planned files' : 'Files so far'}</span><div class="run-files">${fileList.slice(0, 24).map(f => `<span class="run-file ${f === draft?.path ? 'is-now' : written.has(f) ? 'is-done' : 'is-next'}">${written.has(f) && f !== draft?.path ? '✓ ' : ''}${esc(f)}</span>`).join('')}${fileList.length > 24 ? `<span class="run-file is-next">+${fileList.length - 24} more</span>` : ''}</div></div>` : '';
    const recent = work.rows.filter(r => r.type !== 'thought' && !(r.type === 'step' && r.e.status === 'awaiting')).slice(-3);
    const isOpen = open.has(m.id || 'live');
    const notes = (queued.get(conversation()) || []).filter(n => !work.entries.some(e => kindOf(e) === 'note' && e.summary === n));
    return `<section class="run-card" aria-label="Working"><div class="run-head"><span class="run-pulse" aria-hidden="true"></span><div class="grow"><strong>${esc(now)}</strong><small><span data-since="${new Date(began).toISOString()}">${esc(clock(Date.now() - began))}</span> · ${s.steps} step${s.steps === 1 ? '' : 's'}${s.files.length ? ` · ${s.files.length} file${s.files.length === 1 ? '' : 's'} changed` : ''}</small></div></div><ol class="run-rail">${rail}</ol>${waiting.map(w => `<div class="run-ask">${approval(w)}${w.detail ? `<pre>${esc(w.detail)}</pre>` : ''}</div>`).join('')}${nowBlock}${thoughtText ? `<div class="run-thought">${icon('orbit')}<div class="grow"><span class="eyebrow">Latest thought</span><p>${esc(latest(thoughtText))}</p></div></div>` : ''}${files}${recent.length ? `<div class="run-section"><div class="spread"><span class="eyebrow">Recent steps</span><button type="button" class="tr-link" data-action="trace-toggle" data-id="${esc(m.id || 'live')}" aria-expanded="${isOpen}">${isOpen ? 'Hide steps' : `Show all ${s.steps}`}</button></div><ol class="trace-list compact">${recent.map(row).join('')}</ol></div>` : ''}${notes.map(n => `<p class="run-queued">${icon('clock')}<span><span class="run-queued-label">Queued for the next step</span>${esc(n)}</span></p>`).join('')}</section>${isOpen ? trace(m, work) : ''}`;
  }
  function steerForm() {
    return `<form class="steer-form" id="steer-form" data-conversation="${esc(conversation() || '')}"><label for="steer-text">Steer</label><input id="steer-text" name="steer-text" autocomplete="off" maxlength="2000" placeholder="Add a note without stopping, like “make the mane darker”" value="${esc(steerDraft)}"><button class="btn small primary" type="submit">Send at next step</button></form>`;
  }

  // ---- When it is done ----
  function resultCard(m, work, s) {
    if (!s.files.length) return '';
    const stopped = m.status !== 'completed';
    const checks = [
      s.check === 'ok' ? '<li><span class="tr-ok">✓</span>The preview compiles</li>' : s.check === 'error' ? '<li><span class="tr-bad">✗</span>The preview had an error at the end</li>' : '',
      s.fixes ? `<li><span class="tr-warn">!</span>${s.fixes} problem${s.fixes === 1 ? '' : 's'} found and fixed on the way</li>` : '',
      stopped ? `<li><span class="tr-warn">!</span>Stopped before finishing${m.errorMessage ? `: ${esc(m.errorMessage)}` : ''}</li>` : '',
    ].join('');
    const id = conversation();
    return `<section class="result-card" aria-label="What changed"><div class="result-top"><div class="grow"><span class="eyebrow">${stopped ? 'Changed so far' : 'Done'}${s.span ? ` · ${esc(duration(s.span))}` : ''}</span><strong>${s.files.length} file${s.files.length === 1 ? '' : 's'} changed <span class="tr-add">+${s.added}</span>${s.removed ? ` <span class="tr-del">−${s.removed}</span>` : ''}</strong>${checks ? `<ul class="result-checks">${checks}</ul>` : ''}</div><div class="result-actions">${s.served ? `<button type="button" class="btn small primary" data-action="turn-open-preview">${icon('play')}Open preview</button>` : ''}<button type="button" class="btn small" data-action="turn-open-files">${icon('file')}Files</button>${id ? `<a class="btn small ghost" href="${esc(chatPath(id))}/workspace/download" download>${icon('download')}Download</a>` : ''}</div></div><div class="result-files">${s.files.map(f => `<button type="button" class="result-file" data-action="turn-open-file" data-path="${esc(f.path)}"><span>${esc(f.path)}</span><span>${f.added ? `<span class="tr-add">+${f.added}</span>` : ''}${f.removed ? ` <span class="tr-del">−${f.removed}</span>` : ''}</span></button>`).join('')}</div></section>`;
  }
  function checkpoints(s) {
    if (!s.checkpoints.length) return '';
    const list = s.checkpoints.slice(-6);
    return `<section class="checkpoints" aria-label="Checkpoints"><div class="spread"><span class="eyebrow">Checkpoints</span><small>Saved after each change that compiled${s.checkpoints.length > list.length ? ` · last ${list.length} of ${s.checkpoints.length}` : ''}</small></div><div class="checkpoint-list">${list.map((c, i) => `<div class="checkpoint ${i === list.length - 1 ? 'is-latest' : ''}"><strong>${esc(c.summary)}</strong><small>${esc(c.stat || 'Project saved')}</small><button type="button" class="tr-link" data-action="checkpoint-restore" data-sha="${esc(c.sha)}" data-label="${esc(c.summary)}">${i === list.length - 1 ? 'Restore this' : 'Restore'}</button></div>`).join('')}</div></section>`;
  }

  /** Above the answer: the live card, or the result card. */
  function before(m) {
    const work = workOf(m);
    if (live(m)) return `<div class="turn-work">${runCard(m, work)}</div>`;
    return `<div class="turn-work">${resultCard(m, work, stats(work))}</div>`;
  }
  /** Below the answer: checkpoints and the "Show work" trace. */
  function after(m) {
    if (live(m)) return '';
    const work = workOf(m), s = stats(work), id = m.id || 'live', isOpen = open.has(id);
    const facts = [`${s.steps} step${s.steps === 1 ? '' : 's'}`, s.thoughtMs ? `thought for ${duration(s.thoughtMs)}` : '', s.deadEnds ? `${s.deadEnds} dead end${s.deadEnds === 1 ? '' : 's'}` : '', s.fixes ? `${s.fixes} fix${s.fixes === 1 ? '' : 'es'}` : ''].filter(Boolean).join(' · ');
    return `<div class="turn-after">${checkpoints(s)}<button type="button" class="show-work" data-action="trace-toggle" data-id="${esc(id)}" aria-expanded="${isOpen}">${icon('panel')}<strong>${isOpen ? 'Hide work' : 'Show work'}</strong><span>${esc(facts)}</span>${icon('down')}</button>${isOpen ? trace(m, work) : ''}</div>`;
  }
  function signature(m) {
    const work = Array.isArray(m.activity) ? m.activity : [];
    const id = m.id || 'live';
    return JSON.stringify([live(m), work.map(e => [e.id, e.status, e.check, e.tick]), m._draft?.lines, m._draft?.id, live(m) ? Math.floor((m.reasoningContent || '').length / 120) : 0, open.has(id), filters.get(id), (queued.get(conversation()) || []).length]);
  }

  const actions = {
    'trace-toggle': el => { const id = el.dataset.id; if (open.has(id)) open.delete(id); else open.add(id); return 'render'; },
    'trace-filter': el => { filters.set(el.dataset.id, el.dataset.filter); return 'render'; },
  };
  async function submit(form, api, toast) {
    const input = form.elements.namedItem('steer-text'), text = input.value.trim(), id = form.dataset.conversation;
    if (!text || !id) return;
    await api.request(`${chatPath(id)}/steer`, { method: 'POST', body: { text } });
    const list = queued.get(id) || []; list.push(text); queued.set(id, list);
    input.value = ''; steerDraft = '';
    toast('Sent. The model reads it at its next step.');
  }
  /** A finished reply never read some notes: give them back to the composer. */
  function unread(m) {
    const id = conversation(), list = queued.get(id);
    if (!list?.length || live(m)) return [];
    const read = new Set((m.activity || []).filter(e => kindOf(e) === 'note').map(e => e.summary));
    queued.delete(id);
    return list.filter(n => !read.has(n));
  }
  if (typeof document !== 'undefined') {
    setInterval(() => document.querySelectorAll('[data-since]').forEach(el => { const t = Date.parse(el.dataset.since); if (Number.isFinite(t)) el.textContent = clock(Date.now() - t); }), 1000);
  }
  return { applies, before, after, signature, actions, submit, unread, steerForm, live, setDraft: v => { steerDraft = v; } };
}
