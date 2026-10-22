import { icon, esc, badge, iconButton } from './manage-ui.js';

// Chat's browser: the composer toggle, the Browser tab of the side pane (a live view
// of this chat's tab, which the person can take over), the cards a reply ends with
// to ask for the browser or hand the tab over, and the browser settings card.
export function createBrowser({ api, state, route, toast, chatPath, capabilities, render, send, modal, close, assistantName = () => 'The assistant' }) {
  const chats = new Map(), offers = new Map();
  let pane = null;
  const enabled = () => !!capabilities.features?.browser;
  const chat = id => {
    if (!chats.has(id)) chats.set(id, { on: false, status: 'stopped', controller: 'model', url: '', title: '', errorMessage: '', shown: false, loaded: false });
    return chats.get(id);
  };
  const on = id => !!id && !!chats.get(id)?.on;
  const visible = id => on(id) && chat(id).shown;
  const take = (id, r) => Object.assign(chat(id), { on: !!r.on, status: r.status || 'stopped', controller: r.controller || 'model', url: r.url || '', title: r.title || '', errorMessage: r.errorMessage || '', loaded: true });

  async function load(id) {
    if (!enabled() || !id) return;
    const before = on(id);
    try { take(id, await api.request(`${chatPath(id)}/browser`)); }
    catch { chat(id).loaded = true; return; }
    if (before !== on(id) && route().id === id) render(); else pane?.update();
  }
  async function setOn(id, value) {
    take(id, await api.request(`${chatPath(id)}/browser`, { method: 'PUT', body: { on: value } }));
    chat(id).shown = value;
    if (!value && live?.id === id) disconnect();
    if (value) pane?.showTab(id, 'browser'); else pane?.update();
  }
  async function setController(id, controller) {
    await api.request(`${chatPath(id)}/browser/control`, { method: 'POST', body: { controller } });
    chat(id).controller = controller;
    drawStatus(id);
  }

  // ---- The live view ----
  const view = document.createElement('div');
  view.className = 'browser-view';
  view.innerHTML = '<div class="browser-banner" role="status"></div><form class="browser-bar" autocomplete="off"></form><div class="browser-screen"><canvas tabindex="0" aria-label="The chat\'s browser tab"></canvas><div class="browser-note"></div></div><p class="browser-foot"></p>';
  const banner = view.querySelector('.browser-banner'), bar = view.querySelector('.browser-bar'), screen = view.querySelector('.browser-screen');
  const canvas = view.querySelector('canvas'), paint = canvas.getContext('2d'), note = view.querySelector('.browser-note'), foot = view.querySelector('.browser-foot');
  canvas.width = 1280; canvas.height = 720;
  let live = null, shownId = null;

  function disconnect() {
    if (!live) return;
    live.closed = true; try { live.socket.close(); } catch { /* already closed */ }
    live = null;
  }
  function connect(id, wake = false) {
    disconnect();
    const socket = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}${chatPath(id)}/browser/live${wake ? '?wake=1' : ''}`);
    socket.binaryType = 'arraybuffer';
    const mine = live = { id, socket, w: 1280, h: 720, closed: false, frames: 0 };
    showNote(id, wake ? 'starting' : 'connecting');
    socket.onmessage = async event => {
      if (typeof event.data === 'string') {
        let message; try { message = JSON.parse(event.data); } catch { return; }
        if (message.t === 'size' && message.w > 0 && message.h > 0) { mine.w = message.w; mine.h = message.h; }
        if (message.t === 'state') { Object.assign(chat(id), { controller: message.controller || 'model', url: message.url || '', title: message.title || '', status: 'running' }); if (shownId === id) drawStatus(id); }
        return;
      }
      let bitmap;
      try { bitmap = await createImageBitmap(new Blob([event.data], { type: 'image/jpeg' })); } catch { return; }
      if (live !== mine) { bitmap.close(); return; }
      if (canvas.width !== bitmap.width || canvas.height !== bitmap.height) { canvas.width = bitmap.width; canvas.height = bitmap.height; }
      paint.drawImage(bitmap, 0, 0); bitmap.close();
      if (!mine.frames++) { chat(id).status = 'running'; screen.classList.add('has-frame'); note.hidden = true; }
    };
    socket.onclose = () => {
      if (mine.closed || live !== mine) return;
      live = null;
      load(id).then(() => { if (shownId === id) showNote(id, chat(id).status === 'running' ? 'closed' : null); });
    };
  }
  function showNote(id, why = null) {
    const c = chat(id);
    screen.classList.remove('has-frame'); note.hidden = false;
    const kind = why || (c.status === 'error' ? 'error' : c.status === 'starting' ? 'starting' : 'asleep');
    note.innerHTML = {
      connecting: `${icon('orbit')}<strong>Connecting to the browser…</strong>`,
      starting: `${icon('orbit')}<strong>Starting your browser…</strong><p>It takes a few seconds the first time.</p>`,
      closed: `${icon('globe')}<strong>The live view closed.</strong><button type="button" class="btn small" data-action="browser-wake">Reconnect</button>`,
      error: `${icon('info')}<strong>The browser could not start.</strong><p>${esc(c.errorMessage || 'Try again in a moment.')}</p><button type="button" class="btn small" data-action="browser-wake">Try again</button>`,
      asleep: `${icon('globe')}<strong>The browser is asleep.</strong><p>It wakes when ${esc(assistantName())} browses, or open it to look around yourself.</p><button type="button" class="btn small" data-action="browser-wake">Open the browser</button>`,
    }[kind];
  }
  function drawStatus(id) {
    const c = chat(id), mine = c.controller === 'user';
    const busy = !mine && state.streaming && state.streamChatId === id;
    banner.className = `browser-banner ${mine ? 'is-user' : busy ? 'is-busy' : ''}`;
    banner.innerHTML = mine
      ? `${icon('hand')}<strong>You have control</strong><span class="grow">Clicks and keys go to the page. ${esc(assistantName())} is paused and sees nothing.</span><button type="button" class="btn primary small" data-action="browser-handback">Hand back</button>`
      : `${busy ? '<span class="browser-pulse" aria-hidden="true"></span>' : icon('globe')}<strong>${busy ? `${esc(assistantName())} is browsing` : 'This chat’s tab'}</strong><span class="grow">${busy ? 'Watch along, or take over.' : `${esc(assistantName())} uses it when it browses.`}</span><button type="button" class="btn small" data-action="browser-take">${icon('hand')}Take over</button>`;
    // Leave the address alone while the person types in it.
    if (document.activeElement?.id !== 'browser-url' || !mine) {
      const off = mine ? '' : 'disabled', address = c.url && c.url !== 'about:blank' ? c.url : '';
      // type="button": Enter in the address submits the form, not the first button.
      const nav = (glyph, action, title) => `<button type="button" class="icon-btn" data-action="${action}" aria-label="${title}" title="${title}" ${off}>${icon(glyph)}</button>`;
      bar.innerHTML = `<span class="flip">${nav('right', 'browser-back', 'Back')}</span>${nav('right', 'browser-forward', 'Forward')}${nav('refresh', 'browser-reload', 'Reload')}<label class="sr-only" for="browser-url">Address</label><input id="browser-url" name="browser-url" class="browser-url" value="${esc(address)}" placeholder="${mine ? 'Type an address and press Enter' : 'Take over to type an address'}" ${mine ? '' : 'readonly'} spellcheck="false">`;
    }
    canvas.classList.toggle('is-controlled', mine);
    foot.textContent = `Chrome · yours · public internet only${c.title ? ` · ${c.title}` : ''}`;
  }

  /** Shows the live view in the pane's container; called on every pane update. */
  function show(container, id) {
    if (view.parentNode !== container) container.replaceChildren(view);
    if (shownId !== id) { disconnect(); shownId = id; paint.clearRect(0, 0, canvas.width, canvas.height); if (!chat(id).loaded) load(id); }
    drawStatus(id);
    if (live?.id === id) return;
    if (chat(id).status === 'running') connect(id); else showNote(id);
  }
  function hide() { disconnect(); shownId = null; }

  // The person's mouse and keys reach the tab only while they have control.
  const controlling = () => live && live.socket.readyState === WebSocket.OPEN && chat(live.id).controller === 'user';
  const sendInput = message => { if (controlling()) live.socket.send(JSON.stringify(message)); };
  const modifiers = e => (e.altKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.metaKey ? 4 : 0) | (e.shiftKey ? 8 : 0);
  const point = e => { const r = canvas.getBoundingClientRect(); return { x: (e.clientX - r.left) * (live.w / r.width), y: (e.clientY - r.top) * (live.h / r.height) }; };
  const buttonOf = e => ['left', 'middle', 'right'][e.button] || 'none';
  let moved = 0;
  canvas.addEventListener('mousedown', e => { if (!controlling()) return; canvas.focus(); e.preventDefault(); sendInput({ t: 'mouse', type: 'mousePressed', ...point(e), button: buttonOf(e), clickCount: e.detail || 1, modifiers: modifiers(e) }); });
  canvas.addEventListener('mouseup', e => { if (!controlling()) return; e.preventDefault(); sendInput({ t: 'mouse', type: 'mouseReleased', ...point(e), button: buttonOf(e), clickCount: e.detail || 1, modifiers: modifiers(e) }); });
  canvas.addEventListener('mousemove', e => { if (!controlling() || Date.now() - moved < 40) return; moved = Date.now(); sendInput({ t: 'mouse', type: 'mouseMoved', ...point(e), modifiers: modifiers(e) }); });
  canvas.addEventListener('wheel', e => { if (!controlling()) return; e.preventDefault(); sendInput({ t: 'mouse', type: 'mouseWheel', ...point(e), deltaX: e.deltaX, deltaY: e.deltaY, modifiers: modifiers(e) }); }, { passive: false });
  canvas.addEventListener('contextmenu', e => { if (controlling()) e.preventDefault(); });
  canvas.addEventListener('keydown', e => {
    if (!controlling()) return;
    // Paste goes through the paste event so the text arrives whole.
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'v') return;
    e.preventDefault();
    const text = e.key === 'Enter' ? '\r' : e.key.length === 1 && !e.ctrlKey && !e.metaKey ? e.key : '';
    sendInput({ t: 'key', type: text ? 'keyDown' : 'rawKeyDown', key: e.key, code: e.code, keyCode: e.keyCode, text, modifiers: modifiers(e) });
  });
  canvas.addEventListener('keyup', e => { if (!controlling()) return; e.preventDefault(); sendInput({ t: 'key', type: 'keyUp', key: e.key, code: e.code, keyCode: e.keyCode, modifiers: modifiers(e) }); });
  canvas.addEventListener('paste', e => { if (!controlling()) return; e.preventDefault(); const text = e.clipboardData?.getData('text/plain'); if (text) sendInput({ t: 'text', text }); });
  bar.addEventListener('submit', e => { e.preventDefault(); const url = bar.elements['browser-url']?.value.trim(); if (url) { sendInput({ t: 'go', url }); canvas.focus(); } });

  // ---- The composer toggle ----
  function toggleButton() {
    if (!enabled()) return '';
    const id = route().id, isOn = id ? on(id) : !!state.browserDraft;
    return `<button type="button" class="btn ghost small browser-toggle ${isOn ? 'on' : ''}" data-action="browser-toggle" aria-pressed="${isOn}" title="${isOn ? 'The model can search the web and open pages in a browser you can watch' : 'Let the model search the web and open pages in a browser you can watch'}" ${state.streaming ? 'disabled' : ''}>${icon('globe')}<span class="option-text">Browser</span></button>`;
  }

  // ---- Cards at the end of a reply ----
  const entryOf = (m, kind) => (Array.isArray(m?.activity) ? m.activity : []).find(e => e.kind === kind);
  const keyOf = (m, index) => m.id || `message-${index}`;
  const messageAt = index => state.active?.messages?.[Number(index)];
  const isLast = index => Number(index) === (state.active?.messages?.length || 0) - 1;
  const open = (m, index) => isLast(index) && m.status === 'completed' && !state.streaming;
  function card(m, index) {
    if (m.role !== 'assistant') return '';
    const id = state.active?.id, request = entryOf(m, 'browser-request'), handoff = entryOf(m, 'browser-handoff');
    let html = '';
    if (request) {
      const offer = offers.get(keyOf(m, index));
      if (on(id) || offer === 'done') html += `<div class="question-card is-closed">${icon('globe')}<div class="grow"><p><strong>Browser</strong> Turned on for this chat.</p></div></div>`;
      else if (!open(m, index) || offer === 'dismissed') html += `<div class="question-card is-closed">${icon('globe')}<div class="grow"><p><strong>Browser</strong> ${esc(request.reason || 'Asked to use the browser.')}</p>${offer === 'dismissed' ? '<p class="faint">Not turned on. Use the Browser button below whenever you want it.</p>' : ''}</div></div>`;
      else {
        const busy = offer === 'busy';
        html += `<section class="question-card workspace-offer" aria-label="Turn on the browser"><span class="workspace-offer-icon">${icon('globe')}</span><div class="grow"><strong>This needs the web</strong><p>${esc(request.reason || 'A browser for this chat, where the model searches and reads pages while you watch.')}</p></div><div class="workspace-offer-actions"><button type="button" class="btn ghost small" data-action="browser-offer-dismiss" data-index="${index}" ${busy ? 'disabled' : ''}>Not now</button><button type="button" class="btn primary small" data-action="browser-offer-accept" data-index="${index}" ${busy ? 'disabled' : ''}>${busy ? 'Turning on…' : 'Turn on browser'}</button></div></section>`;
      }
    }
    if (handoff) {
      const waiting = open(m, index) && on(id) && chat(id).controller === 'user';
      html += waiting
        ? `<section class="question-card workspace-offer browser-handoff" aria-label="Your turn in the browser"><span class="workspace-offer-icon">${icon('hand')}</span><div class="grow"><strong>Your turn in the browser</strong><p>${esc(handoff.reason || 'The page needs you.')}</p><p class="faint">${esc(assistantName())} is paused and sees nothing, not even what you type, until you hand back.</p></div><div class="workspace-offer-actions"><button type="button" class="btn ghost small" data-action="browser-show">Show browser</button><button type="button" class="btn primary small" data-action="browser-handback">Hand back</button></div></section>`
        : `<div class="question-card is-closed">${icon('hand')}<div class="grow"><p><strong>Browser</strong> ${esc(handoff.reason || 'Asked you to take over.')}</p>${isLast(index) && chat(id).controller !== 'user' ? '<p class="faint">Handed back.</p>' : ''}</div></div>`;
    }
    return html;
  }
  function signature(m, index) {
    if (!entryOf(m, 'browser-request') && !entryOf(m, 'browser-handoff')) return '';
    const id = state.active?.id;
    return JSON.stringify([open(m, index), on(id), chat(id || '').controller, offers.get(keyOf(m, index))]);
  }
  function redrawCards() { if (state.active?.id) render(); }

  /** Watches the reply's events: browsing opens the pane, a handoff hands over. */
  function observe(event, conversationId) {
    if (event.event !== 'tool' || !enabled()) return;
    const tool = String(event.data?.tool || ''), c = chat(conversationId);
    if (tool.startsWith('browser_') || event.data?.kind === 'browser-handoff') {
      c.on = true; c.status = 'running';
      if (event.data?.kind === 'browser-handoff') c.controller = 'user';
      if (event.data?.url) { c.url = event.data.url; c.title = event.data.title || c.title; }
      if (!c.shown) { c.shown = true; pane?.showTab(conversationId, 'browser'); }
      else if (shownId === conversationId) { drawStatus(conversationId); if (!live) connect(conversationId); }
    }
  }

  // ---- Settings ----
  const settings = { data: null, loading: false, error: '' };
  async function loadSettings() {
    if (!enabled()) return;
    settings.loading = true;
    try { settings.data = await api.request('/api/browser'); settings.error = ''; }
    catch (e) { settings.error = e.message; }
    finally { settings.loading = false; render(); }
  }
  function settingsCard() {
    if (!enabled()) return '';
    const d = settings.data;
    if (!d) { if (!settings.loading && !settings.error) loadSettings(); return `<section class="setting-card card"><h2>Browser</h2><p class="inline-note">${esc(settings.error || 'Loading…')}</p></section>`; }
    const label = { running: 'Running', starting: 'Starting', stopped: 'Asleep', error: 'Error' }[d.status] || d.status;
    const engines = [['duckduckgo', 'DuckDuckGo'], ['google', 'Google'], ['bing', 'Bing']];
    return `<section class="setting-card card browser-settings"><div class="spread"><h2>Browser</h2>${badge(label, d.status === 'running' ? 'accent' : d.status === 'error' ? 'amber' : '')}</div><p>One Chrome for you, shared by your chats: each chat with the browser on has its own tab. It sleeps after ${esc(d.idleMinutes)} minutes unused and keeps your sign-ins.</p>${d.tabs?.length ? `<div class="browser-tabs-list">${d.tabs.slice(0, 5).map(t => `<a class="detail-row" href="#chat/${encodeURIComponent(t.conversationId)}"><span>${esc(t.title || 'Untitled chat')}</span><span>${t.controller === 'user' ? 'You have control' : esc(hostOf(t.url) || 'No page yet')}</span></a>`).join('')}</div>` : ''}<div class="management-field"><label class="form-label" for="browser-engine">Search with</label><select id="browser-engine" class="form-control" data-browser-engine>${engines.map(([v, n]) => `<option value="${v}" ${d.searchEngine === v ? 'selected' : ''}>${n}</option>`).join('')}</select><p class="field-help">The model searches by opening this search page in your browser.</p></div><h3 class="browser-settings-sub">Ask before</h3><p class="inline-note">Submitting a form, or pressing a button that buys, sends, posts or deletes something, asks you first. Passwords, sign-ins and captchas are always yours: the model hands you the tab.</p>${d.sites?.length ? `<h3 class="browser-settings-sub">Always allowed to submit on</h3>${d.sites.map(s => `<div class="detail-row"><span>${esc(s.host)}</span>${iconButton('x', 'browser-forget-site', `Ask again on ${s.host}`, `data-host="${esc(s.host)}"`)}</div>`).join('')}` : ''}<div class="modal-actions">${d.status === 'running' ? `<button type="button" class="btn small" data-action="browser-sleep">Sleep now</button>` : ''}<button type="button" class="btn small danger" data-action="browser-forget">Forget all browser data</button></div></section>`;
  }
  const hostOf = url => { try { return url ? new URL(url).host : ''; } catch { return ''; } };
  async function input(el) {
    if (!el.matches?.('[data-browser-engine]')) return false;
    try { await api.request('/api/browser', { method: 'PUT', body: { searchEngine: el.value } }); settings.data.searchEngine = el.value; toast('Search engine saved'); }
    catch (e) { toast(e.message); }
    return true;
  }

  const current = () => route().id;
  const actions = {
    'browser-toggle': async () => {
      const id = current();
      if (!id) { state.browserDraft = !state.browserDraft; return 'render'; }
      if (!on(id)) { await setOn(id, true); toast('Browser on. Ask for something to look up.'); return 'render'; }
      const c = chat(id);
      // A second press hides the tab: back to the workspace's preview, or no pane.
      if (c.shown && pane?.showing(id, 'browser')) { c.shown = false; pane?.showTab(id, 'preview'); }
      else { c.shown = true; pane?.showTab(id, 'browser'); }
      return 'render';
    },
    'browser-off': async () => { const id = current(); if (id) { await setOn(id, false); toast('Browser off for this chat'); return 'render'; } },
    'browser-wake': () => { const id = current(); if (id) { chat(id).status = 'starting'; connect(id, true); } },
    'browser-take': async () => { const id = current(); if (!id) return; await setController(id, 'user'); canvas.focus(); redrawCards(); },
    'browser-handback': async () => {
      const id = current(); if (!id) return;
      await setController(id, 'model');
      const messages = state.active?.messages || [], last = messages[messages.length - 1];
      if (!state.streaming && entryOf(last, 'browser-handoff')) send('I’m done in the browser. Carry on.');
      else redrawCards();
    },
    'browser-show': () => { const id = current(); if (id) { chat(id).shown = true; pane?.showTab(id, 'browser'); } },
    'browser-back': () => sendInput({ t: 'back' }),
    'browser-forward': () => sendInput({ t: 'forward' }),
    'browser-reload': () => sendInput({ t: 'reload' }),
    'browser-offer-accept': async el => {
      const index = el.dataset.index, m = messageAt(index), id = current();
      if (!entryOf(m, 'browser-request') || !id || !isLast(index)) return;
      const key = keyOf(m, index);
      offers.set(key, 'busy'); redrawCards();
      try { await setOn(id, true); } catch (error) { offers.delete(key); redrawCards(); throw error; }
      offers.set(key, 'done');
      send('The browser is on. Go ahead.');
    },
    'browser-offer-dismiss': el => { const m = messageAt(el.dataset.index); if (m) offers.set(keyOf(m, el.dataset.index), 'dismissed'); return 'render'; },
    'browser-sleep': async () => { await api.request('/api/browser/sleep', { method: 'POST' }); chats.forEach(c => { c.status = 'stopped'; }); disconnect(); await loadSettings(); pane?.update(); },
    'browser-forget': () => modal('Forget all browser data?', `<p>Your sign-ins, cookies and history are deleted, every chat’s tab closes, and sites you allowed to submit forms ask again.</p><div class="modal-actions"><button class="btn" data-action="close">Keep it</button><button class="btn danger" data-action="browser-forget-confirm">Forget everything</button></div>`),
    'browser-forget-confirm': async () => {
      await api.request('/api/browser', { method: 'DELETE' }); close(); chats.forEach(c => { c.status = 'stopped'; c.url = ''; c.title = ''; c.controller = 'model'; }); disconnect(); toast('Browser data deleted'); await loadSettings(); pane?.update();
    },
    'browser-forget-site': async el => { await api.request(`/api/browser/sites/${encodeURIComponent(el.dataset.host)}`, { method: 'DELETE' }); await loadSettings(); },
  };

  return {
    enabled, load, setOn, on, visible, show, hide, toggleButton, card, signature, observe, settingsCard, input, actions,
    attach: hooks => { pane = hooks; },
    reset: () => { disconnect(); chats.clear(); offers.clear(); shownId = null; settings.data = null; },
  };
}
