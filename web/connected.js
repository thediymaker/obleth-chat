import { icon, esc, badge, btn, iconButton, displayName } from './manage-ui.js';
import { branding, brand, brandLogo, brandIcon, siteFooter, copy as brandCopy, pageTitle } from './branding.js';
import { themeControl } from './theme-control.js';
import { ApiClient, readEvents } from './client.js';
import { markdown } from './markdown.js';
import { createSettings } from './settings.js';
import { createAdmin } from './admin.js';
import { createHistory } from './history.js';
import { createAttachments } from './attachments.js';
import { createWorkspace } from './workspace.js';
import { createBrowser } from './browser.js';
import { createConnections } from './connections.js';
import { createClusters } from './clusters.js';
import { createKnowledge } from './knowledge.js';
import { createLibrary } from './library.js';
import { createProjects } from './projects.js';
import { createGroups } from './groups.js';
import { createSharing } from './sharing.js';
import { createCode } from './code.js';
import { createQuestions } from './questions.js';
import { createResearch } from './research.js';
import { createTurn } from './turn.js';
import { generationValues } from './generation.js';
import { canonicalSettingsHash, settingsTabs } from './settings-navigation.js';
import { conversationBusy, isPendingMessage, mergeProgress, renderSignature, responseVersion, suggestionsPending } from './conversation-state.js';
export { markdown } from './markdown.js';

const efforts = ['none', 'low', 'medium', 'high'];
const effortLabel = value => value[0].toUpperCase() + value.slice(1);
const compactNumber = new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 1 });

const $ = (selector, root = document) => root.querySelector(selector);
// The sidebar: what people come to do. Other pages belong to one of these.
const nav = [['chat', 'chat', 'Chat'], ['projects', 'folder', 'Projects'], ['library', 'bookmark', 'Library'], ['compute', 'server', 'Compute'], ['groups', 'users', 'Groups']];
const sections = { history: 'chat', knowledge: 'library', workspaces: 'compute', clusters: 'compute' };
const libraryTabs = [['library', 'Saved', 'bookmark'], ['knowledge', 'Collections', 'book'], ['library/generated', 'Generated', 'image']];
const pageTitles = { history: 'All chats', knowledge: 'Library', workspaces: 'Compute', clusters: 'Compute' };
// Pages that moved: Playground is part of Chat, Classroom became Groups.
const moved = { playground: 'chat', classroom: 'groups' };

