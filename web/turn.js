import { icon, esc } from './manage-ui.js';

// How an assistant reply that used tools shows its work. While it runs: one live
// card (what it is doing now, its latest thought, the files it plans, a Steer box).
// When it is done: what changed and whether it works, checkpoints, and a "Show
// work" trace in which each thought sits before the step it led to.

const TOOL_ICONS = { search_knowledge: 'book', write_file: 'edit', edit_file: 'edit', read_file: 'file', list_files: 'grid', run_command: 'terminal', run_in_terminal: 'terminal', start_server: 'play', server_logs: 'file', plan: 'grid', research_setup: 'flask', research_status: 'grid', experiment_create: 'branch', experiment_checkout: 'branch', experiment_run: 'play', experiment_wait: 'clock', run_log: 'file', report_check: 'shield', delegate: 'users', output_limit: 'info', connection: 'info', thinking_limit: 'info', check: 'check', browser_search: 'search', browser_open: 'globe', browser_click: 'cursor', browser_type: 'edit', browser_read: 'file', browser_scroll: 'down', browser_back: 'globe' };
const RESEARCH = new Set(['research_setup', 'research_status', 'experiment_create', 'experiment_checkout', 'experiment_run', 'experiment_wait', 'run_log', 'report_check']);
const kindOf = e => e.kind || 'tool';
// Work on a cluster or other service goes through its own stages: look, ask, act,
// then check what the change did.
const STAGES = {
  kubernetes: { name: 'Kubernetes', icon: 'cube', look: 'Inspect', ask: 'Approve', act: 'Apply', check: 'Verify', rail: ['Inspect', 'Approve', 'Apply', 'Verify'] },
  slurm: { name: 'Slurm', icon: 'server', look: 'Inspect', ask: 'Submit', act: 'Submit', check: 'Watch', rail: ['Inspect', 'Submit', 'Watch', 'Results'] },
  mcp: { name: '', icon: 'link', look: 'Inspect', ask: 'Approve', act: 'Act', check: 'Check', rail: ['Inspect', 'Approve', 'Act', 'Check'] },
};
const stagesOf = e => STAGES[e.service] || STAGES.mcp;
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
  const acted = new Set();
  const phaseOf = e => {
    if (e.connection) {
      const st = stagesOf(e);
      if (e.readOnly === false) return e.status === 'awaiting' ? st.ask : st.act;
      return acted.has(e.connection) ? st.check : st.look;
    }
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
    if (e.connection && e.readOnly === false && e.status === 'completed') acted.add(e.connection);
    pending.splice(0).forEach(t => rows.push({ ...t, phase }));
    rows.push({ type: 'step', e, phase });
  }
  // Thinking after the last step led to the answer.
  pending.forEach(t => rows.push({ ...t, phase: phase ? 'Answer' : 'Thinking' }));
  return { rows: fold(rows), steps: list.filter(e => kindOf(e) === 'tool'), entries: list, reasoning, thoughtsRecorded };
}

// A delegate step shows each worker's outcome itself, so it never folds away.
/** The last lines of a step's output. */
const lastLines = (text, n) => String(text || '').replace(/\s+$/, '').split('\n').slice(-n).join('\n');
const minutes = m => !m ? '' : m >= 60 && m % 60 === 0 ? `${m / 60} h` : m >= 60 ? `${Math.floor(m / 60)} h ${m % 60} min` : `${m} min`;
/**
 * What a change waiting for approval does, read loosely from its arguments: the
 * resources a Kubernetes manifest applies or deletes, or the Slurm job a script
 * submits. Null when there is nothing to read.
 */
