import { esc, icon, badge, btn, empty, errorBox, field, textInput, formEnd, getValue, isChecked, when } from './manage-ui.js';

// Library: things saved while working (functions, notes, links, excerpts), the images
// models generated, and, on its Collections tab, the documents chats search. Saving
// happens from a chat (a code block, a message, selected text, or the model's
// save_item tool) or here.

const KINDS = { code: ['code', 'Code', 'Function'], note: ['file', 'Notes', 'Note'], link: ['link', 'Links', 'Link'], excerpt: ['bookmark', 'Excerpts', 'Excerpt'] };
const size = n => n < 1048576 ? `${Math.max(1, Math.round(n / 1024))} KB` : `${(n / 1048576).toFixed(1)} MB`;
const host = url => { try { return new URL(url).host; } catch { return url; } };
const parseTags = text => [...new Set(String(text || '').split(/[\s,]+/).map(t => t.replace(/^#/, '').toLowerCase()).filter(Boolean))].slice(0, 12);

export function createLibrary({ api, state, capabilities, render, modal, close, toast, go, projects = () => [] }) {
  const local = { items: null, tags: [], kinds: [], more: false, query: '', kind: '', tag: '', error: '', loading: false, sequence: 0, images: null, usage: null, undone: new Set(), project: '' };
  const enabled = () => !!capabilities.features?.knowledge;
  const path = id => `/api/saved/${encodeURIComponent(id)}`;

  async function load(id = '') {
    if (!enabled()) return;
    const sequence = ++local.sequence; local.loading = true; local.error = ''; render();
    try {
      if (id === 'generated') {
        const r = await api.request('/api/images');
        if (sequence === local.sequence) { local.images = r.images || []; local.usage = r.usage; }
      } else {
        const q = new URLSearchParams(Object.entries({ q: local.query, kind: local.kind, tag: local.tag, project: local.project }).filter(([, v]) => v));
        const r = await api.request(`/api/saved${q.size ? `?${q}` : ''}`);
        if (sequence === local.sequence) Object.assign(local, { items: r.items || [], tags: r.tags || [], kinds: r.kinds || [], more: !!r.more });
        if (id && id !== 'generated') openItem(id);
      }
    } catch (e) { if (e.name !== 'AbortError' && sequence === local.sequence) local.error = e.message; }
    finally { if (sequence === local.sequence) { local.loading = false; render(); } }
  }

  // ---- Pages ----
  function preview(item) {
    if (item.kind === 'code') return `<pre class="saved-code"><code>${esc(item.body.split('\n').slice(0, 7).join('\n'))}</code></pre>`;
    if (item.kind === 'link') return `${item.url ? `<span class="saved-host">${icon('external')}${esc(host(item.url))}</span>` : ''}${item.body ? `<p>${esc(item.body)}</p>` : ''}`;
    if (item.kind === 'excerpt') return `<blockquote>${esc(item.body)}</blockquote>`;
    return item.body ? `<p>${esc(item.body)}</p>` : '';
  }
  function card(item) {
    const [glyph, , label] = KINDS[item.kind] || KINDS.note;
    const kindLabel = item.kind === 'code' && item.language ? `${label} · ${item.language}` : label;
    return `<article class="saved-card card" data-action="saved-open" data-id="${esc(item.id)}" tabindex="0" role="button" aria-label="Open ${esc(item.title)}"><div class="saved-kind">${icon(glyph)}${esc(kindLabel)}${item.searchable ? '' : `<span class="saved-private" title="Chats cannot find it">${icon('lock')}</span>`}</div><h3>${esc(item.title)}</h3>${preview(item)}${item.tags.length ? `<div class="saved-tags">${item.tags.map(t => `<span>#${esc(t)}</span>`).join('')}</div>` : ''}<div class="saved-meta">${item.source ? `${icon('chat')}<span>From “${esc(item.source.title || 'a chat')}”</span>` : `${icon('clock')}<span>${esc(when(item.createdAt))}</span>`}</div></article>`;
  }
  function savedView() {
    const items = local.items;
    const total = local.kinds.reduce((n, k) => n + k.count, 0);
    const countOf = kind => local.kinds.find(k => k.kind === kind)?.count || 0;
    const kindChip = (kind, label, glyph, n) => `<button type="button" class="filter-chip ${local.kind === kind ? 'active' : ''}" data-action="saved-kind" data-kind="${kind}" aria-pressed="${local.kind === kind}">${icon(glyph)}${label}${n ? `<span>${n}</span>` : ''}</button>`;
    const filters = `<div class="saved-filters">${kindChip('', 'All', 'bookmark', total)}${Object.entries(KINDS).map(([k, [g, l]]) => kindChip(k, l, g, countOf(k))).join('')}</div>`;
    const tags = local.tags.length ? `<div class="saved-tag-filter">${local.tags.slice(0, 16).map(t => `<button type="button" class="tag-chip ${local.tag === t.tag ? 'active' : ''}" data-action="saved-tag" data-tag="${esc(t.tag)}" aria-pressed="${local.tag === t.tag}">#${esc(t.tag)}<span>${t.count}</span></button>`).join('')}${local.tag ? `<button type="button" class="tr-link" data-action="saved-tag" data-tag="">Clear</button>` : ''}</div>` : '';
    const search = `<form id="library-search" class="library-search">${icon('search')}<input id="library-query" name="library-query" value="${esc(local.query)}" placeholder="Search saved things, like “that throughput function”" maxlength="500" aria-label="Search saved things"><button class="btn small" type="submit">Search</button>${local.query ? '<button type="button" class="btn ghost small" data-action="saved-clear">Clear</button>' : ''}</form>`;
    const body = items === null ? '<p class="inline-note" role="status">Loading…</p>'
      : !items.length ? (local.query || local.kind || local.tag || local.project ? '<p class="inline-note">Nothing matches. Try other words or clear the filters.</p>' : empty('Nothing saved yet', 'In a chat, use Save on a code block or a reply, select text and choose Save, or say “remember this”. You can also add a note or a link here.', btn('New note', 'saved-new', { icon: 'plus', tone: 'primary', attrs: 'data-kind="note"' })))
      : `<div class="saved-grid">${items.map(card).join('')}</div>${local.more ? '<p class="inline-note">Showing the newest 60. Search or filter to find older ones.</p>' : ''}`;
    const all = projects(), chosen = all.find(p => p.id === local.project);
    const projectFilter = all.length ? `<label class="saved-project-filter">${icon('folder')}<select id="library-project" aria-label="Project"><option value="">All my saved things</option>${all.map(p => `<option value="${esc(p.id)}" ${p.id === local.project ? 'selected' : ''}>${esc(p.name)}${p.mine ? '' : ` · ${esc(p.ownerName)}`}</option>`).join('')}</select></label>` : '';
    return `${search}<div class="saved-filter-row">${filters}${projectFilter}</div>${chosen && !chosen.mine ? '<p class="inline-note">Showing everything saved in this shared project, by anyone in it. Kind and tag counts are your own.</p>' : ''}${tags}${errorBox(local.error, 'saved-reload')}${body}`;
  }
  function generatedView() {
    const images = local.images, u = local.usage;
    if (images === null) return '<p class="inline-note" role="status">Loading…</p>';
    const meter = u ? `<div class="knowledge-storage"><div class="spread"><span>Generated images</span><span>${size(u.used)} of ${size(u.limit)}</span></div><progress class="meter" max="${u.limit}" value="${Math.min(u.used, u.limit)}" aria-label="Storage used"></progress></div>` : '';
    return `${meter}${errorBox(local.error, 'saved-reload')}${images.length ? `<div class="generated-grid">${images.map(i => `<figure class="generated-tile card"><a href="${esc(i.url)}" target="_blank" rel="noopener noreferrer"><img src="${esc(i.url)}" alt="Generated in ${esc(i.conversationTitle)}" loading="lazy"></a><figcaption><a href="#chat/${encodeURIComponent(i.conversationId)}" title="${esc(i.conversationTitle)}">${esc(i.conversationTitle)}</a><span>${esc(when(i.createdAt))} · ${size(i.size)}</span><button type="button" class="icon-btn" data-action="image-remove" data-id="${esc(i.id)}" aria-label="Delete image" title="Delete">${icon('trash')}</button></figcaption></figure>`).join('')}</div>` : empty('No generated images', 'Images a model makes in a chat are kept here. They are deleted with their chat.')}`;
  }
  function page(id, tabs) {
    const generated = id === 'generated';
    const head = `<div class="page-head spread"><div><h1>Everything worth keeping.</h1><p>Functions, articles, notes and pictures you saved while thinking, and the documents your chats search.</p></div><div class="row">${btn('New note', 'saved-new', { icon: 'file', attrs: 'data-kind="note"' })}${btn('Save a link', 'saved-new', { icon: 'plus', tone: 'primary', attrs: 'data-kind="link"' })}</div></div>`;
    return `<div class="page library-page">${head}${tabs}${generated ? generatedView() : savedView()}</div>`;
  }

  // ---- The save dialog, for new items and for editing ----
  function editor(item, { title = 'Save to Library' } = {}) {
    const kind = item.kind || 'note';
    const suggestions = suggest(item).filter(t => !(item.tags || []).includes(t));
    const kinds = Object.entries(KINDS).map(([k, [g, , l]]) => `<label class="kind-option"><input type="radio" name="saved-kind" value="${k}" ${k === kind ? 'checked' : ''}><span>${icon(g)}${l}</span></label>`).join('');
    const source = item.source ? `<p class="inline-note saved-source">${icon('chat')}From <a href="#chat/${encodeURIComponent(item.source.conversationId)}">${esc(item.source.title || 'a chat')}</a></p>` : item.sourceConversationId ? '<p class="inline-note saved-source">Keeps a link back to this chat.</p>' : '';
    modal(title, `<form id="saved-form" data-id="${esc(item.id || '')}" data-conversation="${esc(item.sourceConversationId || '')}" data-message="${esc(item.sourceMessageId || '')}">${field('saved-title', 'Title', textInput('saved-title', item.title || '', 'maxlength="200" placeholder="What is it?"'))}<div class="management-field"><span class="form-label">Kind</span><div class="kind-options">${kinds}</div></div><div data-for="link" ${kind === 'link' ? '' : 'hidden'}>${field('saved-url', 'Address', textInput('saved-url', item.url || '', 'type="url" maxlength="2048" placeholder="https://"'))}</div>${field('saved-body', kind === 'link' ? 'Notes' : 'Content', `<textarea id="saved-body" name="saved-body" class="form-control ${kind === 'code' ? 'mono' : ''}" rows="${kind === 'code' ? 10 : 6}" maxlength="65536">${esc(item.body || '')}</textarea>`)}<div data-for="code" ${kind === 'code' ? '' : 'hidden'}>${field('saved-language', 'Language', textInput('saved-language', item.language || '', 'maxlength="40" placeholder="python"'))}</div>${projectField(item)}${field('saved-tags', 'Tags', textInput('saved-tags', (item.tags || []).map(t => `#${t}`).join(' '), 'maxlength="400" placeholder="#research #python"'), 'Separate tags with spaces.')}${suggestions.length ? `<div class="tag-suggestions"><span>Suggested</span>${suggestions.map(t => `<button type="button" class="tag-chip" data-action="saved-suggest" data-tag="${esc(t)}">+ ${esc(t)}</button>`).join('')}</div>` : ''}<label class="check-option" for="saved-searchable"><input type="checkbox" id="saved-searchable" name="saved-searchable" ${item.searchable === false ? '' : 'checked'}><span><strong>Chats can find it</strong><small>Indexed like a document, so a later chat can find it when you ask about it.</small></span></label>${source}${formEnd(item.id ? 'Save changes' : 'Save')}${item.id ? `<div class="danger-zone"><div class="grow"><strong>Delete</strong><small>Removes it from Library and from chat searches.</small></div>${btn('Copy', 'saved-copy', { tone: 'small', attrs: `data-id="${esc(item.id)}"` })}${btn('Delete', 'saved-delete', { tone: 'small danger', attrs: `data-id="${esc(item.id)}"` })}</div>` : ''}</form>`);
  }
  function projectField(item) {
    const all = projects();
    if (!all.length || (item.id && item.ownerId && item.ownerId !== state.user?.id)) return '';
    return field('saved-project', 'Project', `<select id="saved-project" name="saved-project" class="form-control"><option value="">No project</option>${all.map(p => `<option value="${esc(p.id)}" ${p.id === item.projectId ? 'selected' : ''}>${esc(p.name)}</option>`).join('')}</select>`, 'Chats in the project can find it, and so can everyone the project is shared with.');
  }
  /** Tags worth offering: the code's language and the person's own tags that the text mentions. */
  function suggest(item) {
    const text = `${item.title || ''} ${item.body || ''}`.toLowerCase();
    const out = [];
    if (item.language) out.push(item.language.toLowerCase());
    for (const t of local.tags) if (out.length < 5 && t.tag.length > 2 && text.includes(t.tag.replace(/-/g, ' ')) && !out.includes(t.tag)) out.push(t.tag);
    return out;
  }
  function openItem(id) {
    const item = local.items?.find(i => i.id === id);
    if (item) { editor(item, { title: item.title }); return; }
    api.request(path(id)).then(r => editor(r.item, { title: r.item.title })).catch(e => toast(e.message));
  }
  /** Opens the save dialog for something in a chat. */
  async function saveFrom({ kind, title = '', body = '', language = '', conversationId, messageId, projectId = '' }) {
    if (!local.items) { try { const r = await api.request('/api/saved'); Object.assign(local, { items: r.items || [], tags: r.tags || [], kinds: r.kinds || [] }); } catch { /* suggestions only */ } }
    editor({ kind, title, body, language, tags: language ? [language.toLowerCase()] : [], sourceConversationId: conversationId, sourceMessageId: messageId, projectId });
  }
  async function submit(form) {
    if (form.id === 'library-search') { local.query = getValue(form, 'library-query').trim(); await load(); return; }
    const id = form.dataset.id, error = form.querySelector('.form-error');
    const kind = form.querySelector('input[name="saved-kind"]:checked')?.value || 'note';
    const body = { kind, title: getValue(form, 'saved-title').trim(), body: getValue(form, 'saved-body'), tags: parseTags(getValue(form, 'saved-tags')), searchable: isChecked(form, 'saved-searchable') };
    if (kind === 'code') body.language = getValue(form, 'saved-language').trim();
    if (kind === 'link') body.url = getValue(form, 'saved-url').trim();
    if (form.elements.namedItem('saved-project')) body.projectId = getValue(form, 'saved-project') || null;
    if (!id && form.dataset.conversation) { body.sourceConversationId = form.dataset.conversation; if (form.dataset.message) body.sourceMessageId = form.dataset.message; }
    try {
      await api.request(id ? path(id) : '/api/saved', { method: id ? 'PATCH' : 'POST', body });
      close(); toast(id ? 'Saved your changes' : 'Saved to Library');
      if (location.hash.startsWith('#library')) await load(); else { local.items = null; if (location.hash.startsWith('#projects/')) go(location.hash.slice(1)); }
    } catch (e) { if (error) error.textContent = e.message; else throw e; }
  }
  function input(event) {
    const t = event.target;
    if (t.id === 'library-project') { local.project = t.value; load(); return true; }
    if (t.name === 'saved-kind') {
      const form = t.form, kind = t.value;
      form.querySelectorAll('[data-for]').forEach(el => { el.hidden = el.dataset.for !== kind; });
      form.querySelector('#saved-body').classList.toggle('mono', kind === 'code');
      return true;
    }
    return false;
  }

  // ---- In replies: the card for something the model saved ----
  const savedEntries = m => (Array.isArray(m.activity) ? m.activity : []).filter(e => e.kind === 'saved');
  function cards(m) {
    const entries = savedEntries(m);
    if (!entries.length) return '';
    return entries.map(e => {
      if (e.status === 'error') return `<div class="saved-note is-error">${icon('bookmark')}<span class="grow">Could not save: ${esc(e.detail || '')}</span></div>`;
      if (local.undone.has(e.savedId)) return `<div class="saved-note is-undone">${icon('bookmark')}<span class="grow">Removed “${esc(e.title)}” from Library</span></div>`;
      return `<div class="saved-note">${icon('bookmark')}<span class="grow">Saved <button type="button" class="tr-link" data-action="saved-open" data-id="${esc(e.savedId)}">“${esc(e.title)}”</button> to Library ${(e.tags || []).map(t => `<span class="saved-note-tag">#${esc(t)}</span>`).join('')}</span><button type="button" class="tr-link" data-action="saved-undo" data-id="${esc(e.savedId)}">Undo</button></div>`;
    }).join('');
  }
  function signature(m) { return JSON.stringify(savedEntries(m).map(e => [e.savedId, e.status, local.undone.has(e.savedId)])); }

  const actions = {
    'saved-open': el => openItem(el.dataset.id),
    'saved-new': el => editor({ kind: el.dataset.kind || 'note' }, { title: el.dataset.kind === 'link' ? 'Save a link' : 'New note' }),
    'saved-kind': el => { local.kind = el.dataset.kind; return load(); },
    'saved-tag': el => { local.tag = local.tag === el.dataset.tag ? '' : el.dataset.tag; return load(); },
    'saved-clear': () => { local.query = ''; return load(); },
    'saved-reload': () => load(location.hash.split('/')[1] || ''),
    'saved-suggest': el => { const input = document.querySelector('#saved-tags'); if (input) input.value = `${input.value} #${el.dataset.tag}`.trim(); el.remove(); },
    'saved-copy': async el => { const item = local.items?.find(i => i.id === el.dataset.id) || (await api.request(path(el.dataset.id))).item; await navigator.clipboard.writeText(item.kind === 'link' ? item.url || item.body : item.body); toast('Copied'); },
    'saved-delete': async el => { await api.request(path(el.dataset.id), { method: 'DELETE' }); close(); toast('Deleted'); if (location.hash.startsWith('#library')) await load(); else if (location.hash.startsWith('#projects/')) go(location.hash.slice(1)); },
    'saved-undo': async el => { await api.request(path(el.dataset.id), { method: 'DELETE' }).catch(e => { if (e.status !== 404) throw e; }); local.undone.add(el.dataset.id); local.items = null; render(); toast('Removed from Library'); },
    'image-remove': async el => { await api.request(`/api/images/${encodeURIComponent(el.dataset.id)}`, { method: 'DELETE' }); local.images = local.images.filter(i => i.id !== el.dataset.id); render(); toast('Image deleted'); },
  };
  function clear() { local.sequence++; Object.assign(local, { items: null, tags: [], kinds: [], more: false, query: '', kind: '', tag: '', error: '', loading: false, images: null, usage: null, project: '' }); local.undone.clear(); }
  return { enabled, load, page, actions, submit, input, clear, saveFrom, cards, signature, filterProject: id => { Object.assign(local, { project: id || '', query: '', kind: '', tag: '' }); } };
}
