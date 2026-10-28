import { esc, icon, when, btn, badge, empty, errorBox, field, formEnd } from './manage-ui.js';

const MAX_IMPORT_BYTES = 200 * 1024 * 1024;
const SOURCES = { 'open-webui': 'Open WebUI', chatgpt: 'ChatGPT', claude: 'Claude' };
const count = (n, one, many = `${one}s`) => `${Number(n).toLocaleString('en-US')} ${n === 1 ? one : many}`;
const were = n => n === 1 ? 'was' : 'were';
/** What an import did, in plain sentences. */
export function importSummary(r) {
  const from = SOURCES[r.source] ? ` from ${SOURCES[r.source]}` : '';
  const lines = [];
  if (r.imported) lines.push(`Imported ${count(r.imported, 'chat')}${from}.`);
  else if (r.alreadyImported && !r.failed) lines.push(`Nothing new to import${from}. ${r.alreadyImported === 1 ? 'That chat is' : `All ${count(r.alreadyImported, 'chat')} are`} already here.`);
  else lines.push(`No chats were imported${from}.`);
  if (r.imported && r.alreadyImported) lines.push(`${count(r.alreadyImported, 'chat')} ${were(r.alreadyImported)} already here.`);
  if (r.overLimit) lines.push(`${count(r.overLimit, 'older chat')} ${were(r.overLimit)} left out because you can keep up to 1,000 chats. Delete some and import the file again to add them.`);
  if (r.alternativesLeftOut) lines.push(`${count(r.alternativesLeftOut, 'alternative reply or edit', 'alternative replies or edits')} ${were(r.alternativesLeftOut)} left out.`);
  if (r.messagesLeftOut) lines.push(`The earliest ${count(r.messagesLeftOut, 'message')} of very long chats ${were(r.messagesLeftOut)} left out.`);
  if (r.empty) lines.push(`${count(r.empty, 'empty chat')} ${were(r.empty)} skipped.`);
  if (r.failed) lines.push(`${count(r.failed, 'chat')} could not be imported: ${String(r.error || 'something went wrong').replace(/\.?$/, '.')} Import the same file again to add ${r.failed === 1 ? 'it' : 'them'}.`);
  return lines;
}
export function createHistory({ api, state, render, modal, close, toast, refreshSidebar }) {
  const local = { query: '', archived: 'false', offset: 0, conversations: [], total: 0, hasMore: false, loading: false, error: '', sequence: 0, selected: new Set() };
  async function load() {
    const sequence = ++local.sequence; local.loading = true; local.error = ''; render();
    try { const r = await api.request(`/api/conversations?query=${encodeURIComponent(local.query)}&archived=${local.archived}&limit=50&offset=${local.offset}`); if (sequence !== local.sequence) return; local.conversations = r.conversations || []; local.total = r.total ?? local.conversations.length; local.hasMore = !!r.hasMore; local.selected.clear(); if (local.offset > 0 && !local.conversations.length) { local.offset = Math.max(0, Math.floor(Math.max(0,local.total - 1) / 50) * 50); return load(); } }
    catch(e) { if (sequence === local.sequence && e.name !== 'AbortError') local.error = e.message; }
    finally { if (sequence === local.sequence) { local.loading = false; render(); } }
  }
  function view() {
    const n = local.selected.size;
    return `<div class="page history-page"><div class="page-head spread"><div><div class="eyebrow">Only you</div><h1>Your conversations</h1><p>Find, organize, and keep a copy of your work.</p></div>${btn('Import', 'history-import', { icon: 'upload', tone: 'small' })}</div><form id="history-search-form" class="history-search"><label class="sr-only" for="history-query">Search conversations</label><input class="form-control" id="history-query" name="query" placeholder="Search titles and messages…" value="${esc(local.query)}" maxlength="256"><label class="sr-only" for="history-archived">Conversation visibility</label><select class="form-control" id="history-archived" name="archived"><option value="false" ${local.archived === 'false' ? 'selected' : ''}>Active</option><option value="true" ${local.archived === 'true' ? 'selected' : ''}>Archived</option><option value="all" ${local.archived === 'all' ? 'selected' : ''}>All conversations</option></select><button class="btn" type="submit">Search</button></form>${errorBox(local.error, 'history-refresh')}<div class="history-bulk"><label class="history-select-all"><input type="checkbox" id="history-select-all" ${local.conversations.length && n === local.conversations.length ? 'checked' : ''} ${!local.conversations.length ? 'disabled' : ''}>${n ? `${n} selected` : 'Select page'}</label><div class="row">${btn('Export', 'history-export', { icon: 'download', tone: 'small', attrs: n ? '' : 'disabled' })}${btn(local.archived === 'true' ? 'Restore' : 'Archive', 'history-bulk-archive', { icon: 'archive', tone: 'small', attrs: n ? '' : 'disabled' })}${btn('Delete', 'history-delete', { icon: 'trash', tone: 'danger small', attrs: n ? '' : 'disabled' })}</div></div>${local.loading ? '<p class="inline-note" role="status">Loading conversations…</p>' : ''}${local.conversations.length ? `<div class="history-list">${local.conversations.map(c => `<article class="history-item"><input type="checkbox" aria-label="Select ${esc(c.title)}" data-history-select="${esc(c.id)}" ${local.selected.has(c.id) ? 'checked' : ''}><a class="grow" href="#chat/${encodeURIComponent(c.id)}"><h2>${c.pinned ? icon('pin') : ''}${esc(c.title)}${c.archived ? badge('Archived') : ''}</h2><p>${esc(state.models.find(m => m.id === c.model)?.name || c.model)}<span>·</span>${esc(when(c.updatedAt || c.createdAt))}</p></a><div class="row">${btn(c.pinned ? 'Unpin' : 'Pin', 'history-pin', { icon: 'pin', tone: 'ghost small', attrs: `data-id="${esc(c.id)}"` })}${btn(c.archived ? 'Restore' : 'Archive', 'history-archive', { icon: 'archive', tone: 'ghost small', attrs: `data-id="${esc(c.id)}"` })}</div></article>`).join('')}</div><div class="history-pagination"><span>${local.offset + 1}–${local.offset + local.conversations.length} of ${local.total}</span><div class="row">${btn('Previous', 'history-prev', { tone: 'small', attrs: local.offset ? '' : 'disabled' })}${btn('Next', 'history-next', { tone: 'small', attrs: local.hasMore ? '' : 'disabled' })}</div></div>` : !local.loading ? empty(local.query ? 'No conversations found' : local.archived === 'true' ? 'Nothing archived' : 'A fresh start', local.query ? 'Try another search or include archived conversations.' : 'Your conversations will appear here as you use Chat.') : ''}</div>`;
  }
  async function update(id, body) { const c = await api.request(`/api/conversations/${encodeURIComponent(id)}`, { method: 'PATCH', body }); if (state.active?.id === id) Object.assign(state.active, c.conversation || c); await refreshSidebar(); if (location.hash.startsWith('#history')) await load(); else render(); }
  async function exportChats(ids) { if (!ids.length) return; const archive = await api.request('/api/conversations/export', { method: 'POST', body: { ids } }); const url = URL.createObjectURL(new Blob([JSON.stringify(archive, null, 2)], { type: 'application/json' })); const a = document.createElement('a'); a.href = url; a.download = `obleth-chat-${new Date().toISOString().slice(0,10)}.json`; a.click(); setTimeout(() => URL.revokeObjectURL(url), 1000); if (archive.leftOut?.length) toast(`Exported without ${archive.leftOut.length === 1 ? `${archive.leftOut[0]}, which is` : `${archive.leftOut.length} files that are`} too large to include. The export says where ${archive.leftOut.length === 1 ? 'it was' : 'they were'}.`); }
  async function bulk(action) { await api.request('/api/conversations/bulk', { method: 'POST', body: { ids: [...local.selected], action } }); close(); await refreshSidebar(); await load(); toast(action === 'delete' ? 'Conversations deleted' : action === 'archive' ? 'Conversations archived' : 'Conversations restored'); }
  const actions = {
    'history-refresh': load,
    'history-prev': () => { local.offset = Math.max(0, local.offset - 50); return load(); },
    'history-next': () => { local.offset += 50; return load(); },
    'history-pin': el => { const c = local.conversations.find(c => c.id === el.dataset.id); return update(c.id, { pinned: !c.pinned }); },
    'history-archive': el => { const c = local.conversations.find(c => c.id === el.dataset.id); return update(c.id, { archived: !c.archived }); },
    'history-export': () => exportChats([...local.selected]),
    'history-bulk-archive': () => bulk(local.archived === 'true' ? 'unarchive' : 'archive'),
    'history-delete': () => modal(`Delete ${local.selected.size} conversation${local.selected.size === 1 ? '' : 's'}?`, `<p>This permanently deletes the selected conversations and their messages.</p><div class="modal-actions">${btn('Keep conversations', 'close')}${btn('Delete conversations', 'history-confirm-delete', { tone: 'danger' })}</div>`),
    'history-confirm-delete': () => bulk('delete'),
    'history-import': () => modal('Import conversations', `<form id="history-import-form"><p class="inline-note">Bring your chats over from Chat, Open WebUI, ChatGPT or Claude. They are added to your account, and your existing conversations are kept.</p>${field('history-file', 'Export file', '<input id="history-file" name="history-file" type="file" class="form-control" accept=".json,.zip,application/json,application/zip" required>', 'Open WebUI: Settings, Data Controls, Export Chats. ChatGPT and Claude: the zip from Export data, or the conversations.json file inside it. Up to 200 MB.')}<p class="inline-note" id="history-import-status" role="status" hidden>Importing… A large export can take a few minutes.</p>${formEnd('Import conversations')}</form>`),
  };
  function input(event) {
    if (event.target.id === 'history-select-all') { local.selected = new Set(event.target.checked ? local.conversations.map(c => c.id) : []); render(); }
    if (event.target.dataset.historySelect) { if (event.target.checked) local.selected.add(event.target.dataset.historySelect); else local.selected.delete(event.target.dataset.historySelect); render(); }
  }
  async function submit(form) {
    if (form.id === 'history-search-form') { local.query = form.elements.query.value.trim(); local.archived = form.elements.archived.value; local.offset = 0; await load(); }
    if (form.id === 'history-import-form') {
      const sequence = local.sequence, file = form.elements['history-file'].files[0]; if (!file) return;
      if (file.size > MAX_IMPORT_BYTES) throw new Error(/\.zip$/i.test(file.name) ? 'This export is larger than 200 MB. Unzip it and choose the conversations.json file inside.' : 'Choose an export of 200 MB or less.');
      // The file goes as it is, only to this Chat; the server reads its format.
      const status = form.querySelector?.('#history-import-status'); if (status) status.hidden = false;
      let r;
      try { r = await api.request('/api/conversations/import', { method: 'POST', raw: file, contentType: file.type || 'application/octet-stream' }); }
      finally { if (status) status.hidden = true; }
      if (sequence !== local.sequence) return;
      close(); await refreshSidebar(); await load();
      modal('Import finished', `${importSummary(r).map(line => `<p>${esc(line)}</p>`).join('')}<div class="modal-actions">${btn('Done', 'close', { tone: 'primary' })}</div>`);
    }
  }
  function clear() { local.sequence++; Object.assign(local, { query: '', archived: 'false', offset: 0, conversations: [], total: 0, hasMore: false, error: '', loading: false, selected: new Set() }); }
  return { view, load, clear, actions, submit, input, update, exportChats };
}
