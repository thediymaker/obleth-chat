import { esc, icon, btn, field, empty, when } from './manage-ui.js';
const sizeLabel = n => n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`;
// Text a conversation can hold in all (store::MAX_CONTEXT_BYTES); pasted text over it can't be read whole.
const CONTEXT_BYTES = 100 * 1024;
const stamp = () => new Date().toTimeString().slice(0, 8).replaceAll(':', '.');
// Documents Chat reads the text of: what each is called, its icon and what its parts are.
const DOCUMENTS = { pdf: ['PDF', 'file', 'page'], docx: ['Word', 'file', ''], pptx: ['PowerPoint', 'monitor', 'slide'], xlsx: ['Excel', 'grid', 'sheet'] };
const DOCUMENT_BYTES = 20 * 1024 * 1024;
const extension = name => (name.match(/\.([^.]+)$/)?.[1] || '').toLowerCase();
// The icon and the line under a file's name: "PDF · 12 pages · 1.2 MB".
export function describe(a) {
  const doc = a.kind === 'document' && DOCUMENTS[extension(a.name)];
  if (!doc) return { glyph: a.kind === 'image' ? 'image' : 'file', detail: sizeLabel(a.size), note: '' };
  const [type, glyph, unit] = doc;
  const parts = unit && a.parts ? `${a.parts.toLocaleString()} ${unit}${a.parts === 1 ? '' : 's'}` : '';
  return { glyph, detail: [type, parts, sizeLabel(a.size)].filter(Boolean).join(' · '), note: a.searched ? 'Large document: Chat searches it instead of reading it all.' : '' };
}
const accepted = '.pdf,.docx,.pptx,.xlsx,.txt,.md,.markdown,.csv,.json,.jsonl,.yaml,.yml,.toml,.xml,.html,.css,.js,.jsx,.ts,.tsx,.py,.rs,.go,.java,.c,.h,.cpp,.hpp,.sh,.sql,.r,.jl,.ipynb,text/plain,text/markdown,application/json,image/png,image/jpeg,image/webp';
export function createAttachments({ api, state, render, modal, close, toast }) {
  let selected = [], uploading = false, sequence = 0, library = [];
  // Pasted text by attachment id, so its chip can show it.
  const pasted = new Map();
  const url = id => `/api/attachments/${encodeURIComponent(id)}`;
  // With more to add than files, the button opens a menu of what can be added instead.
  function toolbar(menu = false) { return `<input id="message-files" class="sr-only" type="file" accept="${accepted}" multiple tabindex="-1">${menu ? `<button type="button" class="icon-btn attachment-trigger" data-action="composer-add" aria-haspopup="menu" aria-expanded="false" ${state.streaming ? 'disabled' : ''} title="Attach files or add knowledge" aria-label="Attach or add">${icon('plus')}</button>` : `<button type="button" class="icon-btn attachment-trigger" data-action="attach-files" ${state.streaming || uploading || selected.length >= 4 ? 'disabled' : ''} title="Attach text, code, a document or an image (you can also paste or drop them)" aria-label="Attach files">${icon('paperclip')}</button>`}`; }
  function chips() { return selected.length || uploading ? `<div class="draft-attachments">${selected.map(a => { const d = describe(a); return `<div class="attachment-chip">${icon(d.glyph)}<span class="grow">${pasted.has(a.id) ? `<button type="button" class="chip-open" data-action="open-pasted" data-id="${esc(a.id)}" title="Show the pasted text"><strong>${esc(a.name)}</strong></button>` : `<strong>${esc(a.name)}</strong>`}<small>${esc(d.detail)}${pasted.has(a.id) ? ` · ${pasted.get(a.id).split('\n').length.toLocaleString()} lines` : ''}</small>${d.note ? `<small class="chip-note">${esc(d.note)}</small>` : ''}</span><button type="button" class="icon-btn" data-action="detach-file" data-id="${esc(a.id)}" aria-label="Remove ${esc(a.name)}" ${state.streaming ? 'disabled' : ''}>${icon('x')}</button></div>`; }).join('')}${uploading ? '<span class="upload-status" role="status">Uploading files…</span>' : ''}</div>` : ''; }
  function messageFiles(message) { return message.attachments?.length ? `<div class="message-attachments">${message.attachments.map(a => { const d = describe(a); return `<a class="message-attachment ${a.kind === 'image' ? 'has-image' : ''}" href="${esc(a.url || url(a.id))}" target="_blank" rel="noopener noreferrer" title="Open ${esc(a.name)}">${a.kind === 'image' ? `<img src="${esc(a.url || url(a.id))}" alt="${esc(a.name)}" loading="lazy">` : icon(d.glyph)}<span><strong>${esc(a.name)}</strong><small>${esc(d.detail)}</small>${d.note ? `<small>${esc(d.note)}</small>` : ''}</span>${icon('external')}</a>`; }).join('')}</div>` : ''; }
  async function upload(input) { const files = [...input.files]; input.value = ''; await add(files); }
  // Files from the picker, a paste or a drop. Pasted screenshots arrive as image.png.
  async function add(files, text) {
    files = files.map((file, i) => file.name === 'image.png' || !file.name ? new File([file], `Pasted image ${stamp()}${files.length > 1 ? `-${i + 1}` : ''}.${file.type.split('/')[1] || 'png'}`, { type: file.type }) : file);
    if (!files.length || state.streaming || uploading) return;
    if (selected.length + files.length > 4) { toast('Attach up to four files per message.'); return; }
    const epoch = sequence; uploading = true; render();
    try {
      for (const file of files) {
        const image = ['image/png','image/jpeg','image/webp'].includes(file.type);
        const doc = Boolean(DOCUMENTS[extension(file.name)]);
        if (file.size > (image ? 5 * 1024 * 1024 : doc ? DOCUMENT_BYTES : 256 * 1024)) throw new Error(image ? 'Images must be 5 MB or smaller.' : doc ? 'Documents must be 20 MB or smaller.' : 'Text and code files must be 256 KB or smaller.');
        const bytes = new Uint8Array(await file.arrayBuffer()); if (epoch !== sequence) return; let binary = ''; for (let i = 0; i < bytes.length; i += 16384) binary += String.fromCharCode(...bytes.subarray(i, i + 16384));
        const r = await api.request('/api/attachments', { method: 'POST', body: { name: file.name, mimeType: file.type || 'text/plain', dataBase64: btoa(binary) } });
        if (epoch !== sequence) return; const attachment = r.attachment || r; selected.push(attachment); if (text !== undefined) pasted.set(attachment.id, text);
      }
    } catch(e) { if (epoch === sequence && e.name !== 'AbortError') toast(e.message); }
    finally { if (epoch === sequence) { uploading = false; render(); } }
  }
  // Text too long for the message box, attached as a file instead; nothing is cut off.
  async function addText(text) {
    const bytes = new TextEncoder().encode(text).length;
    if (bytes > CONTEXT_BYTES) { toast(`That text is ${sizeLabel(bytes)}, and a conversation can hold ${sizeLabel(CONTEXT_BYTES)}. Paste a shorter part.`); return; }
    if (state.streaming || uploading) { toast(state.streaming ? 'Paste it again once the reply has finished.' : 'Paste it again once the files have uploaded.'); return; }
    await add([new File([text], `Pasted text ${stamp()}.txt`, { type: 'text/plain' })], text);
  }
  function libraryBody() { return `<p>Files are private to your account. Attach text, code, documents (PDF, Word, PowerPoint, Excel) or images to a conversation.</p>${library.length ? `<div class="attachment-library">${library.map(a => `<div class="stored-attachment">${icon(describe(a).glyph)}<div class="grow"><h3>${esc(a.name)}</h3><p>${esc(describe(a).detail)}${a.createdAt ? ' · ' + esc(when(a.createdAt)) : ''}</p></div><div class="user-actions">${btn('Attach', 'attach-existing', { tone: 'small', attrs: `data-id="${esc(a.id)}" ${selected.length >= 4 || selected.some(s => s.id === a.id) ? 'disabled' : ''}` })}${btn('Delete', 'delete-file', { tone: 'small', attrs: `data-id="${esc(a.id)}"` })}</div></div>`).join('')}</div>` : empty('No uploaded files', 'Use the paperclip in Chat to attach a supported file.')}<div class="modal-actions">${btn('Done', 'close')}</div>`; }
  async function openLibrary() { modal('Your files', '<p role="status">Loading files…</p>'); const r = await api.request('/api/attachments'); library = r.attachments || []; modal('Your files', libraryBody()); }
  const actions = {
    'attach-files': () => document.querySelector('#message-files')?.click(),
    'detach-file': el => { selected = selected.filter(a => a.id !== el.dataset.id); pasted.delete(el.dataset.id); render(); },
    'open-pasted': el => { const a = selected.find(a => a.id === el.dataset.id); if (a && pasted.has(a.id)) modal(a.name, `<pre class="pasted-text">${esc(pasted.get(a.id))}</pre><div class="modal-actions">${btn('Close', 'close')}</div>`); },
    'personal-files': openLibrary,
    'attach-existing': el => { if (state.streaming || selected.length >= 4) return; const a = library.find(a => a.id === el.dataset.id); if (a && !selected.some(s => s.id === a.id)) selected.push(a); close(); location.hash = 'chat'; render(); toast('File attached to your next message'); },
    'delete-file': el => modal('Delete this file?', `<p>Files referenced by saved conversations cannot be deleted.</p><div class="modal-actions">${btn('Keep file', 'close')}${btn('Delete file', 'confirm-delete-file', { tone: 'danger', attrs: `data-id="${esc(el.dataset.id)}"` })}</div>`),
    'confirm-delete-file': async el => { await api.request(url(el.dataset.id), { method: 'DELETE' }); selected = selected.filter(a => a.id !== el.dataset.id); await openLibrary(); render(); toast('File deleted'); },
  };
  function restore(files) { selected = [...selected, ...files.filter(a => !selected.some(b => a.id === b.id))].slice(0,4); }
  function clear() { sequence++; selected = []; uploading = false; library = []; pasted.clear(); }
  return { toolbar, chips, messageFiles, upload, add, addText, actions, clear, current: () => [...selected], consume: () => { selected = []; }, restore, busy: () => uploading, full: () => uploading || selected.length >= 4 };
}
