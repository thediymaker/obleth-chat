import { icon, esc, badge, iconButton } from './manage-ui.js';

// Workspace UI: the composer toggle, tool activity inside responses, and the
// preview pane. The pane lives outside the re-rendered app so the running preview
// is never reloaded by chat updates. It also holds the chat's browser tab, so a chat
// with the browser on and no workspace gets a pane with only that tab.
export function createWorkspace({ api, state, route, toast, chatPath, capabilities, render, modal, close, browser = null, details = null }) {
  const spaces = new Map();
  const pane = document.createElement('aside');
  pane.className = 'workspace-pane'; pane.hidden = true; pane.setAttribute('aria-label', 'Workspace');
  pane.innerHTML = '<div class="workspace-bar"></div><div class="workspace-body"><div class="workspace-preview"><div class="workspace-empty"></div><iframe title="Workspace preview" sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-downloads" referrerpolicy="no-referrer" hidden></iframe></div><div class="workspace-files" hidden></div><div class="workspace-terminal" hidden></div><div class="workspace-browser" hidden></div><div class="workspace-details" hidden></div></div>';
  document.body.append(pane);
  // Drag the pane's left edge to resize it. The width is remembered in this browser;
  // double-click resets it.
  const widthKey = 'obleth-chat.workspace-width';
  const handle = document.createElement('div');
  handle.className = 'workspace-resize'; handle.tabIndex = 0; handle.title = 'Drag to resize. Double-click to reset.';
  handle.setAttribute('role', 'separator'); handle.setAttribute('aria-orientation', 'vertical'); handle.setAttribute('aria-label', 'Resize workspace');
  pane.prepend(handle);
  let preferredWidth = null;
  const sidebarWidth = () => document.querySelector('.sidebar')?.getBoundingClientRect().width || 0;
  // Keep at least 480px for the chat and 360px for the pane.
  const clampWidth = width => Math.round(Math.max(360, Math.min(width, innerWidth - sidebarWidth() - 480)));
  const applyWidth = () => {
    if (preferredWidth == null) document.documentElement.style.removeProperty('--workspace-w');
    else document.documentElement.style.setProperty('--workspace-w', `${clampWidth(preferredWidth)}px`);
  };
  function setWidth(width, save = false) {
    preferredWidth = width == null ? null : clampWidth(width);
    applyWidth();
    if (save) try { if (preferredWidth == null) localStorage.removeItem(widthKey); else localStorage.setItem(widthKey, String(preferredWidth)); } catch { /* storage unavailable */ }
  }
  try { const saved = Number(localStorage.getItem(widthKey)); if (saved > 0) { preferredWidth = saved; applyWidth(); } } catch { /* storage unavailable */ }
  handle.addEventListener('pointerdown', event => {
    if (event.button !== 0) return;
    event.preventDefault(); handle.setPointerCapture(event.pointerId); document.body.classList.add('workspace-resizing');
  });
  handle.addEventListener('pointermove', event => { if (handle.hasPointerCapture(event.pointerId)) setWidth(innerWidth - event.clientX); });
  const endDrag = event => {
    if (!document.body.classList.contains('workspace-resizing')) return;
    document.body.classList.remove('workspace-resizing');
    if (handle.hasPointerCapture(event.pointerId)) handle.releasePointerCapture(event.pointerId);
    setWidth(preferredWidth, true);
  };
  handle.addEventListener('pointerup', endDrag); handle.addEventListener('pointercancel', endDrag);
  handle.addEventListener('dblclick', () => setWidth(null, true));
  handle.addEventListener('keydown', event => {
    const step = event.shiftKey ? 96 : 32, width = pane.getBoundingClientRect().width;
    if (event.key === 'ArrowLeft') setWidth(width + step, true); else if (event.key === 'ArrowRight') setWidth(width - step, true); else return;
    event.preventDefault();
  });
  addEventListener('resize', applyWidth);
  const frame = pane.querySelector('iframe'), empty = pane.querySelector('.workspace-empty');
  const terminalEl = pane.querySelector('.workspace-terminal'), browserEl = pane.querySelector('.workspace-browser'), detailsEl = pane.querySelector('.workspace-details'), encoder = new TextEncoder();
  let shownId = null, frameSrc = '', polling = null, shell = null, xterm = null, diskLimit = 0;

  const enabled = () => !!capabilities.features?.workspaces;
  const entry = id => { if (!spaces.has(id)) spaces.set(id, { workspace: null, loaded: false, tab: 'preview', open: true, files: null, file: null, url: '' }); return spaces.get(id); };
  // The pane shows beside a chat, or full width on a project's page under Workspaces.
  const full = () => route().page === 'workspaces' && !!route().id;
  const browsing = id => !!browser?.visible(id);
  // The chat's details open in the pane as their own tab, with or without a workspace.
  const detailing = id => !!details && route().page === 'chat' && !!spaces.get(id)?.details;
  const current = () => {
    const r = route(), id = r.page === 'chat' || full() ? r.id : null;
    if (!id) return null;
    if (spaces.get(id)?.workspace) return { id, space: spaces.get(id) };
    return browsing(id) || detailing(id) ? { id, space: entry(id) } : null;
  };
  const listing = { items: null, error: '', loading: false };

  async function load(id) {
    if (!enabled() || !id) return;
    const space = entry(id);
    try {
      const result = await api.request(`${chatPath(id)}/workspace`);
      const appeared = !!result.workspace !== !!space.workspace;
      space.workspace = result.workspace; space.loaded = true; if (result.diskLimit) diskLimit = result.diskLimit;
      watch(id);
      // The composer toggle and the side panel depend on whether a workspace exists.
      if (appeared && route().id === id) render(); else update();
    } catch { space.loaded = true; }
  }
  async function create(id) {
    const space = entry(id);
    const result = await api.request(`${chatPath(id)}/workspace`, { method: 'POST' });
    space.workspace = result.workspace; space.open = true; watch(id); update();
    return result.workspace;
  }
  function watch(id) {
    clearTimeout(polling);
    const space = spaces.get(id);
    if (!space?.workspace || route().id !== id) return;
    // Keep checking while the pod wakes, then briefly while Chat restarts its dev server.
    const w = space.workspace, waking = w.status === 'starting' || w.status === 'stopped';
    if (waking) space.wakePolls = 0;
    else if (w.status !== 'running' || w.previewPort || (space.wakePolls = (space.wakePolls || 0) + 1) > 20) return;
    polling = setTimeout(async () => { await load(id); }, 2000);
  }
  let reloading = null;
  async function showPreview(id, force = false) {
    const space = entry(id);
    if ((!space.workspace?.previewPort && !force) || space.linking) return;
    space.linking = true;
    try {
      const page = space.page ? `?path=${encodeURIComponent(space.page)}` : '';
      const link = await api.request(`${chatPath(id)}/workspace/preview${page}`, { method: 'POST' });
      space.url = link.url; space.origin = link.origin;
      if (shownId === id) { frameSrc = link.url; frame.src = link.url; }
      update();
    } catch (error) { toast(error.message); }
    finally { space.linking = false; }
  }
  async function loadFiles(id) {
    const space = entry(id);
    try { space.files = (await api.request(`${chatPath(id)}/workspace/files`)).entries; }
    catch (error) { space.files = []; space.filesError = error.message; }
    update();
  }
  const imageFile = path => /\.(png|jpe?g|gif|webp|svg)$/i.test(path);
  async function openFile(id, path) {
    const space = entry(id);
    if (imageFile(path)) { space.file = { path, image: `${chatPath(id)}/workspace/raw?path=${encodeURIComponent(path)}` }; update(); return; }
    space.file = { path, content: null };
    update();
    try { const result = await api.request(`${chatPath(id)}/workspace/file?path=${encodeURIComponent(path)}`); space.file = { path, content: result.content, truncated: result.truncated }; }
    catch (error) { space.file = { path, content: '', error: error.message }; }
    update();
  }

  function statusLabel(w) {
    return { starting: 'Starting', running: 'Running', stopped: 'Asleep', error: 'Error' }[w.status] || w.status;
  }
  function bar(id, space) {
    const w = space.workspace;
    const tabs = available(id, space).map(([key, label]) => `<button class="tab ${space.tab === key ? 'active' : ''}" data-action="workspace-tab" data-tab="${key}" ${space.tab === key ? 'aria-current="true"' : ''}>${label}</button>`).join('');
    if (space.tab === 'browser' || space.tab === 'details') return `<div class="workspace-tabs">${tabs}</div><span class="grow"></span><div class="workspace-actions">${space.tab === 'browser' ? iconButton('power', 'browser-off', 'Turn off the browser in this chat') : ''}${full() ? '' : iconButton('x', 'workspace-close', 'Hide pane')}</div>`;
    const address = space.origin ? `<span class="workspace-address" title="${esc(space.origin)}">${esc(space.origin.replace(/^https?:\/\//, ''))}</span>` : '<span class="workspace-address muted">No preview yet</span>';
    const used = w.diskBytes, diskFull = used != null && diskLimit && used > diskLimit;
    const usage = used != null && diskLimit ? `<span class="workspace-usage ${diskFull ? 'full' : ''}" title="${diskFull ? 'Your workspaces are full: delete files to keep working' : 'Disk used by all of your workspaces'}">${(used / 2 ** 30).toFixed(1)} / ${Math.round(diskLimit / 2 ** 30)} GB</span>` : '';
    return `<div class="workspace-tabs">${tabs}</div>${address}<div class="workspace-actions">${usage}${badge(statusLabel(w), w.status === 'running' ? 'accent' : w.status === 'error' ? 'amber' : '')}${iconButton('refresh', 'workspace-reload', { files: 'Refresh files', terminal: 'Restart terminal' }[space.tab] || 'Reload preview', w.previewPort || space.tab !== 'preview' ? '' : 'disabled')}${w.status !== 'starting' ? `<a class="icon-btn" href="${esc(`${chatPath(id)}/workspace/download`)}" download aria-label="Download project" title="Download project (.zip, without dependencies)">${icon('download')}</a>` : ''}${space.origin ? `<a class="icon-btn" href="${esc(space.url || space.origin)}" target="_blank" rel="noopener" aria-label="Open preview in a new tab" title="Open in a new tab">${icon('external')}</a>` : ''}${full() ? '' : iconButton('x', 'workspace-close', 'Hide workspace')}</div>`;
  }
  function available(id, space) {
    return [...(space.workspace ? [['preview', 'Preview'], ['files', 'Files'], ['terminal', 'Terminal']] : []), ...(browser?.on(id) ? [['browser', 'Browser']] : []), ...(details && route().page === 'chat' ? [['details', 'Details']] : [])];
  }
  function emptyState(space) {
    const w = space.workspace;
    if (w.status === 'starting') return `<div class="workspace-note" role="status">${icon('orbit')}<strong>Starting your workspace…</strong><p>A private Linux environment is being prepared for this conversation.</p></div>`;
    if (w.status === 'error') return `<div class="workspace-note">${icon('info')}<strong>The workspace could not start.</strong><p>${esc(w.errorMessage || 'Try again in a moment.')}</p></div>`;
    if (w.status === 'stopped') return `<div class="workspace-note">${icon('clock')}<strong>The workspace is asleep.</strong><p>Your files are kept. Send a message and the assistant will wake it up.</p></div>`;
    return `<div class="workspace-note">${icon('terminal')}<strong>Nothing to preview yet.</strong><p>Ask for something to build, like “Build a site for my lab's reading group.” The running app appears here.</p></div>`;
  }
  function filesView(id, space) {
    if (space.file) {
      const f = space.file;
      const page = /\.html?$/i.test(f.path) ? `<button class="btn ghost small" data-action="workspace-file-preview" data-path="${esc(f.path)}">${icon('external')}Open in preview</button>` : '';
      const head = `<div class="workspace-file-head"><span class="flip">${iconButton('right', 'workspace-file-back', 'Back to files')}</span><code>${esc(f.path)}</code>${page}</div>`;
      if (f.image) return `${head}<div class="workspace-image"><img src="${esc(f.image)}" alt="${esc(f.path)}"></div>`;
      if (f.content && !f.error && /\.csv$/i.test(f.path)) return `${head}${csvTable(f.content, f.truncated)}`;
      return `${head}${f.content == null ? '<p class="workspace-loading">Opening…</p>' : f.error ? `<p class="inline-note">${esc(f.error)}</p>` : `<pre class="workspace-code"><code>${esc(f.content)}</code></pre>${f.truncated ? '<p class="inline-note">Showing the first 200 KB.</p>' : ''}`}`;
    }
    if (space.workspace.status !== 'running') return emptyState(space);
    if (!space.files) return '<p class="workspace-loading">Loading files…</p>';
    if (!space.files.length) return '<div class="workspace-note">' + icon('file') + '<strong>No files yet.</strong><p>Files the assistant creates appear here.</p></div>';
    return `<ul class="workspace-tree">${space.files.map(e => {
      const depth = e.path.split('/').length - 1, name = e.path.split('/').pop();
      return e.type === 'directory' ? `<li class="dir" style="--depth:${depth}">${icon('grid')}<span>${esc(name)}</span></li>` : `<li style="--depth:${depth}"><button data-action="workspace-file" data-path="${esc(e.path)}">${icon('file')}<span>${esc(name)}</span><small>${formatBytes(e.size)}</small></button></li>`;
    }).join('')}</ul>`;
  }
  // A small CSV reader for the file viewer: quoted fields, first 200 rows.
  function csvRows(text, limit) {
    const rows = []; let row = [], field = '', quoted = false;
    for (let i = 0; i < text.length && rows.length < limit; i++) {
      const c = text[i];
      if (quoted) { if (c === '"' && text[i + 1] === '"') { field += '"'; i++; } else if (c === '"') quoted = false; else field += c; }
      else if (c === '"') quoted = true;
      else if (c === ',') { row.push(field); field = ''; }
      else if (c === '\n' || c === '\r') { if (c === '\r' && text[i + 1] === '\n') i++; row.push(field); rows.push(row); row = []; field = ''; }
      else field += c;
    }
    if ((field || row.length) && rows.length < limit) { row.push(field); rows.push(row); }
    return rows;
  }
  function csvTable(text, truncated) {
    const rows = csvRows(text, 201), [header = [], ...body] = rows, more = rows.length > 200 || truncated;
    return `<div class="workspace-table"><table><thead><tr>${header.map(h => `<th>${esc(h)}</th>`).join('')}</tr></thead><tbody>${body.slice(0, 199).map(r => `<tr>${r.map(v => `<td>${esc(v)}</td>`).join('')}</tr>`).join('')}</tbody></table></div>${more ? '<p class="inline-note">Showing the first 200 rows.</p>' : ''}`;
  }
  function formatBytes(n) { return n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1048576).toFixed(1)} MB`; }

  // Called after every app render and whenever workspace state changes.
  function update() {
    const shown = current(), visible = !!shown && (full() || ((shown.space.workspace ? shown.space.open || detailing(shown.id) : browsing(shown.id) || detailing(shown.id)) && route().page === 'chat'));
    document.body.classList.toggle('workspace-open', visible && !full());
    // Details alone need less room than a preview or a page.
    document.body.classList.toggle('pane-details-only', visible && !shown.space.workspace && !browsing(shown.id));
    document.body.classList.toggle('workspace-full', visible && full());
    pane.hidden = !visible;
    if (!visible) { if (!shown) shownId = null; browser?.hide(); return; }
    const { id, space } = shown;
    if (shownId !== id) { closeTerminal(); shownId = id; frameSrc = ''; frame.removeAttribute('src'); if (space.url) { frameSrc = space.url; frame.src = space.url; } }
    // A tab that is gone (the browser turned off, details closed) falls back to the next one there.
    const tabs = available(id, space).map(([key]) => key).filter(key => key !== 'details' || space.details || !space.workspace && !browsing(id));
    if (!tabs.includes(space.tab)) space.tab = tabs.includes('preview') ? 'preview' : space.details || !browsing(id) ? 'details' : 'browser';
    pane.querySelector('.workspace-bar').innerHTML = bar(id, space);
    const browsingTab = space.tab === 'browser', detailsTab = space.tab === 'details';
    browserEl.hidden = !browsingTab;
    detailsEl.hidden = !detailsTab;
    if (detailsTab) {
      pane.querySelector('.workspace-preview').hidden = true;
      pane.querySelector('.workspace-files').hidden = true;
      terminalEl.hidden = true;
      browser?.hide();
      detailsEl.innerHTML = details(id);
      return;
    }
    if (browsingTab) {
      pane.querySelector('.workspace-preview').hidden = true;
      pane.querySelector('.workspace-files').hidden = true;
      terminalEl.hidden = true;
      browser.show(browserEl, id);
      return;
    }
    browser?.hide();
    // The preview can come back after the pane opened, for example once a woken pod's server is up.
    if (space.workspace.previewPort && !space.url) showPreview(id);
    const previewing = space.tab === 'preview', hasPreview = !!(space.workspace.previewPort && frameSrc);
    pane.querySelector('.workspace-preview').hidden = !previewing;
    pane.querySelector('.workspace-files').hidden = space.tab !== 'files';
    terminalEl.hidden = space.tab !== 'terminal';
    if (space.tab === 'terminal') showTerminal(id);
    frame.hidden = !hasPreview; empty.hidden = hasPreview;
    if (!hasPreview) empty.innerHTML = emptyState(space);
    if (space.tab === 'files') {
      if (space.files == null && !space.loadingFiles && space.workspace.status === 'running' && !space.file) { space.loadingFiles = true; loadFiles(id).finally(() => { space.loadingFiles = false; }); }
      pane.querySelector('.workspace-files').innerHTML = filesView(id, space);
    }
  }

  // One terminal at a time, for the conversation shown in the pane. xterm.js loads on
  // first use. Press Enter after the session ends to reconnect.
  async function loadXterm() {
    if (!xterm) {
      const link = document.createElement('link'); link.rel = 'stylesheet'; link.href = './vendor/xterm/xterm-6.0.0.css'; document.head.append(link);
      const [{ Terminal }, { FitAddon }] = await Promise.all([import('./vendor/xterm/xterm-6.0.0.mjs'), import('./vendor/xterm/addon-fit-0.11.0.mjs')]);
      xterm = { Terminal, FitAddon };
    }
    return xterm;
  }
  function terminalTheme() {
    const css = getComputedStyle(document.documentElement), color = name => css.getPropertyValue(name).trim();
    return { background: color('--bg'), foreground: color('--text'), cursor: color('--text'), cursorAccent: color('--bg'), selectionBackground: color('--selection') };
  }
  window.addEventListener('obleth-theme-change', () => {
    if (shell?.term) shell.term.options.theme = terminalTheme();
  });
  function closeTerminal() {
    if (!shell) return;
    shell.closed = true; shell.socket?.close(); shell.observer.disconnect(); shell.term.dispose(); terminalEl.replaceChildren(); shell = null;
  }
  async function showTerminal(id) {
    if (shell?.id === id) { requestAnimationFrame(() => { try { shell?.fit.fit(); } catch { /* hidden */ } shell?.term.focus(); }); return; }
    if (shell?.loading === id) return;
    closeTerminal();
    shell = { loading: id };
    let lib;
    try { lib = await loadXterm(); } catch { shell = null; toast('The terminal could not load.'); return; }
    if (shell?.loading !== id || current()?.id !== id) { if (shell?.loading === id) shell = null; return; }
    const term = new lib.Terminal({ cursorBlink: true, fontSize: 13, fontFamily: 'ui-monospace, SFMono-Regular, Consolas, monospace', scrollback: 5000, minimumContrastRatio: 4.5, theme: terminalTheme() });
    const fit = new lib.FitAddon(); term.loadAddon(fit); term.open(terminalEl);
    const observer = new ResizeObserver(() => { if (!terminalEl.hidden) try { fit.fit(); } catch { /* not laid out yet */ } });
    observer.observe(terminalEl);
    shell = { id, term, fit, observer, socket: null, closed: false };
    const current_ = shell;
    term.onData(data => { if (current_.socket?.readyState === WebSocket.OPEN) current_.socket.send(encoder.encode(data)); else if (data === '\r' && !current_.socket) connect(current_); });
    term.onResize(({ cols, rows }) => { if (current_.socket?.readyState === WebSocket.OPEN) current_.socket.send(JSON.stringify({ type: 'resize', cols, rows })); });
    connect(current_);
    requestAnimationFrame(() => { try { fit.fit(); } catch { /* hidden */ } term.focus(); });
  }
  function connect(s) {
    s.term.write('\x1b[2mConnecting to your workspace…\x1b[0m\r\n');
    const socket = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}${chatPath(s.id)}/workspace/terminal`);
    socket.binaryType = 'arraybuffer'; s.socket = socket;
    socket.onopen = () => { try { s.fit.fit(); } catch { /* hidden */ } socket.send(JSON.stringify({ type: 'resize', cols: s.term.cols, rows: s.term.rows })); };
    socket.onmessage = event => { if (typeof event.data !== 'string') s.term.write(new Uint8Array(event.data)); };
    socket.onclose = () => { if (s.closed) return; s.socket = null; s.term.write('\r\n\x1b[2m[The terminal closed. Press Enter to reconnect.]\x1b[0m\r\n'); };
  }

  // Workspaces page: every project, and one project with its pane full width.
  async function loadList() {
    listing.loading = true; listing.error = '';
    try { const r = await api.request('/api/workspaces'); listing.items = r.workspaces || []; if (r.diskLimit) diskLimit = r.diskLimit; }
    catch (e) { listing.error = e.message; listing.items ||= []; }
    finally { listing.loading = false; render(); }
  }
  const ago = value => { const m = Math.round((Date.now() - new Date(value)) / 60000); return m < 1 ? 'just now' : m < 60 ? `${m} min ago` : m < 1440 ? `${Math.round(m / 60)} h ago` : `${Math.round(m / 1440)} d ago`; };
  function page() {
    const id = route().id;
    if (id) {
      const w = spaces.get(id)?.workspace, item = listing.items?.find(p => p.conversationId === id);
      const title = item?.title || state.chats.find(c => c.id === id)?.title || 'Project';
      const missing = spaces.get(id)?.loaded && !w;
      return `<div class="workspace-page-head"><a class="btn ghost small" href="#workspaces"><span class="flip">${icon('right')}</span>Workspaces</a><h1 title="${esc(title)}">${esc(title)}</h1><a class="btn small" href="#chat/${encodeURIComponent(id)}">${icon('chat')}Open chat</a></div>${missing ? `<div class="page"><p class="inline-note">This conversation has no workspace. <a href="#workspaces">Back to your workspaces</a></p></div>` : ''}`;
    }
    const items = listing.items;
    const head = `<div class="page-head spread"><div><h1>Workspaces</h1><p>Every conversation with a workspace is a project folder on your private Linux machine. Open one to browse its files, use a terminal or download it.</p></div><button class="btn primary small" data-action="starter" data-workspace="1" data-prompt="Build me a site for ">${icon('plus')}New project</button></div>`;
    if (items == null) return `<div class="page workspaces-page">${head}<p class="inline-note" role="status">Loading your workspaces…</p></div>`;
    if (!items.length) return `<div class="page workspaces-page">${head}${listing.error ? `<p class="inline-note">${esc(listing.error)}</p>` : ''}<section class="feature-empty card"><span class="tile-icon">${icon('terminal')}</span><h2>Describe it, then watch it run.</h2><p>Turn on Workspace in the composer and ask for something to build. The assistant writes the files, runs the commands and starts the app, and the live preview opens beside the chat.</p><button class="btn primary" data-action="starter" data-workspace="1" data-prompt="Build me a site for ">${icon('play')}Start building</button></section></div>`;
    const machine = items[0], used = machine.diskBytes;
    const summary = `<section class="card workspace-machine">${icon('server')}<div class="grow"><h2>Your machine ${badge(statusLabel(machine), machine.status === 'running' ? 'accent' : machine.status === 'error' ? 'amber' : '')}</h2><p>${machine.status === 'stopped' ? 'Asleep: it wakes when you open a project or the assistant needs it. Files are kept.' : 'It sleeps after an hour unused and keeps your files.'}</p></div>${used != null && diskLimit ? `<div class="workspace-disk"><span>${(used / 2 ** 30).toFixed(1)} of ${Math.round(diskLimit / 2 ** 30)} GB used</span><progress value="${used}" max="${diskLimit}" aria-label="Disk used"></progress></div>` : ''}</section>`;
    const rows = items.map(p => {
      const id = encodeURIComponent(p.conversationId), live = p.status === 'running' && p.previewPort;
      return `<div class="workspace-row"><a class="grow workspace-row-main" href="#workspaces/${id}">${icon('terminal')}<span class="grow"><strong>${esc(p.title || 'Untitled conversation')}</strong><small>${live ? 'Preview running · ' : ''}Used ${esc(ago(p.lastUsedAt))}</small></span></a><div class="user-actions"><a class="btn small" href="#workspaces/${id}">Open</a><a class="btn small ghost" href="#chat/${id}">Chat</a><a class="btn small ghost" href="${esc(chatPath(p.conversationId))}/workspace/download" download title="Download project (.zip, without dependencies)">${icon('download')}</a>${iconButton('trash', 'workspace-remove', `Delete the workspace of ${p.title}`, `data-id="${esc(p.conversationId)}"`)}</div></div>`;
    }).join('');
    return `<div class="page workspaces-page">${head}${listing.error ? `<p class="inline-note">${esc(listing.error)}</p>` : ''}${summary}<section class="card workspace-list">${rows}</section></div>`;
  }

  function hasWorkspace(id) { return !!spaces.get(id)?.workspace; }
  function toggleButton() {
    if (!enabled()) return '';
    const id = route().id, on = id ? hasWorkspace(id) : !!state.workspaceDraft, shown = on && id && ['preview', 'files', 'terminal'].some(tab => showing(id, tab));
    return `<button type="button" class="btn ghost small workspace-toggle ${on ? 'on' : ''} ${shown ? 'showing' : ''}" data-action="workspace-toggle" aria-pressed="${on}" title="${on ? 'This conversation has a workspace: the model can create files, run commands and show a live preview' : 'Give the model a private Linux workspace with a live preview'}" ${state.streaming ? 'disabled' : ''}>${icon('terminal')}<span class="option-text">Workspace</span></button>`;
  }

  const toolIcons = { write_file: 'edit', edit_file: 'edit', read_file: 'file', list_files: 'grid', run_command: 'terminal', start_server: 'play', server_logs: 'file' };
  const stepState = { running: 'Running…', awaiting: 'Needs your answer', error: 'Failed', declined: 'Declined' };
  function step(s) {
    const asking = s.status === 'awaiting';
    const question = asking ? `<div class="approval"><span>${s.question ? esc(s.question) : `Allow ${esc(s.connection || 'this tool')} to run this?`}</span><div class="approval-actions">${[['allow', s.question ? 'Allow once' : 'Allow', 'primary'], ['chat', s.remember || 'Allow for this chat', ''], ['deny', 'Deny', '']].map(([d, label, tone]) => `<button type="button" class="btn small ${tone}" data-action="tool-decision" data-decision="${d}" data-call="${esc(s.id)}" ${d === 'chat' ? `title="Run ${esc(s.summary)} without asking for the rest of this conversation"` : ''}>${label}</button>`).join('')}</div></div>` : '';
    const time = stepState[s.status] || (s.durationMs != null ? `${(s.durationMs / 1000).toFixed(1)}s` : '');
    return `<li class="activity-step ${esc(s.status)}">${icon(s.connection ? 'link' : toolIcons[s.tool] || 'terminal')}<div class="grow"><span>${esc(s.summary)}</span>${s.detail && s.status !== 'running' ? `<pre>${esc(s.detail)}</pre>` : ''}${question}</div><small>${esc(time)}</small></li>`;
  }
  function activity(m) {
    // Thoughts show in the thinking panel and knowledge searches in the Sources card.
    const steps = (Array.isArray(m.activity) ? m.activity : []).filter(s => s.kind !== 'sources' && s.kind !== 'thought' && s.kind !== 'saved' && s.kind !== 'question' && s.kind !== 'workspace' && s.kind !== 'research' && s.kind !== 'browser-request' && s.kind !== 'browser-handoff' && s.tool !== 'search_knowledge' && s.tool !== 'save_item');
    if (!steps.length) return '';
    const live = steps.find(s => s.status === 'awaiting') || steps.find(s => s.status === 'running'), failed = steps.filter(s => s.status === 'error').length;
    const kind = steps.every(s => !s.connection) ? 'workspace step' : 'step';
    const label = live ? (live.status === 'awaiting' ? `Waiting for you: ${live.summary}` : live.summary) : `${steps.length} ${kind}${steps.length === 1 ? '' : 's'}${failed ? ` · ${failed} failed` : ''}`;
    return `<details class="message-activity" ${live || state.openActivity?.has(m.id) ? 'open' : ''} data-activity="${esc(m.id || '')}"><summary>${icon(live?.status === 'running' ? 'orbit' : live ? 'shield' : 'terminal')}<span class="activity-label ${live ? 'is-running' : ''}">${esc(label)}</span>${icon('down')}</summary><ol class="activity-list">${steps.map(step).join('')}</ol></details>`;
  }
  function activitySignature(m) { return JSON.stringify((m.activity || []).map(s => [s.id, s.status])); }

  function onEvent(event, assistant, conversationId) {
    if (event.event === 'tool') {
      const steps = assistant.activity ||= [];
      const index = steps.findIndex(s => s.id === event.data.id);
      if (index >= 0) steps[index] = event.data; else steps.push(event.data);
      if (event.data.tool === 'start_server' || event.data.tool === 'write_file' || event.data.tool === 'edit_file') { const space = spaces.get(conversationId); if (space) space.files = null; }
      // A plain web server has no live reload, so reload the preview after each change.
      if (event.data.reload) { clearTimeout(reloading); reloading = setTimeout(() => showPreview(conversationId), 600); }
      return true;
    }
    if (event.event === 'draft') { assistant._draft = event.data; return true; }
    if (event.event === 'preview') {
      const space = entry(conversationId);
      if (space.workspace) { space.workspace.previewPort = event.data.port; space.workspace.status = 'running'; }
      const reopened = !space.open; space.tab = 'preview'; space.open = true;
      if (reopened && route().id === conversationId) render();
      showPreview(conversationId, true);
      return true;
    }
    return false;
  }

  const actions = {
    'tool-decision': async el => {
      const id = route().id, remember = el.dataset.decision === 'chat', decision = remember ? 'allow' : el.dataset.decision;
      el.closest('.approval')?.querySelectorAll('button').forEach(b => { b.disabled = true; });
      try { await api.request(`${chatPath(id)}/approvals`, { method: 'POST', body: { callId: el.dataset.call, decision, remember } }); }
      catch (e) { toast(e.message); }
      const waiting = (state.active?.messages || []).flatMap(m => m.activity || []).find(s => s.id === el.dataset.call && s.status === 'awaiting');
      if (waiting) waiting.status = decision === 'allow' ? 'running' : 'declined';
      return 'render';
    },
    'workspace-toggle': async () => {
      const id = route().id;
      if (!id) { state.workspaceDraft = !state.workspaceDraft; return 'render'; }
      // A press shows the workspace's tabs, or hides the pane when they are showing.
      if (hasWorkspace(id)) {
        const space = spaces.get(id), own = ['preview', 'files', 'terminal'];
        if (own.some(tab => showing(id, tab))) { space.open = false; space.details = false; } else { space.open = true; if (!own.includes(space.tab)) space.tab = 'preview'; }
        update(); return 'render';
      }
      await create(id); toast('Workspace created. Ask for something to build.'); return 'render';
    },
    'workspace-tab': el => { const c = current(); if (!c) return; c.space.tab = el.dataset.tab; if (c.space.tab === 'details') c.space.details = true; if (c.space.tab === 'browser') browser?.reveal(c.id); c.space.file = null; if (c.space.tab === 'files') c.space.files = null; update(); },
    'workspace-reload': () => { const c = current(); if (!c) return; if (c.space.tab === 'terminal') { closeTerminal(); update(); return; } if (c.space.tab === 'files') { c.space.files = null; c.space.file = null; update(); } else showPreview(c.id); },
    'workspace-close': () => {
      const c = current(); if (!c) return;
      const onlyDetails = !c.space.workspace && !browsing(c.id);
      c.space.details = false;
      if (c.space.workspace) c.space.open = false; else if (!onlyDetails) browser?.dismiss(c.id);
      update(); return 'render';
    },
    'workspace-remove': el => { const item = listing.items?.find(p => p.conversationId === el.dataset.id); modal(`Delete this workspace?`, `<p>The project folder of “${esc(item?.title || 'this conversation')}” and everything in it are deleted. The conversation stays.</p><div class="modal-actions"><button class="btn" data-action="close">Keep it</button><button class="btn danger" data-action="workspace-confirm-remove" data-id="${esc(el.dataset.id)}">Delete workspace</button></div>`); },
    'workspace-confirm-remove': async el => { await api.request(`${chatPath(el.dataset.id)}/workspace`, { method: 'DELETE' }); spaces.delete(el.dataset.id); close(); toast('Workspace deleted'); await loadList(); },
    'turn-open-file': el => { const id = route().id, space = spaces.get(id); if (!space?.workspace) return; space.open = true; space.tab = 'files'; space.files = null; openFile(id, el.dataset.path); return 'render'; },
    'turn-open-files': () => { const id = route().id, space = spaces.get(id); if (!space?.workspace) return; space.open = true; space.tab = 'files'; space.file = null; space.files = null; update(); return 'render'; },
    'workspace-file-preview': el => { const id = route().id, space = spaces.get(id); if (!space?.workspace) return; space.page = el.dataset.path; space.tab = 'preview'; space.file = null; update(); showPreview(id, true); },
    'turn-open-preview': () => { const id = route().id, space = spaces.get(id); if (!space?.workspace) return; space.open = true; space.tab = 'preview'; update(); showPreview(id, true); return 'render'; },
    'checkpoint-restore': el => modal(`Restore ${el.dataset.label || 'this checkpoint'}?`, `<p>The project goes back to how it was at this checkpoint: changed files are restored and files added since are removed. Installed packages stay. The current state is saved first, so you can come back to it.</p><div class="modal-actions"><button class="btn" data-action="close">Cancel</button><button class="btn primary" data-action="checkpoint-confirm" data-sha="${esc(el.dataset.sha)}" data-label="${esc(el.dataset.label || '')}">Restore</button></div>`),
    'checkpoint-confirm': async el => {
      const id = route().id; el.disabled = true;
      let result;
      try { result = await api.request(`${chatPath(id)}/workspace/checkpoints/${encodeURIComponent(el.dataset.sha)}/restore`, { method: 'POST' }); }
      catch (e) { close(); toast(e.message); return; }
      const space = spaces.get(id); if (space) { space.files = null; space.file = null; }
      showPreview(id, true); update();
      if (el.dataset.undo) { close(); toast('Back to where you were'); return; }
      modal('Checkpoint restored', `<p>The project is back at ${esc(el.dataset.label || 'the checkpoint')}. The version you had is saved, so you can go back to it.</p><div class="modal-actions"><button class="btn" data-action="checkpoint-confirm" data-sha="${esc(result?.before || '')}" data-undo="1">Undo</button><button class="btn primary" data-action="close">Done</button></div>`);
    },
    'workspace-file': el => { const c = current(); if (c) openFile(c.id, el.dataset.path); },
    'workspace-file-back': () => { const c = current(); if (c) { c.space.file = null; update(); } },
  };

  /** Opens the pane on a tab, such as the browser when the model starts browsing. */
  function showTab(id, tab) {
    const space = entry(id);
    space.tab = tab; if (space.workspace) space.open = true;
    if (route().id === id) render(); else update();
  }
  // From state, not the DOM, so the composer renders the right buttons before the pane updates.
  function visibleFor(id) {
    const space = spaces.get(id);
    if (!space || route().id !== id) return false;
    if (full()) return true;
    return route().page === 'chat' && (space.workspace ? space.open || detailing(id) : browsing(id) || detailing(id));
  }
  const showing = (id, tab) => visibleFor(id) && entry(id).tab === tab;
  /** Opens the chat's details in the pane, or closes them and goes back to the tab before. */
  function toggleDetails(id) {
    if (!id || !details) return;
    const space = entry(id);
    if (showing(id, 'details')) {
      space.details = false;
      if (space.before && space.before !== 'details') space.tab = space.before; else if (space.workspace) space.open = false;
    } else {
      space.before = !pane.hidden && shownId === id ? space.tab : null;
      space.details = true; space.tab = 'details';
      if (space.workspace) space.open = true;
    }
    update();
  }
  return { load, loadList, page, create, update, toggleButton, activity, activitySignature, onEvent, actions, hasWorkspace, enabled, showTab, showing, toggleDetails, isOpen: id => (!!spaces.get(id)?.workspace && spaces.get(id).open) || browsing(id) || detailing(id), reset: () => { closeTerminal(); spaces.clear(); shownId = null; frame.removeAttribute('src'); update(); } };
}
