// Usage insights for administrators: what kind of work chats were, from the labels Chat
// keeps once a chat has gone quiet. Nothing here shows a chat, a message or a person.
import { esc, icon, btn, empty, errorBox, field, checkbox, textInput, when, titleCase } from './manage-ui.js';

export const PERIODS = [7, 28, 90];
export const KINDS = [['research', 'Research', 'green'], ['learning', 'Learning', 'blue'], ['work', 'Work', 'violet'], ['personal', 'Personal', 'gold'], ['play', 'Play', 'rose'], ['trying_it_out', 'Trying it out', 'gray']];
const NAMES = {
  activity: { coding: 'Coding', data_analysis: 'Data analysis', writing: 'Writing', research: 'Finding information', explaining: 'Explaining a topic', computing_help: 'Computing help', planning_admin: 'Planning and admin', creative: 'Creative work', other: 'Other' },
  field: { computing: 'Computing', engineering: 'Engineering', physical_sciences: 'Physical sciences and math', life_health: 'Life and health sciences', social_sciences: 'Social sciences', business: 'Business', humanities_arts: 'Humanities and arts', education: 'Education', law_policy: 'Law and policy', general: 'No particular field' },
  output: { code: 'Code', document: 'Documents', analysis: 'Analysis', answer: 'Answers', creative: 'Creative pieces', plan: 'Plans and lists', none: 'Nothing in particular' },
  kind: Object.fromEntries(KINDS.map(([id, name]) => [id, name])),
};
export const FLAGS = [['seeks_harm', 'Harmful or illegal requests'], ['jailbreak', 'Attempts to get around a model’s rules'], ['explicit', 'Sexual or explicit content'], ['harassment_hate', 'Harassment or hate'], ['integrity', 'Graded coursework done for them'], ['secrets', 'Pasted keys or passwords'], ['self_harm', 'Mentions of self-harm']];

export const nameOf = (group, id) => NAMES[group]?.[id] || titleCase(id || 'unknown');
const count = n => Number(n || 0).toLocaleString();
export const tokens = n => { n = Number(n || 0); return n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e4 ? `${Math.round(n / 1e3)}K` : n.toLocaleString(); };
const share = (n, total) => total ? n / total : 0;
export const percent = x => x > 0 && x < 0.005 ? '<1%' : `${Math.round(x * 100)}%`;

function toolbar(data, { days, group }) {
  const periods = PERIODS.map(d => `<button type="button" class="filter-chip ${d === days ? 'active' : ''}" data-action="admin-insights-days" data-days="${d}" aria-pressed="${d === days}">${d} days</button>`).join('');
  const groups = data?.groups?.length ? `<label class="sr-only" for="admin-insights-group">Group</label><select class="form-control insights-group" id="admin-insights-group"><option value="">Everyone</option>${data.groups.map(g => `<option value="${esc(g.id)}" ${g.id === group ? 'selected' : ''}>${esc(g.name)}</option>`).join('')}</select>` : '';
  return `<div class="management-toolbar"><div class="row" role="group" aria-label="Period">${periods}${groups}</div>${btn('Settings', 'admin-insights-settings', { icon: 'settings', tone: 'small' })}</div>`;
}

function stack(kinds, total) {
  const by = Object.fromEntries((kinds || []).map(k => [k.name, k.sessions]));
  const present = KINDS.filter(([id]) => by[id]);
  const studied = share((by.research || 0) + (by.learning || 0), total);
  const segments = present.map(([id, name, color]) => `<span data-color="${color}" style="width:${(100 * share(by[id], total)).toFixed(2)}%" title="${esc(name)} ${count(by[id])}"></span>`).join('');
  const legend = present.map(([id, name, color]) => `<li><i class="insights-swatch" data-color="${color}"></i>${esc(name)} <strong>${percent(share(by[id], total))}</strong> <small>${count(by[id])}</small></li>`).join('');
  return `<section class="card insights-card"><div class="spread"><h2>Kind of use</h2><span class="inline-note">${percent(studied)} research and learning</span></div><div class="insights-stack" role="img" aria-label="${esc(present.map(([id, name]) => `${name} ${percent(share(by[id], total))}`).join(', '))}">${segments}</div><ul class="insights-legend">${legend}</ul></section>`;
}

function counts(title, group, list, total) {
  const max = Math.max(1, ...(list || []).map(x => x.sessions));
  const rows = (list || []).map(x => `<li><span>${esc(nameOf(group, x.name))}</span><span class="insights-meter"><span style="width:${(100 * x.sessions / max).toFixed(1)}%"></span></span><strong>${percent(share(x.sessions, total))}</strong></li>`).join('');
  return `<section class="card insights-card"><h2>${esc(title)}</h2>${rows ? `<ul class="insights-counts">${rows}</ul>` : '<p class="inline-note">Nothing yet.</p>'}</section>`;
}

