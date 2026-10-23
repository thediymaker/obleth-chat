import { esc, icon, btn } from './manage-ui.js';

// Sharing a chat: read-only and live, with one group or with anyone signed in who has
// the link. The link is the chat's own address; people it is not shared with get a
// not-found page. Viewers can fork it into a chat of their own.

const CHOICES = [
  ['none', 'Only you', 'Nobody else can open it.'],
  ['group', 'A group', 'Members find it on the group’s page.'],
  ['signed_in', 'Anyone signed in with the link', 'Every account here can open it from the link.'],
];

export function createSharing({ api, modal, toast, onChange = () => {} }) {
  const path = id => `/api/conversations/${encodeURIComponent(id)}/share`;
  const link = id => `${location.origin}${location.pathname}#chat/${encodeURIComponent(id)}`;
  let current = null;

  function body(id, share, groups) {
    const scope = share.scope || 'none';
    const options = share.groupId && !groups.some(g => g.id === share.groupId) ? [...groups, { id: share.groupId, name: share.groupName || 'Current group' }] : groups;
    const radios = CHOICES.map(([value, label, note]) => {
      const off = value === 'group' && !options.length;
      return `<label class="check-option"><input type="radio" name="share-scope" value="${value}" ${scope === value ? 'checked' : ''} ${off ? 'disabled' : ''}><span><strong>${label}</strong><small>${esc(off ? 'You are not in any groups yet.' : note)}</small></span></label>`;
    }).join('');
    const select = options.length && scope === 'group' ? `<select id="share-group" class="form-control" aria-label="Group">${options.map(g => `<option value="${esc(g.id)}" ${g.id === share.groupId ? 'selected' : ''}>${esc(g.name)}</option>`).join('')}</select>` : '';
    const copy = scope === 'none' ? '' : `<div class="share-link"><input class="form-control" readonly value="${esc(link(id))}" aria-label="Link to this chat">${btn('Copy link', 'share-copy', { icon: 'link', tone: 'small primary', attrs: `data-id="${esc(id)}"` })}</div>`;
    return `<form id="share-form" class="share-form" data-id="${esc(id)}"><div class="knowledge-sharing">${radios}${select}</div>${copy}<p class="inline-note">${icon('lock')}People see the chat as it grows, read-only, and can fork it into their own copy. They see its messages, attachments and images, not your workspace, browser or connections.</p><div class="modal-actions"><button class="btn primary" type="button" data-action="close">Done</button></div></form>`;
  }

  async function open(id) {
    try {
      current = await api.request(path(id));
      modal('Share this chat', body(id, current.share, current.groups || []));
    } catch (e) { toast(e.message); }
  }

  /** Saves a change to the form as soon as it is made. */
  async function input(event) {
    const form = event.target.closest('#share-form');
    if (!form || !current) return false;
    const scope = form.querySelector('input[name="share-scope"]:checked')?.value || 'none';
    const group = form.querySelector('#share-group');
    // Picking "A group" first shares with the group it had, or the first one offered.
    const payload = { scope, ...(scope === 'group' ? { groupId: group?.value || current.share.groupId || current.groups?.[0]?.id } : {}) };
    try {
      const r = await api.request(path(form.dataset.id), { method: 'PUT', body: payload });
      current.share = r.share;
      form.outerHTML = body(form.dataset.id, r.share, current.groups || []);
      toast(r.share.scope === 'none' ? 'Only you can open it now' : r.share.scope === 'group' ? `Shared with ${r.share.groupName}` : 'Anyone signed in with the link can open it');
      onChange(form.dataset.id, r.share);
    } catch (e) { toast(e.message); }
    return true;
  }

  const actions = {
    'share-copy': async el => { await navigator.clipboard.writeText(link(el.dataset.id)); toast('Link copied'); },
  };
  return { open, input, actions };
}
