import test from 'node:test';
import assert from 'node:assert/strict';
import { insightsView, settingsForm, percent, tokens } from '../web/insights.js';

const report = { id: 'r1', status: 'ready', model: 'big', finishedAt: '2026-11-01T00:00:00Z', workstreams: [
  { name: 'Lab data analysis', description: 'Fitting assay data', private: false, sessions: 3, tokens: 9000, kinds: { research: 3 }, outputs: { analysis: 3 }, rows: [{ topic: 'Fitting a <b>curve</b>', kind: 'research', output: 'analysis', field: 'life_health', turns: 2, tokens: 3000 }] },
  { name: 'Personal and unlisted chats', description: 'Counted, never listed.', private: true, sessions: 1, tokens: 100000, kinds: { personal: 1 }, outputs: { answer: 1 }, rows: [] },
] };
const data = { settings: { enabled: true, model: 'judge' }, gateway: true, groups: [{ id: 'g1', name: 'Lab' }], sessions: 4, people: 6, tokens: 109000, kinds: [{ name: 'research', sessions: 3, tokens: 9000 }, { name: 'personal', sessions: 1, tokens: 100000 }], fields: [{ name: 'life_health', sessions: 3 }], activities: [], outputs: [], flags: { secrets: 1 }, report };

test('insights show labels, workstreams and flags, escaping topics', () => {
  const html = insightsView(data, { open: new Set(['Lab data analysis']), group: 'g1' });
  assert.match(html, /75% research and learning/);
  assert.match(html, /Lab data analysis/);
  assert.match(html, /Fitting a &lt;b&gt;curve&lt;\/b&gt;/);
  assert.doesNotMatch(html, /<b>curve/);
  assert.match(html, /Pasted keys or passwords<\/span><strong class="insights-flagged">1/);
  assert.match(html, /<option value="g1" selected>Lab<\/option>/);
  // Sorted by tokens, the unlisted chats come first; they are never listed.
  assert.ok(html.indexOf('Personal and unlisted chats') < html.indexOf('Lab data analysis'));
  const sorted = insightsView(data, { sort: 'sessions' });
  assert.ok(sorted.indexOf('Lab data analysis') < sorted.indexOf('Personal and unlisted chats'));
});

test('insights explain when they are off, too small or still building', () => {
  assert.match(insightsView({ ...data, settings: { enabled: false } }), /Usage insights are off/);
  assert.match(insightsView({ ...data, settings: { enabled: false }, gateway: false }), /no gateway connected/);
  assert.match(insightsView({ ...data, tooFew: true, people: 2, minPeople: 5 }), /2 people. A group shows from 5 people/);
  assert.match(insightsView({ ...data, report: { id: 'r', status: 'building', model: 'big', done: 20, total: 80 } }), /20 of 80 topics placed/);
  assert.match(insightsView({ ...data, report: null }), /Find workstreams/);
  assert.match(insightsView({ ...data, sessions: 0 }), /No labelled chats yet/);
});

test('insight numbers read plainly', () => {
  assert.equal(percent(0.004), '<1%');
  assert.equal(percent(0), '0%');
  assert.equal(tokens(1_250_000), '1.3M');
  assert.equal(tokens(48_000), '48K');
  assert.equal(tokens(950), '950');
  assert.match(settingsForm({ enabled: true, model: 'judge' }, ['judge', 'big']), /id="insights-enabled" name="insights-enabled" checked/);
});
