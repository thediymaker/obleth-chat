import { esc, icon, btn, field, empty, when } from './manage-ui.js';
const sizeLabel = n => n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`;
const accepted = '.txt,.md,.markdown,.csv,.json,.jsonl,.yaml,.yml,.toml,.xml,.html,.css,.js,.jsx,.ts,.tsx,.py,.rs,.go,.java,.c,.h,.cpp,.hpp,.sh,.sql,.r,.jl,.ipynb,text/plain,text/markdown,application/json,image/png,image/jpeg,image/webp';
export function createAttachments({ api, state, render, modal, close, toast }) {
  let selected = [], uploading = false, sequence = 0, library = [];
  const url = id => `/api/attachments/${encodeURIComponent(id)}`;
  function toolbar() { return `<input id="message-files" class="sr-only" type="file" accept="${accepted}" multiple tabindex="-1"><button type="button" class="icon-btn attachment-trigger" data-action="attach-files" ${state.streaming || uploading || selected.length >= 4 ? 'disabled' : ''} title="Attach text, code, or an image (you can also paste or drop them)" aria-label="Attach files">${icon('paperclip')}</button>`; }
  function chips() { return selected.length || uploading ? `<div class="draft-attachments">${selected.map(a => `<div class="attachment-chip">${icon(a.kind === 'image' ? 'image' : 'file')}<span class="grow"><strong>${esc(a.name)}</strong><small>${sizeLabel(a.size)}</small></span><button type="button" class="icon-btn" data-action="detach-file" data-id="${esc(a.id)}" aria-label="Remove ${esc(a.name)}" ${state.streaming ? 'disabled' : ''}>${icon('x')}</button></div>`).join('')}${uploading ? '<span class="upload-status" role="status">Uploading files…</span>' : ''}</div>` : ''; }
  function messageFiles(message) { return message.attachments?.length ? `<div class="message-attachments">${message.attachments.map(a => `<a class="message-attachment ${a.kind === 'image' ? 'has-image' : ''}" href="${url(a.id)}" target="_blank" rel="noopener noreferrer" title="Open ${esc(a.name)}">${a.kind === 'image' ? `<img src="${url(a.id)}" alt="${esc(a.name)}" loading="lazy">` : icon('file')}<span><strong>${esc(a.name)}</strong><small>${sizeLabel(a.size)}</small></span>${icon('external')}</a>`).join('')}</div>` : ''; }
  async function upload(input) { const files = [...input.files]; input.value = ''; await add(files); }
  // Files from the picker, a paste or a drop. Pasted screenshots arrive as image.png.
  async function add(files) {
    files = files.map((file, i) => file.name === 'image.png' || !file.name ? new File([file], `Pasted image ${new Date().toTimeString().slice(0, 8).replaceAll(':', '.')}${files.length > 1 ? `-${i + 1}` : ''}.${file.type.split('/')[1] || 'png'}`, { type: file.type }) : file);
    if (!files.length || state.streaming || uploading) return;
    if (selected.length + files.length > 4) { toast('Attach up to four files per message.'); return; }
    const epoch = sequence; uploading = true; render();
    try {
      for (const file of files) {
        const image = ['image/png','image/jpeg','image/webp'].includes(file.type);
        if (file.size > (image ? 5 * 1024 * 1024 : 256 * 1024)) throw new Error(image ? 'Images must be 5 MB or smaller.' : 'Text and code files must be 256 KB or smaller.');
        const bytes = new Uint8Array(await file.arrayBuffer()); if (epoch !== sequence) return; let binary = ''; for (let i = 0; i < bytes.length; i += 16384) binary += String.fromCharCode(...bytes.subarray(i, i + 16384));
        const r = await api.request('/api/attachments', { method: 'POST', body: { name: file.name, mimeType: file.type || 'text/plain', dataBase64: btoa(binary) } });
        if (epoch !== sequence) return; selected.push(r.attachment || r);
      }
    } catch(e) { if (epoch === sequence && e.name !== 'AbortError') toast(e.message); }
    finally { if (epoch === sequence) { uploading = false; render(); } }
  }
  function libraryBody() { return `<p>Files are private to your account. Attach text, code, or images to a conversation; document retrieval is not enabled.</p>${library.length ? `<div class="attachment-library">${library.map(a => `<div class="stored-attachment">${icon(a.kind === 'image' ? 'image' : 'file')}<div class="grow"><h3>${esc(a.name)}</h3><p>${sizeLabel(a.size)}${a.createdAt ? ' · ' + esc(when(a.createdAt)) : ''}</p></div><div class="user-actions">${btn('Attach', 'attach-existing', { tone: 'small', attrs: `data-id="${esc(a.id)}" ${selected.length >= 4 || selected.some(s => s.id === a.id) ? 'disabled' : ''}` })}${btn('Delete', 'delete-file', { tone: 'small', attrs: `data-id="${esc(a.id)}"` })}</div></div>`).join('')}</div>` : empty('No uploaded files', 'Use the paperclip in Chat to attach a supported file.')}<div class="modal-actions">${btn('Done', 'close')}</div>`; }
  async function openLibrary() { modal('Your files', '<p role="status">Loading files…</p>'); const r = await api.request('/api/attachments'); library = r.attachments || []; modal('Your files', libraryBody()); }
  const actions = {
    'attach-files': () => document.querySelector('#message-files')?.click(),
    'detach-file': el => { selected = selected.filter(a => a.id !== el.dataset.id); render(); },
    'personal-files': openLibrary,
    'attach-existing': el => { if (state.streaming || selected.length >= 4) return; const a = library.find(a => a.id === el.dataset.id); if (a && !selected.some(s => s.id === a.id)) selected.push(a); close(); location.hash = 'chat'; render(); toast('File attached to your next message'); },
    'delete-file': el => modal('Delete this file?', `<p>Files referenced by saved conversations cannot be deleted.</p><div class="modal-actions">${btn('Keep file', 'close')}${btn('Delete file', 'confirm-delete-file', { tone: 'danger', attrs: `data-id="${esc(el.dataset.id)}"` })}</div>`),
    'confirm-delete-file': async el => { await api.request(url(el.dataset.id), { method: 'DELETE' }); selected = selected.filter(a => a.id !== el.dataset.id); await openLibrary(); render(); toast('File deleted'); },
  };
  function restore(files) { selected = [...selected, ...files.filter(a => !selected.some(b => a.id === b.id))].slice(0,4); }
  function clear() { sequence++; selected = []; uploading = false; library = []; }
  return { toolbar, chips, messageFiles, upload, add, actions, clear, current: () => [...selected], consume: () => { selected = []; }, restore, busy: () => uploading };
}
