import { icon, esc, badge, btn, iconButton, errorBox, field, textInput, getValue, isChecked } from './manage-ui.js';

// Code in projects. Each project chat's workspace is a git repository; Sync saves its
// changes as a commit, brings in what others synced, and shares the result as the
// project's main line, which Chat keeps. The workspace's Git tab shows the chat's
// changes and settles conflicts. The project's Code tab shows the history, who changed
// each line and in which chat, and takes the code out as a zip or to GitHub.

const ago = value => {
  if (value == null) return '';
  const t = typeof value === 'number' ? value * 1000 : new Date(value).getTime(), m = Math.round((Date.now() - t) / 60000);
  return m < 1 ? 'just now' : m < 60 ? `${m} min ago` : m < 1440 ? `${Math.round(m / 60)} h ago` : m < 2880 ? 'yesterday' : `${Math.round(m / 1440)} d ago`;
};
const count = (n, one, many = `${one}s`) => `${Number(n || 0).toLocaleString()} ${n === 1 ? one : many}`;
const short = sha => String(sha || '').slice(0, 7);
const initial = name => esc(String(name || '?').trim().slice(0, 1).toUpperCase() || '?');
const LETTER = { added: 'A', modified: 'M', deleted: 'D', conflict: '!' };
const enc = encodeURIComponent;

/** A unified diff as rows, numbered by the new file's lines. */
export function diffRows(text, limit = 4000) {
  const rows = []; let line = 0;
  for (const raw of String(text || '').split('\n')) {
    if (rows.length >= limit) { rows.push({ kind: 'more', text: 'The rest of this diff is not shown.' }); break; }
    if (/^(diff --git|diff --cc|index |--- |\+\+\+ |new file mode|deleted file mode|similarity |rename |old mode|new mode)/.test(raw)) continue;
    const hunk = raw.match(/^@@+ [^@]*\+(\d+)(?:,\d+)? @@+ ?(.*)$/);
    if (hunk) { line = Number(hunk[1]); rows.push({ kind: 'hunk', text: hunk[2] ? `line ${hunk[1]} · ${hunk[2]}` : `line ${hunk[1]}` }); continue; }
    if (raw.startsWith('Binary files')) { rows.push({ kind: 'note', text: 'A binary file changed.' }); continue; }
    if (raw.startsWith('\\')) continue;
    if (raw.startsWith('+')) rows.push({ kind: 'add', n: line++, text: raw.slice(1) });
    else if (raw.startsWith('-')) rows.push({ kind: 'del', n: '', text: raw.slice(1) });
    else if (raw.startsWith(' ') || (raw === '' && rows.length && rows[rows.length - 1].kind !== 'hunk' && line)) rows.push({ kind: 'ctx', n: line++, text: raw.slice(1) });
  }
  while (rows.length && rows[rows.length - 1].kind === 'ctx' && rows[rows.length - 1].text === '') rows.pop();
  return rows;
}
function diffView(text) {
  const rows = diffRows(text);
  if (!rows.length) return '<p class="inline-note">No line changes.</p>';
  return `<div class="git-diff" role="table">${rows.map(r => r.kind === 'hunk' ? `<div class="git-hunk">@@ ${esc(r.text)}</div>` : r.kind === 'more' || r.kind === 'note' ? `<div class="git-hunk">${esc(r.text)}</div>` : `<div class="git-line ${r.kind}"><span class="git-n">${esc(r.n)}</span><span class="git-sign">${r.kind === 'add' ? '+' : r.kind === 'del' ? '−' : ''}</span><code>${esc(r.text) || ' '}</code></div>`).join('')}</div>`;
}

/** The two sides of each conflict in a file with conflict markers. */
export function conflictRegions(text, limit = 3) {
  const regions = []; let current = null, side = null, n = 0;
  for (const line of String(text || '').split('\n')) {
    n++;
    if (line.startsWith('<<<<<<<')) { current = { line: n, mine: [], theirs: [] }; side = 'mine'; continue; }
    if (current && line.startsWith('|||||||')) { side = 'base'; continue; }
    if (current && line.startsWith('=======')) { side = 'theirs'; continue; }
    if (current && line.startsWith('>>>>>>>')) { regions.push(current); current = null; if (regions.length >= limit) break; continue; }
    if (current && side !== 'base') current[side].push(line);
  }
  return regions;
}


