import { esc, icon, empty, errorBox } from './manage-ui.js';

// Groups: the people someone works with and what is shared with them. A project or
// collection shared with a group is open to everyone in it. Administrators create
// groups and add people under Settings → Administration → Groups.

const count = (n, one, many = `${one}s`) => `${Number(n || 0).toLocaleString()} ${n === 1 ? one : many}`;
const initials = name => String(name || '?').split(/\s+/).slice(0, 2).map(s => s[0] || '').join('').toUpperCase();

export function createGroups({ api, state, render }) {
  const local = { list: null, detail: null, canManage: false, error: '', sequence: 0 };

  async function load(id = '') {
    const sequence = ++local.sequence; local.error = ''; render();
    try {
      const r = await api.request('/api/groups');
      if (sequence !== local.sequence) return;
      local.list = r.groups || []; local.canManage = !!r.canManage;
      const chosen = id || local.list[0]?.id;
      local.detail = chosen ? await api.request(`/api/groups/${encodeURIComponent(chosen)}`) : null;
    } catch (e) { if (e.name !== 'AbortError' && sequence === local.sequence) { local.error = e.message; local.detail = null; } }
    finally { if (sequence === local.sequence) render(); }
  }

  function selected() {
    const d = local.detail;
    if (!d) return '';
    const g = d.group;
    const shared = [
      ...d.projects.map(p => `<a class="group-shared" href="#projects/${encodeURIComponent(p.id)}"><span class="project-dot" data-color="${esc(p.color)}"></span><span class="grow">${esc(p.name)}</span><span class="faint">project · ${p.mine ? 'yours' : esc(p.ownerName)}${p.saved ? ` · ${count(p.saved, 'saved item')}` : ''}</span></a>`),
      ...(d.chats || []).map(c => `<a class="group-shared" href="#chat/${encodeURIComponent(c.id)}">${icon('chat')}<span class="grow">${esc(c.title)}</span><span class="faint">chat · ${c.mine ? 'yours' : esc(c.ownerName)}</span></a>`),
      ...d.collections.map(c => `<a class="group-shared" href="#knowledge/${encodeURIComponent(c.id)}">${icon('book')}<span class="grow">${esc(c.name)}</span><span class="faint">collection · ${count(c.documents, 'doc')}</span></a>`),
    ];
    const head = `<div class="group-head"><span class="group-mark">${esc(initials(g.name))}</span><div class="grow"><h2>${esc(g.name)}</h2><p>${esc(g.description || `${count(g.members, 'person', 'people')} in this group.`)}</p></div></div>`;
    const sharedBox = `<section class="card project-section">${head}<span class="eyebrow">Shared with the group</span>${shared.length ? shared.join('') : '<p class="context-note">Nothing yet. Share a chat from its Share button, a project from its Edit dialog, or a collection from its page, and everyone here can use it.</p>'}</section>`;
    const people = `<section class="card project-section"><span class="eyebrow">People · ${d.members.length}</span>${d.members.map(m => `<div class="detail-row"><span class="avatar small">${esc(initials(m.displayName || m.username))}</span><span class="grow">${m.id === state.user?.id ? 'You' : esc(m.displayName || m.username)}</span><span class="faint">${esc(m.username)}</span></div>`).join('')}${d.canManage ? `<a class="btn small" href="#settings/admin/groups">${icon('settings')}Manage members</a>` : '<p class="context-note">An administrator adds and removes people.</p>'}</section>`;
    return `<div class="group-detail">${sharedBox}</div><div class="project-side">${people}</div>`;
  }

  function page(id) {
    const all = local.list;
    const head = `<div class="page-head spread"><div><h1>Work with your people.</h1><p>A group shares chats, projects and collections with everyone in it. Chats stay private until you share one.</p></div>${local.canManage ? `<a class="btn" href="#settings/admin/groups">${icon('settings')}Manage groups</a>` : ''}</div>`;
    if (all === null) return `<div class="page groups-page">${head}${errorBox(local.error, 'groups-reload')}<p class="inline-note" role="status">Loading groups…</p></div>`;
    if (!all.length) return `<div class="page groups-page">${head}${errorBox(local.error, 'groups-reload')}${empty(local.canManage ? 'No groups yet' : 'You are not in any groups', local.canManage ? 'Create a group and add people under Settings → Administration → Groups.' : 'An administrator adds people to groups. Once you are in one, projects and collections shared with it appear here.')}</div>`;
    const current = local.detail?.group.id || id;
    // Administrators see every group; the ones they are not in are marked.
    const rail = `<section class="card project-section group-rail"><span class="eyebrow">${local.canManage ? 'Groups' : 'Your groups'}</span>${all.map(g => `<a class="group-item ${g.id === current ? 'active' : ''}" href="#groups/${encodeURIComponent(g.id)}" ${g.id === current ? 'aria-current="page"' : ''}><span class="group-mark small">${esc(initials(g.name))}</span><span class="grow"><strong>${esc(g.name)}</strong><small>${g.member ? '' : 'Not a member · '}${count(g.members, 'person', 'people')}${g.projects ? ` · ${count(g.projects, 'project')}` : ''}${g.collections ? ` · ${count(g.collections, 'collection')}` : ''}${g.chats ? ` · ${count(g.chats, 'chat')}` : ''}</small></span></a>`).join('')}</section>`;
    return `<div class="page groups-page">${head}${errorBox(local.error, 'groups-reload')}<div class="groups-layout">${rail}${selected()}</div></div>`;
  }

  const actions = { 'groups-reload': () => load(location.hash.split('/')[1] || '') };
  function clear() { local.sequence++; Object.assign(local, { list: null, detail: null, canManage: false, error: '' }); }
  return { load, page, actions, clear };
}
