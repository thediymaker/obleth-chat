import { esc, icon, badge, btn, empty } from './manage-ui.js';

// A project's Research tab: the goal, the experiment tree (each node a git branch in
// a chat's workspace), progress across runs, and the evidence behind each result.
// The chat drives the research with its tools; this page shows it and can stop runs.

const fmt = v => v == null || !Number.isFinite(v) ? '—' : Math.abs(v) >= 100 ? Number(v.toFixed(1)).toLocaleString() : String(Number(v.toPrecision(4)));
const minutes = (a, b) => { const ms = (b ? new Date(b) : new Date()) - new Date(a); return ms < 60000 ? `${Math.max(1, Math.round(ms / 1000))} s` : `${Math.round(ms / 60000)} min`; };

export function createResearch({ api, state, render, toast, startResearch, workspaces }) {
  const local = { project: null, loaded: null, data: null, error: '', selected: null, run: null, runs: new Map(), timer: null, sequence: 0 };
  const onTab = id => location.hash === `#projects/${encodeURIComponent(id)}/research`;

  async function load(project, quiet = false) {
    const sequence = ++local.sequence;
    if (local.project !== project) { Object.assign(local, { project, loaded: null, data: null, selected: null, run: null, error: '' }); local.runs.clear(); }
    try {
      const data = await api.request(`/api/projects/${encodeURIComponent(project)}/research`);
      if (sequence !== local.sequence) return;
      local.data = data.research ? data : null; local.error = ''; local.loaded = project;
      if (local.data && !local.data.experiments.some(e => e.id === local.selected)) local.selected = defaultNode();
      await loadRun();
    } catch (e) { if (sequence === local.sequence && e.name !== 'AbortError') local.error = e.message; }
    if (sequence !== local.sequence) return;
    clearTimeout(local.timer);
    if (local.data?.runs.some(r => r.status === 'running')) local.timer = setTimeout(() => { if (onTab(project)) load(project, true); }, 5000);
    if (!quiet || onTab(project)) render();
  }
  function stop() { clearTimeout(local.timer); local.timer = null; }

  // ---- Derived facts ----
  const better = (a, b) => local.data.research.direction === 'min' ? a < b : a > b;
  const measured = r => r.status === 'completed' && r.metric != null;
  const runsOf = id => local.data.runs.filter(r => r.experimentId === id);
  const childrenOf = id => local.data.experiments.filter(e => e.parentId === id);
  function bestRun(runs = local.data.runs) { return runs.filter(measured).reduce((b, r) => !b || better(r.metric, b.metric) ? r : b, null); }
  function defaultNode() {
    const best = bestRun(), last = local.data.runs.at(-1);
    return (best || last)?.experimentId || local.data.experiments[0]?.id || null;
  }
  async function loadRun() {
    const runs = local.selected ? runsOf(local.selected) : [];
    const pick = runs.find(r => r.id === local.run) || runs.find(r => r.status === 'running') || bestRun(runs) || runs.at(-1);
    local.run = pick?.id || null;
    if (!pick) return;
    const cached = local.runs.get(pick.id);
    if (cached && cached.status === pick.status && pick.status !== 'running') return;
    try { local.runs.set(pick.id, (await api.request(`/api/research/runs/${encodeURIComponent(pick.id)}`)).run); } catch { /* The summary still shows. */ }
  }
  function chip(e) {
    const runs = runsOf(e.id), last = runs.at(-1), best = bestRun();
    if (runs.some(r => r.status === 'running')) return badge('● running', 'accent');
    if (!e.parentId && e.frozen) return badge('baseline');
    if (e.frozen && childrenOf(e.id).length) return `<span class="pill green">★ promoted</span>`;
    if (e.frozen && best && best.experimentId === e.id) return `<span class="pill green">★ best</span>`;
    if (e.frozen) return badge('kept');
    if (last && ['failed', 'lost'].includes(last.status)) return badge(`${last.status} · open`, 'amber');
    if (last?.status === 'completed') return badge('no metric · open', 'amber');
    return badge(e.parentId ? 'open' : 'baseline · not run');
  }

  // ---- The page ----
  function goal() {
    const r = local.data.research, runs = local.data.runs, best = bestRun();
    const base = local.data.experiments.find(e => !e.parentId), baseBest = base && bestRun(runsOf(base.id));
    const gain = best && baseBest && baseBest.metric ? (best.metric - baseBest.metric) / Math.abs(baseBest.metric) * 100 : null;
    const going = runs.filter(x => x.status === 'running').length;
    const where = local.data.workspace ? `<a class="research-chip" href="#chat/${encodeURIComponent(local.data.workspace.conversationId)}">${icon('terminal')}Runs in “${esc(local.data.workspace.title)}”</a>` : '';
    return `<section class="card research-goal"><div class="grow"><span class="eyebrow">Goal</span><p class="research-goal-text">${esc(r.goal)}</p><div class="research-chips"><span class="research-chip">${icon('chart')}${esc(r.metric)} ${r.direction === 'min' ? '↓ lower is better' : '↑ higher is better'}</span><span class="research-chip mono" title="The same command on every node">${esc(r.runCommand)}</span>${where}</div></div><div class="research-stats"><div><span class="eyebrow">Best so far</span><strong>${best ? fmt(best.metric) : '—'}</strong><small>${best ? `run ${best.number}${gain != null && best.id !== baseBest?.id ? ` · ${gain >= 0 ? '+' : ''}${gain.toFixed(1)}% over baseline` : ''}` : 'No measured runs yet'}</small></div><div><span class="eyebrow">Runs</span><strong>${runs.length}</strong><small>${going ? `${going} running` : 'none running'}</small></div></div></section>`;
  }
  function tree() {
    const rows = [];
    const walk = (parent, depth) => {
      for (const e of local.data.experiments.filter(x => (x.parentId || null) === parent)) {
        const runs = runsOf(e.id), best = bestRun(runs), last = runs.at(-1);
        rows.push(`<button type="button" class="research-node ${local.selected === e.id ? 'selected' : ''}" style="--depth:${depth}" data-action="research-select" data-id="${esc(e.id)}" aria-pressed="${local.selected === e.id}"><span class="research-sha">${esc((last?.commit || '').slice(0, 7))}</span><span class="grow research-node-title">${esc(e.title)}${e.description ? `<small>${esc(e.description)}</small>` : ''}</span><span class="research-metric">${best ? fmt(best.metric) : last?.status === 'running' ? '…' : '—'}</span>${chip(e)}</button>`);
        walk(e.id, depth + 1);
      }
    };
    walk(null, 0);
    return `<section class="card project-section"><div class="spread"><span class="eyebrow">Experiment tree</span><span class="faint">Each node is a branch; every run is tied to its commit</span></div><div class="research-tree">${rows.join('')}</div></section>`;
  }
  function chart() {
    const runs = local.data.runs, points = runs.filter(measured);
    if (!points.length) return '';
    const W = 640, H = 190, x0 = 52, top = 14, bottom = 26, n = Math.max(runs.length, 4);
    const values = points.map(r => r.metric), lo = Math.min(...values), hi = Math.max(...values), pad = (hi - lo || Math.abs(hi) || 1) * 0.12;
    const min = lo - pad, max = hi + pad;
    const X = i => x0 + (i - 1) * (W - x0 - 16) / Math.max(n - 1, 1), Y = v => top + (max - v) / (max - min) * (H - top - bottom);
    const promoted = new Set(local.data.experiments.filter(e => e.frozen && childrenOf(e.id).length).map(e => e.id)), best = bestRun();
    let line = '', b = null;
    for (const r of runs) { if (measured(r) && (b == null || better(r.metric, b))) b = r.metric; if (b != null) line += `${X(r.number).toFixed(0)},${Y(b).toFixed(0)} `; }
    const ticks = [min + pad, (min + max) / 2, max - pad].map(v => `<line x1="${x0}" x2="${W - 8}" y1="${Y(v).toFixed(0)}" y2="${Y(v).toFixed(0)}" class="grid"/><text x="${x0 - 6}" y="${(Y(v) + 4).toFixed(0)}" text-anchor="end">${esc(fmt(v))}</text>`).join('');
    const marks = runs.map(r => {
      const x = X(r.number).toFixed(0);
      if (measured(r)) {
        const kept = promoted.has(r.experimentId) || r.id === best?.id;
        return `<circle cx="${x}" cy="${Y(r.metric).toFixed(0)}" r="${kept ? 5 : 4.5}" class="${kept ? 'kept' : 'other'}"><title>Run ${r.number}: ${esc(fmt(r.metric))}</title></circle>`;
      }
      return `<text x="${x}" y="${H - bottom - 4}" text-anchor="middle" class="${r.status === 'running' ? 'live' : 'bad'}">${r.status === 'running' ? '…' : '×'}<title>Run ${r.number}: ${esc(r.status)}</title></text>`;
    }).join('');
    const labels = runs.map(r => `<text x="${X(r.number).toFixed(0)}" y="${H - 6}" text-anchor="middle">${r.number}</text>`).join('');
    return `<section class="card project-section"><div class="spread"><span class="eyebrow">Progress</span><span class="research-legend"><span><i class="kept"></i>promoted or best</span><span><i class="other"></i>not kept</span><span>×&nbsp;answered nothing</span></span></div><svg class="research-chart" viewBox="0 0 ${W} ${H}" role="img" aria-label="${esc(local.data.research.metric)} by run; the best so far is ${esc(fmt(best?.metric))}">${ticks}<polyline points="${line.trim()}" class="best"/>${marks}${labels}</svg></section>`;
  }
  function diff(text) {
    return text ? `<pre class="research-diff">${text.split('\n').slice(0, 400).map(l => `<span class="${l.startsWith('+') && !l.startsWith('+++') ? 'add' : l.startsWith('-') && !l.startsWith('---') ? 'del' : l.startsWith('@@') ? 'hunk' : ''}">${esc(l)}</span>`).join('\n')}</pre>` : '';
  }
  // What the decisions check found: whether the run really tested its idea, or why it wasn't asked.
  function check(d) {
    if (d.skipped) return esc(`Skipped: ${d.skipped}`);
    const found = `${Math.round(d.tested * 100)}% that it did${d.model ? ` · ${d.model}` : ''}`;
    return d.tested >= 0.5 ? esc(`Tested its idea · ${found}`) : `${badge('May not have tested its idea', 'amber')} ${esc(found)}`;
  }
  function detail() {
    const e = local.data.experiments.find(x => x.id === local.selected);
    if (!e) return '';
    const runs = runsOf(e.id), summary = runs.find(r => r.id === local.run), run = summary && local.runs.get(summary.id);
    const metric = local.data.research.metric;
    const list = runs.length ? `<div class="research-runs">${runs.map(r => `<button type="button" class="research-run ${r.id === local.run ? 'on' : ''}" data-action="research-run" data-id="${esc(r.id)}" aria-pressed="${r.id === local.run}">Run ${r.number}<span>${r.status === 'completed' ? fmt(r.metric) : esc(r.status)}</span></button>`).join('')}</div>` : `<p class="context-note">${e.frozen ? '' : 'Not run yet.'}</p>`;
    let body = '';
    if (summary) {
      const values = Object.entries(summary.metrics || {}).sort(([a], [b]) => (b === metric) - (a === metric)).slice(0, 12);
      const facts = [[metric, summary.status === 'running' ? 'running…' : fmt(summary.metric)], ['Status', summary.status + (summary.exitCode != null && summary.exitCode !== 0 ? ` · exit ${summary.exitCode}` : '')], ['Commit', `<span class="mono">${esc(summary.commit)}</span>`], ['Time', `${minutes(summary.startedAt, summary.finishedAt)}${summary.finishedAt ? '' : ' so far'}`], ...(summary.diffStat ? [['Change', esc(summary.diffStat)]] : []), ...(summary.decision ? [['Check', check(summary.decision)]] : [])];
      body = `<div class="research-facts">${facts.map(([k, v]) => `<div class="detail-row"><span>${esc(k)}</span><span>${k === metric || k === 'Status' || k === 'Time' ? esc(v) : v}</span></div>`).join('')}</div>${values.length > 1 ? `<span class="eyebrow">Values it printed</span><div class="research-values">${values.map(([k, v]) => `<span><b>${esc(k)}</b>${esc(fmt(v))}</span>`).join('')}</div>` : ''}${run?.diff ? `<span class="eyebrow">Change from parent</span>${diff(run.diff)}` : ''}<span class="eyebrow">Output</span>${run ? `<pre class="research-log">${esc((run.log || '').split('\n').slice(-60).join('\n')) || 'Nothing printed yet.'}</pre>` : '<p class="inline-note">Loading…</p>'}${run?.canCancel ? btn('Stop this run', 'research-cancel', { icon: 'stop', tone: 'small danger', attrs: `data-id="${esc(summary.id)}"` }) : ''}`;
    }
    return `<section class="card project-section research-detail"><span class="eyebrow">${e.parentId ? 'Experiment' : 'Baseline'}</span><h3>${esc(e.title)}</h3><p class="faint mono">${esc(e.branch)} · ${e.frozen ? 'frozen' : 'open'}</p>${e.description ? `<p class="research-description">${esc(e.description)}</p>` : ''}${list}${body}</section>`;
  }
  function start(project) {
    const steps = [['Say the goal', 'In a chat in this project, with a workspace: “make decode faster without hurting p95”.'], ['Answer its questions', 'It pins down the metric, whether higher or lower is better, and the one command every run uses.'], ['Baseline', 'Your code as it is becomes the baseline and runs once.'], ['Rounds', 'A few options per decision run side by side; the winner becomes the parent of the next round.']];
    const can = project.mine && workspaces();
    return `<section class="card research-empty">${empty('No research yet', 'Research measures ideas against each other: every experiment is a branch of your code, run with the same command, and the best result carries into the next round.', can ? `${btn('Start research in a chat', 'research-start', { icon: 'flask', tone: 'primary', attrs: `data-id="${esc(project.id)}"` })}<p class="inline-note research-pending">Already started one? It shows here once the chat has agreed the metric and run command with you and set up the baseline. Until then, follow it in the chat.</p>` : `<p class="inline-note">${project.mine ? 'Research runs in a workspace, and workspaces are not enabled for your account.' : 'Only the project’s owner can start research in it.'}</p>`)}<div class="research-steps">${steps.map(([t, d], i) => `<div><span class="research-step">${i + 1}</span><strong>${t}</strong><p>${d}</p></div>`).join('')}</div></section>`;
  }
  function view(project) {
    if (local.project !== project.id || (local.loaded !== project.id && !local.error)) return '<p class="inline-note" role="status">Loading research…</p>';
    if (local.error) return `<div class="connection-notice" role="alert"><span class="grow">${esc(local.error)}</span>${btn('Retry', 'research-reload', { icon: 'refresh', tone: 'small', attrs: `data-id="${esc(project.id)}"` })}</div>`;
    if (!local.data) return start(project);
    return `${goal()}<div class="project-layout research-layout"><div class="project-main">${tree()}${chart()}</div><div class="project-side">${detail()}</div></div>`;
  }

  const actions = {
    'research-select': async el => { local.selected = el.dataset.id; local.run = null; await loadRun(); render(); },
    'research-run': async el => { local.run = el.dataset.id; await loadRun(); render(); },
    'research-reload': el => load(el.dataset.id),
    'research-start': el => startResearch(el.dataset.id),
    'research-cancel': async el => {
      await api.request(`/api/research/runs/${encodeURIComponent(el.dataset.id)}/cancel`, { method: 'POST' });
      local.runs.delete(el.dataset.id); toast('Run stopped'); await load(local.project);
    },
  };
  function clear() { stop(); local.sequence++; Object.assign(local, { project: null, loaded: null, data: null, error: '', selected: null, run: null }); local.runs.clear(); }
  return { load, view, actions, clear, stop };
}
