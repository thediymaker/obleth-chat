import { esc, icon, badge, btn, empty, errorBox, field, textInput, formEnd, getValue, when } from './manage-ui.js';

// Knowledge: collections of documents a chat can search. The pages list and manage
// collections; in Chat, the composer picks the collections a conversation searches and
// replies show their numbered sources, with each [n] linking to the page it came from.

const ACCEPT = '.pdf,.docx,.html,.htm,.md,.markdown,.txt,.rst,.tex,.csv,.tsv,.json,.jsonl,.yaml,.yml,.toml,.xml,.ipynb,.py,.rs,.go,.java,.c,.h,.cc,.cpp,.hpp,.js,.mjs,.ts,.tsx,.jsx,.sh,.sql,.r,.jl,.m,.f90,.cu,.log';
const MAX_PER_CHAT = 8;
const size = n => n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(0)} KB` : n < 1073741824 ? `${(n / 1048576).toFixed(1)} MB` : `${(n / 1073741824).toFixed(2)} GB`;
const count = (n, one, many = `${one}s`) => `${Number(n || 0).toLocaleString()} ${n === 1 ? one : many}`;
const fileUrl = (id, page) => `/api/knowledge/documents/${encodeURIComponent(id)}/file${page ? `#page=${page}` : ''}`;
// Where a numbered source opens: the file at its page, or the saved item in Library.
const sourceLink = s => s.saved ? `href="#library/${encodeURIComponent(s.documentId)}"` : `href="${fileUrl(s.documentId, s.page)}" target="_blank" rel="noopener noreferrer"`;
const busy = d => d.status === 'queued' || d.status === 'processing';

export function createKnowledge({ api, state, capabilities, render, modal, close, toast, go }) {
  const local = { list: null, usage: null, groups: [], maxFileBytes: 50 * 1048576, tab: 'all', error: '', loading: false, sequence: 0,
    detail: null, query: '', results: null, searching: false, uploading: [], timer: null };
  const chat = { id: null, ids: [], draft: [] };
  const enabled = () => !!capabilities.features?.knowledge;
  const isAdmin = () => state.user?.role === 'admin';
  const path = id => `/api/knowledge/collections/${encodeURIComponent(id)}`;
  const route = () => { const [page, id = ''] = location.hash.slice(1).split('/'); return { page, id: decodeURIComponent(id) }; };
  const byId = id => local.list?.find(c => c.id === id);

  function scopeLabel(c) {
    return c.scope === 'everyone' ? 'Everyone' : c.scope === 'group' ? c.groupName || 'Group' : 'Personal';
  }
  function poll(again) {
    clearTimeout(local.timer); local.timer = null;
    if (again) local.timer = setTimeout(() => { if (route().page === 'knowledge') load(route().id, true); }, 2500);
  }
  async function load(id = route().id, quiet = false) {
    if (!enabled()) return;
    const sequence = ++local.sequence;
    if (!quiet) { local.loading = true; local.error = ''; render(); }
    try {
      if (id) {
        const r = await api.request(path(id));
        if (sequence !== local.sequence) return;
        if (local.detail?.collection.id !== id) { local.query = ''; local.results = null; }
        local.detail = r; poll(r.documents.some(busy));
      } else {
        const r = await api.request('/api/knowledge/collections');
        if (sequence !== local.sequence) return;
        Object.assign(local, { list: r.collections || [], usage: r.usage, groups: r.groups || [], maxFileBytes: r.maxFileBytes || local.maxFileBytes });
        local.detail = null; poll(local.list.some(c => c.processing > 0));
      }
    } catch (e) { if (e.name !== 'AbortError' && sequence === local.sequence) { local.error = e.message; if (e.status === 404) local.detail = null; } }
    finally { if (sequence === local.sequence) { local.loading = false; render(); } }
  }
  async function list() {
    if (!local.list) { const r = await api.request('/api/knowledge/collections'); Object.assign(local, { list: r.collections || [], usage: r.usage, groups: r.groups || [], maxFileBytes: r.maxFileBytes || local.maxFileBytes }); }
    return local.list;
  }

  // ---- Pages ----
  function card(c) {
    const facts = [count(c.documents, 'document'), c.processing ? `indexing ${c.processing}` : c.passages ? count(c.passages, 'passage') : '', c.problems ? `${c.problems} need${c.problems === 1 ? 's' : ''} attention` : ''].filter(Boolean).join(' · ');
    return `<a class="collection-card card" href="#knowledge/${encodeURIComponent(c.id)}"><div class="spread"><span class="tile-icon">${icon('book')}</span>${badge(c.mine ? scopeLabel(c) : `${scopeLabel(c)} · read only`, c.scope === 'personal' ? '' : 'accent')}</div><h2>${esc(c.name)}</h2><p>${esc(c.description || (c.mine ? 'Add documents to search them in chats.' : `Shared by ${c.ownerName}.`))}</p><div class="spread collection-facts"><span>${esc(facts)}</span>${icon('right')}</div></a>`;
  }
  function home() {
    const all = local.list || [], mine = all.filter(c => c.mine), shared = all.filter(c => !c.mine);
    const shown = local.tab === 'mine' ? mine : local.tab === 'shared' ? shared : all;
    const tab = (value, label, n) => `<button type="button" class="tab ${local.tab === value ? 'active' : ''}" data-action="knowledge-tab" data-value="${value}" aria-pressed="${local.tab === value}">${label} ${badge(n)}</button>`;
    const u = local.usage, storage = u ? `<div class="knowledge-storage"><div class="spread"><span>Storage</span><span>${size(u.used)} of ${size(u.limit)}</span></div><progress class="meter" max="${u.limit}" value="${Math.min(u.used, u.limit)}" aria-label="Storage used"></progress></div>` : '';
    const body = local.list === null ? '<p class="inline-note" role="status">Loading collections…</p>'
      : !all.length ? empty('No collections yet', 'Make a collection, add PDFs, Word files or notes, then pick it in a chat. Answers cite the page they came from.', btn('New collection', 'knowledge-new', { icon: 'plus', tone: 'primary' }))
      : shown.length ? `<div class="collection-grid">${shown.map(card).join('')}</div>` : `<p class="inline-note">${local.tab === 'shared' ? 'Nothing is shared with you yet.' : 'You have no collections of your own yet.'}</p>`;
    return `<div class="page knowledge-page"><div class="page-head spread"><div><h1>Your knowledge, connected.</h1><p>A home for the context that makes an answer yours.</p></div>${btn('New collection', 'knowledge-new', { icon: 'plus', tone: 'primary' })}</div>${errorBox(local.error, 'knowledge-reload')}${all.length ? `<div class="knowledge-bar"><div class="tabs">${tab('all', 'All collections', all.length)}${tab('mine', 'Personal', mine.length)}${tab('shared', 'Shared with me', shared.length)}</div>${storage}</div>` : ''}${body}<div class="knowledge-info card">${icon('shield')}<div class="grow"><h3>Personal by default. Shared with intention.</h3><p>Every search, citation and download checks that you can open the collection. Files live in object storage; the searchable text stays in Chat's database.</p></div></div></div>`;
  }
  function status(d) {
    if (d.status === 'queued') return badge('Waiting');
    if (d.status === 'processing') return d.stage === 'embedding' && d.total ? badge(`Indexing ${Math.floor(100 * d.done / d.total)}%`, 'accent') : badge('Reading', 'accent');
    if (d.status === 'ready') return `<span class="doc-ready">${icon('check')}Ready</span>`;
    if (d.status === 'needs_ocr') return `<span title="These pages are images with no text layer. Text recognition is not available yet; a PDF with selectable text will work.">${badge('Needs text recognition', 'amber')}</span>`;
    return `<span title="${esc(d.error || '')}">${badge('Failed', 'amber')}</span>`;
  }
  function documentRow(d, mine) {
    const actions = mine ? `${d.status === 'failed' ? btn('Retry', 'knowledge-retry', { tone: 'small ghost', attrs: `data-id="${esc(d.id)}"` }) : ''}<button type="button" class="icon-btn" data-action="knowledge-remove-document" data-id="${esc(d.id)}" data-name="${esc(d.name)}" aria-label="Remove ${esc(d.name)}" title="Remove">${icon('trash')}</button>` : '';
    return `<tr><td><a class="doc-name" href="${fileUrl(d.id)}" target="_blank" rel="noopener noreferrer">${icon('file')}<span>${esc(d.name)}</span></a>${d.status === 'failed' && d.error ? `<small class="doc-error">${esc(d.error)}</small>` : ''}</td><td>${d.pages ?? '—'}</td><td>${d.status === 'ready' ? d.passages.toLocaleString() : '—'}</td><td>${size(d.size)}</td><td>${status(d)}</td><td>${esc(when(d.createdAt))}</td><td class="doc-actions">${actions}</td></tr>`;
  }
  function results() {
    if (local.searching) return '<p class="inline-note" role="status">Searching…</p>';
    if (!local.results) return '';
    if (!local.results.length) return '<p class="inline-note">No passages matched. A chat would answer without this collection.</p>';
    return `<ol class="knowledge-results">${local.results.map(r => `<li><div class="spread"><a href="${fileUrl(r.documentId, r.page)}" target="_blank" rel="noopener noreferrer">${esc(r.name)}${r.page ? ` · page ${r.page}` : ''}</a>${badge(r.matched)}</div><p>${esc(r.excerpt)}</p></li>`).join('')}</ol>`;
  }
  function detail() {
    if (!local.detail) return local.error ? `<div class="page">${errorBox(local.error, 'knowledge-reload')}<a class="btn small" href="#knowledge">All collections</a></div>` : '<div class="empty" role="status">Opening the collection…</div>';
    const { collection: c, documents } = local.detail, mine = c.mine;
    const passages = documents.reduce((n, d) => n + (d.status === 'ready' ? d.passages : 0), 0), bytes = documents.reduce((n, d) => n + d.size, 0);
    const uploads = local.uploading.length ? `<p class="inline-note" role="status">Uploading ${esc(local.uploading.join(', '))}…</p>` : '';
    const table = documents.length ? `<div class="table-wrap"><table class="knowledge-table"><thead><tr><th>Name</th><th>Pages</th><th>Passages</th><th>Size</th><th>Status</th><th>Added</th><th></th></tr></thead><tbody>${documents.map(d => documentRow(d, mine)).join('')}</tbody></table></div>`
      : mine ? '' : '<p class="inline-note">No documents yet.</p>';
    const drop = mine ? `<label class="knowledge-drop ${documents.length ? 'compact' : ''}" for="knowledge-files">${icon('upload')}<span><strong>Drop files here or choose them</strong><small>PDF, Word (.docx), HTML, Markdown, text and code · up to ${size(local.maxFileBytes)} each</small></span><input id="knowledge-files" class="sr-only" type="file" multiple accept="${ACCEPT}"></label>` : '';
    const sharing = mine ? `<div class="knowledge-sharing">${[['personal', 'Only you', 'Private, like your files'], ['group', 'A class or group', local.groups.length ? 'Members search it; only you edit' : 'You are not in any group yet'], ['everyone', 'Everyone', isAdmin() ? 'Every account can search it' : 'Administrators only']].map(([value, label, note]) => `<label class="check-option"><input type="radio" name="knowledge-scope" value="${value}" data-collection="${esc(c.id)}" ${c.scope === value ? 'checked' : ''} ${(value === 'everyone' && !isAdmin()) || (value === 'group' && !local.groups.length) ? 'disabled' : ''}><span><strong>${label}</strong><small>${esc(note)}</small></span></label>`).join('')}${c.scope === 'group' || local.groups.length ? `<select class="form-control" id="knowledge-group" data-collection="${esc(c.id)}" aria-label="Group" ${c.scope === 'group' ? '' : 'hidden'}>${local.groups.map(g => `<option value="${esc(g.id)}" ${g.id === c.groupId ? 'selected' : ''}>${esc(g.name)}</option>`).join('')}</select>` : ''}</div>`
      : `<p class="inline-note">${esc(scopeLabel(c))} · shared by ${esc(c.ownerName)}. You can search it in chats, but only the owner changes it.</p>`;
    return `<div class="page knowledge-page"><nav class="knowledge-crumbs" aria-label="Breadcrumb"><a href="#knowledge">Library</a> / ${esc(c.name)}</nav><div class="page-head spread"><div><div class="row" style="margin-bottom:10px">${badge(mine ? scopeLabel(c) : `${scopeLabel(c)} · read only`)}</div><h1>${esc(c.name)}</h1><p>${esc(c.description || '')}${c.description ? '<br>' : ''}<span class="faint">${count(documents.length, 'document')} · ${count(passages, 'passage')} · ${size(bytes)}</span></p></div><div class="row">${btn('Ask in a new chat', 'knowledge-chat', { icon: 'chat', attrs: `data-id="${esc(c.id)}"` })}${mine ? btn('Edit', 'knowledge-edit', { icon: 'edit', tone: 'ghost' }) : ''}</div></div>${errorBox(local.error, 'knowledge-reload')}<div class="knowledge-columns"><div><section class="setting-card card"><h2>Try a search</h2><p>See which passages a chat would get for a question, before you rely on it.</p><form id="knowledge-search" class="knowledge-search"><input id="knowledge-query" name="knowledge-query" class="form-control" value="${esc(local.query)}" placeholder="Search this collection" maxlength="500" aria-label="Search this collection"><button class="btn" type="submit" ${documents.some(d => d.status === 'ready') ? '' : 'disabled'}>${icon('search')}Search</button></form>${results()}</section><div class="section-title"><h2>Documents</h2></div>${drop}${uploads}${table}</div><aside><section class="setting-card card"><h2>Who can search it</h2>${sharing}</section><section class="setting-card card"><h2>How it searches</h2><div class="detail-row"><span>Meaning</span><span>Embeddings</span></div><div class="detail-row"><span>Words</span><span>Full-text match</span></div><div class="detail-row"><span>Passage size</span><span>About 250 words</span></div><div class="detail-row"><span>Per question</span><span>Best 6 passages</span></div><p class="context-note">Models that use tools can search again with their own words.</p></section></aside></div></div>`;
  }
  function page(id) { return id ? detail() : home(); }

  // ---- Dialogs ----
  function editor(c = null) {
    const scope = c?.scope || 'personal';
    const options = [['personal', 'Only you'], ...(local.groups.length || scope === 'group' ? [['group', 'A class or group']] : []), ...(isAdmin() || scope === 'everyone' ? [['everyone', 'Everyone']] : [])];
    modal(c ? `Edit ${c.name}` : 'Make space for your knowledge', `<form id="knowledge-form" data-id="${esc(c?.id || '')}">${field('knowledge-name', 'Name', textInput('knowledge-name', c?.name || '', 'maxlength="80" required placeholder="e.g. PHYS 241 · Thermodynamics"'))}${field('knowledge-description', 'A little context', `<textarea id="knowledge-description" name="knowledge-description" class="form-control" rows="3" maxlength="500" placeholder="What belongs here?">${esc(c?.description || '')}</textarea>`)}${c ? '' : field('knowledge-scope-new', 'Who can search it', `<select id="knowledge-scope-new" name="knowledge-scope-new" class="form-control">${options.map(([v, l]) => `<option value="${v}" ${v === scope ? 'selected' : ''}>${l}</option>`).join('')}</select>`)}${!c && local.groups.length ? field('knowledge-group-new', 'Group', `<select id="knowledge-group-new" name="knowledge-group-new" class="form-control">${local.groups.map(g => `<option value="${esc(g.id)}">${esc(g.name)}</option>`).join('')}</select>`, 'Used when sharing with a group.') : ''}${formEnd(c ? 'Save' : 'Create collection')}${c ? `<div class="danger-zone"><div class="grow"><strong>Delete this collection</strong><small>Removes its documents and their files for everyone.</small></div>${btn('Delete', 'knowledge-delete', { tone: 'small danger' })}</div>` : ''}</form>`);
  }
  async function submit(form) {
    if (form.id === 'knowledge-search') {
      const query = getValue(form, 'knowledge-query').trim(); local.query = query;
      if (!query || !local.detail) return;
      local.searching = true; render();
      try { local.results = (await api.request('/api/knowledge/search', { method: 'POST', body: { query, collectionIds: [local.detail.collection.id], limit: 6 } })).results || []; }
      finally { local.searching = false; render(); requestAnimationFrame(() => document.querySelector('#knowledge-query')?.focus()); }
      return;
    }
    const id = form.dataset.id, error = form.querySelector('.form-error');
    const body = { name: getValue(form, 'knowledge-name').trim(), description: getValue(form, 'knowledge-description').trim() };
    if (!id) { body.scope = getValue(form, 'knowledge-scope-new') || 'personal'; if (body.scope === 'group') body.groupId = getValue(form, 'knowledge-group-new'); }
    try {
      const r = await api.request(id ? path(id) : '/api/knowledge/collections', { method: id ? 'PATCH' : 'POST', body });
      close(); local.list = null;
      if (id) { await load(id, true); toast('Collection saved'); } else { go(`knowledge/${r.collection.id}`); toast('Collection created. Add documents next.'); }
    } catch (e) { if (error) error.textContent = e.message; else throw e; }
  }
  async function setScope(id, scope, groupId) {
    const body = { scope, ...(scope === 'group' ? { groupId: groupId || local.groups[0]?.id } : {}) };
    try { await api.request(path(id), { method: 'PATCH', body }); local.list = null; toast(scope === 'personal' ? 'Only you can search it now' : scope === 'everyone' ? 'Everyone can search it now' : 'Group members can search it now'); }
    catch (e) { toast(e.message); }
    await load(id, true);
  }
  async function add(files) {
    const c = local.detail?.collection; if (!c?.mine) return;
    files = [...files]; if (!files.length) return;
    for (const file of files) {
      if (file.size > local.maxFileBytes) { toast(`${file.name} is larger than ${size(local.maxFileBytes)}.`); continue; }
      local.uploading.push(file.name); render();
      try { await api.request(`${path(c.id)}/documents?name=${encodeURIComponent(file.name)}`, { method: 'POST', raw: file, contentType: file.type || 'application/octet-stream' }); }
      catch (e) { if (e.name !== 'AbortError') toast(`${file.name}: ${e.message}`); }
      finally { local.uploading = local.uploading.filter(n => n !== file.name); }
      local.list = null; await load(c.id, true);
    }
  }

  // ---- In chats ----
  const chatId = () => (location.hash.match(/^#chat\/([^/]+)/) || [])[1] || null;
  const chosen = () => chat.id ? chat.ids : chat.draft;
  async function loadChat(id) {
    if (!enabled()) return;
    chat.id = id || null; chat.ids = [];
    try {
      const [ids] = await Promise.all([id ? api.request(`/api/conversations/${encodeURIComponent(id)}/knowledge`).then(r => r.collectionIds || []) : Promise.resolve(null), list()]);
      if (id && chat.id === id) chat.ids = ids;
      if (!id) chat.draft = chat.draft.filter(c => byId(c));
    } catch { /* The picker stays empty. */ }
    if (chat.id === (chatId() || null)) render();
  }
  function names() { return chosen().map(id => byId(id)?.name).filter(Boolean); }
  function composerButton() {
    if (!enabled() || (chat.id || null) !== (chatId() || null)) return '';
    const n = chosen().length, label = n === 1 ? names()[0] || 'Knowledge' : 'Knowledge';
    return `<button type="button" class="btn ghost small knowledge-toggle ${n ? 'on' : ''}" data-action="knowledge-picker" title="Collections this chat searches" aria-label="Knowledge: ${n} collection${n === 1 ? '' : 's'}" ${state.streaming ? 'disabled' : ''}>${icon('book')}<span class="option-text">${esc(label.length > 28 ? `${label.slice(0, 27)}…` : label)}</span>${n > 1 ? `<span class="count">${n}</span>` : ''}</button>`;
  }
  function contextSection() {
    if (!enabled()) return '';
    const list = names();
    return `<section class="context-section"><div class="spread"><span class="eyebrow">Knowledge</span>${list.length ? `<span class="faint">${list.length}</span>` : ''}</div>${list.length ? list.map(n => `<div class="context-source">${icon('book')}<span class="grow">${esc(n)}</span>${icon('check')}</div>`).join('') : '<p class="context-note">Pick collections to ground this chat in your documents. Answers cite the page they came from.</p>'}<button class="btn small" data-action="knowledge-picker">${icon('book')}Choose collections</button></section>`;
  }
  function picker() {
    const all = local.list || [], on = new Set(chosen());
    modal('Knowledge in this chat', `<p class="inline-note">The chat searches the collections that are on and cites what it uses. Changes apply from the next message.</p>${all.length ? `<div class="chat-connections">${all.map(c => `<div class="setting-line">${icon('book')}<div class="grow"><h3>${esc(c.name)} ${c.mine ? '' : badge(scopeLabel(c))}</h3><p>${count(c.documents, 'document')}${c.processing ? ` · indexing ${c.processing}` : ''}</p></div><button type="button" class="btn small ${on.has(c.id) ? '' : 'primary'}" data-action="knowledge-switch" data-id="${esc(c.id)}" aria-pressed="${on.has(c.id)}">${on.has(c.id) ? 'On' : 'Off'}</button></div>`).join('')}</div>` : empty('No collections yet', 'Make one on the Knowledge page and add documents to it.')}<div class="modal-actions"><a class="btn ghost small" href="#knowledge">Knowledge</a><button class="btn primary" data-action="close">Done</button></div>`);
  }
  async function toggle(id) {
    const current = chosen(), next = current.includes(id) ? current.filter(c => c !== id) : [...current, id];
    if (next.length > MAX_PER_CHAT) { toast(`A chat can search up to ${MAX_PER_CHAT} collections.`); return; }
    if (chat.id) chat.ids = (await api.request(`/api/conversations/${encodeURIComponent(chat.id)}/knowledge`, { method: 'PUT', body: { collectionIds: next } })).collectionIds || [];
    else chat.draft = next;
    picker(); render();
  }
  /** A new chat was created: store the collections picked before it existed. */
  async function applyDraft(id) {
    const ids = chat.draft; chat.draft = [];
    if (!enabled() || !ids.length) return;
    await api.request(`/api/conversations/${encodeURIComponent(id)}/knowledge`, { method: 'PUT', body: { collectionIds: ids } }).catch(e => toast(e.message));
  }

  // ---- Replies ----
  const sourcesOf = m => (Array.isArray(m.activity) ? m.activity : []).find(e => e.kind === 'sources');
  /** Links each [n] in an answer to its source. Leaves code alone. */
  function cite(html, m) {
    const entry = sourcesOf(m); if (!entry?.items?.length) return html;
    const items = new Map(entry.items.map(s => [s.n, s]));
    let code = 0;
    return html.split(/(<[^>]+>)/).map(part => {
      if (part.startsWith('<')) { if (/^<(pre|code)\b/i.test(part)) code++; else if (/^<\/(pre|code)>/i.test(part)) code = Math.max(0, code - 1); return part; }
      if (code) return part;
      return part.replace(/\[(\d{1,2}(?:\s*,\s*\d{1,2})*)\]/g, (whole, list) => {
        const ns = list.split(',').map(n => Number(n.trim()));
        if (!ns.every(n => items.has(n))) return whole;
        return ns.map(n => { const s = items.get(n); return `<a class="cite" ${sourceLink(s)} title="${s.saved ? 'Saved: ' : ''}${esc(s.name)}${s.page ? `, page ${s.page}` : ''}">${n}</a>`; }).join('');
      });
    }).join('');
  }
  const searching = m => (Array.isArray(m.activity) ? m.activity : []).find(e => e.tool === 'search_knowledge' && e.status === 'running');
  const quote = (q, max = 80) => `“${esc(q.length > max ? `${q.slice(0, max - 1)}…` : q)}”`;
  const passage = s => `<li class="${s.cited ? 'cited' : ''}"><span class="cite-number">${s.n}</span><div class="grow"><div class="spread"><strong>${s.saved ? `${icon('bookmark')} ` : ''}${esc(s.name)}${s.page ? ` <span class="faint">page ${s.page}</span>` : ''}</strong><a class="btn ghost small" ${sourceLink(s)}>${s.saved ? 'Open in Library' : s.page ? 'Open at page' : 'Open'}${s.saved ? '' : icon('external')}</a></div><p>${esc(s.excerpt)}</p></div></li>`;
  /**
   * One card for everything the reply looked up: a single line while closed (what it
   * is searching now, or how much it found), then the searches and the passages the
   * answer cited, with the rest behind a second toggle.
   */
  function sources(m) {
    const entry = sourcesOf(m); if (!entry) return '';
    const items = entry.items || [], queries = entry.queries || [];
    const where = esc((entry.collections || []).join(', ')), live = searching(m);
    const used = new Set([...String(m.content || '').matchAll(/\[(\d{1,2}(?:\s*,\s*\d{1,2})*)\]/g)].flatMap(x => x[1].split(',').map(n => Number(n.trim()))));
    const saved = items.filter(s => s.saved).length, passages = items.length - saved;
    const found = !items.length ? 'nothing matched' : [passages && `${count(passages, 'passage')} from ${count(new Set(items.filter(s => !s.saved).map(s => s.documentId)).size, 'document')}`, saved && count(saved, 'saved item')].filter(Boolean).join(' and ');
    const line = live ? `<span class="activity-label is-running">Searching ${where} for ${quote(String(live.summary || '').replace(/^Searched for “|”$/g, ''), 60)}</span>`
      : queries.length > 1 ? `Searched ${where} · ${count(queries.length, 'search', 'searches')} · ${found}`
      : `Searched ${where}${queries[0] ? ` for ${quote(queries[0])}` : ''} · ${found}`;
    const head = `${icon(live ? 'orbit' : 'book')}<span class="grow">${line}</span>`;
    if (!items.length && !live && queries.length <= 1) return `<div class="knowledge-sources empty-sources">${head}</div>`;
    const marked = items.map(s => ({ ...s, cited: used.has(s.n) }));
    const cited = marked.filter(s => s.cited), rest = marked.filter(s => !s.cited);
    const shown = cited.length ? cited : rest.slice(0, 6), hidden = cited.length ? rest : rest.slice(6);
    const searches = queries.length > 1 ? `<div class="knowledge-queries"><span class="eyebrow">Searches</span><ol>${queries.map(q => `<li>${quote(q, 140)}</li>`).join('')}</ol></div>` : '';
    const more = hidden.length ? `<details class="knowledge-more"><summary>${count(hidden.length, 'more passage')}${cited.length ? ' found but not cited' : ''}</summary><ol>${hidden.map(passage).join('')}</ol></details>` : '';
    const key = esc(m.id || 'live');
    return `<details class="knowledge-sources" data-sources="${key}" ${opened.has(m.id || 'live') ? 'open' : ''}><summary>${head}${icon('down')}</summary>${searches}${shown.length ? `<ol>${shown.map(passage).join('')}</ol>` : ''}${more}</details>`;
  }
  const opened = new Set();
  if (typeof document !== 'undefined') document.addEventListener('toggle', e => { const id = e.target?.dataset?.sources; if (id) { if (e.target.open) opened.add(id); else opened.delete(id); } }, true);
  function signature(m) { const e = sourcesOf(m); return e ? JSON.stringify([e.items?.length, e.queries?.length, searching(m)?.id]) : ''; }

  const actions = {
    'knowledge-tab': el => { local.tab = el.dataset.value; render(); },
    'knowledge-reload': () => load(),
    'knowledge-new': async () => { await list().catch(() => {}); editor(); },
    'knowledge-edit': () => local.detail && editor(local.detail.collection),
    'knowledge-delete': () => { const c = local.detail?.collection; if (c) modal(`Delete ${c.name}?`, `<p>Its ${count(local.detail.documents.length, 'document')} and their files are removed for everyone, and chats stop searching it. This cannot be undone.</p><div class="modal-actions">${btn('Keep it', 'close')}${btn('Delete collection', 'knowledge-confirm-delete', { tone: 'danger' })}</div>`); },
    'knowledge-confirm-delete': async () => { const c = local.detail?.collection; if (!c) return; await api.request(path(c.id), { method: 'DELETE' }); close(); local.list = null; local.detail = null; chat.draft = chat.draft.filter(id => id !== c.id); go('knowledge'); toast('Collection deleted'); },
    'knowledge-remove-document': el => modal(`Remove ${el.dataset.name}?`, `<p>The file and its passages are deleted. Chats stop citing it.</p><div class="modal-actions">${btn('Keep it', 'close')}${btn('Remove', 'knowledge-confirm-remove', { tone: 'danger', attrs: `data-id="${esc(el.dataset.id)}"` })}</div>`),
    'knowledge-confirm-remove': async el => { await api.request(`/api/knowledge/documents/${encodeURIComponent(el.dataset.id)}`, { method: 'DELETE' }); close(); local.list = null; await load(local.detail?.collection.id, true); toast('Document removed'); },
    'knowledge-retry': async el => { await api.request(`/api/knowledge/documents/${encodeURIComponent(el.dataset.id)}/retry`, { method: 'POST' }); await load(local.detail?.collection.id, true); },
    'knowledge-chat': el => { chat.draft = [el.dataset.id]; go('chat'); requestAnimationFrame(() => document.querySelector('#prompt')?.focus()); },
    'knowledge-picker': async () => { await list().catch(() => {}); picker(); },
    'knowledge-switch': el => toggle(el.dataset.id),
  };
  /** Inputs on the collection page: chosen files and the sharing controls. */
  function input(event) {
    const t = event.target;
    if (t.id === 'knowledge-files') { const files = [...t.files]; t.value = ''; add(files); return true; }
    if (t.name === 'knowledge-scope') { setScope(t.dataset.collection, t.value, document.querySelector('#knowledge-group')?.value); return true; }
    if (t.id === 'knowledge-group') { setScope(t.dataset.collection, 'group', t.value); return true; }
    return false;
  }
  function clear() { local.sequence++; poll(false); Object.assign(local, { list: null, usage: null, groups: [], tab: 'all', error: '', loading: false, detail: null, query: '', results: null, searching: false, uploading: [] }); chat.id = null; chat.ids = []; chat.draft = []; }
  return { enabled, load, page, actions, submit, input, add, clear, loadChat, composerButton, contextSection, applyDraft, resetDraft: () => { chat.draft = []; }, sources, cite, signature, stop: () => poll(false) };
}