export function changeOf(s) {
  const text = String(s.detail || '');
  const field = (src, key) => src.match(new RegExp(`^${key}:[ \\t]*["']?([^"'\\n]+?)["']?[ \\t]*$`, 'm'))?.[1];
  if (s.service === 'kubernetes' && s.action === 'apply') {
    const body = text.replace(/^manifest:[ \t]*\n?/, '');
    let docs;
    try {
      const v = JSON.parse(body);
      docs = (Array.isArray(v) ? v : v?.kind === 'List' ? v.items || [] : [v]).map(o => ({ kind: o?.kind, name: o?.metadata?.name, namespace: o?.metadata?.namespace, text: JSON.stringify(o) }));
    } catch {
      docs = body.split(/^---[ \t]*$/m).filter(d => d.trim()).map(d => {
        const meta = d.match(/^metadata:[ \t]*\n((?:[ \t]+.*(?:\n|$))*)/m)?.[1] || '';
        return { kind: field(d, 'kind'), name: meta.match(/^[ \t]+name:[ \t]*["']?([^"'\s]+)/m)?.[1], namespace: meta.match(/^[ \t]+namespace:[ \t]*["']?([^"'\s]+)/m)?.[1], text: d };
      });
    }
    const items = docs.filter(d => d.kind).map(d => {
      const images = [...new Set([...d.text.matchAll(/"?image"?:[ \t]*"?([^"\s,}]+)/g)].map(x => x[1]))];
      const replicas = d.text.match(/"?replicas"?:[ \t]*(\d+)/)?.[1];
      const ports = [...new Set([...d.text.matchAll(/"?(?:containerPort|port)"?:[ \t]*(\d+)/g)].map(x => x[1]))];
      return { verb: 'Apply', what: `${d.kind}/${d.name || '?'}`, where: d.namespace, facts: [['Image', images.join(', ')], ['Replicas', replicas], ['Port', ports.join(', ')]].filter(f => f[1]) };
    });
    return items.length ? { items, raw: 'Manifest', text: body } : null;
  }
  if (s.service === 'kubernetes' && s.action === 'delete') return { items: [{ verb: 'Delete', what: `${field(text, 'kind') || '?'}/${field(text, 'name') || '?'}`, where: field(text, 'namespace'), facts: [] }] };
  if (s.service === 'slurm' && s.action === 'submit') {
    const script = text.match(/^script:[ \t]*\n([\s\S]*?)(?=^(?:cpus|memory_mb|name|nodes|output|partition|tasks|time_limit_minutes|working_directory):|$(?![\s\S]))/m)?.[1];
    const mb = Number(field(text, 'memory_mb'));
    const facts = [['Partition', field(text, 'partition') || 'default'], ['Time limit', minutes(Number(field(text, 'time_limit_minutes')))], ['CPUs', field(text, 'cpus')], ['Nodes', field(text, 'nodes')], ['Memory', mb ? (mb >= 1024 ? `${+(mb / 1024).toFixed(1)} GB` : `${mb} MB`) : '']].filter(f => f[1]);
    return { items: [{ verb: 'Submit', what: field(text, 'name') || 'batch job', facts }], raw: 'Script', text: script?.trimEnd() || text };
  }
  if (s.service === 'slurm' && s.action === 'cancel') return { items: [{ verb: 'Cancel', what: `job ${field(text, 'job_id') || '?'}`, facts: [] }] };
  return null;
}

const failed = r => r.type === 'step' && r.e.tool !== 'delegate' && (r.e.status === 'error' || r.e.check === 'error');
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

export function createTurn({ state, markdown, chatPath, route, previews = () => true }) {
  // Which worker each delegate step shows in its detail panel.
  const open = new Set(), filters = new Map(), queued = new Map(), chosen = new Map();
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
    const mark = s.status === 'error' || s.check === 'error' ? 'bad' : s.status === 'running' || s.status === 'awaiting' ? 'now' : s.status === 'declined' ? 'off' : s.flagged ? 'warn' : 'ok';
    const glyph = icon(s.connection ? 'link' : TOOL_ICONS[s.tool] || 'terminal');
    const thumb = s.thumb && String(s.thumb).startsWith('data:image/jpeg;base64,') ? `<img class="tr-thumb" src="${esc(s.thumb)}" alt="" loading="lazy">` : '';
    const head = `${thumb}<span class="tr-text">${stepText(s)}${s.check === 'error' ? ' <span class="tr-del">· preview broke</span>' : ''}</span><small>${esc(stepTime(s))}</small>`;
    const detail = s.detail && s.status !== 'running' && s.status !== 'awaiting' ? String(s.detail) : '';
    const body = s.tool === 'delegate'
      ? `<div class="tr-line">${head}</div>${agents(s) || (s.status === 'error' && s.detail ? `<pre>${esc(s.detail)}</pre>` : '')}`
      : s.status === 'awaiting'
      ? `<div class="tr-line">${head}</div>${s.detail ? `<pre>${esc(s.detail)}</pre>` : ''}${approval(s)}`
      : detail && !(FILE_TOOLS.has(s.tool) && s.status === 'completed')
        ? `<details class="tr-more" ${s.status === 'error' || s.flagged ? 'open' : ''}><summary class="tr-line">${head}</summary><pre>${esc(detail)}</pre></details>`
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
  // The card keeps one size while it runs: a fixed pane shows whatever is happening
  // now (a file being written, a command's output, the thinking), with one line for
  // the latest thought and three for the latest steps.
  const PHASES = ['Plan', 'Explore', 'Build', 'Run'];
  const COMMANDS = new Set(['run_command', 'start_server', 'run_in_terminal']);
  const plural = (n, word) => `${n} ${word}${n === 1 ? '' : 's'}`;
  /** What the card is working on: the workspace, a cluster, the browser or workers. */
  function target(s, draft) {
    if (!s && !draft) return '';
    if (s?.connection) { const st = stagesOf(s); return `<span class="run-target">${icon(st.icon)}<span class="run-target-name">${esc(s.connection)}</span>${st.name ? `<span class="run-target-kind">· ${esc(st.name)}</span>` : ''}</span>`; }
    if (s && BROWSING(s)) return `<span class="run-target">${icon('globe')}Browser</span>`;
    if (s?.tool === 'delegate') { const model = s.workerModel || s.agents?.[0]?.model; return `<span class="run-target">${icon('users')}<span class="run-target-name mono">${esc(model || 'Workers')}</span></span>`; }
    return `<span class="run-target">${icon('terminal')}Workspace</span>`;
  }
  /** A change waiting for approval: what it does, read from its arguments, with the raw text folded away. */
  function askCard(s) {
    const change = changeOf(s), waits = Date.parse(s.startedAt) + 15 * 60000;
    const head = s.question ? esc(s.question) : s.readOnly === false && s.service !== 'mcp' ? `Allow this change on ${esc(s.connection)}?` : `Allow ${esc(s.connection || 'this tool')} to run this?`;
    const items = change ? `<div class="ask-change">${change.items.map(c => `<div class="ask-item"><div class="ask-what"><span class="ask-verb is-${esc(c.verb.toLowerCase())}">${esc(c.verb)}</span><span class="mono">${esc(c.what)}</span>${c.where ? `<span class="ask-where">in ${esc(c.where)}</span>` : ''}</div>${c.facts.length ? `<dl class="ask-facts">${c.facts.map(([k, v]) => `<div><dt>${esc(k)}</dt><dd class="mono">${esc(v)}</dd></div>`).join('')}</dl>` : ''}</div>`).join('')}</div>` : '';
    const raw = s.detail ? `<details class="ask-raw"${change ? '' : ' open'}><summary>${icon('down')}${esc(change?.raw || 'Arguments')}<span>· ${plural(String(change?.text ?? s.detail).trim().split('\n').length, 'line')}</span></summary><pre>${esc(change?.text ?? s.detail)}</pre></details>` : '';
    const buttons = [['allow', s.question ? 'Allow once' : 'Allow', 'primary'], ['chat', s.remember || 'Allow for this chat', ''], ['deny', 'Deny', '']].map(([d, label, tone]) => `<button type="button" class="btn small ${tone}" data-action="tool-decision" data-decision="${d}" data-call="${esc(s.id)}">${label}</button>`).join('');
    return `<div class="run-ask"><div class="ask-head"><strong>${head}</strong>${s.readOnly === false ? `<span>It changes ${s.service === 'mcp' || !s.service ? 'something outside Chat' : 'the cluster'}, so it waits for you. It runs as your own account.</span>` : ''}</div>${items}${raw}<div class="ask-actions">${buttons}${Number.isFinite(waits) ? `<small>Waits <span data-until="${new Date(waits).toISOString()}">${esc(clock(waits - Date.now()))}</span> more</small>` : ''}</div></div>`;
  }
  /** The workers of a delegate step as a graph: the lead, each worker, and their reports coming back. */
  function workerGraph(s, m) {
    const list = s.agents, n = list.length;
    const height = Math.max(200, n * 72 + 28), first = (height - (n * 72 - 12)) / 2, mid = height / 2;
    const at = i => first + i * 72 + 30;
    const picked = Math.min(n - 1, chosen.has(s.id) ? chosen.get(s.id) : Math.max(0, list.findIndex(a => a.status === 'running')));
    const mark = a => a.status === 'error' ? 'bad' : a.status === 'completed' ? 'ok' : a.status === 'running' ? 'now' : 'next';
    const edges = list.map((a, i) => `<path class="wg-edge is-${mark(a)}" d="M200 ${mid} C250 ${mid} 250 ${at(i)} 300 ${at(i)}"/><path class="wg-edge is-${mark(a) === 'now' || mark(a) === 'next' ? 'wait' : mark(a)} back" d="M700 ${at(i)} C750 ${at(i)} 750 ${mid} 800 ${mid}"/>`).join('');
    const tokens = n => !n ? '' : n >= 1000 ? `${(n / 1000).toFixed(n >= 10000 ? 0 : 1)}k` : String(n);
    const nodes = list.map((a, i) => {
      const progress = a.status === 'completed' || a.status === 'error' ? 100 : Math.min(100, Math.round((a.steps || 0) / 30 * 100));
      const line = a.status === 'completed' ? `Reported · ${plural(a.steps || 0, 'step')}` : a.doing || '';
      return `<button type="button" class="wg-node is-${mark(a)}${i === picked ? ' is-picked' : ''}" style="top:${at(i) - 30}px" data-action="worker-pick" data-step="${esc(s.id)}" data-index="${i}" aria-pressed="${i === picked}"><span class="wg-node-top"><span class="wg-dot" aria-hidden="true"></span><strong>${esc(a.title)}</strong><small class="mono">${esc(tokens(a.tokens))}</small></span><span class="wg-doing">${esc(line)}</span><span class="wg-bar" style="width:${progress}%" aria-hidden="true"></span></button>`;
    }).join('');
    const done = list.filter(a => a.status === 'completed').length, bad = list.filter(a => a.status === 'error').length, left = n - done - bad;
    const lead = m.settings?.resolvedModel || m.settings?.model || 'Lead model';
    const a = list[picked];
    const facts = [a.status === 'completed' || a.status === 'error' ? plural(a.steps || 0, 'step') : `step ${a.steps || 0} of 30`, a.tokens ? `${tokens(a.tokens)} tokens` : ''].filter(Boolean).join(' · ');
    const status = { completed: 'Reported', error: 'Failed', running: 'Working', queued: 'Waiting to start' }[a.status] || '';
    const body = a.report || a.status === 'error'
      ? `<p class="wg-report">${esc(a.report ? gist(a.report, 400) : a.doing)}</p>${a.report ? `<details class="wg-full"><summary>Read the whole report</summary><div class="tr-thinking">${markdown(a.report)}</div></details>` : ''}`
      : `<ol class="wg-steps">${(a.recent?.length ? a.recent : [a.doing]).filter(Boolean).map((t, j, all) => `<li><span class="wg-dot ${j === all.length - 1 && a.status === 'running' ? 'is-now' : 'is-ok'}" aria-hidden="true"></span><span>${esc(t)}</span></li>`).join('')}</ol>`;
    return `<div class="wg"><div class="wg-canvas" style="height:${height}px"><svg class="wg-edges" viewBox="0 0 1000 ${height}" preserveAspectRatio="none" aria-hidden="true">${edges}</svg><div class="wg-end wg-lead" style="top:${mid - 36}px"><span>Lead</span><strong class="mono">${esc(lead)}</strong><small>${left ? `waiting on ${left}` : 'reading reports'}</small></div>${nodes}<div class="wg-end wg-reports" style="top:${mid - 36}px"><span>Reports</span><strong>${done} of ${n}</strong><small>${bad ? `${bad} failed` : left ? 'coming in' : 'all in'}</small></div></div><div class="wg-detail" aria-live="polite"><div class="wg-detail-head"><strong>${esc(a.title)}</strong><span class="is-${mark(a)}">${esc(status)}</span><small>${esc(facts)}</small></div>${a.task ? `<p class="wg-task">Task: ${esc(a.task)}</p>` : ''}${body}</div></div>`;
  }
  /** One of the card's three latest-step rows. */
  const recentRow = s => s ? `<li class="run-step is-${s.status === 'error' || s.check === 'error' ? 'bad' : s.status === 'running' ? 'now' : s.status === 'declined' ? 'off' : 'ok'}"><span class="run-step-dot" aria-hidden="true"></span><span class="run-step-text">${stepText(s)}</span><small>${esc(s.status === 'running' ? 'now' : duration(s.durationMs))}</small></li>` : '<li class="run-step is-empty" aria-hidden="true"></li>';
  function runCard(m, work) {
    const s = stats(work);
    const waiting = work.steps.filter(x => x.status === 'awaiting');
    const running = [...work.steps].reverse().find(x => x.status === 'running');
    const lastStep = work.steps.at(-1);
    const lastThought = [...work.entries].reverse().find(e => kindOf(e) === 'thought');
    const fresh = work.reasoning.slice(lastThought ? lastThought.to : 0).trim();
    const draft = m._draft && !work.steps.some(x => x.id === m._draft.id) ? m._draft : null;
    const answering = !running && !waiting.length && !draft && !!String(m.content || '').trim();
    const asked = waiting[0] && changeOf(waiting[0])?.items[0];
    const now = asked ? `Waiting for you: ${asked.verb.toLowerCase()} ${asked.what}` : waiting[0] ? `Waiting for you: ${waiting[0].summary}` : draft ? `Writing ${draft.path || 'a file'}` : running ? running.summary : answering ? 'Writing the answer' : fresh ? 'Thinking' : 'Working';
    const began = s.began || Date.parse(m.createdAt || m._began || '') || Date.now();
    const stepRows = work.rows.filter(r => r.type === 'step');
    const seen = stepRows.map(r => r.phase);
    const service = [...work.steps].reverse().find(x => x.connection);
    const rail = service ? stagesOf(service).rail : [];
    let current = draft ? 'Build' : waiting[0] ? stepRows.find(r => r.e === waiting[0])?.phase : seen.at(-1);
    if (answering && rail.includes('Results') && current === stagesOf(service).check) current = 'Results';
    // Workspace work goes through the usual phases; a cluster or service through its stages.
    const usual = draft || work.steps.some(x => !x.connection && !BROWSING(x) && x.tool !== 'delegate') ? PHASES.filter(p => p !== 'Plan' || seen.includes('Plan')) : [];
    const phases = [...new Set([...usual, ...rail, ...seen])];
    const count = p => seen.filter(x => x === p).length;
    const railHtml = phases.map((p, at) => {
      const cur = phases.indexOf(current);
      const status = p === current ? (waiting[0] ? 'ask' : 'now') : seen.includes(p) || cur > at ? 'done' : 'next';
      const note = status === 'ask' ? 'your call' : status === 'now' ? (draft ? 'writing' : p === 'Results' ? 'answering' : running ? 'working' : 'thinking') : count(p) ? plural(count(p), 'step') : '';
      return `<li class="run-phase is-${status}"><span class="run-bar" aria-hidden="true"></span><span class="run-phase-label"><span>${esc(p)}</span><small>${esc(note)}</small></span></li>`;
    }).join('');
    // The pane: what is happening now, always the same height.
    const graph = running?.tool === 'delegate' && running.agents?.length ? workerGraph(running, m) : '';
    const output = [...work.steps].reverse().find(x => x.status !== 'running' && x.status !== 'awaiting' && x.detail && !FILE_TOOLS.has(x.tool) && x.tool !== 'delegate');
    let label = '', note = '', body = '';
    if (draft) {
      label = draft.path || 'file'; note = `${plural(draft.lines, 'line')} so far`;
      body = `<pre class="run-code">${esc(draft.tail)}<span class="run-caret">▍</span></pre>`;
    } else if (running && (COMMANDS.has(running.tool) || running.connection || BROWSING(running))) {
      label = running.summary || running.tool; note = 'running';
      body = `<pre class="run-code is-output">${output && output !== running ? `${esc(lastLines(output.detail, 6))}\n` : ''}<span class="run-prompt">›</span> ${esc(running.summary || running.tool)} <span class="run-caret">▍</span></pre>`;
    } else if (fresh && !answering) {
      label = 'Thinking'; note = '';
      body = `<div class="run-think">${esc(latest(fresh, 420))}<span class="run-caret">▍</span></div>`;
    } else if (output) {
      label = output.summary || output.tool; note = output.status === 'error' ? 'failed' : 'output';
      body = `<pre class="run-code is-output">${esc(lastLines(output.detail, 9))}</pre>`;
    } else {
      label = answering ? 'Answer' : 'Starting'; note = '';
      body = `<div class="run-think">${answering ? 'Writing the answer…' : 'Waiting for the model…'}</div>`;
    }
    const pane = waiting[0] ? askCard(waiting[0]) : graph || `<div class="run-pane"><div class="run-pane-head"><span class="run-pane-label mono">${esc(label)}</span>${note ? `<span>${esc(note)}</span>` : ''}<span class="run-live"><span aria-hidden="true"></span>live</span></div><div class="run-pane-body">${body}</div></div>`;
    const thoughtText = lastThought ? work.reasoning.slice(lastThought.from, lastThought.to) : '';
    const ticker = fresh && label !== 'Thinking' ? latest(fresh, 200) : thoughtText ? latest(thoughtText, 200) : '';
    const recent = work.steps.filter(x => x.status !== 'awaiting').slice(-3);
    while (recent.length < 3) recent.push(null);
    const isOpen = open.has(m.id || 'live');
    const notes = (queued.get(conversation()) || []).filter(n => !work.entries.some(e => kindOf(e) === 'note' && e.summary === n));
    return `<section class="run-card" aria-label="Working"><div class="run-head"><span class="run-pulse${waiting[0] ? ' is-ask' : ''}" aria-hidden="true"></span><div class="grow"><strong>${esc(now)}</strong><small><span data-since="${new Date(began).toISOString()}">${esc(clock(Date.now() - began))}</span> · ${plural(s.steps, 'step')}${s.files.length ? ` · ${plural(s.files.length, 'file')} changed` : ''}</small></div>${target(lastStep, draft)}</div>${phases.length > 1 ? `<ol class="run-rail" aria-label="Stages">${railHtml}</ol>` : ''}${pane}<div class="run-thought">${icon('orbit')}<span class="run-thought-text${ticker ? '' : ' is-empty'}">${esc(ticker || 'No thinking yet')}</span></div><div class="run-section"><div class="spread"><span class="eyebrow">Recent steps</span><button type="button" class="tr-link" data-action="trace-toggle" data-id="${esc(m.id || 'live')}" aria-expanded="${isOpen}">${isOpen ? 'Hide steps' : `Show all ${s.steps}`}</button></div><ol class="run-steps">${recent.map(recentRow).join('')}</ol></div>${notes.map(n => `<p class="run-queued">${icon('clock')}<span><span class="run-queued-label">Queued for the next step</span>${esc(n)}</span></p>`).join('')}</section>${isOpen ? trace(m, work) : ''}`;
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
    return `<section class="result-card" aria-label="What changed"><div class="result-top"><div class="grow"><span class="eyebrow">${stopped ? 'Changed so far' : 'Done'}${s.span ? ` · ${esc(duration(s.span))}` : ''}</span><strong>${s.files.length} file${s.files.length === 1 ? '' : 's'} changed <span class="tr-add">+${s.added}</span>${s.removed ? ` <span class="tr-del">−${s.removed}</span>` : ''}</strong>${checks ? `<ul class="result-checks">${checks}</ul>` : ''}</div><div class="result-actions">${s.served && previews() ? `<button type="button" class="btn small primary" data-action="turn-open-preview">${icon('play')}Open preview</button>` : ''}<button type="button" class="btn small" data-action="turn-open-files">${icon('file')}Files</button>${id ? `<button type="button" class="btn small ghost" data-action="turn-download" title="Download project (.zip, without dependencies)">${icon('download')}Download</button>` : ''}</div></div><div class="result-files">${s.files.map(f => `<button type="button" class="result-file" data-action="turn-open-file" data-path="${esc(f.path)}"><span>${esc(f.path)}</span><span>${f.added ? `<span class="tr-add">+${f.added}</span>` : ''}${f.removed ? ` <span class="tr-del">−${f.removed}</span>` : ''}</span></button>`).join('')}</div></section>`;
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
    return JSON.stringify([live(m), work.map(e => [e.id, e.status, e.check, e.tick]), m._draft?.lines, m._draft?.id, live(m) ? Math.floor((m.reasoningContent || '').length / 120) : 0, open.has(id), filters.get(id), (queued.get(conversation()) || []).length, [...chosen]]);
  }

  const actions = {
    'trace-toggle': el => { const id = el.dataset.id; if (open.has(id)) open.delete(id); else open.add(id); return 'render'; },
    'trace-filter': el => { filters.set(el.dataset.id, el.dataset.filter); return 'render'; },
    'worker-pick': el => { chosen.set(el.dataset.step, Number(el.dataset.index)); return 'render'; },
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
    setInterval(() => {
      document.querySelectorAll('[data-since]').forEach(el => { const t = Date.parse(el.dataset.since); if (Number.isFinite(t)) el.textContent = clock(Date.now() - t); });
      document.querySelectorAll('[data-until]').forEach(el => { const t = Date.parse(el.dataset.until); if (Number.isFinite(t)) el.textContent = clock(t - Date.now()); });
    }, 1000);
  }
  return { applies, before, after, signature, actions, submit, unread, steerForm, live, setDraft: v => { steerDraft = v; } };
}
