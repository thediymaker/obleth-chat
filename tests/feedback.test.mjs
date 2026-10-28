import test from 'node:test';
import assert from 'node:assert/strict';
import { REASONS, ratable, nextRating, feedbackButtons, feedbackForm, feedbackDetails } from '../web/feedback.js';
import { createAdmin } from '../web/admin.js';

const reply = (extra = {}) => ({ id: 'm1', role: 'assistant', status: 'completed', content: 'Hi', ...extra });

test('only finished replies in your own chat can be rated', () => {
  assert.equal(ratable(reply(), false), true);
  assert.equal(ratable(reply({ status: 'interrupted' }), false), true);
  assert.equal(ratable(reply(), { ownerName: 'Ada' }), false);
  assert.equal(ratable(reply({ status: 'pending' }), false), false);
  assert.equal(ratable(reply({ status: 'error' }), false), false);
  assert.equal(ratable(reply({ id: undefined }), false), false);
  assert.equal(ratable(reply({ role: 'user' }), false), false);
  assert.equal(feedbackButtons(reply(), 1, { ownerName: 'Ada' }), '');
});

test('the chosen thumb shows pressed, and clicking it again clears it', () => {
  const html = feedbackButtons(reply({ feedback: { rating: -1, reasons: [], comment: null } }), 3, null);
  assert.match(html, /data-action="rate-up"[^>]*data-index="3" aria-pressed="false"/);
  assert.match(html, /data-action="rate-down"[^>]*data-index="3" aria-pressed="true"/);
  assert.equal(nextRating(-1, -1), 0);
  assert.equal(nextRating(-1, 1), 1);
  assert.equal(nextRating(0, -1), -1);
});

test('the thumbs-down form offers every reason and keeps what was chosen', () => {
  const html = feedbackForm(reply({ feedback: { rating: -1, reasons: ['too_long'], comment: 'Shorter <please>' } }), 2);
  assert.equal((html.match(/name="reason"/g) || []).length, REASONS.length);
  assert.match(html, /value="too_long" checked/);
  assert.doesNotMatch(html, /value="wrong" checked/);
  assert.match(html, /maxlength="2000"/);
  assert.match(html, /Shorter &lt;please&gt;/);
  for (const label of ['Wrong or made up', 'Didn’t follow instructions', 'Too long', 'Too short', 'Harmful or offensive', 'Other']) assert.match(html, new RegExp(label));
});

test('the form sends a thumbs down with its reasons and a trimmed comment', () => {
  const form = comment => ({
    querySelectorAll: () => [{ value: 'wrong' }, { value: 'other' }],
    querySelector: () => ({ value: comment }),
  });
  assert.deepEqual(feedbackDetails(form('  Made it up  ')), { rating: -1, reasons: ['wrong', 'other'], comment: 'Made it up' });
  assert.deepEqual(feedbackDetails(form('   ')), { rating: -1, reasons: ['wrong', 'other'] });
});

test('the Feedback tab shows models, reasons and comments for the chosen period', async () => {
  globalThis.location = { hash: '#settings/admin/feedback' };
  const asked = [];
  const admin = createAdmin({
    api: { request: async path => {
      asked.push(path);
      return { days: 30, models: [{ model: 'model-a', up: 3, down: 1, upShare: 0.75 }], reasons: [{ reason: 'too_long', count: 1 }], comments: [{ model: 'model-a', rating: -1, reasons: ['too_long'], comment: 'Too <wordy>', at: '2026-10-01T12:00:00Z' }] };
    } },
    state: { user: { role: 'admin' } }, capabilities: { features: {} }, render() {}, close() {}, toast() {}, modal() {},
  });
  await admin.load();
  assert.deepEqual(asked, ['/api/admin/feedback?days=30']);
  const view = admin.view();
  assert.match(view, /<strong>model-a<\/strong><\/td><td>3<\/td><td>1<\/td><td>75%<\/td>/);
  assert.match(view, /href="\/api\/admin\/feedback\/models.csv\?days=30" download/);
  assert.match(view, /Too long/);
  assert.match(view, /Too &lt;wordy&gt;/);
  await admin.actions['admin-feedback-days']({ dataset: { days: '90' } });
  assert.equal(asked.at(-1), '/api/admin/feedback?days=90');
  assert.match(admin.view(), /data-days="90" aria-pressed="true"/);
});