export async function startConnected(capabilities) {
  const state = {
    phase: 'loading', draftKey: 0, user: null, oidc: !!capabilities.integrations?.oidc?.configured,
    chats: [], models: [], model: '', draft: '', error: '', catalogError: '', chatError: '',
    active: null, loadingChat: false, streaming: false, streamController: null, streamChatId: null,
    context: true, compact: false, users: null, busy: false, loginError: '', shared: null,
    systemPrompt: '', temperature: 0.7, topP: 1, maxTokens: 2048, reasoningEffort: 'medium', preferences: null, preferenceOverrides: {}, presets: [],
    workspaceDraft: false, browserDraft: false, openActivity: new Set(),
  };
  let toastTimer, loadSequence = 0, identityEpoch = 0, menuAnchor = null, sharedSignature = '';
  let effortSaveQueue = Promise.resolve(), effortSequence = 0;
  const operations = new Map(), revisions = new Map(), watches = new Map(), versionSelections = new Map();
  let operationSequence = 0;
  const pendingEfforts = new Map();
  const thinkingOpen = new Map();
  const detailsOpen = new Set();
  const menu = document.createElement('div');
  menu.id = 'conversation-menu'; menu.className = 'conversation-menu'; menu.setAttribute('role', 'menu'); menu.setAttribute('aria-label', 'Conversation options'); menu.hidden = true; document.body.append(menu);
  const modelPopover = document.createElement('div');
  // Tabbing out of the model list closes it; Tab inside moves on to the effort slider.
  modelPopover.addEventListener('focusout', event => { if (!modelPopover.hidden && event.relatedTarget && !modelPopover.contains(event.relatedTarget)) closeModels(false); });
  modelPopover.id = 'model-popover'; modelPopover.className = 'model-popover'; modelPopover.setAttribute('role', 'dialog'); modelPopover.setAttribute('aria-label', 'Choose a model'); modelPopover.hidden = true; document.body.append(modelPopover);
  let modelAnchor = null;
  const api = new ApiClient({ onUnauthorized: error => expire(error.message) });
  const configured = key => !!capabilities.integrations?.[key]?.configured;
  const name = () => displayName(state.user, '');
  const initials = () => name().split(/\s+/).slice(0, 2).map(s => s[0]).join('').toUpperCase();
  const owner = () => { const n = state.shared?.ownerName || '?'; return { name: n, initials: n.split(/\s+/).slice(0, 2).map(s => s[0]).join('').toUpperCase() }; };
  const route = () => {
    const [rawPage = 'chat', id = '', section = 'users'] = canonicalSettingsHash(location.hash).slice(1).split('/');
    if (rawPage === 'settings' && id === 'admin') return { page: 'admin', id: section };
    if (rawPage === 'settings' && (id === 'mcp' || id === 'connections')) return { page: 'connections', id: '' };
    if (moved[rawPage]) return { page: moved[rawPage], id: '' };
    return { page: [...nav.map(n => n[0]), ...Object.keys(sections), 'settings', 'admin', 'activate'].includes(rawPage) ? rawPage : 'chat', id };
  };
  const modelName = id => state.models.find(m => m.id === id)?.name || id || 'Choose a model';
  const chatPath = id => `/api/conversations/${encodeURIComponent(id)}`;
  const sharedPath = id => `/api/shared/${encodeURIComponent(id)}`;
  const unwrap = data => data.conversation || data;
  const close = () => $('#modal').close();
  const toast = message => {
    $('#toast').textContent = message; $('#toast').classList.add('show');
    clearTimeout(toastTimer); toastTimer = setTimeout(() => $('#toast').classList.remove('show'), 5000);
  };
  const attachments = createAttachments({ api, state, render, modal, close, toast: message => toast(message) });
  const browserView = createBrowser({ api, state, route, toast: message => toast(message), chatPath, capabilities, render: () => render(), send: text => send(text), modal: (...args) => modal(...args), close: () => close(), assistantName: () => branding.assistantName });
  const codeView = createCode({ api, state, capabilities, render: () => render(), modal: (...args) => modal(...args), close: () => close(), toast: message => toast(message), chatPath, send: text => send(text), terminal: (id, command) => workspace.runInTerminal(id, command), moveToProject: () => projectsView.actions['project-move']() });
  const workspace = createWorkspace({ api, state, route, toast: message => toast(message), chatPath, capabilities, render: () => render(), modal: (...args) => modal(...args), close: () => close(), browser: browserView, details: id => state.active?.id === id ? chatDetails(state.active) : '', code: codeView });
  codeView.attach(() => workspace.update());
  browserView.attach({ update: () => workspace.update(), showTab: (id, tab) => workspace.showTab(id, tab), showing: (id, tab) => workspace.showing(id, tab) });
  const personal = createSettings({ api, state, render, modal, close, toast: message => toast(message), applyPreferences, applyPreset, workspaces: () => workspace.enabled(), browserCard: () => browserView.settingsCard() });
  const connectionsView = createConnections({ api, state, capabilities, render, modal, close, toast: message => toast(message), onChange: item => clustersView.changed(item) });
  const clustersView = createClusters({ api, route, render, connections: connectionsView });
  const knowledgeView = createKnowledge({ api, state, capabilities, render, modal, close, toast: message => toast(message), go });
  const libraryView = createLibrary({ api, state, capabilities, render, modal, close, toast: message => toast(message), go, projects: () => projectsView.choices() });
  const researchView = createResearch({ api, state, render, toast: message => toast(message), workspaces: () => workspace.enabled(), startResearch: id => {
    projectsView.actions['project-research']({ dataset: { id } }); state.workspaceDraft = true; state.draft = 'Let’s set up research in this project. The goal: ';
    render(); requestAnimationFrame(() => { const p = $('#prompt'); if (p) { p.focus(); p.setSelectionRange(p.value.length, p.value.length); } });
  } });
  const projectsView = createProjects({ api, state, capabilities, render, modal, close, toast: message => toast(message), go, saveTo: item => libraryView.saveFrom(item), openLibrary: id => { libraryView.filterProject(id); go('library'); }, newChat: (options = {}) => { actions['new-chat'](); if (options.auto) connectionsView.startAuto(); }, research: researchView, code: codeView });
  const groupsView = createGroups({ api, state, render });
  const sharing = createSharing({ api, modal, toast: message => toast(message), onChange: (id, share) => { if (state.active?.id === id) { state.active.shareScope = share.scope; render(); } } });
  const questionsView = createQuestions({
    state, send: text => send(text),
    workspace: { has: id => workspace.hasWorkspace(id), create: id => workspace.create(id) },
    // Start research: a project that can hold it, a workspace, and Auto mode.
    research: {
      allowed: () => !!capabilities.features?.workspaces,
      start: async (id, entry) => {
        if (!entry.inProject) await projectsView.researchHome(id, entry.goal);
        if (!workspace.hasWorkspace(id)) await workspace.create(id);
        await connectionsView.enableAuto(id);
      },
    },
  });
  const turnView = createTurn({ state, markdown, chatPath, route });
  const turnActions = Object.fromEntries(Object.entries(turnView.actions).map(([k, f]) => [k, el => { if (f(el) === 'render') render(); }]));
  const administration = createAdmin({ api, state, capabilities, render, modal, close, toast: message => toast(message), onIdentityChanged: session, onCatalogChanged: () => applyPreferences(state.preferences) });
  const historyView = createHistory({ api, state, render, modal, close, toast: message => toast(message), refreshSidebar });
  function modal(title, body) {
    closeMenu(false); closeModels(false);
    $('#modal').innerHTML = `<div class="modal-head"><h2 id="modal-title">${esc(title)}</h2>${iconButton('x', 'close', 'Close dialog')}</div><div class="modal-body">${body}</div>`;
    if (!$('#modal').open) $('#modal').showModal();
  }
  function go(path) {
    close(); document.body.classList.remove('sidebar-open');
    if (location.hash === '#' + path) navigate(); else location.hash = path;
  }
  function clearPrivateState() {
    loadSequence++; identityEpoch++;
    for (const op of operations.values()) op.controller?.abort();
    operations.clear(); revisions.clear(); versionSelections.clear();
    for (const timer of watches.values()) clearTimeout(timer); watches.clear();
    Object.assign(state, { user: null, usage: null, chats: [], models: [], model: '', active: null, shared: null, users: null, draft: '', streaming: false, streamChatId: null, error: '', chatError: '', catalogError: '', systemPrompt: '', reasoningEffort: 'medium', temperature: 0.7, topP: 1, maxTokens: 2048, compact: false, context: true, preferences: null, preferenceOverrides: {}, presets: [], workspaceDraft: false, browserDraft: false });
    state.openActivity.clear(); workspace.reset(); browserView.reset();
    document.body.classList.remove('collapsed', 'sidebar-open');
    delete capabilities.adminGatewayUrl; delete capabilities.gatewayAdminUrl;
    api.clearSession(); codeView.clear(); attachments.clear(); personal.clear(); connectionsView.clear(); clustersView.clear(); knowledgeView.clear(); libraryView.clear(); projectsView.clear(); groupsView.clear(); questionsView.clear(); researchView.clear(); hideSelection(); administration.clear(); historyView.clear(); thinkingOpen.clear(); detailsOpen.clear(); pendingEfforts.clear(); closeMenu(false); closeModels(false); close();
  }
  function expire(message) {
    clearPrivateState(); state.phase = 'login'; state.loginError = message; state.busy = false;
    history.replaceState(null, '', '#chat'); render();
  }
  function alert(message, action = '') {
    return message ? `<div class="connection-notice" role="alert">${icon('server')}<span class="grow">${esc(message)}</span>${action ? btn('Retry', action, { icon: 'refresh', tone: 'small', type: '' }) : ''}</div>` : '';
  }
  function policyModel() {
    return state.models.find(m => m.id === state.model);
  }
  function isLocked(key) {
    const model = policyModel();
    return model?.lockedParameters?.includes(key) || (Array.isArray(model?.allowedOverrides) && !model.allowedOverrides.includes(key));
  }
  function applyModelPolicy(all = false) {
    const model = policyModel();
    for (const key of ['systemPrompt','temperature','topP','maxTokens','reasoningEffort']) if ((all || isLocked(key)) && model?.defaults?.[key] != null) state[key] = model.defaults[key];
  }
  function resetGeneration(conversation = {}) {
    Object.assign(state, generationValues(policyModel(), state.preferenceOverrides, conversation));
  }
  function applyPreferences(preferences, { generation = true, overrides = state.preferenceOverrides } = {}) {
    if (!preferences) return;
    state.preferenceOverrides = overrides || {};
    const preferred = state.models.find(m => m.id === preferences.defaultModel);
    state.preferences = { ...preferences, ...generationValues(preferred, state.preferenceOverrides) };
    state.compact = preferences.density === 'compact'; state.context = preferences.context !== false;
    document.body.classList.toggle('collapsed', !!preferences.focus);
    if (generation) {
      state.model = state.models.some(m => m.id === preferences.defaultModel) ? preferences.defaultModel : state.models[0]?.id || '';
      resetGeneration();
    }
  }
  async function savePreference(body) {
    const r = await api.request('/api/preferences', { method: 'PATCH', body }); applyPreferences(r.preferences, { generation: false, overrides: r.overrides }); render();
  }
  function applyPreset(preset) {
    if (!preset) return;
    if (!state.models.some(m => m.id === preset.model)) { toast('This preset’s model is not available to your account. Edit the preset to choose another model.'); return; }
    state.model = preset.model; resetGeneration(preset.parameters || {}); close(); go('chat'); toast(`Applied ${preset.name}`);
  }
  function modelChoices(query = '') {
    const favorites = state.preferences?.favorites || [];
    const choices = state.models.filter(m => [m.name,m.id,m.description,...(m.tags || [])].join(' ').toLowerCase().includes(query.toLowerCase())).sort((a,b) => (a.id === 'auto' ? -2 : favorites.includes(a.id) ? -1 : 0) - (b.id === 'auto' ? -2 : favorites.includes(b.id) ? -1 : 0));
    const list = $('#model-choices', modelPopover);
    if (!list) return;
    list.innerHTML = choices.map(m => `<button class="model-option ${m.id === state.model ? 'selected' : ''}" role="option" aria-selected="${m.id === state.model}" data-action="select-model" data-value="${esc(m.id)}">${icon(favorites.includes(m.id) ? 'bookmark' : 'cube')}<span class="grow"><strong>${esc(m.name || m.id)}${m.isDefault ? badge('Default') : ''}${m.metadata?.supportsVision ? badge('Vision') : ''}</strong>${(() => { const detail = m.id === 'auto' ? 'Picks a permitted model for each request' : m.description || m.detail || (m.name && m.name !== m.id ? m.id : ''); return detail ? `<small>${esc(detail)}</small>` : ''; })()}</span>${m.id === state.model ? icon('check') : ''}</button>`).join('') || '<p class="model-empty">No matching models.</p>';
  }
  function authScreen() {
    return `<main id="main" class="auth-layout"><div class="auth-story">${brand()}<div class="auth-story-copy">${branding.login.eyebrow ? `<div class="greeting-mark">${brandIcon('greeting')}${brandCopy(branding.login.eyebrow)}</div>` : ''}${branding.login.title ? `<h1>${brandCopy(branding.login.title)}</h1>` : ''}${branding.login.description ? `<p>${brandCopy(branding.login.description)}</p>` : ''}</div>${branding.login.footer ? `<div class="auth-foot">${brandCopy(branding.login.footer)}</div>` : ''}</div><section class="auth-panel"><div class="auth-card">${state.phase === 'activate' ? `<div class="eyebrow">Your account</div><h2>Choose your password.</h2><p class="muted">Finish setting up access to ${esc(branding.siteName)}.</p><form id="activation-form"><label class="form-label" for="activation-password">New password</label><input class="form-control" id="activation-password" type="password" minlength="12" autocomplete="new-password" required><label class="form-label" for="activation-confirm">Confirm password</label><input class="form-control" id="activation-confirm" type="password" minlength="12" autocomplete="new-password" required><div class="form-error" role="alert"></div><button type="submit" class="btn primary auth-submit">Set password and sign in${icon('right')}</button></form>` : state.phase === 'password-required' ? `<div class="eyebrow">Account security</div><h2>Choose a new password.</h2><p class="muted">Your administrator requires a password change before you continue.</p><form id="required-password-form"><input type="hidden" autocomplete="username" value="${esc(state.user?.username)}"><label class="form-label" for="required-current">Current password</label><input class="form-control" id="required-current" type="password" autocomplete="current-password" required><label class="form-label" for="required-password">New password</label><input class="form-control" id="required-password" type="password" minlength="12" autocomplete="new-password" required><label class="form-label" for="required-confirm">Confirm password</label><input class="form-control" id="required-confirm" type="password" minlength="12" autocomplete="new-password" required><div class="form-error" role="alert"></div><button type="submit" class="btn primary auth-submit">Update password${icon('right')}</button></form>${btn('Sign out', 'logout', { tone: 'ghost small', type: '' })}` : state.phase === 'loading' ? `<div class="loading-mark">${brandIcon('loading')}</div><h2>Opening your workspace…</h2><p class="muted">Checking your secure session.</p>` : state.phase === 'error' ? `<h2>We couldn’t open your workspace.</h2><p class="muted">${esc(state.error)}</p>${btn('Try again', 'retry-session', { icon: 'refresh', tone: 'primary', type: '' })}` : `<div class="eyebrow">Welcome to ${esc(branding.siteName)}</div>${branding.login.welcomeTitle ? `<h2>${brandCopy(branding.login.welcomeTitle)}</h2>` : ''}${branding.login.welcomeDescription ? `<p class="muted">${brandCopy(branding.login.welcomeDescription)}</p>` : ''}${alert(state.loginError)}<form id="login-form"><label class="form-label" for="username">Username</label><input id="username" class="form-control" name="username" autocomplete="username" maxlength="128" required autofocus><label class="form-label" for="password">Password</label><input id="password" class="form-control" name="password" type="password" autocomplete="current-password" required><button class="btn primary auth-submit" type="submit" ${state.busy ? 'disabled' : ''}>${state.busy ? 'Signing in…' : 'Sign in'}${icon('right')}</button></form>${state.oidc ? `<div class="auth-divider"><span>or</span></div><a class="btn auth-oidc" href="/api/auth/oidc/login">${icon('shield')}Continue with single sign-on</a>` : ''}${branding.login.help ? `<p class="auth-help">${brandCopy(branding.login.help)}</p>` : ''}`}</div>${siteFooter()}</section></main>`;
  }
  /** The pages inside Compute this server offers: workspaces and clusters. */
  function computeTabs() {
    return [workspace.enabled() && ['workspaces', 'Workspaces', 'terminal'], capabilities.features?.clusters && ['clusters', 'Clusters', 'server']].filter(Boolean);
  }
  function tabBar(tabs, current, label) {
    return tabs.length > 1 ? `<nav class="section-tabs" aria-label="${label}">${tabs.map(([path, name, glyph]) => `<a href="#${path}" class="${current === path ? 'active' : ''}" ${current === path ? 'aria-current="page"' : ''}>${icon(glyph)}${name}</a>`).join('')}</nav>` : '';
  }
  /** Puts a section's tab bar right under the page heading, where every section shows it. */
  function withTabs(html, bar) {
    const start = html.indexOf('<div class="page-head');
    if (!bar || start < 0) return html;
    const tags = /<\/?div\b[^>]*>/g; tags.lastIndex = start;
    for (let depth = 0, m; (m = tags.exec(html));) {
      depth += m[0][1] === '/' ? -1 : 1;
      if (!depth) { const end = m.index + m[0].length; return html.slice(0, end) + bar + html.slice(end); }
    }
    return html;
  }
  function shell(content) {
    const { page, id } = route();
    const section = sections[page] || page, title = pageTitles[page] || nav.find(n => n[0] === page)?.[2] || 'Settings';
    return `<aside class="sidebar" aria-label="Main navigation">${brand()}<button class="btn primary new-chat" data-action="new-chat">${icon('plus')}New chat <kbd>⇧⌘O</kbd></button><nav class="nav">${nav.filter(([path]) => (path !== 'compute' || computeTabs().length) && (path !== 'library' || libraryView.enabled())).map(([path, i, label]) => `<a href="#${path}" class="nav-item ${section === path ? 'active' : ''}" ${section === path ? 'aria-current="page"' : ''}>${icon(i)}<span>${label}</span></a>`).join('')}</nav>${projectsView.sidebar(page === 'projects' ? id : '')}<div class="recent"><div class="recent-header spread"><span class="eyebrow">Recent</span>${iconButton('search', 'search', 'Search conversations')}</div>${state.chats.length ? state.chats.slice(0, 15).map(c => `<div class="recent-row ${id === c.id ? 'active' : ''}"><a class="recent-link" href="#chat/${encodeURIComponent(c.id)}" title="${esc(c.title)}">${esc(c.title)}</a>${iconButton('more', 'chat-menu', `Options for ${c.title}`, `data-id="${esc(c.id)}" aria-haspopup="menu" aria-expanded="false"`)}</div>`).join('') : '<p class="sidebar-empty">Your next idea starts here.</p>'}${state.chats.length ? `<a class="recent-all ${page === 'history' ? 'active' : ''}" href="#history">All chats${icon('right')}</a>` : ''}</div><div class="sidebar-bottom"><a class="nav-item ${['settings','admin','connections'].includes(page) ? 'active' : ''}" href="#settings" ${['settings','admin','connections'].includes(page) ? 'aria-current="page"' : ''}>${icon('settings')}Settings</a><button class="profile" data-action="profile" aria-haspopup="menu" aria-expanded="false"><span class="avatar">${esc(initials())}</span><span class="grow"><span class="profile-name">${esc(name())}</span><span class="profile-caption">${esc(state.user.role)} · Personal</span></span>${icon('down')}</button></div></aside><div class="app-main"><header class="topbar"><div class="breadcrumb"><button class="icon-btn mobile-menu" data-action="menu" aria-label="Toggle navigation">${icon('panel')}</button><span class="workspace-label">Personal workspace</span><span class="slash">/</span><span>${title}</span></div><div class="top-actions">${themeControl('header-theme')}${iconButton('search', 'search', 'Search conversations')}<button class="btn ghost small context-toggle" data-action="${page === 'chat' && id && !state.shared ? 'context' : 'focus'}">${icon('panel')}<span>${page === 'chat' && id && !state.shared ? 'Details' : 'Focus mode'}</span></button></div></header><main id="main" class="content" tabindex="-1">${alert(state.error, 'refresh')}${content}</main><footer class="bottom-bar"><div class="row">${statusMetrics()}</div><div class="row"><span>${esc(branding.siteName)}</span><button data-action="shortcuts" aria-label="Keyboard shortcuts">⌘ K</button></div></footer></div>`;
  }
  function statusMetrics() {
    const today = state.usage?.today, limits = state.usage?.limits || {}, count = value => compactNumber.format(Number(value) || 0);
    const usage = today ? `${count(today.requests)}${limits.dailyRequests ? ` / ${count(limits.dailyRequests)}` : ''} requests · ${count(today.totalTokens)}${limits.dailyTokens ? ` / ${count(limits.dailyTokens)}` : ''} tokens` : '—';
    return [['Models', state.catalogError ? 'Unavailable' : state.models.length ? `${state.models.filter(m => m.id !== 'auto').length} available` : '—'], ['Today', usage]].map(([label, value], i) => `${i ? '<span class="status-divider status-optional"></span>' : ''}<span class="status-metric ${i ? 'status-optional' : ''}" data-status="${label.toLowerCase()}"><span>${label}</span><span>${value}</span></span>`).join('');
  }
  async function refreshUsage() {
    const epoch = identityEpoch;
    try { const usage = await api.request('/api/usage'); if (epoch !== identityEpoch || !state.user) return; state.usage = usage; } catch { return; }
    const row = $('.bottom-bar .row'); if (row) row.innerHTML = statusMetrics();
  }
  function composer(home = false) {
    const available = state.models.length > 0, op = currentOperation();
    const hint = state.switchingVersion ? 'Switching response version…' : state.streaming ? op?.stopRequested ? 'Stopping the response…' : op?.phase === 'recovering' ? 'Reconnecting to the saved response…' : op?.phase === 'sending' ? 'Sending your message…' : 'Generating a response…' : 'AI can make mistakes. Check important information.';
    // What the model can use sits in one dock; on ones show their names while there is room.
    const dock = `${workspace.toggleButton()}${browserView.toggleButton()}${connectionsView.composerButton()}`;
    const knowledge = route().id ? '' : knowledgeView.pill(false);
    return `<form class="composer" id="chat-form">${projectsView.composerChip()}${knowledge ? `<div class="composer-chips">${knowledge}</div>` : ''}${attachments.chips()}<textarea id="prompt" aria-label="Message" rows="${home ? 3 : 2}" placeholder="${available ? 'Ask a question, write some code, follow an idea…' : 'Connect your gateway to start a conversation…'}" maxlength="12000" ${!available ? 'disabled' : ''}>${esc(state.draft)}</textarea><div class="composer-bottom"><div class="composer-options">${attachments.toolbar(knowledgeView.enabled())}${dock ? `<div class="dock" role="group" aria-label="What the model can use">${dock}</div>` : ''}</div>${engineButton(available)}${sendButton(op, available)}</div></form><div class="composer-hint">${esc(hint)}</div>`;
  }
  /** One pill for the model and its thinking effort; it opens the model list with effort and the other parameters. */
  function engineButton(available) {
    const name = modelName(state.model), level = efforts.indexOf(state.reasoningEffort);
    return `<button type="button" class="engine-button" data-action="models" aria-haspopup="dialog" aria-expanded="false" aria-controls="model-popover" aria-label="Model: ${esc(name)}. Thinking effort: ${effortLabel(state.reasoningEffort)}" title="${esc(name)} · thinking ${effortLabel(state.reasoningEffort).toLowerCase()}" ${!available || state.streaming ? 'disabled' : ''}>${icon('cube')}<span class="engine-name">${esc(name)}</span><span class="effort-bars" aria-hidden="true">${[1, 2, 3].map(i => `<i class="${level >= i ? 'lit' : ''}"></i>`).join('')}</span>${icon('down')}</button>`;
  }
  /** How much of the model's window the chat uses: the last reply's input and output. */
  function contextUse(c = state.active) {
    const usage = route().id && c ? [...(c.messages || [])].reverse().find(m => m.role === 'assistant' && m.usage)?.usage : null;
    const input = Number(usage?.prompt_tokens) || 0, output = Number(usage?.completion_tokens) || 0;
    const windowSize = Number(policyModel()?.metadata?.contextWindow) || 0, used = input + output;
    // Text runs about four bytes a token; the rest of the request is tools, instructions and page text.
    const bytes = (c?.messages || []).filter(m => m.status === 'completed').reduce((n, m) => n + new TextEncoder().encode(m.content || '').length, new TextEncoder().encode(state.systemPrompt).length);
    const messages = Math.round(bytes / 4);
    return { input, output, used, windowSize, share: windowSize ? Math.min(1, used / windowSize) : 0, bytes, messages, rest: Math.max(0, input - messages) };
  }
  const tokens = n => n < 1000 ? String(n) : n < 10000 ? `${(n / 1000).toFixed(1)}k` : n < 1e6 ? `${Math.round(n / 1000)}k` : `${(n / 1e6).toFixed(1)}M`;
  function sendButton(op, available) {
    const use = contextUse(), round = 2 * Math.PI * 17, level = use.share > .9 ? 'full' : use.share > .75 ? 'high' : '';
    const ring = `<svg class="context-ring" viewBox="0 0 40 40" aria-hidden="true"><circle cx="20" cy="20" r="17"></circle>${use.share ? `<circle class="used" cx="20" cy="20" r="17" stroke-dasharray="${Math.max(1.5, use.share * round).toFixed(1)} ${round.toFixed(1)}"></circle>` : ''}</svg>`;
    const amount = use.windowSize ? `${tokens(use.used)} / ${tokens(use.windowSize)}` : use.used ? tokens(use.used) : '—';
    const card = `<div class="context-card" id="context-card" role="tooltip"><div class="spread"><strong>Context</strong><span>${amount}</span></div>${use.windowSize ? `<progress class="meter" max="${use.windowSize}" value="${Math.min(use.used, use.windowSize)}" aria-label="Context used"></progress>` : ''}${use.used ? `<div class="detail-row"><span>Messages</span><span>about ${tokens(use.messages)}</span></div><div class="detail-row"><span>Tools, instructions and pages</span><span>about ${tokens(use.rest)}</span></div><div class="detail-row"><span>Last reply</span><span>${tokens(use.output)} out</span></div>${use.windowSize ? '' : '<p>This model does not report its window.</p>'}` : '<p>Shows after the first reply.</p>'}${route().id ? `<button type="button" class="context-card-link" data-action="context">Chat details${icon('right')}</button>` : ''}</div>`;
    return `<div class="send-wrap ${level}">${ring}<button class="send-btn" type="${state.streaming ? 'button' : 'submit'}" ${state.streaming ? 'data-action="stop"' : ''} ${(state.streaming ? op?.stopRequested : state.switchingVersion || !available || attachments.busy()) ? 'disabled' : ''} aria-label="${state.streaming ? 'Stop response' : 'Send message'}" aria-describedby="context-card">${icon(state.streaming ? 'stop' : 'arrow')}</button>${card}</div>`;
  }
  function home() {
    return `<div class="home"><div class="home-heading">${branding.home.eyebrow ? `<div class="greeting-mark">${brandIcon('greeting')}${brandCopy(branding.home.eyebrow)}</div>` : ''}${branding.home.title ? `<h1>${brandCopy(branding.home.title)}</h1>` : ''}${branding.home.description ? `<p>${brandCopy(branding.home.description)}</p>` : ''}</div>${alert(state.catalogError, 'refresh')}${composer(true)}<div class="shortcuts">${[...(workspace.enabled() ? [['terminal', 'Build an app', 'Describe it, then watch it run in a live preview.', 'Build me a site for ', true]] : [['code', 'Build something', 'Go from a question to working code.', 'Help me plan a small coding project. Start by asking what I want to build.']]), ['chat', 'Think it through', 'Find a fresh way into a tricky problem.', 'Help me think through a problem. Ask me what I am working on.'], ['book', 'Learn something', 'Make a complex idea feel approachable.', 'Help me learn something new. Ask what topic interests me.']].map(([i, title, copy, prompt, space]) => `<button class="shortcut" data-action="starter" data-prompt="${esc(prompt)}" ${space ? 'data-workspace="1"' : ''}>${icon(i)}<h3>${title}</h3><p>${copy}</p></button>`).join('')}</div><section class="home-recents"><div class="section-title"><h2>${state.chats.length ? 'Pick up where you left off' : 'A fresh start, just for you.'}</h2>${state.chats.length ? btn('All conversations', 'search', { icon: 'right', tone: 'ghost small', type: '' }) : ''}</div><div class="resume-list">${state.chats.slice(0, 4).map(c => `<a class="resume-row" href="#chat/${encodeURIComponent(c.id)}">${icon('chat')}<div class="grow"><div class="resume-title">${esc(c.title)}</div><div class="resume-desc">${esc(modelName(c.model))}</div></div>${icon('right')}</a>`).join('')}</div>${!state.chats.length ? '<p class="inline-note">Your conversations appear here after your first message.</p>' : ''}</section>${branding.home.footer ? `<div class="home-foot">${brandCopy(branding.home.footer)}</div>` : ''}</div>`;
  }
  function effortDescription() {
    return { none: 'Ask for a direct answer. Some models still reason.', low: 'Quick reasoning for everyday questions.', medium: 'A balance of depth and response time.', high: 'More room to work through complex problems.' }[state.reasoningEffort];
  }
  function effortControl() {
    const value = efforts.indexOf(state.reasoningEffort);
    return `<div class="effort-control" data-effort="${state.reasoningEffort}"><div class="effort-title"><label for="reasoning-effort">Thinking effort</label><span class="effort-mode" aria-hidden="true">${state.reasoningEffort === 'high' ? 'Warp speed' : ''}</span></div><div class="effort-slider"><div class="effort-track" aria-hidden="true"><span class="effort-progress"></span><span class="warp-streaks"><i></i><i></i><i></i><i></i><i></i></span><span class="effort-stops"><i></i><i></i><i></i><i></i></span></div><input id="reasoning-effort" ${state.streaming || isLocked('reasoningEffort') ? 'disabled' : ''} type="range" min="0" max="3" step="1" value="${value}" aria-valuetext="${effortLabel(state.reasoningEffort)}" aria-describedby="effort-description effort-help"><div class="effort-labels" aria-hidden="true">${efforts.map((v, i) => `<span class="${i === value ? 'selected' : ''}">${effortLabel(v)}</span>`).join('')}</div></div><p id="effort-description" class="effort-description">${effortDescription()}</p><p id="effort-help" class="effort-help">${isLocked('reasoningEffort') ? 'Thinking effort is locked by the model policy.' : 'Requested effort; model support varies.'}</p><button type="button" class="effort-more" data-action="parameters">${icon('settings')}<span class="grow">Temperature, output limit, instructions</span>${icon('right')}</button></div>`;
  }
  function thinking(m, index) {
    if (!m.reasoningContent) return '';
    const key = m.id || `pending-${index}`;
    const open = thinkingOpen.has(key) ? thinkingOpen.get(key) : m.status === 'streaming';
    return `<details class="thinking-panel" data-thinking="${esc(key)}" ${open ? 'open' : ''}><summary>${brandIcon('loading')}<span class="thinking-label">${m.status === 'streaming' && !m.content ? 'Thinking' : 'Thinking process'}</span>${icon('down')}</summary><div class="thinking-content">${markdown(m.reasoningContent)}</div></details>`;
  }
  function responseStatus(m) {
    if (isPendingMessage(m)) return currentOperation()?.stopRequested ? 'Stopping…' : (m.activity || []).some(s => s.status === 'awaiting') ? 'Waiting for your answer…' : m.reasoningContent && !m.content ? 'Thinking…' : m.content ? 'Responding…' : 'Waiting for the model…';
    const status = m.status || 'completed', model = m.settings?.resolvedModel || m.settings?.model;
    const usage = m.usage?.total_tokens ? `${Number(m.usage.total_tokens).toLocaleString()} tokens` : '';
    const label = { completed: '', interrupted: 'Stopped', cancelled: 'Stopped', error: 'Response failed' }[status] ?? status;
    const chosen = model && model !== 'auto' ? `${m.settings?.auto ? 'Auto · ' : ''}${modelName(model)}` : '';
    return [label, chosen, usage].filter(Boolean).join(' · ');
  }
  function versionControls(m) {
    const versions = m.versions || [];
    if (versions.length < 2 || state.shared) return '';
    const index = m.versionIndex || versions.findIndex(v => v.id === m.id) + 1;
    return `<div class="message-versions" role="group" aria-label="${m.role === 'user' ? 'Message' : 'Response'} versions">${iconButton('down', 'select-version', 'Previous version', `data-id="${esc(responseVersion(m,-1) || '')}" ${state.streaming || state.switchingVersion || !responseVersion(m,-1) ? 'disabled' : ''}`)}<span aria-label="Version ${index} of ${versions.length}">${index} / ${versions.length}</span>${iconButton('down', 'select-version', 'Next version', `data-id="${esc(responseVersion(m,1) || '')}" ${state.streaming || state.switchingVersion || !responseVersion(m,1) ? 'disabled' : ''}`)}</div>`;
  }
  function followUps(m, index) {
    if (state.shared || m.role !== 'assistant' || m.status !== 'completed' || index !== state.active?.messages?.length - 1) return '';
    const questions = (m.followUps || []).map((q,i) => ({q,i})).filter(({q}) => typeof q === 'string' && q.trim());
    const heading = '<div class="eyebrow">Suggested follow-ups</div>';
    if (questions.length) return `<section class="follow-ups" aria-label="Suggested follow-up questions">${heading}<div class="follow-up-list">${questions.map(({q,i}) => `<button class="follow-up-question" data-action="follow-up" data-index="${index}" data-question="${i}" ${state.streaming || state.switchingVersion ? 'disabled' : ''}><span>${esc(q)}</span>${icon('plus')}</button>`).join('')}</div></section>`;
    return m.followUpsStatus === 'pending' ? `<section class="follow-ups" role="status" aria-label="Finding follow-up ideas">${heading}<div class="follow-up-list" aria-hidden="true">${'<span class="follow-up-skeleton skeleton"></span>'.repeat(3)}</div></section>` : '';
  }
  function answerContent(m) {
    // A reply that ends by asking questions has no answer text on purpose.
    const thinkingOnly = m.role === 'assistant' && m.status === 'completed' && m.reasoningContent && !String(m.content || '').trim() && !(m.activity || []).some(e => e.kind === 'question' || e.kind === 'workspace' || e.kind === 'research');
    return (m.role === 'assistant' ? knowledgeView.cite(markdown(m.content), m) : markdown(m.content)) + (m.finishReason === 'length' && String(m.content || '').trim() ? '<p class="turn-notice">The response reached its output limit. Increase the limit in Parameters or lower thinking effort to leave more room for the answer.</p>' : '') + (m.errorMessage && m.status === 'error' ? `<p class="turn-notice">${esc(m.errorMessage)}</p>` : '') + (thinkingOnly ? '<p class="reasoning-only-note">The model returned thinking without a final answer. Try increasing the output limit in Parameters or lowering thinking effort.</p>' : '');
  }
  function turnDetails(m, key) {
    if (m.role !== 'assistant' || (!m.settings && !m.createdAt)) return '';
    const p = m.settings || {}, usage = m.usage || {}, count = value => value != null ? Number(value).toLocaleString() : null;
    const cells = [['Model', p.resolvedModel || p.model], ['Effort', p.reasoningEffort && effortLabel(p.reasoningEffort)], ['Temperature', p.temperature], ['Top P', p.topP], ['Output limit', p.maxTokens && `${count(p.maxTokens)} tokens`], ['Input tokens', count(usage.prompt_tokens)], ['Output tokens', count(usage.completion_tokens)], ['Finish', m.finishReason], ['Created', m.createdAt && new Date(m.createdAt).toLocaleString(), 'wide'], ['Request ID', m.requestId, 'wide']];
    return `<div class="message-details" id="details-${esc(key)}" ${detailsOpen.has(key) ? '' : 'hidden'}><div class="detail-grid">${cells.filter(([, v]) => v != null && v !== '').map(([k, v, wide]) => `<div class="detail-cell ${wide || ''}"><span>${esc(k)}</span><strong title="${esc(v)}">${esc(v)}</strong></div>`).join('')}</div>${p.systemPrompt ? `<div class="message-details-prompt"><span>System instructions</span><pre>${esc(p.systemPrompt)}</pre></div>` : ''}</div>`;
  }
  function savedNotes(m) { const html = libraryView.cards(m); return html ? `<div class="saved-notes">${html}</div>` : ''; }
  // Auto mode's watchdog writes these when an unattended turn stopped; they read as a status line.
  const NUDGE = '[Auto mode] ';
  function message(m, index) {
    if (m.role === 'user' && String(m.content || '').startsWith(NUDGE)) return `<article class="message auto-nudge" data-message="${index}"><p>${icon('refresh')}<span><strong>Auto mode</strong> ${esc(m.content.slice(NUDGE.length))}</span></p></article>`;
    const key = m.id || `pending-${index}`, details = turnDetails(m, key);
    const assistantActions = `${iconButton('copy', 'copy-message', 'Copy response', `data-index="${index}"`)}${m.id && libraryView.enabled() && m.status === 'completed' && !state.shared ? iconButton('bookmark', 'save-message', 'Save to Library', `data-index="${index}"`) : ''}${m.id && !state.streaming && !state.switchingVersion && !state.shared ? `${iconButton('refresh', 'retry-message', m.status === 'completed' ? 'Regenerate response' : 'Retry response', `data-index="${index}"`)}${iconButton('branch', 'branch', 'Branch conversation', `data-index="${index}"`)}` : ''}${details ? iconButton('info', 'toggle-details', 'Response details', `data-key="${esc(key)}" aria-controls="details-${esc(key)}" aria-expanded="${detailsOpen.has(key)}"`) : ''}`;
    const status = m.role === 'assistant' ? `<span class="response-meta ${isPendingMessage(m) ? 'is-generating' : ''} ${m.status === 'error' ? 'is-error' : ''}" ${isPendingMessage(m) ? 'role="status"' : ''}>${esc(responseStatus(m))}</span>` : '';
    return `<article class="message ${m.role === 'user' ? 'user' : 'assistant'}" data-message="${index}"><div class="message-heading">${m.role === 'user' ? `<span class="avatar">${esc(state.shared ? owner().initials : initials())}</span>${state.shared ? esc(owner().name) : 'You'}` : `${brandLogo() || brandIcon('assistant')}${esc(branding.assistantName)}`}</div><div class="message-text">${m.role === 'assistant' ? turnView.applies(m) ? turnView.before(m) + (turnView.live(m) && !state.shared ? turnView.steerForm() : '') : thinking(m, index) + workspace.activity(m) : ''}<div class="answer-content">${answerContent(m)}</div>${m.role === 'assistant' ? knowledgeView.sources(m) + savedNotes(m) + questionsView.card(m, index) + questionsView.workspaceCard(m, index) + questionsView.researchCard(m, index) + browserView.card(m, index) : ''}${turnView.applies(m) ? turnView.after(m) : ''}${attachments.messageFiles(m)}</div><div class="message-actions">${m.role === 'assistant' ? assistantActions : m.id && !state.streaming && !state.switchingVersion && !state.shared ? iconButton('edit', 'edit-message', 'Edit and resend message', `data-index="${index}"`) : ''}${versionControls(m)}${status}</div>${details}${followUps(m,index)}</article>`;
  }
  function updateMessage(m, index) {
    const article = $(`[data-message="${index}"]`);
    if (!article) return;
    const body = $('.message-text', article);
    if (!body) return;
    if (turnView.applies(m)) {
      $('.thinking-panel', article)?.remove(); $('.message-activity', article)?.remove();
      const signature = turnView.signature(m);
      if (article._turn !== signature) {
        const work = $('.turn-work', article), html = turnView.before(m);
        if (work) work.outerHTML = html; else body.insertAdjacentHTML('afterbegin', html);
        const below = $('.turn-after', article), belowHtml = turnView.after(m);
        if (below) { if (belowHtml) below.outerHTML = belowHtml; else below.remove(); }
        else if (belowHtml) $('.answer-content', article).insertAdjacentHTML('afterend', belowHtml);
        article._turn = signature;
      }
      const form = $('.steer-form', article);
      if (turnView.live(m) && !form && !state.shared) $('.turn-work', article).insertAdjacentHTML('afterend', turnView.steerForm());
      if (!turnView.live(m) && form) form.remove();
    }
    let panel = turnView.applies(m) ? null : $('.thinking-panel', article);
    if (m.reasoningContent && !panel && !turnView.applies(m)) { body.insertAdjacentHTML('afterbegin', thinking(m, index)); panel = $('.thinking-panel', article); }
    if (panel) {
      const content = $('.thinking-content', panel), atEnd = content.scrollTop + content.clientHeight >= content.scrollHeight - 40;
      if (content._renderedText !== m.reasoningContent) {
        content.innerHTML = markdown(m.reasoningContent); content._renderedText = m.reasoningContent;
        if (atEnd) content.scrollTop = content.scrollHeight;
      }
      const label = $('.thinking-label', panel), text = m.status === 'streaming' && !m.content ? 'Thinking' : 'Thinking process';
      if (label.textContent !== text) label.textContent = text;
    }
    const signature = workspace.activitySignature(m);
    if (!turnView.applies(m) && article._activity !== signature) {
      const current = $('.message-activity', article), html = workspace.activity(m);
      if (current) { if (html) current.outerHTML = html; else current.remove(); }
      else if (html) (panel ? panel.insertAdjacentHTML('afterend', html) : body.insertAdjacentHTML('afterbegin', html));
      article._activity = signature;
    }
    const answer = $('.answer-content', article);
    if (answer._renderedText !== m.content || answer._renderedStatus !== m.status || answer._renderedFinish !== m.finishReason || answer._renderedError !== m.errorMessage) {
      answer.innerHTML = answerContent(m); answer._renderedText = m.content; answer._renderedStatus = m.status; answer._renderedFinish = m.finishReason; answer._renderedError = m.errorMessage;
    }
    const notesSignature = libraryView.signature(m);
    if (article._notes !== notesSignature) {
      const current = $('.saved-notes', article), html = savedNotes(m);
      if (current) { if (html) current.outerHTML = html; else current.remove(); }
      else if (html) answer.insertAdjacentHTML('afterend', html);
      article._notes = notesSignature;
    }
    const questionSignature = questionsView.signature(m, index) + browserView.signature(m, index);
    if (article._questions !== questionSignature) {
      const [current, ...extra] = article.querySelectorAll('.question-card'), html = questionsView.card(m, index) + questionsView.workspaceCard(m, index) + questionsView.researchCard(m, index) + browserView.card(m, index);
      extra.forEach(card => card.remove());
      if (current) { if (html) current.outerHTML = html; else current.remove(); }
      else if (html) ($('.saved-notes', article) || $('.knowledge-sources', article) || answer).insertAdjacentHTML('afterend', html);
      article._questions = questionSignature;
    }
    const sourcesSignature = knowledgeView.signature(m);
    if (article._sources !== sourcesSignature || (sourcesSignature && answer._citedText !== m.content && !isPendingMessage(m))) {
      const current = $('.knowledge-sources', article), html = knowledgeView.sources(m);
      if (current) { if (html) current.outerHTML = html; else current.remove(); }
      else if (html) answer.insertAdjacentHTML('afterend', html);
      article._sources = sourcesSignature; answer._citedText = m.content;
    }
    const status = $('.response-meta', article), text = responseStatus(m);
    if (status.textContent !== text) status.textContent = text;
    status.classList.toggle('is-generating', isPendingMessage(m));
    status.classList.toggle('is-error', m.status === 'error');
  }
  function patchFollowUps(c) {
    const index = (c.messages?.length || 0) - 1, article = $(`[data-message="${index}"]`);
    if (!article || route().id !== c.id) return false;
    const current = $('.follow-ups', article), html = followUps(c.messages[index], index);
    if (current) { if (html) current.outerHTML = html; else current.remove(); }
    else if (html) article.insertAdjacentHTML('beforeend', html);
    return true;
  }
  /** Everything about the chat, shown in the side pane's Details tab. */
  function chatDetails(c) {
    const use = contextUse(c), selected = modelName(state.model || c.model), { bytes, messages, rest } = use, saved = c.messages?.length || 0;
    const resolved = [...(c.messages || [])].reverse().find(m => m.role === 'assistant' && m.settings?.resolvedModel)?.settings.resolvedModel;
    const stat = (label, value, attrs = '') => `<div><span>${label}</span><strong ${attrs}>${esc(value)}</strong></div>`;
    const contextSection = `<section class="context-section"><span class="eyebrow">Context</span>${use.used ? `<div class="details-total"><strong>${tokens(use.used)}</strong><span>${use.windowSize ? `of ${tokens(use.windowSize)} tokens` : 'tokens'}</span></div>${use.windowSize ? `<progress class="meter" max="${use.windowSize}" value="${Math.min(use.used, use.windowSize)}" aria-label="Context used"></progress>` : ''}<div class="detail-row"><span>Messages</span><span>about ${tokens(messages)}</span></div><div class="detail-row"><span>Tools, instructions and pages</span><span>about ${tokens(rest)}</span></div><div class="detail-row"><span>Last reply</span><span>${tokens(use.output)} out</span></div>` : '<p class="context-note">Shows after the first reply.</p>'}<p class="context-note">This chat holds ${(bytes / 1024).toFixed(1)} of 100 KiB of text and ${saved} of 200 messages.</p></section>`;
    const engine = `<section class="context-section"><div class="spread"><span class="eyebrow">Engine</span>${btn('Adjust', 'parameters', { icon: 'settings', tone: 'ghost small', type: '' })}</div><div class="context-stats details-stats">${stat('Model', selected, `title="${esc(selected)}"`)}${stat('Thinking', effortLabel(state.reasoningEffort), 'data-effort-summary')}${stat('Temperature', String(state.temperature))}${stat('Output', Number(state.maxTokens).toLocaleString())}</div>${resolved && modelName(resolved) !== selected ? `<p class="context-note">Last answer came from ${esc(modelName(resolved))}.</p>` : ''}<p class="context-note">Instructions: ${state.systemPrompt ? 'set for this chat' : 'your defaults'}.</p></section>`;
    return `<div class="chat-details">${contextSection}${engine}${projectsView.contextSection()}${knowledgeView.contextSection()}${connectionsView.contextSection()}</div>`;
  }
  function chatPage() {
    if (state.loadingChat) return '<div class="empty" role="status">Opening your conversation…</div>';
    if (state.chatError) return `<div class="page">${alert(state.chatError, 'reload-chat')}${btn('Start a new chat', 'new-chat', { icon: 'plus', type: '' })}</div>`;
    if (!state.active || !route().id) return home();
    const c = state.active;
    if (state.shared) {
      const sh = state.shared, where = sh.scope === 'group' && sh.groupName ? `shared with ${esc(sh.groupName)}` : 'shared by link';
      return `<div class="chat-layout chat-readonly"><div class="chat-column"><div class="conversation-header"><div class="conversation-header-title"><h2>${esc(c.title)}</h2>${badge(`Shared by ${sh.ownerName}`)}</div><div class="row">${btn('Fork', 'shared-fork', { icon: 'branch', tone: 'small', type: '' })}</div></div><div class="messages" id="messages">${(c.messages || []).map(message).join('')}${!c.messages?.length ? '<div class="empty">Nothing here yet. New messages show up as they arrive.</div>' : ''}</div><div class="shared-bar">${icon('lock')}<span class="grow">You are reading ${esc(sh.ownerName)}’s chat, ${where}. It updates as they keep going.</span>${btn('Fork to continue', 'shared-fork', { icon: 'branch', tone: 'primary small', type: '' })}</div></div></div>`;
    }
    return `<div class="chat-layout"><div class="chat-column"><div class="conversation-header"><div class="conversation-header-title"><h2>${esc(c.title)}</h2>${c.archived ? badge('Archived') : ''}${c.shareScope && c.shareScope !== 'none' ? `<button type="button" class="badge-button" data-action="share-chat" data-id="${esc(c.id)}" title="Change who can see it">${badge('Shared', 'accent')}</button>` : ''}</div><div class="row">${iconButton('link', 'share-chat', 'Share chat', `data-id="${esc(c.id)}"`)}${iconButton('branch', 'alternatives', 'Conversation alternatives')}${iconButton('more', 'chat-menu', 'Conversation options', `data-id="${esc(c.id)}" aria-haspopup="menu" aria-expanded="false"`)}</div></div><div class="chat-line">${projectsView.pill()}${knowledgeView.pill()}</div><div class="messages" id="messages">${(c.messages || []).map(message).join('')}${!c.messages?.length ? '<div class="empty">A new conversation, ready for your first idea.</div>' : ''}</div><div class="chat-composer-wrap">${alert(state.catalogError, 'refresh')}${composer()}</div></div></div>`;
  }
  function unavailable(page) {
    const copy = {
      knowledge: ['book', 'Your knowledge, connected.', 'Knowledge is not enabled.', 'Document collections with citations need object storage and pgvector on this server.'],
      library: ['bookmark', 'Everything worth keeping.', 'The library is not enabled.', 'Saved items and document collections need object storage and pgvector on this server.'],
      compute: ['server', 'Where your code runs.', 'Compute is not enabled.', 'Workspaces and cluster connections appear here once they are configured for this server.'],
      groups: ['users', 'Work with your people.', 'Groups are not enabled.', 'Shared projects and collections for teams and classes appear here once they are configured for this server.'],
      workspaces: ['terminal', 'A workspace for your next experiment.', 'Workspaces are not enabled.', 'Private Linux workspaces with a live preview appear here once Kubernetes is configured for this server.'],
      clusters: ['server', 'Your clusters, in chat.', 'Clusters are not enabled.', 'Slurm and Kubernetes clusters appear here once connections are configured for this server.'],
    }[page];
    return `<div class="page"><div class="page-head"><h1>${copy[1]}</h1><p>Your models. Your knowledge. Your compute.</p></div><section class="feature-empty card"><span class="tile-icon">${icon(copy[0])}</span>${badge('Not set up on this server')}<h2>${copy[2]}</h2><p>${copy[3]}</p>${btn('Back to chat', 'new-chat', { icon: 'chat', tone: 'primary', type: '' })}</section></div>`;
  }
  function settingsPage() {
    const page = route().page, section = page === 'admin' ? 'admin' : page === 'connections' ? 'connections' : 'personal';
    return `<div class="page settings-page"><div class="page-head"><h1>Settings</h1><p>Your account and preferences${state.user.role === 'admin' ? ', and how this installation runs' : ''}.</p></div>${settingsTabs(section, state.user.role === 'admin', !!capabilities.features?.connections)}${page === 'admin' ? administration.view() : page === 'connections' ? connectionsView.view() : personal.view()}</div>`;
  }
  function render() {
    syncStreamingState();
    const focusedPrompt = document.activeElement?.id === 'prompt' ? { start: document.activeElement.selectionStart, end: document.activeElement.selectionEnd, height: document.activeElement.style.height } : null;
    document.querySelectorAll('[data-thinking]').forEach(panel => thinkingOpen.set(panel.dataset.thinking, panel.open));
    closeMenu(false); closeModels(false);
    const ready = state.phase === 'ready';
    $('#app').classList.toggle('auth-mode', !ready);
    document.body.classList.toggle('compact', state.compact);
    const page = route().page;
    $('#app').innerHTML = ready ? shell(page === 'chat' ? chatPage() : page === 'history' ? historyView.view() : ['settings','admin','connections'].includes(page) ? settingsPage() : page === 'workspaces' && workspace.enabled() ? (route().id ? workspace.page() : withTabs(workspace.page(), tabBar(computeTabs(), 'workspaces', 'Compute'))) : page === 'clusters' && capabilities.features?.connections ? withTabs(clustersView.page(), tabBar(computeTabs(), 'clusters', 'Compute')) : page === 'knowledge' && knowledgeView.enabled() ? (route().id ? knowledgeView.page(route().id) : withTabs(knowledgeView.page(''), tabBar(libraryTabs, 'knowledge', 'Library'))) : page === 'projects' ? projectsView.page(route().id) : page === 'groups' ? groupsView.page(route().id) : page === 'library' && libraryView.enabled() ? libraryView.page(route().id, tabBar(libraryTabs, route().id === 'generated' ? 'library/generated' : 'library', 'Library')) : unavailable(page)) : authScreen();
    const sectionTabs = $('.admin-tabs'), activeSection = sectionTabs && $('[aria-current="page"]', sectionTabs);
    if (activeSection) { const outer = sectionTabs.getBoundingClientRect(), selected = activeSection.getBoundingClientRect(); sectionTabs.scrollLeft += selected.right > outer.right ? selected.right - outer.right : selected.left < outer.left ? selected.left - outer.left : 0; }
    if (focusedPrompt && $('#prompt')) { $('#prompt').focus({ preventScroll: true }); $('#prompt').setSelectionRange(focusedPrompt.start, focusedPrompt.end); $('#prompt').style.height = focusedPrompt.height; }
    workspace.update();
    document.title = pageTitle(ready ? state.active && page === 'chat' ? state.active.title : (pageTitles[page] || nav.find(n => n[0] === page)?.[2] || 'Settings') : 'Sign in');
  }
  async function session() {
    const epoch = identityEpoch;
    if (route().page === 'activate' && route().id) { clearPrivateState(); state.phase = 'activate'; state.loginError = ''; render(); return; }
    state.phase = 'loading'; state.error = ''; render();
    try {
      const result = await api.request('/api/session');
      if (epoch !== identityEpoch) return;
      state.oidc = result.oidcEnabled ?? state.oidc;
      if (!result.authenticated) { state.phase = 'login'; render(); return; }
      state.user = result.user; api.csrfToken = result.csrfToken;
      if (result.passwordChangeRequired) { state.phase = 'password-required'; render(); return; }
      state.phase = 'ready'; state.loginError = ''; render(); await refresh();
    } catch (error) {
      if (state.phase === 'login' || epoch !== identityEpoch) return;
      state.phase = 'error'; state.error = error.message; render();
    }
  }
  async function refresh() {
    const epoch = identityEpoch;
    state.error = ''; state.catalogError = '';
    const results = await Promise.allSettled([api.request('/api/conversations'), api.request('/api/models'), api.request('/api/preferences'), api.request('/api/presets'), api.request('/api/capabilities')]);
    if (!state.user || epoch !== identityEpoch) return;
    if (results[0].status === 'fulfilled') state.chats = results[0].value.conversations || [];
    else state.error = results[0].reason.message;
    if (results[1].status === 'fulfilled') {
      state.models = results[1].value.models || [];
      if (!state.models.some(m => m.id === state.model)) state.model = state.models[0]?.id || '';
      if (!state.models.length) state.catalogError = 'No models are available to this account. Contact your administrator.';
    } else state.catalogError = results[1].reason.message;
    if (results[2].status === 'fulfilled') { state.defaultAgentInstructions = results[2].value.defaultAgentInstructions || ''; applyPreferences(results[2].value.preferences, { overrides: results[2].value.overrides }); } else state.error = results[2].reason.message;
    if (results[3].status === 'fulfilled') state.presets = results[3].value.presets || []; else state.error = results[3].reason.message;
    if (results[4].status === 'fulfilled') Object.assign(capabilities, results[4].value);
    refreshUsage(); projectsView.refresh();
    await navigate();
  }
  async function navigate() {
    const canonical = canonicalSettingsHash(location.hash);
    if (canonical !== location.hash) history.replaceState(null, '', canonical);
    if (route().page === 'activate' && route().id) { clearPrivateState(); state.phase = 'activate'; render(); return; }
    if (state.phase !== 'ready') return;
    document.body.classList.remove('sidebar-open');
    let { page, id } = route();
    // Section pages open their first tab.
    const first = { compute: computeTabs()[0]?.[0] }[page];
    if (first) { history.replaceState(null, '', `#${first}`); ({ page, id } = route()); }
    if (moved[location.hash.slice(1).split('/')[0]]) history.replaceState(null, '', `#${page}`);
    state.chatError = ''; state.shared = null; loadSequence++;
    if (page !== 'knowledge') knowledgeView.stop();
    if (page !== 'projects') researchView.stop();
    if (page !== 'chat' || !id) {
      state.loadingChat = false; state.active = null; render();
      if (page === 'chat') { connectionsView.loadChat(null); knowledgeView.loadChat(null); projectsView.loadChat(null); }
      if (page === 'projects') await projectsView.load(id);
      if (page === 'groups') await groupsView.load(id);
      if (page === 'knowledge' && knowledgeView.enabled()) await knowledgeView.load(id);
      if (page === 'library' && libraryView.enabled()) await libraryView.load(id);
      if (page === 'settings') await personal.load();
      if (page === 'connections') await connectionsView.load();
      if (page === 'history') await historyView.load();
      if (page === 'clusters' && capabilities.features?.connections) await clustersView.load();
      if (page === 'workspaces' && workspace.enabled()) { if (id) workspace.load(id); await workspace.loadList(); }
      if (page === 'admin' && state.user.role === 'admin') await administration.load({ query: '' });
      return;
    }
    const running = operations.get(id);
    if (running?.conversation) {
      state.active = running.conversation; state.loadingChat = false; workspace.load(id); browserView.load(id); connectionsView.loadChat(id); knowledgeView.loadChat(id); projectsView.loadChat(id);
      if (state.models.some(m => m.id === state.active.model)) state.model = state.active.model;
      resetGeneration({ ...(state.active.settings || {}), reasoningEffort: state.active.reasoningEffort });
      render(); scrollToEnd(); scheduleWatch(id, 100); return;
    }
    const sequence = loadSequence, revision = revisions.get(id) || 0, requestedEffort = pendingEfforts.get(id); state.loadingChat = true; render();
    try {
      let c = unwrap(await api.request(chatPath(id)));
      if (sequence !== loadSequence || !state.user) return;
      if ((revisions.get(id) || 0) !== revision) { if (operations.get(id)?.conversation) c = operations.get(id).conversation; else { scheduleWatch(id, 0); return; } }
      const desiredEffort = pendingEfforts.get(id) || requestedEffort;
      if (desiredEffort) c.reasoningEffort = desiredEffort.value;
      state.active = c; workspace.load(c.id); browserView.load(c.id); connectionsView.loadChat(c.id); knowledgeView.loadChat(c.id); projectsView.loadChat(c.id);
      if (c.model && state.models.some(m => m.id === c.model)) state.model = c.model;
      resetGeneration({ ...(c.settings || {}), ...(efforts.includes(c.reasoningEffort) ? { reasoningEffort: c.reasoningEffort } : {}) });
      observeConversation(c);
    } catch (error) {
      if (sequence !== loadSequence || !state.user) return;
      state.active = null; state.chatError = error.message;
      // Not theirs: it may be someone else's chat shared with them.
      if (error.status === 404) {
        const r = await api.request(sharedPath(id)).catch(() => null);
        if (sequence !== loadSequence || !state.user) return;
        if (r && !r.mine) openShared(r, sequence);
        else state.chatError = 'This chat does not exist, or it is not shared with you.';
      }
    } finally {
      if (sequence === loadSequence && state.user) { state.loadingChat = false; render(); if (state.active) scrollToEnd(); }
    }
  }
  /** Someone else's chat, read-only. It is fetched again while open, so it grows live. */
  function openShared(r, sequence) {
    Object.assign(state, { active: r.conversation, shared: r.share, chatError: '' });
    connectionsView.loadChat(null); knowledgeView.loadChat(null); projectsView.loadChat(null);
    sharedSignature = signatureOf(r.conversation);
    watchShared(r.conversation.id, sequence);
  }
  const signatureOf = c => JSON.stringify([c.title, (c.messages || []).map(m => [m.id, m.status, m.content?.length, m.reasoningContent?.length, (m.activity || []).length])]);
  function watchShared(id, sequence) {
    const busy = (state.active?.messages || []).some(isPendingMessage);
    setTimeout(async () => {
      if (sequence !== loadSequence || !state.shared || state.active?.id !== id) return;
      if (!document.hidden) {
        try {
          const r = await api.request(sharedPath(id));
          if (sequence !== loadSequence || !state.shared) return;
          const next = signatureOf(r.conversation);
          if (next !== sharedSignature) {
            const list = $('#messages'), near = !list || list.scrollHeight - list.scrollTop - list.clientHeight < 160;
            sharedSignature = next; Object.assign(state, { active: r.conversation, shared: r.share }); render();
            if (near) scrollToEnd();
          }
        } catch (e) {
          if (e.status === 404 && sequence === loadSequence) { Object.assign(state, { active: null, shared: null, chatError: 'This chat is no longer shared with you.' }); render(); return; }
        }
      }
      watchShared(id, sequence);
    }, busy ? 1500 : 4000);
  }
  // Opening a conversation shows its latest message; a second frame catches late layout.
  function scrollToEnd() {
    const end = () => window.scrollTo(0, document.documentElement.scrollHeight);
    requestAnimationFrame(() => { end(); requestAnimationFrame(end); });
  }
  async function refreshSidebar() {
    const r = await api.request('/api/conversations'); state.chats = r.conversations || [];
  }
  function upsert(c) {
    state.chats = [...(c.archived ? [] : [c]), ...state.chats.filter(other => other.id !== c.id)].sort((a,b) => Number(!!b.pinned) - Number(!!a.pinned));
  }
  function currentOperation() {
    const r = route();
    return r.page === 'chat' ? operations.get(r.id || `draft:${state.draftKey}`) : null;
  }
  function syncStreamingState() {
    const op = currentOperation(), r = route();
    state.streaming = !!op || (r.page === 'chat' && r.id === state.active?.id && conversationBusy(state.active));
    state.switchingVersion = r.page === 'chat' && versionSelections.has(r.id);
    state.streamController = op?.controller || null;
    state.streamChatId = state.streaming ? op?.conversation?.id || r.id || null : null;
  }
  function touchConversation(id) { revisions.set(id, (revisions.get(id) || 0) + 1); }
  function operationIsCurrent(op) { return op.epoch === identityEpoch && operations.get(op.key) === op; }
  function showConversation(c) {
    upsert(c);
    if (route().page === 'chat' && route().id === c.id) state.active = c;
  }
  function observeConversation(c) {
    if (conversationBusy(c) && !operations.has(c.id)) {
      operations.set(c.id, { key: c.id, epoch: identityEpoch, conversation: c, generationId: c.activeGenerationId,
        assistant: c.messages.find(isPendingMessage), accepted: true, transportActive: false,
        phase: 'recovering', stopRequested: !!c.generationCancelRequested, stopSent: !!c.generationCancelRequested });
    }
    if (conversationBusy(c) || suggestionsPending(c)) scheduleWatch(c.id);
    // Auto mode may start a turn on its own; an open chat looks for it.
    else if (connectionsView.isAuto(c.id) && route().page === 'chat' && route().id === c.id) scheduleWatch(c.id, 30000);
  }
  function scheduleWatch(id, delay) {
    if (!id || !state.user) return;
    const op = operations.get(id);
    delay ??= op?.transportActive && op.accepted && !op.stopRequested && !op.terminal ? 12000 : route().id === id ? 1200 : 3000;
    clearTimeout(watches.get(id));
    watches.set(id, setTimeout(() => { watches.delete(id); reconcileConversation(id); }, delay));
  }
  function finishOperation(op, saved) {
    if (!operationIsCurrent(op)) return;
    operations.delete(op.key); op.finished = true; refreshUsage();
    clearTimeout(watches.get(op.key)); watches.delete(op.key);
    op.controller?.abort();
    if (saved) showConversation(saved);
    if ((op.rejected || op.restoreDraft) && op.kind === 'send' && route().page === 'chat' && route().id === saved?.id && !state.draft) {
      state.draft = op.prompt; attachments.restore(op.attached || []);
    }
    if (saved) observeConversation(saved);
  }
  async function reconcileConversation(id) {
    if (versionSelections.has(id)) { scheduleWatch(id); return; }
    const epoch = identityEpoch, revision = revisions.get(id) || 0, op = operations.get(id);
    try {
      const saved = unwrap(await api.request(chatPath(id)));
      if (epoch !== identityEpoch || !state.user) return;
      if (versionSelections.has(id) || (revisions.get(id) || 0) !== revision || (op && !operationIsCurrent(op))) { scheduleWatch(id); return; }
      const errorBefore = state.error;
      if (state.error === 'Connection interrupted. Reconnecting to the saved response…' && route().id === id) state.error = '';
      const ownMessage = op?.assistant?.id && saved.messages?.find(m => m.id === op.assistant.id);
      const authoritativeTerminal = ownMessage && !isPendingMessage(ownMessage);
      const unconfirmedTransport = op && !op.accepted && !op.transportActive;
      if (unconfirmedTransport && !op.rejected) {
        op.restoreDraft = saved.messages.every(m => op.beforeMessageIds.includes(m.id));
        if (op.restoreDraft && route().id === id) state.error = 'The connection ended before the message was confirmed. Your draft is ready to try again.';
      }
      if (op && (unconfirmedTransport || authoritativeTerminal || (!conversationBusy(saved) && (!op.transportActive || op.terminal)) || op.rejected)) {
        finishOperation(op, saved);
      } else if (op) {
        if (!op.transportActive || op.terminal) {
          op.conversation = mergeProgress(saved, op.conversation);
          op.assistant = op.conversation.messages.find(m => m.id === op.assistant?.id) || op.conversation.messages.find(isPendingMessage);
        } else {
          op.conversation.activeGenerationId = saved.activeGenerationId || op.generationId;
          op.conversation.generationCancelRequested = saved.generationCancelRequested;
        }
        if (saved.generationCancelRequested) op.stopRequested = true;
        showConversation(op.conversation);
        if (op.stopRequested && !op.stopSent && op.generationId) requestStop(op);
        scheduleWatch(id);
      } else {
        const shown = route().page === 'chat' && state.active?.id === id ? renderSignature(state.active) : null;
        showConversation(saved); observeConversation(saved);
        if (shown && shown === renderSignature(saved) && state.error === errorBefore && patchFollowUps(saved)) return;
      }
      if (route().page === 'chat' && route().id === id && (!op?.transportActive || op.finished || op.terminal)) render();
    } catch (error) {
      if (epoch !== identityEpoch || error.name === 'AbortError') return;
      if (versionSelections.has(id) || (revisions.get(id) || 0) !== revision) { scheduleWatch(id); return; }
      if (op && !operationIsCurrent(op)) return;
      if (op?.terminal) {
        op.conversation.activeGenerationId = null; op.conversation.generationCancelRequested = false;
        finishOperation(op, op.conversation);
        if (route().id === id) { state.error = 'The response finished, but saved details could not be refreshed. Try reloading when your connection returns.'; render(); }
      } else if (error.status === 404) {
        if (op) finishOperation(op); watches.delete(id);
        if (route().id === id) { state.chatError = error.message; render(); }
      } else {
        if (op) op.phase = 'recovering';
        if (route().page === 'chat' && route().id === id) { state.error = 'Connection interrupted. Reconnecting to the saved response…'; render(); }
        scheduleWatch(id, 3000);
      }
    }
  }
  async function requestStop(op) {
    if (!operationIsCurrent(op) || op.stopSent || !op.generationId || !op.conversation?.id) return;
    op.stopSent = true;
    try {
      const saved = unwrap(await api.request(`${chatPath(op.conversation.id)}/stop`, { method: 'POST', body: { generationId: op.generationId } }));
      if (!operationIsCurrent(op)) return;
      if (!conversationBusy(saved)) finishOperation(op, saved);
      else { op.conversation.generationCancelRequested = true; scheduleWatch(saved.id, 100); }
      if (route().id === saved.id) render();
    } catch (error) {
      if (!operationIsCurrent(op) || error.name === 'AbortError') return;
      op.stopSent = false; op.stopRequested = false;
      if (route().id === op.conversation?.id) render();
      toast(error.message);
    }
  }
  async function stopCurrent() {
    const op = currentOperation();
    if (!op || op.stopRequested) return;
    op.stopRequested = true;
    if (!op.conversation?.id) { op.rejected = true; op.controller?.abort(); }
    else if (op.generationId) await requestStop(op);
    else scheduleWatch(op.conversation.id, 100);
    render();
  }
  async function selectMessageVersion(messageId) {
    const c = state.active;
    if (!c || state.streaming || state.switchingVersion || !messageId) return;
    const epoch = identityEpoch; touchConversation(c.id);
    const revision = revisions.get(c.id); versionSelections.set(c.id, revision); render();
    try {
      const saved = unwrap(await api.request(`${chatPath(c.id)}/messages/${encodeURIComponent(messageId)}/select`, { method: 'POST' }));
      if (epoch !== identityEpoch || revisions.get(c.id) !== revision) return;
      touchConversation(c.id); showConversation(saved); observeConversation(saved);
    } finally {
      if (epoch === identityEpoch && versionSelections.get(c.id) === revision) {
        versionSelections.delete(c.id); if (route().id === c.id) render();
      }
    }
  }
  async function forkAndSend(messageId, mode, content) {
    const c = state.active;
    if (!c || state.streaming || state.switchingVersion) return;
    const index = c.messages.findIndex(m => m.id === messageId);
    if (index < 0) return;
    const userIndex = c.messages[index].role === 'user' ? index : index - 1;
    const user = c.messages[userIndex];
    if (!user || user.role !== 'user') return;
    const previousSettings = c.messages[index].settings || c.settings || {};
    await send(mode === 'edit' ? content : user.content, { kind: mode, messageId, userIndex, attached: user.attachments || [],
      settings: { ...previousSettings, model: state.model, systemPrompt: state.systemPrompt, temperature: state.temperature, topP: state.topP, maxTokens: state.maxTokens, reasoningEffort: state.reasoningEffort } });
  }
  async function send(prompt, options = {}) {
    syncStreamingState();
    prompt = String(prompt || '').trim();
    const kind = options.kind || 'send', attached = options.attached || attachments.current();
    if ((!prompt && !attached.length && kind !== 'regenerate') || state.streaming || state.switchingVersion || !state.model || !state.user || attachments.busy()) return;
    if (attached.some(a => a.kind === 'image') && state.model !== 'auto' && policyModel()?.metadata?.supportsVision !== true) { toast('Choose a vision-capable model to send an image.'); return; }
    const settings = options.settings || { model: state.model, systemPrompt: state.systemPrompt, temperature: state.temperature, topP: state.topP, maxTokens: state.maxTokens, reasoningEffort: state.reasoningEffort };
    const selected = route().page === 'chat' && route().id === state.active?.id ? state.active : null;
    const op = { key: selected?.id || `draft:${state.draftKey}`, draftKey: state.draftKey, sequence: ++operationSequence, epoch: identityEpoch,
      conversation: selected, controller: new AbortController(), transportActive: true, accepted: false, terminal: false,
      phase: 'sending', kind, prompt, attached, beforeMessageIds: selected?.messages?.map(m => m.id) || [], stopRequested: false, stopSent: false };
    operations.set(op.key, op); if (selected) touchConversation(selected.id);
    if (kind === 'send') { state.draft = ''; attachments.consume(); }
    state.error = ''; close(); render();
    let paintTimer, paintCost = 0;
    const paint = () => {
      clearTimeout(paintTimer); paintTimer = null;
      if (!operationIsCurrent(op) || state.active !== op.conversation || route().page !== 'chat' || !op.assistant) return;
      const follow = window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - 160, started = performance.now();
      updateMessage(op.assistant, op.conversation.messages.indexOf(op.assistant));
      paintCost = performance.now() - started;
      if (follow) requestAnimationFrame(() => window.scrollTo(0, document.documentElement.scrollHeight));
    };
    try {
      if (!op.conversation) {
        const c = unwrap(await api.request('/api/conversations', { method: 'POST', body: { title: (prompt || attached.map(a => a.name).join(', ')).slice(0,80), model: settings.model, reasoningEffort: settings.reasoningEffort }, signal: op.controller.signal }));
        if (!operationIsCurrent(op)) return;
        operations.delete(op.key); op.key = c.id; op.conversation = c; c.messages ||= []; operations.set(c.id, op); touchConversation(c.id); upsert(c);
        if (route().page === 'chat' && !route().id && state.draftKey === op.draftKey) { state.active = c; history.replaceState(null, '', `#chat/${encodeURIComponent(c.id)}`); }
        if (state.workspaceDraft) { state.workspaceDraft = false; await workspace.create(c.id); if (!operationIsCurrent(op)) return; }
        if (state.browserDraft) { state.browserDraft = false; await browserView.setOn(c.id, true); if (!operationIsCurrent(op)) return; }
        await Promise.all([connectionsView.applyDraft(c.id), knowledgeView.applyDraft(c.id), projectsView.applyDraft(c.id)]); connectionsView.loadChat(c.id); knowledgeView.loadChat(c.id); projectsView.loadChat(c.id);
      }
      const c = op.conversation; c.model = settings.model; c.reasoningEffort = settings.reasoningEffort; c.settings = { ...settings };
      if (kind === 'regenerate') {
        c.messages = c.messages.slice(0, options.userIndex + 1); op.userMessage = c.messages[options.userIndex];
      } else {
        if (kind === 'edit') c.messages = c.messages.slice(0, options.userIndex);
        op.userMessage = { role: 'user', content: prompt, attachments: attached, status: 'completed' }; c.messages.push(op.userMessage);
      }
      op.assistant = { role: 'assistant', content: '', reasoningContent: '', status: 'pending' }; c.messages.push(op.assistant);
      showConversation(c); render(); scheduleWatch(c.id);
      const body = { content: kind === 'regenerate' ? '' : prompt, ...settings, ...(kind === 'regenerate' ? { regenerateMessageId: options.messageId } : { attachmentIds: attached.map(a => a.id), ...(kind === 'edit' ? { editMessageId: options.messageId } : {}) }) };
      const response = await api.request(`${chatPath(c.id)}/messages`, { method: 'POST', body, signal: op.controller.signal, stream: true });
      for await (const event of readEvents(response.body)) {
        if (!operationIsCurrent(op)) return;
        const assistant = op.assistant;
        if (event.event === 'meta') {
          assistant._began ||= new Date().toISOString();
          op.accepted = true; op.phase = 'generating'; op.generationId = event.data.generationId; c.activeGenerationId = op.generationId; scheduleWatch(c.id);
          Object.assign(assistant, { id: event.data.messageId, requestId: event.data.requestId, settings: { ...event.data.settings, resolvedModel: event.data.resolvedModel } });
          if (event.data.userMessageId) op.userMessage.id = event.data.userMessageId;
          c.settings = event.data.settings || c.settings;
          if (route().id === c.id) render();
          if (op.stopRequested) requestStop(op);
        }
        // Auto: the gateway says which model it chose once the reply starts.
        if (event.event === 'model') { assistant.settings = { ...assistant.settings, resolvedModel: event.data.model, auto: true }; }
        if (event.event === 'delta') { assistant.content += event.data.content || ''; assistant.status = 'streaming'; }
        if (event.event === 'reasoning') { assistant.reasoningContent += event.data.content || ''; assistant.status = 'streaming'; assistant._draft = null; }
        browserView.observe(event, c.id);
        if (workspace.onEvent(event, assistant, c.id)) assistant.status = 'streaming';
        if (event.event === 'done') { Object.assign(assistant, event.data); assistant.status = event.data.status || 'completed'; op.terminal = true; }
        if (event.event === 'error') { assistant.status = 'error'; assistant.errorMessage = event.data.message || 'The model could not complete this response.'; op.terminal = true; }
        if (op.terminal) {
          const unread = turnView.unread(assistant);
          if (unread.length) { state.draft = [state.draft, ...unread].filter(Boolean).join('\n'); toast('The reply finished before reading your note, so it is back in the composer.'); }
          touchConversation(c.id); paint(); break;
        }
        if (!paintTimer) paintTimer = setTimeout(paint, Math.max(40, Math.min(250, paintCost * 4)));
      }
    } catch (error) {
      if (!operationIsCurrent(op)) return;
      if (error.name !== 'AbortError') {
        op.rejected = !!error.status;
        if (op.rejected) op.error = error.message;
        else op.phase = 'recovering';
      }
    } finally {
      clearTimeout(paintTimer);
      if (!operationIsCurrent(op)) return;
      op.transportActive = false;
      if (!op.conversation?.id) {
        operations.delete(op.key);
        if (route().page === 'chat' && !route().id && state.draftKey === op.draftKey) { if (!state.draft) { state.draft = prompt; attachments.restore(attached); } state.error = op.error || (op.stopRequested ? '' : 'The conversation could not be created. Please try again.'); render(); }
      } else {
        if (op.error && route().id === op.conversation.id) state.error = op.error;
        await reconcileConversation(op.conversation.id);
      }
    }
  }
  /** Opens the save dialog for part of the current chat, linked to its message. */
  function saveFromChat(el, item) {
    if (!libraryView.enabled() || !state.active) return;
    const m = state.active.messages[Number(el.closest('[data-message]')?.dataset.message)];
    // From someone else's chat it is saved as the viewer's own, without a link back.
    if (state.shared) { libraryView.saveFrom(item); return; }
    libraryView.saveFrom({ ...item, conversationId: state.active.id, messageId: m?.id, projectId: projectsView.chatProject() });
  }
  // Selecting text in a reply offers Save, Ask about this and Copy.
  const selection = { bar: null, pick: null };
  function hideSelection() { selection.pick = null; if (selection.bar) selection.bar.hidden = true; }
  function showSelection() {
    const sel = window.getSelection(), text = sel?.toString().trim();
    const node = sel?.rangeCount ? sel.getRangeAt(0).commonAncestorContainer : null;
    const answer = (node?.nodeType === 1 ? node : node?.parentElement)?.closest?.('.message.assistant .answer-content');
    if (!text || text.length < 3 || !answer || !libraryView.enabled()) { hideSelection(); return; }
    if (!selection.bar) {
      selection.bar = document.createElement('div'); selection.bar.className = 'selection-bar'; selection.bar.setAttribute('role', 'toolbar'); selection.bar.setAttribute('aria-label', 'Selected text');
      selection.bar.innerHTML = `<button type="button" data-action="selection-save">${icon('bookmark')}Save</button>${state.shared ? '' : `<button type="button" data-action="selection-ask">${icon('chat')}Ask about this</button>`}<button type="button" data-action="selection-copy">${icon('copy')}Copy</button>`;
      selection.bar.addEventListener('mousedown', event => event.preventDefault());
      document.body.append(selection.bar);
    }
    const rect = sel.getRangeAt(0).getBoundingClientRect();
    selection.pick = { text: text.slice(0, 20000), anchor: answer };
    selection.bar.hidden = false;
    const width = selection.bar.offsetWidth;
    selection.bar.style.left = `${Math.max(8, Math.min(window.innerWidth - width - 8, rect.left + rect.width / 2 - width / 2))}px`;
    selection.bar.style.top = `${Math.max(8, rect.top - selection.bar.offsetHeight - 8) + window.scrollY}px`;
  }
  document.addEventListener('mouseup', () => setTimeout(showSelection, 0));
  document.addEventListener('keyup', event => { if (event.shiftKey || event.key === 'Escape') setTimeout(showSelection, 0); });
  document.addEventListener('mousedown', event => { if (!selection.bar?.contains(event.target)) hideSelection(); });
  async function perform(action) { try { await action(); } catch (error) { if (state.user && error.name !== 'AbortError') toast(error.message); } }
  const actions = {
    close, 'retry-session': session, refresh, 'reload-chat': navigate,
    go: el => go(el.dataset.path),
    menu: () => { document.body.classList.remove('collapsed'); document.body.classList.toggle('sidebar-open'); },
    focus: () => savePreference({ focus: !document.body.classList.contains('collapsed') }),
    // With the side pane open there is no room to dock it, so it opens over the chat until dismissed.
    // Chat details open as a tab in the side pane, so they sit beside the chat instead of over it.
    context: () => { closeModels(false); closeMenu(false); workspace.toggleDetails(route().id); render(); },
    'project-menu': el => openListMenu(el, 'Project', `${projectsView.menuItems()}<button role="menuitem" class="menu-item" data-action="context">${icon('panel')}Chat details<kbd>⌘.</kbd></button>`),
    'composer-add': el => knowledgeView.enabled() ? openListMenu(el, 'Add to this message', `<button role="menuitem" class="menu-item" data-action="attach-files" ${attachments.full() ? 'disabled' : ''}>${icon('paperclip')}Attach files</button><button role="menuitem" class="menu-item" data-action="knowledge-picker">${icon('book')}Knowledge collections</button>`) : attachments.actions['attach-files'](),
    compact: () => savePreference({ density: state.compact ? 'comfortable' : 'compact' }),
    'new-chat': () => { state.draftKey++; state.draft = ''; state.workspaceDraft = false; state.browserDraft = false; connectionsView.resetDraft(); knowledgeView.resetDraft(); projectsView.resetDraft(); attachments.clear(); applyPreferences(state.preferences); go('chat'); requestAnimationFrame(() => $('#prompt')?.focus()); },
    starter: el => { if (route().page !== 'chat') { state.draftKey++; go('chat'); } state.draft = el.dataset.prompt; if (el.dataset.workspace && !route().id) state.workspaceDraft = true; render(); const prompt = $('#prompt'); if (prompt) { prompt.focus(); prompt.setSelectionRange(prompt.value.length, prompt.value.length); } },
    stop: stopCurrent,
    profile: openProfileMenu,
    logout: async () => { await api.request('/api/auth/logout', { method: 'POST' }); expire(''); document.body.classList.remove('collapsed', 'sidebar-open'); },
    models: el => openModels(el),
    // The model you pick becomes your default, so new chats start with the last one you used.
    'select-model': async el => { state.model = el.dataset.value; resetGeneration(); closeModels(); render(); if (state.preferences?.defaultModel !== state.model) await savePreference({ defaultModel: state.model }); },
    parameters: () => { closeModels(false); modal('Generation parameters', `<p>Applies to your next message. Locked controls are managed by your administrator.</p>${state.presets.length ? btn('Use a saved preset', 'presets', { icon: 'grid', tone: 'small', type: '' }) : ''}<form id="parameters-form"><label class="form-label" for="system-prompt">System instructions ${isLocked('systemPrompt') ? badge('Locked', 'accent') : ''}</label><textarea id="system-prompt" class="form-control" rows="4" maxlength="8192" ${isLocked('systemPrompt') ? 'disabled' : ''}>${esc(state.systemPrompt)}</textarea><div class="form-columns"><div><label class="form-label" for="temperature">Temperature ${isLocked('temperature') ? badge('Locked', 'accent') : ''}</label><input id="temperature" class="form-control" type="number" min="0" max="2" step="0.1" value="${state.temperature}" ${isLocked('temperature') ? 'disabled' : ''} required></div><div><label class="form-label" for="top-p">Top P ${isLocked('topP') ? badge('Locked', 'accent') : ''}</label><input id="top-p" class="form-control" type="number" min="0.01" max="1" step="0.01" value="${state.topP}" ${isLocked('topP') ? 'disabled' : ''} required></div></div><label class="form-label" for="max-tokens">Maximum output tokens ${isLocked('maxTokens') ? badge('Locked', 'accent') : ''}</label><input id="max-tokens" class="form-control" type="number" min="1" max="32768" step="1" value="${state.maxTokens}" ${isLocked('maxTokens') ? 'disabled' : ''} required><div class="modal-actions"><button class="btn primary" type="submit">Save parameters</button></div></form>`); },
    presets: () => modal('Saved presets', state.presets.map(p => `<button class="select-option" data-action="apply-preset" data-id="${esc(p.id)}">${icon('grid')}<span class="grow"><h3>${esc(p.name)}</h3><p>${esc(modelName(p.model))}</p></span>${icon('right')}</button>`).join('')),
    'apply-preset': el => applyPreset(state.presets.find(p => p.id === el.dataset.id)),
    search: () => { go('history'); setTimeout(() => $('#history-query')?.focus(), 150); },
    shortcuts: () => modal('Keyboard shortcuts', '<div class="detail-row"><span>Search conversations</span><span>⌘ / Ctrl K</span></div><div class="detail-row"><span>New conversation</span><span>⌘ / Ctrl Shift O</span></div><div class="detail-row"><span>Send a message</span><span>Enter</span></div><div class="detail-row"><span>New line</span><span>Shift Enter</span></div>'),
    'chat-menu': openMenu,
    'rename-chat': el => {
      const c = state.chats.find(c => c.id === el.dataset.id) || state.active;
      modal('Rename conversation', `<form id="rename-form" data-id="${esc(c.id)}"><label class="form-label" for="chat-title">Conversation title</label><input id="chat-title" class="form-control" maxlength="80" value="${esc(c.title)}" required><div class="modal-actions"><button type="submit" class="btn primary">Save title</button></div></form>`);
      $('#chat-title').focus(); $('#chat-title').select();
    },
    'delete-chat': el => modal('Delete this conversation?', `<p>This permanently deletes this conversation from your account.</p><div class="modal-actions">${btn('Keep conversation', 'close', { type: '' })}${btn('Delete conversation', 'confirm-delete', { tone: 'danger', attrs: `data-id="${esc(el.dataset.id)}"`, type: '' })}</div>`),
    'confirm-delete': async el => { const id = el.dataset.id; await api.request(chatPath(id), { method: 'DELETE' }); state.chats = state.chats.filter(c => c.id !== id); close(); if (state.active?.id === id) { state.active = null; go('chat'); } else render(); toast('Conversation deleted'); },
    'share-chat': el => { closeMenu(false); return sharing.open(el.dataset.id || state.active?.id); },
    'shared-fork': async () => {
      const id = state.active?.id; if (!id || !state.shared) return;
      const r = await api.request(`${sharedPath(id)}/fork`, { method: 'POST' });
      toast('Forked into a chat of your own'); await refreshSidebar(); location.hash = `#chat/${encodeURIComponent(r.conversation.id)}`;
    },
    'export-chat': el => historyView.exportChats([el.dataset.id || state.active?.id].filter(Boolean)),
    'pin-chat': async el => { const c = state.chats.find(c => c.id === el.dataset.id) || state.active; await historyView.update(c.id, { pinned: !c.pinned }); },
    'archive-chat': async el => { const c = state.chats.find(c => c.id === el.dataset.id) || state.active; await historyView.update(c.id, { archived: !c.archived }); },
    'edit-message': el => { const m = state.active.messages[Number(el.dataset.index)]; modal('Edit and resend', `<form id="edit-message-form" data-id="${esc(m.id)}"><p class="inline-note">Creates a new version in this conversation. Previous versions remain available.</p><label class="form-label" for="edited-message">Message</label><textarea id="edited-message" class="form-control" rows="7" maxlength="12000" ${m.attachments?.length ? '' : 'required'}>${esc(m.content)}</textarea><div class="form-error" role="alert"></div><div class="modal-actions"><button class="btn primary" type="submit">Send edited message</button></div></form>`); },
    'select-version': el => selectMessageVersion(el.dataset.id),
    'follow-up': el => { const question = state.active?.messages?.[Number(el.dataset.index)]?.followUps?.[Number(el.dataset.question)]; if (question) return send(question); },
    'retry-message': el => forkAndSend(state.active.messages[Number(el.dataset.index)].id, 'regenerate'),
    alternatives: async () => { const r = await api.request(`${chatPath(state.active.id)}/branches`); modal('Conversation alternatives', `<div class="alternatives-list">${(r.conversations || []).map(c => `<button class="select-option ${c.id === state.active.id ? 'selected' : ''}" data-action="go" data-path="chat/${esc(c.id)}">${icon('branch')}<span class="grow"><h3>${esc(c.title)}</h3><p>${esc(modelName(c.model))}${c.id === state.active.id ? ' · Current conversation' : ''}</p></span>${icon('right')}</button>`).join('') || '<p class="inline-note">No separate branches yet. Use Branch conversation on a response to create one.</p>'}</div>`); },
    'toggle-details': el => {
      const key = el.dataset.key, panel = document.getElementById(`details-${key}`), open = !detailsOpen.has(key);
      if (!panel) return;
      if (open) detailsOpen.add(key); else detailsOpen.delete(key);
      panel.hidden = !open; el.setAttribute('aria-expanded', String(open));
    },
    'copy-message': el => copy(state.active.messages[Number(el.dataset.index)].content),
    'copy-code': el => copy($('code', el.closest('.code-block')).textContent),
    'save-code': el => { const block = el.closest('.code-block'), body = $('code', block).textContent, language = $('.code-label span', block)?.textContent || ''; saveFromChat(el, { kind: 'code', body, language: language === 'text' ? '' : language, title: (body.match(/(?:def|function|fn|func|class|const|let)\s+([A-Za-z_$][\w$]*)/) || [])[1] || body.split('\n').find(l => l.trim())?.trim().slice(0, 80) || 'Code' }); },
    'save-message': el => { const m = state.active.messages[Number(el.dataset.index)]; saveFromChat(el, { kind: 'excerpt', body: m.content, title: state.active.title }); },
    'selection-save': () => { const pick = selection.pick; hideSelection(); if (pick) saveFromChat(pick.anchor, { kind: 'excerpt', body: pick.text, title: pick.text.split(/[.\n]/)[0].slice(0, 80) }); },
    'selection-ask': () => { const pick = selection.pick; hideSelection(); if (!pick) return; state.draft = `${pick.text.split('\n').map(l => `> ${l}`).join('\n')}\n\n${state.draft}`; render(); requestAnimationFrame(() => { const p = $('#prompt'); if (p) { p.focus(); p.setSelectionRange(p.value.length, p.value.length); } }); },
    'selection-copy': () => { const pick = selection.pick; hideSelection(); if (pick) copy(pick.text); },
    branch: async el => { const source = state.active; const c = unwrap(await api.request(`${chatPath(source.id)}/branch`, { method: 'POST', body: { throughMessageId: source.messages[Number(el.dataset.index)].id } })); upsert(c); go(`chat/${c.id}`); toast('Created a separate conversation branch'); },
    password: () => modal('Change your password', `<form id="password-form"><input type="hidden" name="username" autocomplete="username" value="${esc(state.user.username)}"><p class="inline-note">Use at least 12 characters for your new password.</p><label class="form-label" for="current-password">Current password</label><input id="current-password" class="form-control" type="password" autocomplete="current-password" required><label class="form-label" for="new-password">New password</label><input id="new-password" class="form-control" type="password" autocomplete="new-password" minlength="12" required><div class="form-error" role="alert"></div><div class="modal-actions"><button class="btn primary" type="submit">Update password</button></div></form>`),
  };
  function closeModels(restore = true) {
    if (modelPopover.hidden) return;
    modelPopover.hidden = true;
    const anchor = modelAnchor; modelAnchor = null;
    anchor?.setAttribute('aria-expanded', 'false');
    if (restore && anchor?.isConnected) anchor.focus({ preventScroll: true });
  }
  function openModels(anchor) {
    if (state.streaming) return;
    if (!modelPopover.hidden) { closeModels(); return; }
    closeMenu(false); modelAnchor = anchor;
    modelPopover.innerHTML = `<label class="model-search">${icon('search')}<input id="model-search" aria-label="Search models" placeholder="Search models" autocomplete="off"></label><div id="model-choices" role="listbox" aria-label="Available models"></div><div class="engine-effort">${effortControl()}</div>`;
    modelPopover.hidden = false; modelChoices();
    anchor.setAttribute('aria-expanded', 'true');
    // Open downward like a dropdown; flip above the button only when it would not fit.
    const rect = anchor.getBoundingClientRect(), fitsBelow = rect.bottom + 8 + modelPopover.offsetHeight <= window.innerHeight - 12;
    modelPopover.style.left = `${Math.max(12, Math.min(rect.left, window.innerWidth - modelPopover.offsetWidth - 12))}px`;
    modelPopover.style.top = `${fitsBelow ? rect.bottom + 8 : Math.max(12, rect.top - modelPopover.offsetHeight - 8)}px`;
    $('#model-search', modelPopover).focus({ preventScroll: true });
  }
  function closeMenu(restore = true) {
    if (menu.hidden) return;
    menu.hidden = true;
    const anchor = menuAnchor; menuAnchor = null;
    anchor?.setAttribute('aria-expanded', 'false');
    if (restore && anchor?.isConnected) anchor.focus({ preventScroll: true });
  }
  function openMenu(anchor) {
    if (!menu.hidden && menuAnchor === anchor) { closeMenu(); return; }
    closeMenu(false); menuAnchor = anchor;
    const id = anchor.dataset.id || state.active?.id;
    if (!id) return;
    menu.setAttribute('aria-label', 'Conversation options');
    const c = state.chats.find(c => c.id === id) || state.active;
    menu.innerHTML = [['edit', 'Rename', 'rename-chat'], ['pin', c?.pinned ? 'Unpin' : 'Pin', 'pin-chat'], ['archive', c?.archived ? 'Restore' : 'Archive', 'archive-chat'], ['link', 'Share…', 'share-chat'], ['download', 'Export JSON', 'export-chat'], ['trash', 'Delete', 'delete-chat']].map(([name, label, action]) => `<button role="menuitem" data-action="${action}" data-id="${esc(id)}" class="menu-item ${action === 'delete-chat' ? 'danger' : ''}" ${action === 'delete-chat' && state.streaming && state.streamChatId === id ? 'disabled' : ''}>${icon(name)}${label}</button>`).join('');
    menu.hidden = false; anchor.setAttribute('aria-expanded', 'true');
    const rect = anchor.getBoundingClientRect();
    menu.style.left = `${Math.max(8, Math.min(rect.right - menu.offsetWidth, window.innerWidth - menu.offsetWidth - 8))}px`;
    menu.style.top = `${rect.bottom + menu.offsetHeight + 8 > window.innerHeight ? Math.max(8, rect.top - menu.offsetHeight - 5) : rect.bottom + 5}px`;
    $('button', menu)?.focus({ preventScroll: true });
  }
  /** A small menu under its button, or above it when there is no room below. */
  function openListMenu(anchor, label, items) {
    if (!menu.hidden && menuAnchor === anchor) { closeMenu(); return; }
    closeMenu(false); closeModels(false); menuAnchor = anchor;
    menu.setAttribute('aria-label', label); menu.innerHTML = items;
    menu.hidden = false; anchor.setAttribute('aria-expanded', 'true');
    const rect = anchor.getBoundingClientRect();
    menu.style.left = `${Math.max(8, Math.min(rect.left, window.innerWidth - menu.offsetWidth - 8))}px`;
    menu.style.top = `${rect.bottom + menu.offsetHeight + 8 > window.innerHeight ? Math.max(8, rect.top - menu.offsetHeight - 5) : rect.bottom + 5}px`;
    $('button:not(:disabled)', menu)?.focus({ preventScroll: true });
  }
  function openProfileMenu(anchor) {
    if (!menu.hidden && menuAnchor === anchor) { closeMenu(); return; }
    closeMenu(false); menuAnchor = anchor;
    menu.setAttribute('aria-label', 'Account');
    menu.innerHTML = `<div class="menu-head"><strong>${esc(name())}</strong><small>${esc(state.user.username)} · ${esc(state.user.role)}</small></div><button role="menuitem" data-action="go" data-path="settings" class="menu-item">${icon('settings')}Settings</button><button role="menuitem" data-action="logout" class="menu-item">${icon('right')}Sign out</button>`;
    menu.hidden = false; anchor.setAttribute('aria-expanded', 'true');
    const rect = anchor.getBoundingClientRect();
    menu.style.left = `${rect.left}px`;
    menu.style.top = `${Math.max(8, rect.top - menu.offsetHeight - 6)}px`;
    $('button', menu)?.focus({ preventScroll: true });
  }
  async function copy(text) {
    try { await navigator.clipboard.writeText(text); toast('Copied to clipboard'); }
    catch { modal('Copy text', `<textarea class="form-control" rows="8" readonly>${esc(text)}</textarea>`); }
  }
  document.addEventListener('click', event => {
    const el = event.target.closest('[data-action]');
    if (!modelPopover.hidden && !modelPopover.contains(event.target) && el?.dataset.action !== 'models') closeModels(false);
    if (!menu.hidden && !menu.contains(event.target) && !['chat-menu', 'profile', 'project-menu', 'composer-add'].includes(el?.dataset.action)) closeMenu(false);
    // A link in a dialog that goes to another page takes the dialog with it.
    if (event.target.closest('#modal a[href^="#"]') && $('#modal').open) close();
    if (!el || el.disabled) return;
    if (menu.contains(el)) closeMenu(false);
    event.preventDefault();
    if (state.phase !== 'ready' && !['retry-session', 'close', 'logout'].includes(el.dataset.action)) return;
    const workspaceAction = workspace.actions[el.dataset.action] || browserView.actions[el.dataset.action];
    if (workspaceAction) { perform(async () => { if (await workspaceAction(el) === 'render') render(); }); return; }
    perform(() => (actions[el.dataset.action] || personal.actions[el.dataset.action] || connectionsView.actions[el.dataset.action] || clustersView.actions[el.dataset.action] || knowledgeView.actions[el.dataset.action] || libraryView.actions[el.dataset.action] || sharing.actions[el.dataset.action] || projectsView.actions[el.dataset.action] || questionsView.actions[el.dataset.action] || researchView.actions[el.dataset.action] || groupsView.actions[el.dataset.action] || codeView.actions[el.dataset.action] || turnActions[el.dataset.action] || administration.actions[el.dataset.action] || historyView.actions[el.dataset.action] || attachments.actions[el.dataset.action])?.(el));
  });
  document.addEventListener('submit', async event => {
    event.preventDefault(); const form = event.target;
    if (form.id === 'chat-form') { send($('#prompt').value); return; }
    const submit = $('button[type="submit"]', form); if (submit?.disabled) return;
    if (submit) submit.disabled = true;
    try {
      if (form.id.startsWith('history-')) { await historyView.submit(form); return; }
      if (form.id === 'edit-message-form') { await forkAndSend(form.dataset.id, 'edit', $('#edited-message').value); return; }
      if (form.id.startsWith('personal-')) { await personal.submit(form); return; }
      if (form.id.startsWith('knowledge-')) { await knowledgeView.submit(form); return; }
      if (form.id === 'saved-form' || form.id === 'library-search') { await libraryView.submit(form); return; }
      if (form.id === 'project-form') { await projectsView.submit(form); return; }
      if (form.id === 'code-github-form') { await codeView.submit(form); return; }
      if (form.id === 'steer-form') { await turnView.submit(form, api, toast); render(); return; }
      if (form.id === 'connection-form' || form.id === 'connection-credential-form') { await connectionsView.submit(form); return; }
      if (form.id.startsWith('admin-')) { if (state.user?.role !== 'admin') return; await administration.submit(form); return; }
      if (form.id === 'activation-form') {
        if ($('#activation-password').value !== $('#activation-confirm').value) throw new Error('The passwords do not match.');
        const result = await api.request('/api/auth/activate', { method: 'POST', body: { token: route().id, newPassword: $('#activation-password').value } });
        if (result.csrfToken) api.csrfToken = result.csrfToken; history.replaceState(null, '', '#chat'); await session(); return;
      }
      if (form.id === 'required-password-form') {
        if ($('#required-password').value !== $('#required-confirm').value) throw new Error('The passwords do not match.');
        const result = await api.request('/api/auth/password', { method: 'POST', body: { currentPassword: $('#required-current').value, newPassword: $('#required-password').value } });
        if (result.csrfToken) api.csrfToken = result.csrfToken; await session(); return;
      }
      if (form.id === 'login-form') {
        const credentials = { username: $('#username').value.trim(), password: $('#password').value };
        const result = await api.request('/api/auth/login', { method: 'POST', body: credentials });
        if (result.csrfToken) api.csrfToken = result.csrfToken;
        await session();
      } else if (form.id === 'rename-form') {
        const title = $('#chat-title').value.trim(); if (!title) return;
        const id = form.dataset.id;
        const c = unwrap(await api.request(chatPath(id), { method: 'PATCH', body: { title } }));
        const existing = state.chats.find(c => c.id === id); if (existing) existing.title = c.title || title;
        if (state.active?.id === id) state.active.title = c.title || title; close(); render();
      } else if (form.id === 'parameters-form') {
        state.systemPrompt = $('#system-prompt').value; state.temperature = Number($('#temperature').value); state.topP = Number($('#top-p').value); state.maxTokens = Number($('#max-tokens').value); applyModelPolicy();
        if (state.active) { const settings = Object.fromEntries(['systemPrompt','temperature','topP','maxTokens','reasoningEffort'].map(k => [k,state[k]])); const c = unwrap(await api.request(chatPath(state.active.id), { method: 'PATCH', body: { settings } })); state.active.settings = c.settings || settings; }
        close(); render();
      } else if (form.id === 'password-form') {
        const result = await api.request('/api/auth/password', { method: 'POST', body: { currentPassword: $('#current-password').value, newPassword: $('#new-password').value } });
        if (result.csrfToken) api.csrfToken = result.csrfToken;
        close(); await session(); toast('Password updated');

      }
    } catch (error) {
      if (form.id === 'login-form') { state.loginError = error.message; render(); }
      else if ($('.form-error', form)) $('.form-error', form).textContent = error.message;
      else if (state.user) toast(error.message);
    } finally { if (submit?.isConnected) submit.disabled = false; }
  });
  document.addEventListener('input', event => {
    if (questionsView.input(event)) return;
    if (codeView.input(event)) return;
    if (event.target.id === 'steer-text') turnView.setDraft(event.target.value);
    if (event.target.id === 'prompt') { state.draft = event.target.value; event.target.style.height = 'auto'; event.target.style.height = `${Math.min(event.target.scrollHeight, 220)}px`; }
    if (event.target.id === 'reasoning-effort') {
      state.reasoningEffort = efforts[Number(event.target.value)] || 'none';
      event.target.setAttribute('aria-valuetext', effortLabel(state.reasoningEffort));
      event.target.title = `Thinking effort: ${effortLabel(state.reasoningEffort)}. Requested effort; model support varies.`;
      const control = event.target.closest('.effort-control'); control.dataset.effort = state.reasoningEffort;
      $('.effort-mode', control).textContent = state.reasoningEffort === 'high' ? 'Warp speed' : '';
      $('.effort-description', control).textContent = effortDescription();
      const engine = $('.engine-button');
      if (engine) {
        engine.querySelectorAll('.effort-bars i').forEach((bar, i) => bar.classList.toggle('lit', Number(event.target.value) > i));
        engine.setAttribute('aria-label', `Model: ${modelName(state.model)}. Thinking effort: ${effortLabel(state.reasoningEffort)}`); engine.title = `${modelName(state.model)} · thinking ${effortLabel(state.reasoningEffort).toLowerCase()}`;
      }
      control.querySelectorAll('.effort-labels span').forEach((label, i) => label.classList.toggle('selected', i === Number(event.target.value)));
      document.querySelectorAll('[data-effort-summary]').forEach(label => label.textContent = effortLabel(state.reasoningEffort));
    }
    if (event.target.id === 'model-search') modelChoices(event.target.value);
    administration.input(event);
  });
  // Paste or drop images and files straight into the message box.
  const composerFiles = list => [...(list || [])].filter(file => file.size > 0);
  document.addEventListener('paste', event => {
    if (event.target.id !== 'prompt') return;
    const files = composerFiles(event.clipboardData?.files);
    // Text copied from documents can carry a picture of itself; keep it as text.
    if (!files.length || event.clipboardData.getData('text/plain')) return;
    event.preventDefault(); attachments.add(files);
  });
  document.addEventListener('dragover', event => { if (event.target.closest?.('#chat-form') && event.dataTransfer?.types?.includes('Files')) { event.preventDefault(); event.target.closest('#chat-form').classList.add('dropping'); } });
  document.addEventListener('dragleave', event => { if (event.target.closest?.('#chat-form') && !event.target.closest('#chat-form').contains(event.relatedTarget)) event.target.closest('#chat-form').classList.remove('dropping'); });
  document.addEventListener('dragover', event => { const zone = event.target.closest?.('.knowledge-drop'); if (zone && event.dataTransfer?.types?.includes('Files')) { event.preventDefault(); zone.classList.add('dropping'); } });
  document.addEventListener('dragleave', event => { const zone = event.target.closest?.('.knowledge-drop'); if (zone && !zone.contains(event.relatedTarget)) zone.classList.remove('dropping'); });
  document.addEventListener('drop', event => {
    const zone = event.target.closest?.('.knowledge-drop');
    if (zone) { event.preventDefault(); zone.classList.remove('dropping'); knowledgeView.add(event.dataTransfer?.files || []); return; }
    const form = event.target.closest?.('#chat-form'); if (!form) return;
    event.preventDefault(); form.classList.remove('dropping');
    const files = composerFiles(event.dataTransfer?.files); if (files.length) attachments.add(files);
  });
  document.addEventListener('change', event => {
    administration.input(event); historyView.input(event);
    if (event.target.id === 'message-files') { attachments.upload(event.target); return; }
    if (knowledgeView.input(event) || libraryView.input(event)) return;
    if (event.target.closest?.('#share-form')) { sharing.input(event); return; }
    if (event.target.matches?.('[data-browser-engine]')) { browserView.input(event.target); return; }
    if (event.target.id !== 'reasoning-effort' || !state.active || state.streaming) return;
    const id = state.active.id, effort = state.reasoningEffort, epoch = identityEpoch, sequence = ++effortSequence;
    pendingEfforts.set(id, { value: effort, sequence });
    effortSaveQueue = effortSaveQueue.catch(() => {}).then(async () => {
      if (epoch !== identityEpoch || pendingEfforts.get(id)?.sequence !== sequence) return;
      try {
        await api.request(chatPath(id), { method: 'PATCH', body: { reasoningEffort: effort } });
        if (pendingEfforts.get(id)?.sequence === sequence) {
          pendingEfforts.delete(id);
          const chat = state.chats.find(c => c.id === id); if (chat) chat.reasoningEffort = effort;
          if (state.active?.id === id) state.active.reasoningEffort = effort;
        }
      } catch (error) {
        if (pendingEfforts.get(id)?.sequence === sequence) pendingEfforts.delete(id);
        if (error.status !== 409 && error.name !== 'AbortError' && epoch === identityEpoch) toast(error.message);
      }
    });
  });
  document.addEventListener('keydown', event => {
    if (state.phase !== 'ready') return;
    // The workspace terminal owns its keys (Ctrl+K deletes to the end of a shell line).
    if (event.target.closest?.('.workspace-terminal')) return;
    if (questionsView.keydown(event)) return;
    if (!modelPopover.hidden) {
      const options = [...modelPopover.querySelectorAll('.model-option')], index = options.indexOf(document.activeElement);
      if (event.key === 'Escape') { event.preventDefault(); closeModels(); return; }
      if (event.key === 'Enter' && event.target.id === 'model-search') { event.preventDefault(); options[0]?.click(); return; }
      if (['ArrowDown', 'ArrowUp'].includes(event.key) && options.length && event.target.id !== 'reasoning-effort') { event.preventDefault(); options[index < 0 ? (event.key === 'ArrowDown' ? 0 : options.length - 1) : (index + (event.key === 'ArrowDown' ? 1 : -1) + options.length) % options.length].focus(); return; }
    }
    if (!menu.hidden) {
      const items = [...menu.querySelectorAll('button:not(:disabled)')], index = items.indexOf(document.activeElement);
      if (event.key === 'Escape') { event.preventDefault(); closeMenu(); return; }
      if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) { event.preventDefault(); items[event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1 : (index + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length]?.focus(); return; }
      if (event.key === 'Tab') closeMenu();
    }
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') { event.preventDefault(); actions.search(); }
    if ((event.metaKey || event.ctrlKey) && event.key === '.' && route().page === 'chat' && route().id && !state.shared) { event.preventDefault(); actions.context(); }
    if ((event.metaKey || event.ctrlKey) && event.shiftKey && event.key.toLowerCase() === 'o') { event.preventDefault(); actions['new-chat'](); }
    if (event.target.id === 'prompt' && event.key === 'Enter' && !event.shiftKey && !event.isComposing) { event.preventDefault(); send(event.target.value); }
  });
  $('#modal').addEventListener('click', event => {
    if (event.target === $('#modal')) { const rect = event.target.getBoundingClientRect(); if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) close(); }
  });
  document.addEventListener('toggle', event => {
    if (event.target.matches('[data-thinking]')) thinkingOpen.set(event.target.dataset.thinking, event.target.open);
    if (event.target.matches('[data-activity]') && event.target.dataset.activity) { if (event.target.open) state.openActivity.add(event.target.dataset.activity); else state.openActivity.delete(event.target.dataset.activity); }
  }, true);
  window.addEventListener('resize', () => { closeMenu(false); closeModels(false); });
  window.addEventListener('scroll', event => { if (modelPopover.contains(event.target)) return; closeMenu(false); closeModels(false); }, true);
  window.addEventListener('hashchange', navigate);
  await session();
}