export function createCode({ api, state, capabilities, render, modal, close, toast, chatPath, send, terminal, moveToProject }) {
  const chats = new Map(), project = { id: null, overview: null, log: null, commit: null, files: null, file: null, versions: null, error: '', loading: 0 };
  let paneUpdate = () => {};
  const enabled = () => !!capabilities.features?.code;
  const entry = id => { if (!chats.has(id)) chats.set(id, { data: null, loading: false, at: 0, error: '', view: null, message: null, busy: '', regions: {} }); return chats.get(id); };
  const projectPath = id => `/api/projects/${enc(id)}/code`;

  // ---- A chat's Git tab ----
  async function load(id, { quiet = false } = {}) {
    if (!enabled() || !id) return;
    const c = entry(id);
    if (c.loading) return;
    c.loading = true; if (!quiet) paneUpdate();
    try {
      c.data = await api.request(`${chatPath(id)}/code`); c.error = ''; c.at = Date.now();
      if (c.data.merging) loadRegions(id);
    } catch (e) { c.error = e.message; }
    finally { c.loading = false; paneUpdate(); }
  }
  /** Loads the chat's Git state when it is older than `age` ms. */
  function refresh(id, age = 15000) { const c = entry(id); if (!c.loading && Date.now() - c.at > age) load(id, { quiet: !!c.data }); }
  let soon = null;
  /** The model changed files: the Git tab's count follows a moment later. */
  function changed(id) { if (!enabled() || !chats.get(id)?.data?.project) return; clearTimeout(soon); soon = setTimeout(() => load(id, { quiet: true }), 2500); }
  async function loadRegions(id) {
    const c = entry(id);
    for (const path of c.data?.conflicts || []) {
      if (c.regions[path]) continue;
      c.regions[path] = { loading: true };
      try { const f = await api.request(`${chatPath(id)}/workspace/file?path=${enc(path)}`); c.regions[path] = { list: conflictRegions(f.content) }; }
      catch { c.regions[path] = { list: [] }; }
      paneUpdate();
    }
  }
  /** The Git tab's label, with how many files changed. */
  function label(id) {
    const d = chats.get(id)?.data, n = d?.merging ? d.conflicts?.length : d?.changes?.length;
    return `Git${n ? ` <span class="tab-count ${d.merging ? 'amber' : ''}">${n}</span>` : ''}`;
  }
  const note = (glyph, title, text, actions = '') => `<div class="workspace-note">${icon(glyph)}<strong>${esc(title)}</strong><p>${text}</p>${actions ? `<div class="row">${actions}</div>` : ''}</div>`;

  function view(id) {
    const c = entry(id), d = c.data;
    if (!d) return c.error ? note('info', 'Git could not load', esc(c.error), btn('Retry', 'code-reload', { icon: 'refresh', tone: 'small' })) : '<p class="workspace-loading">Loading…</p>';
    if (!d.project) return note('branch', 'Git works in project chats', 'Move this chat into a project. Sync then shares its code with the project’s other chats and brings theirs in, with every change kept in the project’s history.', btn('Move to project', 'code-move', { icon: 'folder', tone: 'small' }));
    if (!d.running) return note('clock', d.workspace ? 'The workspace is asleep' : 'No workspace yet', d.workspace ? 'Its files are kept. Sync wakes it, saves its changes and brings in the project’s latest code.' : 'Turn on Workspace and ask for something to build. Its code can then be synced with the project.', d.workspace ? btn(c.busy === 'sync' ? 'Syncing…' : 'Sync', 'code-sync', { icon: 'refresh', tone: 'small primary', attrs: c.busy ? 'disabled' : '' }) : '');
    const head = `<div class="git-head">${icon('branch')}<a class="grow" href="#projects/${enc(d.project.id)}/code">${esc(d.project.name)}</a><span class="faint">main${d.main ? ` · ${count(d.main.commits, 'commit')}` : ''}</span>${iconButton('refresh', 'code-reload', 'Check again', c.loading ? 'disabled' : '')}</div>`;
    if (d.merging) return `${head}${conflictView(id, c, d)}`;
    if (c.view) return `${head}${fileView(c)}`;
    return `${head}${errorBox(c.error)}${mainView(id, c, d)}`;
  }
  function mainView(id, c, d) {
    const behind = d.behind ? `<p class="git-notice">${icon('download')}${count(d.behind, 'new commit')} in the project since this chat last synced.</p>` : !d.main ? `<p class="git-notice">${icon('info')}Nobody has synced yet. The first Sync starts the project’s code.</p>` : '';
    const changes = d.changes || [];
    const list = changes.length ? `<ul class="git-changes">${changes.map(f => `<li><button type="button" data-action="code-diff" data-path="${esc(f.path)}"><span class="git-letter ${esc(f.status)}" title="${esc(f.status)}">${LETTER[f.status] || 'M'}</span><span class="grow git-path">${esc(f.path)}</span><span class="git-stat">${f.added ? `<b>+${f.added}</b>` : ''}${f.removed ? ` <i>−${f.removed}</i>` : ''}</span></button></li>`).join('')}</ul>` : `<p class="inline-note git-pad">No changes since the last save${d.ahead ? `. ${count(d.ahead, 'saved commit')} not synced yet.` : '.'}</p>`;
    const message = c.message ?? (changes.length ? d.draft : '');
    const say = d.behind && (changes.length || d.ahead) ? `Sync saves your changes, brings in the project’s ${count(d.behind, 'new commit')}, and shares yours with everyone.` : d.behind ? `Sync brings in the project’s ${count(d.behind, 'new commit')}.` : changes.length || d.ahead ? 'Sync saves your changes and shares them with everyone in the project.' : 'Everything here is in the project.';
    const busy = !!c.busy;
    const form = changes.length || d.ahead || d.behind || !d.main ? `<div class="git-commit">${changes.length ? `<label class="form-label" for="code-message">What changed</label><textarea id="code-message" class="form-control" rows="2" maxlength="2000" data-code-message="${esc(id)}" placeholder="${esc(d.draft)}">${esc(message)}</textarea><small class="faint">${c.message == null ? 'Drafted from the changes. Edit it freely.' : 'Kept with the commit, along with a link to this chat.'}</small>` : ''}<div class="row">${btn(c.busy === 'sync' ? 'Syncing…' : 'Sync', 'code-sync', { icon: 'refresh', tone: 'primary small', attrs: busy ? 'disabled' : '' })}${changes.length ? btn(c.busy === 'save' ? 'Saving…' : 'Save without syncing', 'code-save', { tone: 'ghost small', attrs: busy ? 'disabled' : '' }) : ''}</div><small class="faint">${esc(say)}</small></div>` : `<p class="inline-note git-pad">${esc(say)}</p>`;
    const log = (d.log || []).slice(0, 8);
    const history = log.length ? `<div class="git-section"><div class="spread"><span class="eyebrow">History</span><a class="tr-link" href="#projects/${enc(d.project.id)}/code/history">All${icon('right')}</a></div><ol class="git-log">${log.map((e, i) => `<li><span class="git-dot ${i < d.ahead ? 'local' : ''}"></span><span class="grow"><span class="git-subject">${esc(e.subject)}</span><small>${esc(e.author)} · ${esc(ago(e.time))}${i < d.ahead ? ' · not synced' : ''}</small></span><code>${esc(short(e.sha))}</code></li>`).join('')}</ol></div>` : '';
    return `${behind}<div class="git-section"><div class="spread"><span class="eyebrow">Changes${changes.length ? ` · ${count(changes.length, 'file')}` : ''}</span>${changes.length ? `<button type="button" class="tr-link" data-action="code-undo-all" ${busy ? 'disabled' : ''}>Undo all</button>` : ''}</div>${list}</div>${form}${history}<div class="git-foot"><span class="faint">Kept by Chat while the project runs</span><span class="row">${btn('Export…', 'code-export', { tone: 'small ghost', attrs: `data-project="${esc(d.project.id)}" data-name="${esc(d.project.name)}"` })}${btn('Open in terminal', 'code-terminal', { icon: 'terminal', tone: 'small ghost' })}</span></div>`;
  }
  function fileView(c) {
    const v = c.view, f = (c.data.changes || []).find(x => x.path === v.path);
    const head = `<div class="workspace-file-head"><span class="flip">${iconButton('right', 'code-back', 'Back to changes')}</span><code class="grow">${esc(v.path)}</code>${f ? `<span class="git-stat">${f.added ? `<b>+${f.added}</b>` : ''}${f.removed ? ` <i>−${f.removed}</i>` : ''}</span>` : ''}${btn('Undo this file', 'code-undo', { tone: 'small ghost', attrs: `data-path="${esc(v.path)}" ${c.busy ? 'disabled' : ''}` })}</div>`;
    return `${head}${v.diff == null ? '<p class="workspace-loading">Loading…</p>' : v.error ? `<p class="inline-note git-pad">${esc(v.error)}</p>` : `${diffView(v.diff)}${v.truncated ? '<p class="inline-note git-pad">Showing the first 200 KB.</p>' : ''}`}`;
  }
  function conflictView(id, c, d) {
    const files = d.conflicts || [], combined = (d.changes || []).filter(f => f.status !== 'conflict');
    const regions = path => {
      const r = c.regions[path];
      if (!r || r.loading) return '<p class="workspace-loading">Loading…</p>';
      return r.list.map(x => `<div class="git-sides"><div><span class="eyebrow">Yours · line ${x.line}</span><pre>${esc(x.mine.join('\n')) || '<span class="faint">(nothing)</span>'}</pre></div><div><span class="eyebrow">The project’s</span><pre>${esc(x.theirs.join('\n')) || '<span class="faint">(nothing)</span>'}</pre></div></div>`).join('') || '<p class="inline-note">Changed or removed on both sides.</p>';
    };
    const busy = c.busy ? 'disabled' : '';
    return `<div class="git-paused"><strong>${icon('info')}Sync paused</strong><p>Your changes are saved in this workspace. Someone synced changes to the same lines, so ${files.length ? count(files.length, 'file needs', 'files need') : 'nothing more needs'} a decision before they can be combined.</p><p class="faint">Nothing is lost whichever way you go. Stop syncing puts every file back the way it was before Sync.</p></div>
      <div class="git-section"><div class="row git-wrap">${files.length ? btn('Resolve with the model', 'code-resolve', { icon: 'bolt', tone: 'small primary', attrs: busy }) : ''}${btn(c.busy === 'finish' ? 'Finishing…' : 'Finish syncing', 'code-finish', { icon: 'check', tone: `small ${files.length ? '' : 'primary'}`, attrs: busy })}${btn('Open in terminal', 'code-terminal', { icon: 'terminal', tone: 'small ghost' })}${btn('Stop syncing', 'code-stop', { tone: 'small ghost', attrs: busy })}</div>${files.length ? '<small class="faint">Finish once each file is combined: by the model, by hand, or with Keep mine or Keep the project’s.</small>' : '<small class="faint">Every file is settled. Finish syncing to share the result.</small>'}</div>
      ${files.map(path => `<div class="git-conflict"><div class="spread"><code class="grow">${esc(path)}</code><span class="row">${btn('Keep mine', 'code-choose', { tone: 'small', attrs: `data-path="${esc(path)}" data-side="ours" ${busy}` })}${btn('Keep the project’s', 'code-choose', { tone: 'small', attrs: `data-path="${esc(path)}" data-side="theirs" ${busy}` })}</span></div>${regions(path)}</div>`).join('')}
      ${combined.length ? `<p class="inline-note git-pad">${icon('check')}Combined on their own: ${combined.map(f => esc(f.path)).join(', ')}</p>` : ''}`;
  }

  async function run(id, busy, request, done) {
    const c = entry(id); if (c.busy) return;
    c.busy = busy; paneUpdate();
    try { const result = await request(); c.busy = ''; await done?.(result); }
    catch (e) { c.busy = ''; toast(e.message); }
    finally { c.busy = ''; await load(id, { quiet: true }); }
  }
  const currentChat = () => state.active?.id;
  const message = id => { const c = entry(id); return c.message ?? undefined; };
  const actions = {
    'code-reload': () => { const id = currentChat(); if (id) { entry(id).regions = {}; load(id); } },
    'code-move': () => moveToProject(),
    'code-sync': () => { const id = currentChat(); if (!id) return; run(id, 'sync', () => api.request(`${chatPath(id)}/code/sync`, { method: 'POST', body: { message: message(id) } }), r => {
      const c = entry(id); c.message = null; c.view = null;
      if (r.state === 'conflict') { c.regions = {}; toast('Sync paused: some files changed in both places'); }
      else toast(r.shared ? 'Synced. Everyone in the project has your changes.' : 'Up to date with the project');
    }); },
    'code-save': () => { const id = currentChat(); if (!id) return; run(id, 'save', () => api.request(`${chatPath(id)}/code/save`, { method: 'POST', body: { message: message(id) } }), r => { entry(id).message = null; toast(r.saved ? 'Saved in this chat. Sync shares it.' : 'Nothing to save'); }); },
    'code-diff': async el => {
      const id = currentChat(), c = entry(id), path = el.dataset.path;
      c.view = { path, diff: null }; paneUpdate();
      try { const r = await api.request(`${chatPath(id)}/code/diff?path=${enc(path)}`); if (c.view?.path === path) Object.assign(c.view, { diff: r.diff, truncated: r.truncated }); }
      catch (e) { if (c.view?.path === path) Object.assign(c.view, { diff: '', error: e.message }); }
      paneUpdate();
    },
    'code-back': () => { const id = currentChat(); if (id) { entry(id).view = null; paneUpdate(); } },
    'code-undo': el => modal('Undo the changes to this file?', `<p><code>${esc(el.dataset.path)}</code> goes back to how it was at the last save. A new file is removed.</p><div class="modal-actions">${btn('Keep it', 'close')}${btn('Undo changes', 'code-undo-confirm', { tone: 'danger', attrs: `data-path="${esc(el.dataset.path)}"` })}</div>`),
    'code-undo-all': () => modal('Undo every change?', '<p>Every file goes back to how it was at the last save, and new files are removed. Installed packages and build output stay.</p><div class="modal-actions">' + btn('Keep them', 'close') + btn('Undo all', 'code-undo-confirm', { tone: 'danger' }) + '</div>'),
    'code-undo-confirm': el => { close(); const id = currentChat(); if (!id) return; const path = el.dataset.path || null; run(id, 'undo', () => api.request(`${chatPath(id)}/code/undo`, { method: 'POST', body: { path } }), () => { const c = entry(id); c.view = null; c.message = null; toast(path ? 'Changes undone' : 'All changes undone'); }); },
    'code-choose': el => { const id = currentChat(); if (!id) return; run(id, 'choose', () => api.request(`${chatPath(id)}/code/choose`, { method: 'POST', body: { path: el.dataset.path, side: el.dataset.side } }), () => { delete entry(id).regions[el.dataset.path]; }); },
    'code-finish': () => { const id = currentChat(); if (!id) return; run(id, 'finish', () => api.request(`${chatPath(id)}/code/finish`, { method: 'POST' }), r => { if (r.state === 'conflict') { entry(id).regions = {}; toast('More changes came in that need a decision'); } else toast('Synced. Everyone in the project has the combined code.'); }); },
    'code-stop': () => modal('Stop syncing?', '<p>Every file goes back to how it was before Sync, with your changes saved. Nothing is shared, and the next Sync tries again.</p><div class="modal-actions">' + btn('Keep going', 'close') + btn('Stop syncing', 'code-stop-confirm', { tone: 'danger' }) + '</div>'),
    'code-stop-confirm': () => { close(); const id = currentChat(); if (!id) return; run(id, 'stop', () => api.request(`${chatPath(id)}/code/stop`, { method: 'POST' }), () => { entry(id).regions = {}; toast('Sync stopped. Your changes are as they were.'); }); },
    'code-resolve': () => {
      const id = currentChat(), files = entry(id).data?.conflicts || [];
      if (!files.length) return;
      send(`Sync paused because ${files.length === 1 ? `\`${files[0]}\` was` : `${files.map(f => `\`${f}\``).join(', ')} were`} changed both in this chat and in the project. In each file, combine the two versions between the conflict markers (<<<<<<<, =======, >>>>>>>) so that both changes still work, and remove the markers. Don't run git commands. Then tell me briefly what you kept from each side, and I'll finish the sync.`);
    },
    'code-terminal': () => terminal(currentChat(), 'git status'),
    'code-export': el => exportDialog(el.dataset.project, el.dataset.name),
    'code-zip': el => {
      const form = el.closest('.modal-body'), history = form?.querySelector('#code-zip-history')?.checked, withChats = form?.querySelector('#code-zip-chats')?.checked;
      const a = document.createElement('a'); a.href = `${projectPath(el.dataset.project)}/export.zip?history=${!!history}&chats=${!!withChats}`; a.download = ''; document.body.append(a); a.click(); a.remove();
    },
    'code-commit-file': el => togglePatch(el.dataset.path),
    'code-lines': el => loadVersions(el.dataset.path, Number(el.dataset.start), Number(el.dataset.end)),
    'code-lines-close': () => { project.versions = null; render(); },
  };
  function input(event) {
    const id = event.target.dataset?.codeMessage;
    if (!id) return false;
    entry(id).message = event.target.value; return true;
  }
  /** Renders the Git tab into `el`, keeping the message box's focus across renders. */
  let last = '';
  function show(el, id) {
    if (!chats.get(id)?.data && !entry(id).loading && !entry(id).error) load(id);
    else refresh(id);
    const html = view(id);
    if (html === last && el.innerHTML) return;
    const focused = document.activeElement?.dataset?.codeMessage === id ? [document.activeElement.selectionStart, document.activeElement.selectionEnd] : null;
    el.innerHTML = html; last = html;
    if (focused) { const box = el.querySelector('[data-code-message]'); if (box) { box.focus(); box.setSelectionRange(...focused); } }
  }

  // ---- Export ----
  const slug = name => String(name || 'project').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 80) || 'project';
  function exportDialog(id, name) {
    const o = project.id === id ? project.overview : null, last = o?.exports?.find(e => e.kind === 'github');
    const [owner, repo] = last ? last.target.split('/') : ['', slug(name)];
    modal('Take the code with you', `<p class="inline-note">The code, its history, and if you like the chats that made it. Export as often as you want; each push to the same repository updates it.</p>
      <section class="export-part"><h3>${icon('download')}Download a zip</h3><p class="faint">The code as it is on main, for a class hand-in, an archive, or another host.</p>
        <label class="check-option"><input type="checkbox" id="code-zip-history" checked><span><strong>Its history</strong><small>As a git bundle: <code>git clone ${esc(slug(name))}.bundle</code> gets every commit back.</small></span></label>
        <label class="check-option"><input type="checkbox" id="code-zip-chats"><span><strong>Your chats in the project</strong><small>As Markdown in <code>chats/</code>, without tool steps.</small></span></label>
        <div class="row">${btn('Download zip', 'code-zip', { icon: 'download', tone: 'small', attrs: `data-project="${esc(id)}"` })}</div></section>
      <form id="code-github-form" class="export-part" data-project="${esc(id)}"><h3>${icon('upload')}Push to GitHub</h3><p class="faint">Every commit goes along. The repository is created if it doesn’t exist yet.</p>
        <div class="export-where">${field('code-gh-owner', 'Owner', textInput('code-gh-owner', owner, 'placeholder="Your account" maxlength="39" autocomplete="off" spellcheck="false"'))}<span class="slash">/</span>${field('code-gh-name', 'Repository', textInput('code-gh-name', repo, 'required maxlength="100" autocomplete="off" spellcheck="false"'))}</div>
        <div class="management-field"><span class="form-label">Visibility</span><div class="row"><label class="check-option inline"><input type="radio" name="code-gh-private" value="1" checked><span><strong>Private</strong></span></label><label class="check-option inline"><input type="radio" name="code-gh-private" value=""><span><strong>Public</strong></span></label></div></div>
        ${field('code-gh-token', 'GitHub token', textInput('code-gh-token', '', 'type="password" required autocomplete="off" spellcheck="false" placeholder="github_pat_…"'), 'Used for this push only and never kept. A fine-grained token needs Contents: read and write, plus Administration: read and write to create the repository.')}
        <div class="form-error" role="alert"></div><div class="modal-actions"><button class="btn primary" type="submit">${icon('upload')}Push to GitHub</button></div></form>
      <section class="export-part"><h3>${icon('globe')}Put a site online</h3><p class="faint">Push to GitHub first. Then import the repository in Vercel or Netlify, or turn on GitHub Pages in its settings. Each later push redeploys it.</p></section>`);
  }
  async function submit(form) {
    const id = form.dataset.project, error = form.querySelector('.form-error');
    const body = { owner: getValue(form, 'code-gh-owner').trim() || undefined, name: getValue(form, 'code-gh-name').trim(), private: !!form.querySelector('input[name="code-gh-private"]:checked')?.value, token: getValue(form, 'code-gh-token').trim() };
    form.querySelector('[type=submit]').textContent = 'Pushing…';
    try {
      const r = await api.request(`${projectPath(id)}/github`, { method: 'POST', body });
      modal('Pushed to GitHub', `<p>The code and its history are at <a href="${esc(r.url)}" target="_blank" rel="noopener">${esc(r.target)}</a>. Push again from the project any time to update it.</p><p class="faint">To put a site online, import that repository in Vercel or Netlify, or turn on GitHub Pages in its settings.</p><div class="modal-actions"><a class="btn" href="${esc(r.url)}" target="_blank" rel="noopener">${icon('external')}Open on GitHub</a>${btn('Done', 'close', { tone: 'primary' })}</div>`);
      if (project.id === id) { project.overview = null; loadProject(id); }
    } catch (e) {
      form.querySelector('[type=submit]').innerHTML = `${icon('upload')}Push to GitHub`;
      if (error) error.textContent = e.message; else throw e;
    }
  }

  // ---- The project's Code tab ----
  const parts = () => location.hash.slice(1).split('/');
  const sub = () => ['history', 'files'].includes(parts()[3]) ? parts()[3] : 'overview';
  const param = () => { try { return decodeURIComponent(parts().slice(4).join('/')); } catch { return ''; } };
  async function get(id, url, key) {
    project.loading++;
    try { const r = await api.request(url); if (project.id === id) return r; }
    catch (e) { if (project.id === id) project.error = e.message; }
    finally { project.loading--; }
    return null;
  }
  async function loadProject(id) {
    if (!enabled()) return;
    if (project.id !== id) Object.assign(project, { id, overview: null, log: null, commit: null, files: null, file: null, versions: null, error: '' });
    project.error = '';
    const here = sub(), arg = param(), jobs = [];
    if (!project.overview) jobs.push(get(id, projectPath(id), 'overview').then(r => { if (r) project.overview = r; }));
    if ((here === 'overview' || here === 'history') && !project.log) jobs.push(get(id, `${projectPath(id)}/log?limit=200`).then(r => { if (r) project.log = r.commits; }));
    if (here === 'history') {
      const sha = arg || null;
      if (sha && project.commit?.sha !== sha) { project.commit = { sha, data: null }; jobs.push(get(id, `${projectPath(id)}/commits/${enc(sha)}`).then(r => { if (r && project.commit?.sha === sha) { project.commit.data = r; if (r.files.length && r.files.length <= 3) r.files.forEach(f => togglePatch(f.path)); } })); }
      if (!sha) project.commit = null;
    }
    if (here === 'files') {
      if (!project.files) jobs.push(get(id, `${projectPath(id)}/files`).then(r => { if (r) project.files = r.files; }));
      const path = arg || null;
      if (path && project.file?.path !== path) {
        project.file = { path, info: null, blame: null }; project.versions = null;
        jobs.push(get(id, `${projectPath(id)}/file?path=${enc(path)}`).then(async info => {
          if (!info || project.file?.path !== path) return;
          project.file.info = info;
          if (!info.binary && !info.tooLarge) { const b = await get(id, `${projectPath(id)}/blame?path=${enc(path)}`); if (b && project.file?.path === path) project.file.blame = b.groups; }
          render();
        }));
      }
      if (!path) { project.file = null; project.versions = null; }
    }
    render();
    await Promise.all(jobs);
    render();
  }
  async function loadVersions(path, start, end) {
    const id = project.id; project.versions = { path, start, end, list: null }; render();
    const r = await get(id, `${projectPath(id)}/lines?path=${enc(path)}&start=${start}&end=${end}`);
    if (project.versions?.start === start && project.versions.path === path) { project.versions.list = r?.versions || []; render(); }
  }
  const base = p => `#projects/${enc(p.id)}/code`;
  const chatLink = (chat, turn) => !chat ? '' : chat.readable ? `<a href="#chat/${enc(chat.id)}${turn ? '' : ''}">${esc(chat.title || 'Untitled chat')}</a>` : `<span>${esc(chat.ownerName)}’s chat</span>`;
  function projectView(p) {
    if (!enabled()) return '';
    const o = project.id === p.id ? project.overview : null;
    const here = sub(), link = (t, label) => `<a href="${base(p)}${t === 'overview' ? '' : `/${t}`}" class="${here === t ? 'active' : ''}" ${here === t ? 'aria-current="page"' : ''}>${label}</a>`;
    const nav = `<nav class="code-tabs" aria-label="Code">${link('overview', 'Overview')}${link('history', 'History')}${link('files', 'Files')}</nav>`;
    const exportButton = o?.main ? btn('Export', 'code-export', { icon: 'download', tone: 'small', attrs: `data-project="${esc(p.id)}" data-name="${esc(p.name)}"` }) : '';
    const top = `<div class="spread code-top">${nav}${exportButton}</div>${errorBox(project.error, 'code-project-reload')}`;
    if (!o) return `${top}<p class="inline-note" role="status">Loading the code…</p>`;
    if (!o.main) return `${top}<section class="card project-section code-empty"><span class="tile-icon">${icon('branch')}</span><h2>No code yet</h2><p>Each chat in this project can build in its own workspace. When someone presses Sync in a chat’s Git tab, its code lands here and the other chats bring it in with their next Sync. Every change remembers the chat that made it.</p>${btn('New chat in project', 'project-chat', { icon: 'plus', tone: 'primary small', attrs: `data-id="${esc(p.id)}"` })}</section>`;
    if (here === 'history') return `${top}${historyView(p)}`;
    if (here === 'files') return `${top}${filesView(p)}`;
    return `${top}${overviewView(p, o)}`;
  }
  function overviewView(p, o) {
    const latest = project.log?.[0];
    const recent = (project.log || []).slice(0, 6).map(e => commitRow(p, e)).join('');
    const working = o.chats.length ? o.chats.map(w => `<div class="detail-row code-chat"><span class="avatar small">${initial(w.chat.ownerName)}</span><span class="grow"><strong>${w.chat.readable ? `<a href="#chat/${enc(w.chat.id)}">${esc(w.chat.title || 'Untitled chat')}</a>` : `${esc(w.chat.ownerName)}’s chat`}</strong><small>${w.chat.mine ? 'You' : esc(w.chat.ownerName)} · synced ${esc(ago(w.syncedAt))}</small></span>${w.behind ? badge(`${count(w.behind, 'commit')} behind`, 'amber') : badge('Up to date')}</div>`).join('') : '<p class="context-note">No chat has synced yet.</p>';
    const exports = o.exports.map(e => `<div class="detail-row">${icon('upload')}<span class="grow">Pushed to <a href="${esc(e.url)}" target="_blank" rel="noopener">${esc(e.target)}</a><small>${esc(ago(e.at))} · ${e.behind ? `${count(e.behind, 'commit')} since` : 'up to date'}</small></span></div>`).join('');
    const used = o.main.bytes / o.limits.bytes;
    return `<div class="project-layout"><div class="project-main">
      <section class="card project-section"><div class="spread"><span class="eyebrow">Latest</span><a class="tr-link" href="${base(p)}/history">History${icon('right')}</a></div><p class="code-latest">${count(o.main.commits, 'commit')}${latest ? ` · latest: <a href="${base(p)}/history/${esc(latest.sha)}">${esc(latest.subject)}</a>` : ''} · ${esc(o.main.updatedBy || 'someone')}, ${esc(ago(o.main.updatedAt))}</p><ol class="code-log compact">${recent}</ol></section>
      <section class="card project-section"><span class="eyebrow">Chats working on the code</span>${working}<p class="context-note">Every chat in the project works on its own copy. Sync shares its changes with everyone; the next Sync elsewhere brings them in.</p></section>
    </div><div class="project-side">
      <section class="card project-section"><span class="eyebrow">Take it with you</span><button type="button" class="code-out" data-action="code-export" data-project="${esc(p.id)}" data-name="${esc(p.name)}">${icon('upload')}<span class="grow"><strong>Push to GitHub</strong><small>With its history</small></span></button><button type="button" class="code-out" data-action="code-export" data-project="${esc(p.id)}" data-name="${esc(p.name)}">${icon('download')}<span class="grow"><strong>Download a zip</strong><small>The code, and if you like its history and chats</small></span></button>${exports}<p class="context-note">To put a site online, push it to GitHub, then import the repository in Vercel or Netlify, or turn on GitHub Pages.</p></section>
      <section class="card project-section"><span class="eyebrow">Size</span><progress class="meter" max="1" value="${Math.min(used, 1)}" aria-label="Project code size"></progress><p class="context-note">${esc(bytes(o.main.bytes))} of ${Math.round(o.limits.bytes / 1048576)} MB. Data sets and build output stay out of the code.</p></section>
    </div></div>`;
  }
  function commitRow(p, e, selected = '') {
    const merge = e.parents?.length > 1;
    return `<li class="${selected === e.sha ? 'active' : ''}"><a href="${base(p)}/history/${esc(e.sha)}"><span class="git-dot ${merge ? 'merge' : ''}"></span><span class="grow"><span class="git-subject">${esc(e.subject)}</span><small>${esc(e.author)} · ${esc(ago(e.time))}${e.chat ? ` · ${e.chat.readable ? esc(e.chat.title || 'chat') : `${esc(e.chat.ownerName)}’s chat`}` : ''}</small></span><code>${esc(short(e.sha))}</code></a></li>`;
  }
  function historyView(p) {
    const log = project.log;
    if (!log) return '<p class="inline-note">Loading history…</p>';
    const sha = param();
    const detail = !project.commit ? `<section class="card project-section"><p class="context-note">Pick a commit to see what it changed and the chat it came from.</p></section>` : !project.commit.data ? '<section class="card project-section"><p class="inline-note">Loading…</p></section>' : commitDetail(p, project.commit.data);
    return `<div class="code-split"><section class="card project-section code-list"><ol class="code-log">${log.map(e => commitRow(p, e, sha)).join('')}</ol>${log.length >= 200 ? '<p class="context-note">Showing the latest 200 commits.</p>' : ''}</section><div class="code-detail">${detail}</div></div>`;
  }
  function commitDetail(p, d) {
    const c = d.commit, body = d.body, open = project.commit.open || {};
    const from = c.chat ? `<div class="code-from"><span class="eyebrow">From the chat</span><p>${chatLink(c.chat, c.turn)}${c.chat.readable ? '' : '<span class="faint"> · not shared with you</span>'}</p></div>` : '';
    const files = d.files.map(f => {
      const patch = open[f.path];
      return `<li><button type="button" data-action="code-commit-file" data-path="${esc(f.path)}" aria-expanded="${patch !== undefined}"><span class="git-letter">${icon(patch !== undefined ? 'down' : 'right')}</span><span class="grow git-path">${esc(f.path)}</span><span class="git-stat">${f.added ? `<b>+${f.added}</b>` : ''}${f.removed ? ` <i>−${f.removed}</i>` : ''}${f.added == null ? '<small>binary</small>' : ''}</span></button>${patch === undefined ? '' : patch === null ? '<p class="inline-note git-pad">Loading…</p>' : diffView(patch)}</li>`;
    }).join('');
    return `<section class="card project-section"><div class="spread"><code>${esc(short(c.sha))}</code><span class="faint">${esc(new Date(c.time * 1000).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' }))}</span></div><h2 class="code-title">${esc(c.subject)}</h2><p class="faint">${esc(c.author)}${c.parents.length > 1 ? ' · combined two lines of work' : ''}</p>${body ? `<p class="code-body">${esc(body)}</p>` : ''}${from}<span class="eyebrow">${count(d.files.length, 'file')}</span><ul class="git-changes code-commit-files">${files}</ul></section>`;
  }
  async function togglePatch(path) {
    const commit = project.commit, id = project.id; if (!commit?.data) return;
    commit.open ||= {};
    if (commit.open[path] !== undefined) { delete commit.open[path]; render(); return; }
    commit.open[path] = null; render();
    const r = await get(id, `${projectPath(id)}/commits/${enc(commit.sha)}?path=${enc(path)}`);
    if (project.commit === commit) { commit.open[path] = r?.patch ?? ''; render(); }
  }
  function filesView(p) {
    const files = project.files;
    if (!files) return '<p class="inline-note">Loading files…</p>';
    const path = param();
    const list = `<section class="card project-section code-list"><ul class="code-files">${files.map(f => `<li class="${f.path === path ? 'active' : ''}"><a href="${base(p)}/files/${esc(enc(f.path))}">${icon('file')}<span class="grow">${esc(f.path)}</span><small>${esc(bytes(f.size))}</small></a></li>`).join('')}</ul></section>`;
    return `<div class="code-split">${list}<div class="code-detail">${project.file ? blameView(p, project.file) : '<section class="card project-section"><p class="context-note">Pick a file to see who changed each line, and in which chat.</p></section>'}</div></div>`;
  }
  const bytes = n => n == null ? '' : n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1048576).toFixed(1)} MB`;
  function blameView(p, f) {
    const head = `<div class="spread"><code class="grow">${esc(f.path)}</code>${f.info ? `<span class="faint">${esc(bytes(f.info.size))}</span>` : ''}</div>`;
    if (!f.info) return `<section class="card project-section">${head}<p class="inline-note">Loading…</p></section>`;
    if (f.info.binary || f.info.tooLarge) return `<section class="card project-section">${head}<p class="context-note">${f.info.binary ? 'A binary file.' : 'Too large to show here.'} It is in the zip and on GitHub when you export.</p></section>`;
    if (!f.blame) return `<section class="card project-section">${head}<p class="inline-note">Reading who changed each line…</p></section>`;
    const v = project.versions;
    const groups = f.blame.map(g => {
      const end = g.start + g.lines.length - 1;
      return `<div class="blame-group"><div class="blame-who"><span class="avatar small">${initial(g.author)}</span><span class="grow"><a href="${base(p)}/history/${esc(g.sha)}">${esc(g.subject)}</a><small>${esc(g.author)} · ${esc(ago(g.time))}${g.chat ? ` · ${chatLink(g.chat, g.turn)}` : ''}</small></span><button type="button" class="tr-link" data-action="code-lines" data-path="${esc(f.path)}" data-start="${g.start}" data-end="${end}" title="Every version of lines ${g.start}–${end}">History</button></div><div class="blame-lines">${g.lines.map((l, i) => `<div class="git-line"><span class="git-n">${g.start + i}</span><code>${esc(l) || ' '}</code></div>`).join('')}</div></div>`;
    }).join('');
    const versions = v && v.path === f.path ? `<section class="card project-section code-versions"><div class="spread"><span class="eyebrow">Lines ${v.start}–${v.end}, through time</span>${iconButton('x', 'code-lines-close', 'Close')}</div>${v.list == null ? '<p class="inline-note">Loading…</p>' : v.list.length ? v.list.map(e => `<div class="code-version"><p><a href="${base(p)}/history/${esc(e.sha)}">${esc(e.subject)}</a><small class="faint"> · ${esc(e.author)} · ${esc(ago(e.time))}${e.chat ? ` · ${chatLink(e.chat, e.turn)}` : ''}</small></p>${diffView(e.patch)}</div>`).join('') : '<p class="context-note">No earlier versions.</p>'}</section>` : '';
    return `${versions}<section class="card project-section"><div class="spread">${head}</div><span class="eyebrow">Who changed each line</span><div class="blame">${groups}</div></section>`;
  }
  actions['code-project-reload'] = () => { if (project.id) { const id = project.id; project.id = null; loadProject(id); } };

  return {
    enabled, load, refresh, changed, label, show, input, submit, actions, loadProject, projectView,
    attach: update => { paneUpdate = update; },
    clear: () => { chats.clear(); Object.assign(project, { id: null, overview: null, log: null, commit: null, files: null, file: null, versions: null, error: '' }); last = ''; },
  };
}
