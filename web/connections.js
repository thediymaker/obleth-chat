import { esc, icon, badge, btn, empty, errorBox, field, textInput, checkbox, formEnd, getValue, isChecked, when } from './manage-ui.js';

// Connections the model can use in chats: MCP servers (Settings → MCP servers) and Slurm
// or Kubernetes clusters (the Clusters page). Both pages share these dialogs and actions.
export function createConnections({ api, state, capabilities, render, modal, close, toast, onChange = () => {} }) {
  const local = { loading: false, loaded: false, error: '', items: [], sequence: 0 };
  const isAdmin = () => state.user?.role === 'admin';
  const byId = id => local.items.find(c => c.id === id);

  async function load() {
    const sequence = ++local.sequence; local.loading = true; local.error = ''; render();
    try { const r = await api.request('/api/connections'); if (sequence === local.sequence) { local.items = r.connections || []; local.loaded = true; } }
    catch (e) { if (e.name !== 'AbortError' && sequence === local.sequence) local.error = e.message; }
    finally { if (sequence === local.sequence) { local.loading = false; render(); } }
  }
  function replace(item) { const i = local.items.findIndex(c => c.id === item.id); if (i >= 0) local.items[i] = item; else local.items.push(item); onChange(item); }

  const KINDS = {
    mcp: { label: 'MCP server', glyph: 'link', url: 'https://example.com/mcp', urlHelp: 'A Streamable HTTP MCP endpoint.', about: 'Tools from any server that speaks the Model Context Protocol.' },
    slurm: { label: 'Slurm cluster', glyph: 'server', url: 'https://slurm.example.edu:6820', urlHelp: 'The slurmrestd address, without /slurm/v0.0.x.', about: 'Look at partitions and jobs, and submit or cancel jobs as your Slurm user.' },
    kubernetes: { label: 'Kubernetes cluster', glyph: 'cube', url: 'https://k8s.example.edu:6443', urlHelp: 'The API server address from your kubeconfig.', about: 'Read and change resources with your own token, so the cluster’s RBAC decides what is allowed.' },
  };
  const kindOf = c => KINDS[c?.kind] ? c.kind : 'mcp';
  const native = c => kindOf(c) !== 'mcp';
  const clustersOn = () => !!capabilities.features?.clusters;
  const tokenHelp = {
    slurm: 'On a login node run scontrol token lifespan=2592000 and paste the part after SLURM_JWT=. It acts as the Slurm user named above.',
    kubernetes: 'For a service account run kubectl create token NAME -n NAMESPACE --duration=720h, or use the token from your kubeconfig.',
    mcp: 'Your personal token or API key for this server. It is sent as a Bearer token.',
  };

  function tools(c) {
    const list = c.tools || [];
    if (!list.length) return '';
    return `<details class="connection-tools"><summary>${list.length} tool${list.length === 1 ? '' : 's'}</summary><ul>${list.map(t => `<li><div class="spread"><code>${esc(t.title || t.name)}</code>${t.readOnly ? badge('Read-only') : badge(c.confirmActions ? 'Asks first' : 'Runs without asking', 'amber')}</div>${t.description ? `<p>${esc(t.description)}</p>` : ''}</li>`).join('')}</ul></details>`;
  }
  // A person's own token on a shared cluster, or a shared server set to personal tokens.
  function credential(c) {
    if (!c.personalTokens) return '';
    const id = `data-id="${esc(c.id)}"`;
    if (!c.credential) return c.enabled ? `<div class="connection-credential needs"><p>${icon('lock')}Add your own token to use this ${native(c) ? 'cluster' : 'server'}. Tools then act as you.</p>${btn('Add your token', 'connection-credential', { tone: 'small primary', attrs: id })}</div>` : '';
    if (!native(c)) return `<div class="connection-credential"><p>${icon('check')}Using <strong>your own token</strong></p><div class="user-actions">${btn('Replace token', 'connection-credential', { tone: 'small ghost', attrs: id })}${btn('Remove my token', 'connection-credential-remove', { tone: 'small ghost', attrs: id })}</div></div>`;
    const scope = kindOf(c) === 'slurm' ? `Slurm user ${c.credential.user}` : `namespace ${c.credential.namespace}`;
    return `<div class="connection-credential"><p>${icon('check')}Acting as <strong>${esc(c.credential.identity || c.credential.user || 'you')}</strong> · ${esc(scope)}</p><div class="user-actions">${btn('Replace token', 'connection-credential', { tone: 'small ghost', attrs: id })}${btn('Remove my token', 'connection-credential-remove', { tone: 'small ghost', attrs: id })}</div></div>`;
  }
  function row(c) {
    const kind = KINDS[kindOf(c)], changes = (c.tools || []).filter(t => !t.readOnly).length;
    const who = !c.shared && native(c) && c.identity ? `as ${c.identity}` : '';
    const facts = [!native(c) && c.serverName, who, changes ? `${changes} can make changes` : 'all read-only', c.checkedAt && `checked ${when(c.checkedAt)}`].filter(Boolean).join(' · ');
    const toggle = c.usable === false ? '' : `<button type="button" class="btn small ${c.active ? '' : 'primary'}" data-action="connection-toggle" data-id="${esc(c.id)}" aria-label="${c.active ? 'Stop using' : 'Use'} ${esc(c.name)} in my chats">${c.active ? 'On' : 'Off'}</button>`;
    return `<div class="connection-row ${c.active ? '' : 'is-off'}"><div class="connection-head">${icon(kind.glyph)}<div class="grow"><h3>${esc(c.name)} ${native(c) ? badge(kind.label.split(' ')[0]) : ''} ${c.shared ? badge('Shared') : ''} ${c.shared && !c.enabled ? badge('Off for everyone', 'amber') : ''}</h3><p class="connection-url" title="${esc(c.url)}">${esc(c.url)}</p><small>${esc(facts)}</small></div>${toggle}</div>${credential(c)}${tools(c)}${c.canManage ? `<div class="user-actions">${btn(native(c) ? 'Check' : 'Refresh tools', 'connection-refresh', { icon: 'refresh', tone: 'small ghost', attrs: `data-id="${esc(c.id)}"` })}${btn('Edit', 'connection-edit', { tone: 'small', attrs: `data-id="${esc(c.id)}"` })}${btn('Remove', 'connection-remove', { tone: 'small', attrs: `data-id="${esc(c.id)}"` })}</div>` : ''}</div>`;
  }
  // Settings → MCP servers.
  function view() {
    const servers = local.items.filter(c => !native(c));
    const own = servers.filter(c => !c.shared), shared = servers.filter(c => c.shared);
    const list = items => `<div class="connection-list">${items.map(row).join('')}</div>`;
    return `<div class="connections-page"><div class="settings-section-heading spread"><div><h2>MCP servers</h2><p>Servers that give the model extra tools in your chats. Tools that may change something ask you first.${clustersOn() ? ' Slurm and Kubernetes clusters are on the <a href="#clusters">Clusters</a> page.' : ''}</p></div>${btn('Add MCP server', 'connection-add', { icon: 'plus', tone: 'primary small' })}</div>${errorBox(local.error, 'connections-refresh')}${local.loading && !local.items.length ? '<p class="inline-note" role="status">Loading…</p>' : ''}<section class="setting-card card"><h2>Yours</h2>${own.length ? list(own) : empty('No MCP servers yet', 'Add one by its URL, such as https://example.com/mcp, with a token if it needs one.')}</section>${shared.length || isAdmin() ? `<section class="setting-card card"><h2>Shared with everyone</h2><p class="inline-note">${isAdmin() ? 'Added by administrators for every account. Shared servers may use internal addresses.' : 'Added by your administrators. Turn one off to keep it out of your chats.'}</p>${shared.length ? list(shared) : '<p class="inline-note">None yet.</p>'}</section>` : ''}</div>`;
  }

  function pickKind(kinds = Object.keys(KINDS)) {
    modal(kinds.includes('mcp') ? 'Add a connection' : 'Add a cluster', `<div class="connection-kinds">${Object.entries(KINDS).filter(([k]) => kinds.includes(k)).map(([k, v]) => `<button type="button" class="connection-kind" data-action="connection-kind" data-kind="${k}">${icon(v.glyph)}<span><strong>${esc(v.label)}</strong><small>${esc(v.about)}</small></span></button>`).join('')}</div>`);
  }
  function dialog(c = null, kind = kindOf(c)) {
    const shared = c ? c.shared : false, k = KINDS[kind], cluster = kind !== 'mcp';
    const ownToken = !cluster || !shared;
    const sharedNote = cluster ? 'Every account can use it with their own token. Only administrators can change it.' : 'Every account can use it. Only administrators can change it.';
    // A shared MCP server either uses the token entered here for everyone, or each
    // person adds their own and this one only lists the server's tools.
    const personalBox = !cluster && isAdmin() && (!c || shared) ? `<div class="connection-personal-tokens ${c ? '' : 'when-shared'}">${checkbox('connection-personal-tokens', 'Each person adds their own token', !!c?.personalTokens, 'For servers where tokens are personal. Calls then act as that person. The token above is used only to list the server’s tools.')}</div>` : '';
    const fields = [
      field('connection-name', 'Name', textInput('connection-name', c?.name || '', `maxlength="40" required placeholder="${esc(kind === 'mcp' ? 'Research tools' : kind === 'slurm' ? 'HPC cluster' : 'Lab cluster')}"`), 'Tools appear to the model under this name.'),
      field('connection-url', kind === 'mcp' ? 'Server URL' : kind === 'slurm' ? 'slurmrestd URL' : 'API server URL', textInput('connection-url', c?.url || '', `type="url" maxlength="2048" required placeholder="${esc(k.url)}"`), k.urlHelp),
      kind === 'kubernetes' ? field('connection-ca', 'Cluster CA certificate', `<textarea id="connection-ca" name="connection-ca" class="form-control mono" rows="3" maxlength="16384" placeholder="${c?.hasCa ? 'Saved. Leave empty to keep it' : '-----BEGIN CERTIFICATE-----'}"></textarea>`, 'PEM, from certificate-authority-data in your kubeconfig (base64-decoded). Leave empty if the server has a public certificate.') + (c?.hasCa ? checkbox('connection-clear-ca', 'Remove the saved certificate') : '') : '',
      !c && cluster && isAdmin() ? checkbox('connection-shared', 'Share with everyone', false, sharedNote) : '',
      kind === 'slurm' && !shared ? `<div class="connection-personal">${field('connection-user', 'Slurm user name', textInput('connection-user', c?.user || '', 'maxlength="63" autocomplete="off" placeholder="jdoe"'), 'The account the token belongs to. Jobs run as this user.')}</div>` : '',
      kind === 'kubernetes' && !shared ? `<div class="connection-personal">${field('connection-namespace', 'Namespace', textInput('connection-namespace', c?.namespace || '', 'maxlength="63" autocomplete="off" placeholder="From the token"'), 'Where tools work when the model names none. Empty: a service account’s own namespace, or default.')}</div>` : '',
      ownToken ? `<div class="connection-personal">${field('connection-token', 'Access token', textInput('connection-token', '', `type="password" autocomplete="off" maxlength="4096" placeholder="${c?.hasToken ? 'Saved. Leave empty to keep it' : cluster ? '' : 'Optional'}"`), c?.hasToken ? 'Enter a new token to replace the saved one.' : cluster ? `${tokenHelp[kind]} Stored encrypted and never shown again.` : 'Sent as a Bearer token. Stored encrypted and never shown again.')}${c?.hasToken && !cluster ? checkbox('connection-clear-token', 'Remove the saved token') : ''}</div>` : '',
      checkbox('connection-confirm', 'Ask before tools that may change things', c ? c.confirmActions : true, cluster ? 'Submitting, cancelling, applying and deleting. Reading always runs without asking.' : 'Read-only tools always run without asking.'),
      !c && !cluster && isAdmin() ? checkbox('connection-shared', 'Share with everyone', false, sharedNote) : '',
      personalBox,
      c && shared ? checkbox('connection-enabled', 'Available to everyone', c.enabled) : '',
    ];
    modal(c ? `Edit ${c.name}` : `Add a ${k.label}`, `<form id="connection-form" class="${cluster && !c ? 'cluster-form' : ''}" data-id="${esc(c?.id || '')}" data-kind="${kind}">${fields.join('')}${formEnd(c ? 'Save' : 'Connect')}</form>`);
  }
  function credentialDialog(c) {
    const kind = kindOf(c), mine = c.credential || {};
    modal(`Your token for ${c.name}`, `<form id="connection-credential-form" data-id="${esc(c.id)}"><p class="inline-note">Tools on this ${kind === 'mcp' ? 'server' : 'cluster'} act as you, with your permissions. The token is checked now, stored encrypted and never shown again.</p>${kind === 'mcp' ? '' : kind === 'slurm' ? field('credential-user', 'Slurm user name', textInput('credential-user', mine.user || state.user?.username || '', 'maxlength="63" autocomplete="off" required'), 'The account the token belongs to.') : field('credential-namespace', 'Namespace', textInput('credential-namespace', mine.namespace === 'default' ? '' : mine.namespace || '', `maxlength="63" autocomplete="off" placeholder="From the token"`), 'Where tools work when the model names none. Empty: a service account’s own namespace, or default.')}${field('credential-token', 'Token', textInput('credential-token', '', 'type="password" autocomplete="off" maxlength="4096" required'), tokenHelp[kind])}${formEnd(c.credential ? 'Replace token' : 'Save token')}</form>`);
  }

  async function submit(form) {
    const status = form.querySelector('.form-error'); if (status) status.textContent = 'Checking…';
    if (form.id === 'connection-credential-form') {
      const c = byId(form.dataset.id), body = { token: getValue(form, 'credential-token').trim() };
      if (kindOf(c) === 'slurm') body.user = getValue(form, 'credential-user').trim(); else if (kindOf(c) === 'kubernetes') body.namespace = getValue(form, 'credential-namespace').trim();
      const item = await api.request(`/api/connections/${encodeURIComponent(form.dataset.id)}/credential`, { method: 'PUT', body });
      replace(item); close(); render(); toast(`${item.name} ready${native(item) ? ` as ${item.credential?.identity || 'you'}` : ' with your token'}`);
      return;
    }
    if (form.id !== 'connection-form') return;
    const id = form.dataset.id, c = id ? byId(id) : null, kind = form.dataset.kind || 'mcp', token = getValue(form, 'connection-token').trim();
    const shared = c ? c.shared : isChecked(form, 'connection-shared');
    const body = { name: getValue(form, 'connection-name').trim(), url: getValue(form, 'connection-url').trim(), confirmActions: isChecked(form, 'connection-confirm') };
    if (!c) body.kind = kind;
    if (token && !(shared && kind !== 'mcp')) body.token = token; else if (isChecked(form, 'connection-clear-token')) body.token = '';
    if (!c && shared) body.shared = true;
    if (c?.shared) body.enabled = isChecked(form, 'connection-enabled');
    if (kind === 'mcp' && shared && isAdmin()) {
      const personal = isChecked(form, 'connection-personal-tokens');
      if (!c || personal !== !!c.personalTokens) body.options = personal ? { personalTokens: true } : {};
    }
    if (kind !== 'mcp') {
      const options = {}, ca = getValue(form, 'connection-ca').trim();
      if (kind === 'kubernetes' && (ca || isChecked(form, 'connection-clear-ca'))) options.ca = ca;
      if (!shared && kind === 'slurm') options.user = getValue(form, 'connection-user').trim();
      if (!shared && kind === 'kubernetes') options.namespace = getValue(form, 'connection-namespace').trim();
      const changed = !c || 'ca' in options || (options.user ?? c.user) !== c.user || (options.namespace ?? c.namespace) !== c.namespace;
      if (changed) body.options = options;
    }
    const item = await api.request(id ? `/api/connections/${encodeURIComponent(id)}` : '/api/connections', { method: id ? 'PATCH' : 'POST', body });
    replace(item); close(); render();
    if (id) toast('Connection saved');
    else if (kind === 'mcp' && item.personalTokens) { toast(`${item.name} shared. Add your own token to use it.`); credentialDialog(item); }
    else if (kind === 'mcp') toast(`${item.name} connected with ${item.tools.length} tool${item.tools.length === 1 ? '' : 's'}`);
    else if (item.shared) { toast(`${item.name} shared. Add your own token to use it.`); credentialDialog(item); }
    else toast(`${item.name} connected as ${item.identity || 'you'}`);
  }

  const actions = {
    'connections-refresh': load,
    'connection-add': () => dialog(null, 'mcp'),
    'cluster-add': () => pickKind(['slurm', 'kubernetes']),
    'connection-kind': el => dialog(null, el.dataset.kind),
    'connection-edit': el => dialog(byId(el.dataset.id)),
    'connection-credential': el => { const c = byId(el.dataset.id); if (c) credentialDialog(c); },
    'connection-credential-remove': async el => { replace(await api.request(`/api/connections/${encodeURIComponent(el.dataset.id)}/credential`, { method: 'DELETE' })); render(); toast('Your token was removed'); },
    'connection-toggle': async el => {
      const c = byId(el.dataset.id); if (!c) return;
      const body = c.shared ? { optedOut: c.active } : { enabled: !c.active };
      replace(await api.request(`/api/connections/${encodeURIComponent(c.id)}`, { method: 'PATCH', body })); render();
    },
    'connection-refresh': async el => {
      const item = await api.request(`/api/connections/${encodeURIComponent(el.dataset.id)}/refresh`, { method: 'POST' });
      replace(item); render(); toast(native(item) ? `${item.name} is reachable` : `${item.tools.length} tool${item.tools.length === 1 ? '' : 's'} found`);
    },
    'connection-remove': el => { const c = byId(el.dataset.id); modal(`Remove ${c?.name || 'this connection'}?`, `<p>${c?.shared ? 'It will disappear from everyone’s chats.' : 'Its tools will no longer be offered in your chats.'} ${c?.personalTokens ? 'Everyone’s saved tokens for it are deleted.' : 'Its saved token is deleted.'}</p><div class="modal-actions">${btn('Keep it', 'close')}${btn('Remove', 'connection-confirm-remove', { tone: 'danger', attrs: `data-id="${esc(el.dataset.id)}"` })}</div>`); },
    'connection-confirm-remove': async el => { await api.request(`/api/connections/${encodeURIComponent(el.dataset.id)}`, { method: 'DELETE' }); local.items = local.items.filter(c => c.id !== el.dataset.id); onChange({ id: el.dataset.id, removed: true }); close(); render(); toast('Connection removed'); },
  };
  // In chats: every active connection is on unless switched off for that chat. A new
  // chat keeps its choices locally until it is created.
  // Auto mode: the chat's tools run without asking. A new chat keeps it locally too.
  const chat = { id: null, list: null, draftOff: new Set(), auto: false, draftAuto: false };
  const autoAllowed = () => !!capabilities.features?.autoMode;
  const chatId = () => (location.hash.match(/^#chat\/([^/]+)/) || [])[1] || null;
  async function loadChat(id) {
    if (!capabilities.features?.connections) return;
    chat.id = id || null;
    try {
      if (id) { const r = await api.request(`/api/conversations/${encodeURIComponent(id)}/connections`); chat.list = r.connections || []; chat.auto = !!r.auto; }
      else { const r = await api.request('/api/connections'); chat.list = (r.connections || []).filter(c => c.active).map(c => ({ id: c.id, kind: c.kind, name: c.name, shared: c.shared, tools: c.tools.length, on: !chat.draftOff.has(c.id) })); chat.auto = chat.draftAuto; }
    } catch { chat.list = []; chat.auto = false; }
    if (chat.id === (chatId() || null)) render();
  }
  const listed = () => chat.id === chatId() ? chat.list || [] : [];
  function composerButton() {
    const list = listed();
    if (!list.length) return '';
    const on = list.filter(c => c.on).length, auto = chat.auto && autoAllowed();
    return `<button type="button" class="btn ghost small connections-toggle ${on ? 'on' : ''}" data-action="chat-connections" title="Clusters and MCP servers the model can use in this chat${auto ? '. Auto: tools run without asking' : ''}" aria-label="Tools: ${on} of ${list.length} on${auto ? ', Auto mode' : ''}" ${state.streaming ? 'disabled' : ''}>${icon('link')}<span class="option-text">Tools</span><span class="count">${on}/${list.length}</span>${auto ? '<span class="auto-badge">Auto</span>' : ''}</button>`;
  }
  function contextSection() {
    const list = listed();
    if (!list.length) return '';
    return `<section class="context-section"><span class="eyebrow">Tools</span>${chat.auto && autoAllowed() ? '<div class="detail-row"><span>Auto mode</span><span>Runs without asking</span></div>' : ''}${list.map(c => `<div class="detail-row"><span>${esc(c.name)}</span><span>${c.on ? `${c.tools} tool${c.tools === 1 ? '' : 's'}` : 'Off'}</span></div>`).join('')}<button class="btn small" data-action="chat-connections">${icon('link')}Choose tools</button></section>`;
  }
  function chooser() {
    const auto = autoAllowed() ? `<div class="setting-line auto-line">${icon('bolt')}<div class="grow"><h3>Auto mode</h3><p>Every tool runs without asking, including ones that submit, apply or delete. For long research runs you won’t be watching.</p></div><button type="button" class="switch ${chat.auto ? 'on' : ''}" role="switch" data-action="chat-auto-switch" aria-checked="${chat.auto}" aria-label="Auto mode"></button></div>` : '';
    modal('Tools in this chat', `<p class="inline-note">The model can use the clusters and MCP servers that are on. Changes apply from the next message.</p>${auto}<div class="chat-connections">${listed().map(c => `<div class="setting-line">${icon(KINDS[c.kind]?.glyph || 'link')}<div class="grow"><h3>${esc(c.name)} ${c.shared ? badge('Shared') : ''}</h3><p>${c.tools} tool${c.tools === 1 ? '' : 's'}</p></div><button type="button" class="switch ${c.on ? 'on' : ''}" role="switch" data-action="chat-connection-switch" data-id="${esc(c.id)}" aria-checked="${c.on}" aria-label="Use ${esc(c.name)} in this chat"></button></div>`).join('')}</div><div class="modal-actions">${clustersOn() ? '<a class="btn ghost small" href="#clusters">Clusters</a>' : ''}<a class="btn ghost small" href="#settings/mcp">MCP servers</a><button class="btn primary" data-action="close">Done</button></div>`);
  }
  async function switchOne(id) {
    const item = listed().find(c => c.id === id); if (!item) return;
    const on = !item.on;
    if (chat.id) await api.request(`/api/conversations/${encodeURIComponent(chat.id)}/connections`, { method: 'PATCH', body: { connectionId: id, on } });
    else if (on) chat.draftOff.delete(id); else chat.draftOff.add(id);
    item.on = on; chooser(); render();
  }
  async function switchAuto() {
    const auto = !chat.auto;
    if (chat.id) await api.request(`/api/conversations/${encodeURIComponent(chat.id)}/connections`, { method: 'PATCH', body: { auto } });
    else chat.draftAuto = auto;
    chat.auto = auto; chooser(); render();
  }
  /** A new chat was created: store the choices made before it existed. */
  async function applyDraft(id) {
    const off = [...chat.draftOff]; chat.draftOff.clear();
    if (chat.draftAuto) { chat.draftAuto = false; await api.request(`/api/conversations/${encodeURIComponent(id)}/connections`, { method: 'PATCH', body: { auto: true } }).catch(() => {}); }
    await Promise.all(off.map(connectionId => api.request(`/api/conversations/${encodeURIComponent(id)}/connections`, { method: 'PATCH', body: { connectionId, on: false } }).catch(() => {})));
  }
  /** Research turns Auto mode on for its chat, where the installation allows it. */
  async function enableAuto(id) {
    if (!autoAllowed()) return;
    await api.request(`/api/conversations/${encodeURIComponent(id)}/connections`, { method: 'PATCH', body: { auto: true } }).catch(() => {});
    if (chat.id === id) { chat.auto = true; render(); }
  }
  actions['chat-connections'] = () => chooser();
  actions['chat-connection-switch'] = el => switchOne(el.dataset.id);
  actions['chat-auto-switch'] = () => switchAuto();

  function clear() { local.sequence++; local.items = []; local.error = ''; local.loading = false; chat.id = null; chat.list = null; chat.draftOff.clear(); chat.auto = chat.draftAuto = false; }
  return { view, load, actions, submit, clear, items: () => local.items, loaded: () => local.loaded, loading: () => local.loading, error: () => local.error, kinds: KINDS, loadChat, composerButton, contextSection, applyDraft, enableAuto, resetDraft: () => { chat.draftOff.clear(); chat.draftAuto = false; }, startAuto: () => { if (autoAllowed()) chat.draftAuto = chat.auto = true; } };
}
