import { esc, icon, badge, btn, empty, errorBox, field, textInput, formEnd, getValue, when } from './manage-ui.js';

// Projects: a home for one piece of work. A project gives each of its chats standing
// instructions and the collections to search, and keeps what was saved along the way.
// Shared with a group, its members see the project, its collections and saved items;
// each person's chats stay their own.

const COLORS = ['blue', 'green', 'gold', 'rose', 'violet', 'gray'];
const count = (n, one, many = `${one}s`) => `${Number(n || 0).toLocaleString()} ${n === 1 ? one : many}`;
const KIND = { code: ['code', 'Function'], note: ['file', 'Note'], link: ['link', 'Link'], excerpt: ['bookmark', 'Excerpt'] };
const MAX_COLLECTIONS = 8;

export function createProjects({ api, state, capabilities, render, modal, close, toast, go, saveTo, openLibrary, newChat, research, code = null }) {
  const local = { list: null, groups: [], detail: null, error: '', sequence: 0, collections: null };
  const chat = { id: null, project: null, draft: null };
  const path = id => `/api/projects/${encodeURIComponent(id)}`;
  const knowledge = () => !!capabilities.features?.knowledge;
  const chatId = () => (location.hash.match(/^#chat\/([^/]+)/) || [])[1] || null;
  const byId = id => local.list?.find(p => p.id === id);
  const dot = p => `<span class="project-dot" data-color="${esc(p.color)}"></span>`;
  const sharing = p => p.scope === 'group' ? `Shared with ${p.groupName || 'a group'}` : 'Only you';

  async function list(force = false) {
    if (local.list && !force) return local.list;
    const r = await api.request('/api/projects');
    Object.assign(local, { list: r.projects || [], groups: r.groups || [] });
    return local.list;
  }
  // Research runs in workspaces, so its tab exists only where they are enabled.
  const researchOn = () => !!capabilities.features?.workspaces;
  const codeOn = () => !!code?.enabled();
  const tab = () => { const t = location.hash.split('/')[2]; return researchOn() && t === 'research' ? 'research' : codeOn() && t === 'code' ? 'code' : 'overview'; };
  async function load(id = '') {
    const sequence = ++local.sequence; local.error = ''; render();
    if (id && tab() === 'research') research.load(id); else research.stop();
    if (id && tab() === 'code') code.loadProject(id);
    try {
      if (id) { const r = await api.request(path(id)); if (sequence === local.sequence) local.detail = r; }
      else { await list(true); if (sequence === local.sequence) local.detail = null; }
    } catch (e) { if (e.name !== 'AbortError' && sequence === local.sequence) { local.error = e.message; local.detail = null; } }
    finally { if (sequence === local.sequence) render(); }
  }

  // ---- Sidebar ----
  function sidebar(current) {
    const all = local.list || [];
    if (!all.length) return '';
    return `<div class="sidebar-projects"><div class="recent-header spread"><a class="eyebrow" href="#projects">Projects</a></div>${all.slice(0, 5).map(p => `<a class="sidebar-project ${current === p.id ? 'active' : ''}" href="#projects/${encodeURIComponent(p.id)}" title="${esc(p.name)}">${dot(p)}<span>${esc(p.name)}</span></a>`).join('')}</div>`;
  }

  // ---- Pages ----
  function card(p) {
    const facts = [count(p.chats, 'chat'), knowledge() ? count(p.saved, 'saved item') : ''].filter(Boolean).join(' · ');
    return `<a class="project-card card" href="#projects/${encodeURIComponent(p.id)}"><div class="spread"><span class="project-mark" data-color="${esc(p.color)}">${icon('folder')}</span>${badge(p.mine ? sharing(p) : `${p.groupName || 'Shared'} · ${p.ownerName}`, p.scope === 'group' ? 'accent' : '')}</div><h2>${esc(p.name)}</h2><p>${esc(p.description || (p.instructions ? p.instructions.slice(0, 140) : 'No description yet.'))}</p><div class="spread collection-facts"><span>${esc(facts)}</span><span>${esc(when(p.updatedAt))}</span></div></a>`;
  }
  function home() {
    const all = local.list;
    const body = all === null ? '<p class="inline-note" role="status">Loading projects…</p>'
      : !all.length ? empty('No projects yet', 'A project gives its chats standing instructions and documents to search, and keeps what you save along the way. Share it with a group to work on it together.', btn('New project', 'project-new', { icon: 'plus', tone: 'primary' }))
      : `<div class="collection-grid">${all.map(card).join('')}</div>`;
    return `<div class="page projects-page"><div class="page-head spread"><div><h1>Projects worth coming back to.</h1><p>Instructions, documents and saved things that every chat in the project starts with.</p></div>${btn('New project', 'project-new', { icon: 'plus', tone: 'primary' })}</div>${errorBox(local.error, 'project-reload')}${body}</div>`;
  }
  function savedCard(item) {
    const [glyph, label] = KIND[item.kind] || KIND.note;
    const own = item.ownerId === state.user?.id;
    const pin = own ? `<button type="button" class="icon-btn project-pin ${item.pinned ? 'on' : ''}" data-action="project-pin" data-id="${esc(item.id)}" aria-pressed="${item.pinned}" aria-label="${item.pinned ? 'Unpin' : 'Pin'} ${esc(item.title)}" title="${item.pinned ? 'Unpin' : 'Pin to the top'}">${icon('pin')}</button>` : '';
    return `<article class="project-saved card"><div class="saved-kind">${icon(glyph)}${esc(item.kind === 'code' && item.language ? `${label} · ${item.language}` : label)}${pin}</div><button type="button" class="project-saved-title" data-action="saved-open" data-id="${esc(item.id)}">${esc(item.title)}</button>${item.tags.length ? `<div class="saved-tags">${item.tags.slice(0, 3).map(t => `<span>#${esc(t)}</span>`).join('')}</div>` : ''}</article>`;
  }
  function detailPage() {
    const d = local.detail;
    if (!d) return `<div class="page">${errorBox(local.error || 'Loading…', local.error ? 'project-reload' : '')}${local.error ? `<a class="btn small" href="#projects">${icon('left')}All projects</a>` : ''}</div>`;
    const p = d.project, mine = p.mine;
    const facts = [p.description, count(p.chats, 'chat'), knowledge() ? count(p.saved, 'saved item') : ''].filter(Boolean).join(' · ');
    const head = `<div class="project-crumb"><a href="#projects">Projects</a><span class="slash">/</span>${esc(p.name)}</div><div class="project-head"><span class="project-mark large" data-color="${esc(p.color)}">${icon('folder')}</span><div class="grow"><h1>${esc(p.name)}</h1><p>${esc(facts)}</p></div><div class="row">${mine ? btn('Edit', 'project-edit', { icon: 'edit' }) : ''}${btn('New chat in project', 'project-chat', { icon: 'plus', tone: 'primary', attrs: `data-id="${esc(p.id)}"` })}</div></div>`;
    const saved = knowledge() ? `<section class="card project-section"><div class="spread"><span class="eyebrow">Saved in this project</span><span class="row">${p.saved > 6 ? `<button type="button" class="tr-link" data-action="project-library" data-id="${esc(p.id)}">All ${p.saved}${icon('right')}</button>` : ''}${btn('Add note', 'project-saved-new', { icon: 'plus', tone: 'small ghost', attrs: `data-id="${esc(p.id)}"` })}</span></div>${d.saved.length ? `<div class="project-saved-grid">${d.saved.slice(0, 6).map(savedCard).join('')}</div>` : '<p class="context-note">Things saved from this project’s chats land here, and its chats can find them. Pin the ones that matter most.</p>'}</section>` : '';
    const chats = `<section class="card project-section"><div class="spread"><span class="eyebrow">Your chats</span>${p.scope === 'group' ? '<span class="faint">Only you see yours</span>' : ''}</div>${d.chats.length ? d.chats.map(c => `<a class="project-chat" href="#chat/${encodeURIComponent(c.id)}">${icon('chat')}<span class="grow">${esc(c.title)}</span><span class="faint">${esc(when(c.updatedAt))}</span></a>`).join('') : `<p class="context-note">No chats yet. Start one here, or move a chat in from its Context panel.</p>`}</section>`;
    const instructions = `<section class="card project-section"><span class="eyebrow">Instructions for every chat</span>${p.instructions ? `<p class="project-instructions">${esc(p.instructions)}</p>` : `<p class="context-note">${mine ? 'Tell the model what this project is about and how to answer. Every chat here gets these instructions.' : 'No instructions.'}</p>`}${mine ? `<button type="button" class="tr-link" data-action="project-edit">${p.instructions ? 'Edit' : 'Add instructions'}</button>` : ''}</section>`;
    const starts = `<section class="card project-section"><span class="eyebrow">Chats here start with</span>${d.collections.map(c => `<div class="detail-row">${icon('book')}<a class="grow" href="#knowledge/${encodeURIComponent(c.id)}">${esc(c.name)}</a><span class="faint">${count(c.documents, 'doc')}</span></div>`).join('')}${knowledge() ? `<div class="detail-row">${icon('bookmark')}<span class="grow">Saved in this project</span><span class="faint">${count(p.saved, 'item')}</span></div>` : ''}${!d.collections.length && !knowledge() ? '<p class="context-note">Just the instructions.</p>' : ''}${mine && knowledge() ? btn('Choose collections', 'project-collections', { icon: 'book', tone: 'small' }) : ''}</section>`;
    const people = `<section class="card project-section"><span class="eyebrow">People</span><div class="detail-row"><span class="avatar small">${esc((p.ownerName || '?').slice(0, 1).toUpperCase())}</span><span class="grow">${mine ? 'You' : esc(p.ownerName)}</span><span class="faint">owner</span></div>${p.scope === 'group' ? `<p class="context-note">Shared through the group <a href="#groups/${encodeURIComponent(p.groupId)}">${esc(p.groupName || '')}</a>. Members see the instructions, collections and saved items, and can start their own chats here. Only the owner changes the project.</p>` : `<p class="context-note">Only you can see this project.${mine ? ' Share it with a group from Edit.' : ''}</p>`}</section>`;
    const here = tab(), link = (t, label) => `<a href="#projects/${encodeURIComponent(p.id)}${t === 'overview' ? '' : `/${t}`}" class="${here === t ? 'active' : ''}" ${here === t ? 'aria-current="page"' : ''}>${label}</a>`;
    const tabs = researchOn() || codeOn() ? `<nav class="project-tabs" aria-label="Project">${link('overview', 'Overview')}${codeOn() ? link('code', 'Code') : ''}${researchOn() ? link('research', 'Research') : ''}</nav>` : '';
    const content = here === 'research' ? research.view(p) : here === 'code' ? code.projectView(p) : `<div class="project-layout"><div class="project-main">${saved}${chats}</div><div class="project-side">${instructions}${starts}${people}</div></div>`;
    return `<div class="page project-page">${head}${tabs}${errorBox(local.error, 'project-reload')}${content}</div>`;
  }
  function page(id) { return id ? detailPage() : home(); }

  // ---- Editing ----
  function editor(p = {}) {
    const color = p.color || COLORS[(local.list?.length || 0) % COLORS.length];
    // The group it is shared with stays a choice even after its owner leaves that group.
    const groups = p.scope === 'group' && p.groupId && !local.groups.some(g => g.id === p.groupId) ? [...local.groups, { id: p.groupId, name: p.groupName || 'Current group' }] : local.groups;
    const share = `<select id="project-share" name="project-share" class="form-control"><option value="">Only me</option>${groups.map(g => `<option value="${esc(g.id)}" ${p.scope === 'group' && p.groupId === g.id ? 'selected' : ''}>${esc(g.name)}</option>`).join('')}</select>`;
    const colors = COLORS.map(c => `<label class="color-option"><input type="radio" name="project-color" value="${c}" ${c === color ? 'checked' : ''}><span class="project-dot" data-color="${c}" aria-label="${c}"></span></label>`).join('');
    modal(p.id ? 'Edit project' : 'New project', `<form id="project-form" data-id="${esc(p.id || '')}" ${p.fromChat ? 'data-from-chat="1"' : ''}>${field('project-name', 'Name', textInput('project-name', p.name || '', 'maxlength="120" required placeholder="Gaudi benchmarks"'))}${field('project-description', 'Description', textInput('project-description', p.description || '', 'maxlength="1000" placeholder="What is it for?"'))}${field('project-instructions', 'Instructions for every chat', `<textarea id="project-instructions" name="project-instructions" class="form-control" rows="6" maxlength="20000" placeholder="We are benchmarking LLM inference on Gaudi 3. Report tokens per second with batch size and model.">${esc(p.instructions || '')}</textarea>`, 'Added to the start of each chat in the project.')}<div class="management-field"><span class="form-label">Color</span><div class="color-options">${colors}</div></div>${field('project-share', 'Shared with', share, groups.length ? 'Group members see the project and can start their own chats in it.' : 'You are not in any groups. An administrator adds people to groups.')}${formEnd(p.id ? 'Save changes' : 'Create project')}${p.id ? `<div class="danger-zone"><div class="grow"><strong>Delete project</strong><small>Its chats and saved items are kept, outside any project.</small></div>${btn('Delete', 'project-delete', { tone: 'small danger', attrs: `data-id="${esc(p.id)}"` })}</div>` : ''}</form>`);
  }
  async function submit(form) {
    const id = form.dataset.id, error = form.querySelector('.form-error');
    const group = getValue(form, 'project-share');
    const body = { name: getValue(form, 'project-name').trim(), description: getValue(form, 'project-description').trim(), instructions: getValue(form, 'project-instructions'), color: form.querySelector('input[name="project-color"]:checked')?.value || 'blue', scope: group ? 'group' : 'personal', ...(group ? { groupId: group } : {}) };
    try {
      const r = await api.request(id ? path(id) : '/api/projects', { method: id ? 'PATCH' : 'POST', body });
      // Started from a chat: the chat moves into the new project and stays open.
      if (!id && form.dataset.fromChat && chat.id) { await list(true); await place(r.project.id); return; }
      close(); toast(id ? 'Saved your changes' : 'Project created');
      await list(true);
      if (id) await load(id); else go(`projects/${encodeURIComponent(r.project.id)}`);
    } catch (e) { if (error) error.textContent = e.message; else throw e; }
  }
  async function collectionsPicker() {
    const d = local.detail; if (!d) return;
    try { local.collections = (await api.request('/api/knowledge/collections')).collections || []; } catch (e) { toast(e.message); return; }
    const on = new Set(d.collections.map(c => c.id));
    modal('Collections for this project', `<p class="inline-note">Every chat in the project searches these, along with any it picks itself.${d.project.scope === 'group' ? ' Group members can read your own collections through the project. A collection someone else shared with you reaches only the people it is already shared with.' : ''}</p>${local.collections.length ? `<div class="chat-connections">${local.collections.map(c => `<div class="setting-line">${icon('book')}<div class="grow"><h3>${esc(c.name)}</h3><p>${count(c.documents, 'document')}</p></div><button type="button" class="btn small ${on.has(c.id) ? '' : 'primary'}" data-action="project-collection" data-id="${esc(c.id)}" aria-pressed="${on.has(c.id)}">${on.has(c.id) ? 'On' : 'Off'}</button></div>`).join('')}</div>` : empty('No collections yet', 'Make one in Library → Collections and add documents to it.')}<div class="modal-actions"><button class="btn primary" data-action="close">Done</button></div>`);
  }
  async function toggleCollection(id) {
    const d = local.detail, current = d.collections.map(c => c.id);
    const next = current.includes(id) ? current.filter(c => c !== id) : [...current, id];
    if (next.length > MAX_COLLECTIONS) { toast(`A project can use up to ${MAX_COLLECTIONS} collections.`); return; }
    await api.request(`${path(d.project.id)}/collections`, { method: 'PUT', body: { collectionIds: next } });
    await load(d.project.id); collectionsPicker();
  }

  // ---- In chats ----
  async function loadChat(id) {
    chat.id = id || null; chat.project = null;
    try {
      const [r] = await Promise.all([id ? api.request(`/api/conversations/${encodeURIComponent(id)}/project`) : Promise.resolve(null), list().catch(() => [])]);
      if (id && chat.id === id) chat.project = r.project;
    } catch { /* No project shown. */ }
    if (chat.id === (chatId() || null)) render();
  }
  const current = () => chat.id ? chat.project : chat.draft && byId(chat.draft);
  /** The chat's project, for the save dialog's default. */
  const chatProject = () => (chat.id || null) === (chatId() || null) ? current()?.id || '' : '';
  function crumb() {
    const p = current();
    return p ? `<a class="chat-project" href="#projects/${encodeURIComponent(p.id)}">${dot(p)}${esc(p.name)}</a><span class="slash">/</span>` : '';
  }
  /** The chat line's project pill: where the chat lives, with a menu to move it. */
  function pill() {
    const p = current();
    return `<button type="button" class="chat-pill" data-action="project-menu" aria-haspopup="menu" aria-expanded="false" title="${p ? `In ${esc(p.name)}` : 'Not in a project'}">${p ? dot(p) : icon('folder')}<span>${esc(p?.name || 'Personal')}</span>${icon('down')}</button>`;
  }
  /** Menu items for the project pill. */
  function menuItems() {
    const p = current(), item = (glyph, label, action, attrs = '') => `<button role="menuitem" class="menu-item" data-action="${action}" ${attrs}>${icon(glyph)}${label}</button>`;
    return [p && item('right', `Open ${esc(p.name)}`, 'go', `data-path="projects/${esc(encodeURIComponent(p.id))}"`), item('folder', p ? 'Change project…' : 'Move to project…', 'project-move'), item('plus', 'Start a project from this chat', 'project-from-chat')].filter(Boolean).join('');
  }
  function composerChip() {
    const p = !chat.id && chat.draft && byId(chat.draft);
    return p ? `<div class="project-chip">${dot(p)}<span>New chat in <strong>${esc(p.name)}</strong></span><button type="button" class="icon-btn" data-action="project-draft-clear" aria-label="Start outside the project" title="Start outside the project">${icon('x')}</button></div>` : '';
  }
  function contextSection() {
    const p = current(), all = local.list || [];
    if (!p && !all.length) return '';
    return `<section class="context-section"><span class="eyebrow">Project</span>${p ? `<a class="context-source" href="#projects/${encodeURIComponent(p.id)}">${dot(p)}<span class="grow"><strong>${esc(p.name)}</strong>${p.instructions ? '<p>Instructions apply to this chat.</p>' : ''}</span>${icon('right')}</a>` : '<p class="context-note">Move this chat into a project to give it the project’s instructions, collections and saved items.</p>'}<button class="btn small" data-action="project-move">${icon('folder')}${p ? 'Change project' : 'Move to project'}</button></section>`;
  }
  function mover() {
    const p = current(), all = local.list || [];
    modal('Move to project', `<p class="inline-note">The chat gets the project’s instructions and collections from the next message.</p><div class="chat-connections">${all.map(x => `<div class="setting-line">${dot(x)}<div class="grow"><h3>${esc(x.name)}</h3><p>${esc(x.mine ? sharing(x) : `${x.groupName || 'Shared'} · ${x.ownerName}`)}</p></div><button type="button" class="btn small ${p?.id === x.id ? '' : 'primary'}" data-action="project-place" data-id="${esc(x.id)}" ${p?.id === x.id ? 'disabled' : ''}>${p?.id === x.id ? 'Here now' : 'Move here'}</button></div>`).join('')}</div><div class="modal-actions">${p ? btn('Remove from project', 'project-place', { tone: 'ghost', attrs: 'data-id=""' }) : ''}${btn('New project', 'project-new', { icon: 'plus' })}<button class="btn primary" data-action="close">Done</button></div>`);
  }
  async function place(projectId) {
    if (!chat.id) { chat.draft = projectId || null; close(); render(); return; }
    const r = await api.request(`/api/conversations/${encodeURIComponent(chat.id)}/project`, { method: 'PUT', body: { projectId: projectId || null } });
    chat.project = r.project; close(); render(); toast(r.project ? `Moved to ${r.project.name}` : 'Removed from the project');
    list(true).then(render).catch(() => {});
  }
  /** A new chat was created: put it in the project it was started from. */
  async function applyDraft(id) {
    const project = chat.draft; chat.draft = null;
    if (!project) return;
    await api.request(`/api/conversations/${encodeURIComponent(id)}/project`, { method: 'PUT', body: { projectId: project } }).catch(e => toast(e.message));
  }

  const actions = {
    'project-new': () => editor(),
    'project-from-chat': () => editor({ fromChat: true }),
    'project-edit': () => local.detail && editor(local.detail.project),
    'project-reload': () => load((location.hash.split('/')[1] || '')),
    'project-delete': el => modal('Delete this project?', `<p>The project, its instructions and its collection choices are deleted. Its chats and saved items are kept, outside any project.</p><div class="modal-actions">${btn('Keep project', 'close')}${btn('Delete project', 'project-delete-confirm', { tone: 'danger', attrs: `data-id="${esc(el.dataset.id)}"` })}</div>`),
    'project-delete-confirm': async el => { await api.request(path(el.dataset.id), { method: 'DELETE' }); close(); toast('Project deleted'); await list(true); go('projects'); },
    'project-chat': el => { newChat(); chat.draft = el.dataset.id; render(); },
    // Research runs unattended, so its chat starts in Auto mode where that's allowed.
    'project-research': el => { newChat({ auto: true }); chat.draft = el.dataset.id; },
    'project-draft-clear': () => { chat.draft = null; code?.resetDraft(); render(); },
    'project-collections': collectionsPicker,
    'project-collection': el => toggleCollection(el.dataset.id),
    'project-move': mover,
    'project-place': el => place(el.dataset.id),
    'project-saved-new': el => saveTo({ kind: 'note', projectId: el.dataset.id }),
    'project-library': el => openLibrary(el.dataset.id),
    'project-pin': async el => {
      const item = local.detail?.saved.find(i => i.id === el.dataset.id); if (!item) return;
      await api.request(`/api/saved/${encodeURIComponent(item.id)}`, { method: 'PATCH', body: { pinned: !item.pinned } });
      await load(local.detail.project.id);
    },
  };
  function clear() { local.sequence++; Object.assign(local, { list: null, groups: [], detail: null, error: '', collections: null }); Object.assign(chat, { id: null, project: null, draft: null }); }
  /** Moves a chat into a new project named after the research it will run. */
  async function researchHome(id, goal) {
    const name = String(goal || '').trim().replace(/[.\s]+$/, '').slice(0, 120) || 'Research';
    const made = await api.request('/api/projects', { method: 'POST', body: { name, description: 'Research started from a chat.', color: 'violet', scope: 'personal' } });
    const r = await api.request(`/api/conversations/${encodeURIComponent(id)}/project`, { method: 'PUT', body: { projectId: made.project.id } });
    if (chat.id === id) chat.project = r.project;
    await list(true).catch(() => {});
    render();
    return r.project;
  }
  return { load, list, researchHome, page, sidebar, editor, submit, actions, clear, loadChat, crumb, pill, menuItems, composerChip, contextSection, applyDraft, chatProject, choices: () => local.list || [], resetDraft: () => { chat.draft = null; }, refresh: () => list(true).then(render).catch(() => {}) };
}