function flags(data) {
  const f = data.flags || {};
  const rows = FLAGS.map(([id, name]) => `<li><span>${esc(name)}</span><strong class="${f[id] ? 'insights-flagged' : ''}">${count(f[id])}</strong></li>`).join('');
  return `<section class="card insights-card"><h2>Flags</h2><p class="inline-note">Counted only. Flagged chats are not saved or shown.</p><ul class="feedback-counts">${rows}</ul></section>`;
}

function bars(stream, max, sessions, total) {
  const s = share(stream.sessions, sessions), t = share(stream.tokens, total);
  return `<span class="insights-bars"><span class="insights-bar" data-color="blue"><span style="width:${(100 * s / max).toFixed(2)}%"></span></span><span class="insights-value"><strong>${percent(s)}</strong> ${count(stream.sessions)} ${stream.sessions === 1 ? 'chat' : 'chats'}</span><span class="insights-bar" data-color="gold"><span style="width:${(100 * t / max).toFixed(2)}%"></span></span><span class="insights-value"><strong>${percent(t)}</strong> ${tokens(stream.tokens)} tokens</span></span>`;
}

const SHOWN = 15;
function detail(stream, all) {
  const chips = [...Object.entries(stream.kinds || {}).map(([k, n]) => `${nameOf('kind', k)} ${count(n)}`), ...Object.entries(stream.outputs || {}).map(([k, n]) => `${nameOf('output', k)} ${count(n)}`)].map(c => `<span class="pill">${esc(c)}</span>`).join('');
  if (stream.private) return `<div class="insights-detail"><div class="insights-chips">${chips}</div><p class="inline-note">These chats are counted but never listed: they were about the person’s own life, flagged explicit or self-harm, or have no topic line.</p></div>`;
  const listed = stream.rows || [], shown = all ? listed : listed.slice(0, SHOWN);
  const rows = shown.map(r => `<tr><td class="insights-topic">${esc(r.topic)}</td><td>${esc(nameOf('kind', r.kind))}</td><td>${esc(nameOf('output', r.output))}</td><td>${esc(nameOf('field', r.field))}</td><td>${count(r.turns)}</td><td>${tokens(r.tokens)}</td></tr>`).join('');
  const more = shown.length < listed.length ? `<div>${btn(`Show all ${count(listed.length)}`, 'admin-insights-all', { tone: 'small', attrs: `data-name="${esc(stream.name)}"` })}</div>` : stream.sessions > listed.length ? `<p class="inline-note">Showing the ${count(listed.length)} chats with the most tokens.</p>` : '';
  return `<div class="insights-detail"><div class="insights-chips">${chips}</div><div class="table-wrap"><table class="management-table"><thead><tr><th>Topic</th><th>Use</th><th>Wanted</th><th>Field</th><th>Messages</th><th>Tokens</th></tr></thead><tbody>${rows}</tbody></table></div>${more}</div>`;
}

function workstreams(data, { open, all, sort }) {
  const report = data.report;
  const head = action => `<div class="spread insights-streams-head"><div><h2>Workstreams</h2><p class="inline-note">What people use Chat for most, by chats and by tokens. Open one to see its topics.</p></div>${action}</div>`;
  const find = label => btn(label, 'admin-insights-report', { icon: 'refresh', tone: 'small' });
  if (!report) return `<section class="card insights-card">${head(find('Find workstreams'))}<p class="inline-note">A model groups this period’s topic lines into workstreams. It takes a few minutes.</p></section>`;
  if (report.status === 'building') {
    const progress = report.total ? ` ${count(report.done)} of ${count(report.total)} topics placed.` : '';
    return `<section class="card insights-card" aria-busy="true">${head('')}<p class="inline-note insights-building" role="status">Finding workstreams with ${esc(report.model || 'the report model')}…${progress}</p></section>`;
  }
  if (report.status === 'failed') return `<section class="card insights-card">${head(find('Try again'))}${errorBox(report.error || 'The report failed.')}</section>`;
  const streams = [...(report.workstreams || [])].sort((a, b) => (b[sort] - a[sort]) || (b.sessions - a.sessions));
  const sessions = streams.reduce((n, w) => n + w.sessions, 0), total = streams.reduce((n, w) => n + w.tokens, 0);
  const max = Math.max(0.0001, ...streams.flatMap(w => [share(w.sessions, sessions), share(w.tokens, total)]));
  const sorts = [['tokens', 'By tokens'], ['sessions', 'By chats']].map(([id, name]) => `<button type="button" class="filter-chip ${id === sort ? 'active' : ''}" data-action="admin-insights-sort" data-sort="${id}" aria-pressed="${id === sort}">${name}</button>`).join('');
  const list = streams.map(w => { const shown = open.has(w.name); return `<div class="insights-stream"><button type="button" class="insights-stream-head" data-action="admin-insights-stream" data-name="${esc(w.name)}" aria-expanded="${shown}"><span class="insights-stream-name"><strong>${esc(w.name)}</strong><small>${esc(w.description)}</small></span>${bars(w, max, sessions, total)}</button>${shown ? detail(w, all.has(w.name)) : ''}</div>`; }).join('');
  return `<section class="card insights-card">${head(find('Find again'))}<div class="spread insights-streams-tools"><div class="row" role="group" aria-label="Sort workstreams">${sorts}</div><ul class="insights-legend"><li><i class="insights-swatch" data-color="blue"></i>Chats</li><li><i class="insights-swatch" data-color="gold"></i>Tokens</li></ul></div><div class="insights-streams">${list}</div><p class="inline-note">Found by ${esc(report.model || 'a model')} on ${esc(when(report.finishedAt))}.</p></section>`;
}

