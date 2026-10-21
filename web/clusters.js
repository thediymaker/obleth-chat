import { esc, icon, badge, btn, empty, errorBox, when } from './manage-ui.js';

// The Clusters page: each Slurm or Kubernetes cluster with its live state, as the
// viewer's own account sees it. Adding, tokens and editing use the connection dialogs.
export function createClusters({ api, route, render, connections }) {
  const statuses = new Map();
  let timer = null;
  const isCluster = c => c.kind === 'slurm' || c.kind === 'kubernetes';
  const clusters = () => connections.items().filter(isCluster);

  async function fetchStatus(c) {
    if (!isCluster(c)) return;
    const previous = statuses.get(c.id);
    statuses.set(c.id, { ...previous, loading: true }); render();
    try { statuses.set(c.id, await api.request(`/api/connections/${encodeURIComponent(c.id)}/status`)); }
    catch (e) { if (e.name !== 'AbortError') statuses.set(c.id, { state: 'error', error: e.message }); }
    if (route().page === 'clusters') render();
  }
  async function load() {
    await connections.load();
    await Promise.all(clusters().map(fetchStatus));
    clearInterval(timer);
    timer = setInterval(() => {
      if (route().page !== 'clusters') { clearInterval(timer); timer = null; return; }
      if (!document.hidden) clusters().forEach(fetchStatus);
    }, 30000);
  }
  /** A cluster was added, edited or its token changed: show its new state. */
  function changed(item) {
    if (item.removed) { statuses.delete(item.id); return; }
    if (isCluster(item) && route().page === 'clusters') fetchStatus(item);
  }

  const number = n => n == null ? '–' : Number(n).toLocaleString();
  const gib = mb => mb >= 1024 ? `${(mb / 1024).toFixed(mb >= 10240 ? 0 : 1)} GiB` : `${number(mb)} MiB`;
  const meter = (label, used, total, text) => `<div class="cluster-stat"><span class="eyebrow">${esc(label)}</span><strong>${esc(text)}</strong>${total ? `<progress class="meter" max="${total}" value="${Math.min(used, total)}" aria-label="${esc(label)}: ${esc(text)}"></progress>` : ''}</div>`;
  const stat = (label, value, note = '') => `<div class="cluster-stat"><span class="eyebrow">${esc(label)}</span><strong>${esc(value)}</strong>${note ? `<small>${esc(note)}</small>` : ''}</div>`;
  const table = (heads, rows, emptyText) => rows.length
    ? `<div class="table-wrap"><table class="management-table cluster-table"><thead><tr>${heads.map(h => `<th>${esc(h)}</th>`).join('')}</tr></thead><tbody>${rows.map(r => `<tr>${r.map(c => `<td>${c}</td>`).join('')}</tr>`).join('')}</tbody></table></div>`
    : `<p class="inline-note">${esc(emptyText)}</p>`;
  const section = (title, body, count = '') => `<section class="cluster-section"><h3>${esc(title)}${count !== '' ? ` <span class="count">${esc(String(count))}</span>` : ''}</h3>${body}</section>`;
  const tone = state => /RUNNING|Running|Complete|COMPLETED|IDLE|MIXED|UP|Succeeded|true/.test(state) ? 'ok' : /PENDING|Pending|ContainerCreating|ALLOCATED|COMPLETING/.test(state) ? 'amber' : /FAIL|Fail|Error|CrashLoop|DOWN|DRAIN|TIMEOUT|CANCELLED|OUT_OF_MEMORY|ImagePull|false/.test(state) ? 'bad' : '';
  const state = text => `<span class="cluster-state ${tone(text)}">${esc(text || '–')}</span>`;
  const blocked = v => v && !Array.isArray(v) && v.error;

  function slurm(s) {
    const t = s.totals || {};
    const tiles = `<div class="cluster-stats">${meter('CPUs in use', t.allocCpus, t.cpus, `${number(t.allocCpus)} / ${number(t.cpus)}`)}${meter('Memory in use', t.allocMemoryMb, t.memoryMb, `${gib(t.allocMemoryMb || 0)} / ${gib(t.memoryMb || 0)}`)}${stat('Nodes', number(t.nodes), `${(s.partitions || []).length} partition${(s.partitions || []).length === 1 ? '' : 's'}`)}${stat('Queue', `${number(s.queue?.running)} running`, `${number(s.queue?.pending)} pending`)}</div>`;
    const jobs = table(['Job', 'State', 'Partition', 'Time', 'Nodes or reason'], (s.jobs || []).map(j => [`<strong>${esc(j.id)}</strong><small>${esc(j.name)}</small>`, state(j.state), esc(j.partition), `${esc(j.elapsed)} <small>of ${esc(j.timeLimit)}</small>`, esc(j.nodes || j.reason || '–')]), 'No recent jobs of yours. Slurm forgets finished jobs after a few minutes.');
    const partitions = table(['Partition', 'State', 'Nodes', 'CPUs', 'Max time'], (s.partitions || []).map(p => [`<strong>${esc(p.name)}</strong>`, state(p.state), number(p.nodes), number(p.cpus), esc(p.maxTime)]), 'No partitions.');
    const nodes = table(['Node', 'State', 'CPUs in use', 'Memory in use', 'Partitions'], (s.nodes || []).map(n => [`<strong>${esc(n.name)}</strong>`, state(n.state), `${number(n.allocCpus)} / ${number(n.cpus)}`, `${gib(n.allocMemoryMb)} / ${gib(n.memoryMb)}`, esc(n.partitions)]), 'No nodes.');
    return `${tiles}${section(`Your jobs as ${s.user}`, jobs, (s.jobs || []).length)}${section('Partitions', partitions)}${section('Nodes', nodes, (s.nodes || []).length)}`;
  }

  const QUOTA = { 'requests.cpu': 'CPU requested', cpu: 'CPU requested', 'requests.memory': 'Memory requested', memory: 'Memory requested', pods: 'Pod quota', 'limits.cpu': 'CPU limits', 'limits.memory': 'Memory limits', 'requests.storage': 'Storage', persistentvolumeclaims: 'Volumes' };
  const quotaName = r => QUOTA[r] || r;
  const quotaRank = r => { const i = Object.keys(QUOTA).indexOf(r); return i < 0 ? 99 : i; };
  function kubernetes(k) {
    const pods = Array.isArray(k.pods) ? k.pods : [];
    const running = pods.filter(p => p.phase === 'Running').length;
    const quota = Array.isArray(k.quota) ? k.quota : [];
    const tiles = `<div class="cluster-stats">${stat('Namespace', k.namespace, k.homeNamespace && k.homeNamespace !== k.namespace ? `token's own: ${k.homeNamespace}` : '')}${stat('Pods', blocked(k.pods) ? 'Not visible' : `${running} running`, blocked(k.pods) ? '' : `${pods.length} in all`)}${quota.filter(q => q.ratio != null).sort((a, b) => quotaRank(a.resource) - quotaRank(b.resource)).slice(0, 4).map(q => meter(quotaName(q.resource), Math.round(q.ratio * 1000), 1000, `${q.used} / ${q.hard}`)).join('')}${quota.length ? '' : stat('Quota', 'None', 'No ResourceQuota in this namespace')}</div>`;
    const sectionOf = (title, value, heads, map, emptyText) => section(title, blocked(value) ? `<p class="inline-note">${esc(value.error)}</p>` : table(heads, value.map(map), emptyText), blocked(value) ? '' : value.length);
    const noAccess = !k.allowed?.length ? `<div class="connection-notice">${icon('lock')}<span class="grow">This account has no permissions in ${esc(k.namespace)}.${k.homeNamespace && k.homeNamespace !== k.namespace ? ` Its token belongs to namespace ${esc(k.homeNamespace)}; replace the token and leave the namespace empty to use it.` : ''}</span></div>` : '';
    return `${noAccess}${tiles}${sectionOf('Pods', k.pods, ['Pod', 'State', 'Ready', 'Restarts', 'Age'], p => [`<strong>${esc(p.name)}</strong>`, state(p.phase), esc(p.ready), number(p.restarts), esc(p.age)], 'No pods.')}${sectionOf('Deployments', k.deployments, ['Deployment', 'Ready', 'Age'], d => [`<strong>${esc(d.name)}</strong>`, esc(d.ready), esc(d.age)], 'No deployments.')}${sectionOf('Jobs', k.jobs, ['Job', 'State', 'Age'], j => [`<strong>${esc(j.name)}</strong>`, state(j.state), esc(j.age)], 'No jobs.')}${blocked(k.nodes) ? '' : sectionOf('Nodes', k.nodes, ['Node', 'Ready', 'Roles', 'CPU', 'Memory', 'Version'], n => [`<strong>${esc(n.name)}</strong>`, state(String(n.ready)), esc(n.roles || '–'), esc(n.cpu), esc(n.memory), esc(n.version)], 'No nodes.')}${k.allowed?.length ? `<details class="cluster-allowed"><summary>What this account may do in ${esc(k.namespace)}</summary><ul>${k.allowed.map(r => `<li><code>${esc(r.resources || '*')}</code> ${esc(r.verbs)}</li>`).join('')}</ul></details>` : ''}`;
  }

  function card(c) {
    const s = statuses.get(c.id), kind = connections.kinds[c.kind];
    const id = `data-id="${esc(c.id)}"`;
    const pill = !c.enabled ? badge('Off for everyone', 'amber')
      : c.needsCredential ? '<span class="cluster-pill amber">Needs your token</span>'
      : s?.state === 'connected' ? '<span class="cluster-pill ok">Connected</span>'
      : s?.state === 'error' ? '<span class="cluster-pill bad">Unreachable</span>'
      : '<span class="cluster-pill">Checking…</span>';
    const who = c.credential?.identity || c.identity || c.credential?.user || c.user;
    const facts = [who && `as ${who}`, c.url, s?.checkedAt && `updated ${when(s.checkedAt)}`].filter(Boolean);
    const tokenActions = c.shared ? c.credential ? `${btn('Replace token', 'connection-credential', { tone: 'small ghost', attrs: id })}${btn('Remove my token', 'connection-credential-remove', { tone: 'small ghost', attrs: id })}` : '' : '';
    const body = c.needsCredential
      ? (c.enabled ? `<div class="connection-credential needs"><p>${icon('lock')}Add your own token to see this cluster and use it in chats. Everything then runs as you.</p>${btn('Add your token', 'connection-credential', { tone: 'small primary', attrs: id })}</div>` : '')
      : s?.state === 'error' ? `<div class="connection-notice" role="alert">${icon('info')}<span class="grow">${esc(s.error)}</span></div>`
      : s?.state === 'connected' ? (c.kind === 'slurm' ? slurm(s.status) : kubernetes(s.status))
      : '<p class="inline-note" role="status">Checking the cluster…</p>';
    const inChats = c.usable === false ? '' : `<button type="button" class="btn small ${c.active ? '' : 'primary'}" data-action="connection-toggle" ${id} aria-pressed="${c.active}" title="${c.active ? 'Offered to the model in your chats' : 'Kept out of your chats'}">${c.active ? 'In my chats' : 'Not in my chats'}</button>`;
    const ask = c.active ? `<button class="btn small" data-action="starter" data-prompt="${esc(`Using the ${c.name} cluster, `)}">${icon('chat')}Ask in a new chat</button>` : '';
    return `<article class="cluster-card card"><header class="cluster-head">${icon(kind?.glyph || 'server')}<div class="grow"><h2>${esc(c.name)} ${badge(kind?.label.split(' ')[0] || c.kind)} ${c.shared ? badge('Shared') : ''} ${pill}</h2><p class="connection-url">${facts.map(esc).join(' · ')}</p></div><div class="cluster-head-actions">${s?.loading ? '<span class="inline-note">Refreshing…</span>' : ''}<button class="icon-btn" data-action="cluster-refresh" ${id} aria-label="Refresh ${esc(c.name)}" title="Refresh">${icon('refresh')}</button></div></header>${body}<footer class="cluster-actions">${ask}${inChats}${tokenActions}${c.canManage ? `${btn('Edit', 'connection-edit', { tone: 'small ghost', attrs: id })}${btn('Remove', 'connection-remove', { tone: 'small ghost', attrs: id })}` : ''}</footer></article>`;
  }

  function page() {
    const list = clusters();
    const head = `<div class="page-head spread"><div><h1>Clusters</h1><p>Slurm and Kubernetes clusters the model can work on in your chats, with your own account and permissions. Reading runs without asking; submitting, cancelling, applying and deleting ask you first.</p></div>${btn('Add cluster', 'cluster-add', { icon: 'plus', tone: 'primary small' })}</div>`;
    if (!connections.loaded()) return `<div class="page clusters-page">${head}${errorBox(connections.error(), 'clusters-reload')}${connections.error() ? '' : '<p class="inline-note" role="status">Loading clusters…</p>'}</div>`;
    if (!list.length) return `<div class="page clusters-page">${head}${empty('No clusters yet', 'Add a Slurm cluster through slurmrestd or a Kubernetes cluster through its API server, with a token from your account.', btn('Add cluster', 'cluster-add', { icon: 'plus', tone: 'primary small' }))}</div>`;
    return `<div class="page clusters-page">${head}${list.map(card).join('')}</div>`;
  }

  const actions = {
    'cluster-refresh': el => { const c = clusters().find(x => x.id === el.dataset.id); if (c) fetchStatus(c); },
    'clusters-reload': load,
  };
  function clear() { statuses.clear(); clearInterval(timer); timer = null; }
  return { page, load, changed, actions, clear };
}