export function insightsView(data, { days = 28, group = '', open = new Set(), all = new Set(), sort = 'tokens' } = {}) {
  const bar = toolbar(data, { days, group });
  if (!data) return bar;
  if (!data.settings?.enabled) {
    const text = data.gateway ? 'Turn them on to see what kind of work people do in Chat: research, learning, work or play, in which fields, and the workstreams. Chat keeps labels and a one-line topic for each chat, never the messages.' : 'Insights ask models through the gateway, and Chat has no gateway connected.';
    return bar + empty('Usage insights are off', text, data.gateway ? btn('Turn on', 'admin-insights-settings', { tone: 'primary' }) : '');
  }
  const note = `<p class="inline-note management-note">From labels Chat keeps once a chat has been quiet for 30 minutes${data.lastLabeledAt ? `, most recently ${esc(when(data.lastLabeledAt))}` : ''}. Temporary chats are never read, and no chat, message or person is shown here.</p>`;
  if (data.tooFew) return bar + note + empty('Too few people to show', `In the last ${days} days this group has labelled chats from ${count(data.people)} ${data.people === 1 ? 'person' : 'people'}. A group shows from ${data.minPeople} people, so nobody can be singled out.`);
  if (!data.sessions) return bar + note + empty('No labelled chats yet', `Chats are labelled once they have been quiet for 30 minutes. None from the last ${days} days are labelled yet.`);
  const stats = `<div class="personal-usage-stats insights-stats">${[['Chats', count(data.sessions)], ['People', count(data.people)], ['Tokens', tokens(data.tokens)]].map(([name, value]) => `<div><strong>${value}</strong><span>${name}</span></div>`).join('')}</div>`;
  return `${bar}${note}${stats}${stack(data.kinds, data.sessions)}${workstreams(data, { open, all, sort })}<div class="feedback-grid">${counts('Fields', 'field', data.fields, data.sessions)}${counts('Activities', 'activity', data.activities, data.sessions)}${counts('What people wanted', 'output', data.outputs, data.sessions)}${flags(data)}</div>`;
}

export function settingsForm(settings, models = []) {
  const list = `<datalist id="insights-models">${models.map(id => `<option value="${esc(id)}"></option>`).join('')}</datalist>`;
  return `<form id="admin-insights-form">${checkbox('insights-enabled', 'Label chats', !!settings?.enabled, 'Once a chat has been quiet for 30 minutes, ask a model what kind of work it was. Chat keeps the labels and a one-line topic, never the messages, and never reads temporary chats.')}${field('insights-model', 'Labelling model', textInput('insights-model', settings?.model || '', 'maxlength="160" list="insights-models" spellcheck="false" autocomplete="off" placeholder="gemma4-31b-it"'), 'Answers the questions about each chat and names its topic. It must return token probabilities (logprobs). Chat asks as each chat’s owner, so the gateway counts the tokens as theirs.')}${field('insights-report-model', 'Workstreams model', textInput('insights-report-model', settings?.reportModel || '', 'maxlength="160" list="insights-models" spellcheck="false" autocomplete="off" placeholder="The labelling model"'), 'Groups topic lines into workstreams when you ask for them. A larger model names them better.')}${list}<div class="form-error" role="alert"></div><div class="modal-actions"><button class="btn primary" type="submit">Save</button></div></form>`;
}
