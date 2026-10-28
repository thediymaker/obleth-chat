import test from 'node:test';
import assert from 'node:assert/strict';
import { createLoad, loadLabel } from '../web/load.js';

const answer = {
  available: true,
  models: {
    glm: { state: 'full', slots: 40, inFlight: 40, waiting: 12, you: { inFlight: 1, limit: 4 } },
    qwen: { state: 'free', slots: 16, inFlight: 3, waiting: 0 },
    gemma: { state: 'busy', slots: 10, inFlight: 9, waiting: 0 },
    cold: { state: 'offline', slots: 0, inFlight: 0, waiting: 0 },
  },
};
function setup(reply = answer, preferences = { favorites: ['glm', 'qwen'], defaultModel: 'gemma' }) {
  let calls = 0;
  const state = {
    preferences,
    models: [{ id: 'auto' }, { id: 'glm' }, { id: 'qwen' }, { id: 'gemma', metadata: { supportsVision: true } }, { id: 'cold' }],
  };
  const load = createLoad({ api: { request: async () => { calls++; return typeof reply === 'function' ? reply() : reply; } }, state, modelName: id => `${id}-name` });
  return { load, state, calls: () => calls };
}
const waiting = (model, ms = 4000) => [{ role: 'assistant', status: 'streaming', settings: { model } }, 2, { waitingSince: Date.now() - ms }];

test('only a busy, full or serverless model has its icon tinted and a label', async () => {
  const { load } = setup();
  await load.refresh();
  for (const id of ['qwen', 'unknown']) { assert.equal(load.tint(id), ''); assert.equal(load.label(id), ''); }
  assert.equal(load.tint('gemma'), 'load-busy');
  assert.equal(load.label('gemma'), 'Busy: 9 running on 10 slots');
  assert.equal(load.tint('glm'), 'load-full');
  assert.equal(load.label('glm'), 'Full: 40 running on 40 slots, 12 waiting');
  assert.equal(load.tint('cold'), 'load-offline');
  assert.equal(load.label('cold'), 'No servers running right now');
  assert.equal(loadLabel(undefined), '');
  // Servers being removed finish their requests while the slots count only the rest.
  assert.equal(loadLabel({ state: 'full', slots: 32, inFlight: 37, waiting: 0 }), 'Full: 37 running on 32 slots');
});

test('load is fetched at most once per window, and a gateway without it is left alone', async () => {
  const live = setup();
  await live.load.refresh(); await live.load.refresh(5000);
  assert.equal(live.calls(), 1);
  await live.load.refresh(0);
  assert.equal(live.calls(), 2);
  const none = setup({ available: false });
  await none.load.refresh(); await none.load.refresh(0);
  assert.equal(none.calls(), 1);
  assert.equal(none.load.offered(), false);
  assert.equal(none.load.notice(...waiting('glm')), '');
});

test('a waiting reply says what holds it and offers a model that is not full', async () => {
  const { load } = setup();
  await load.refresh();
  assert.equal(load.notice(...waiting('glm', 1000)), '', 'not before three seconds');
  const html = load.notice(...waiting('glm'));
  assert.match(html, /glm-name is full right now: 40 running on 40 slots, 12 waiting\. Your message is in line\./);
  // The first favorite is the full model itself, so the next one is offered.
  assert.match(html, /data-action="switch-waiting" data-index="2" data-value="qwen">Switch to qwen-name/);
  assert.match(load.notice(...waiting('cold')), /cold-name has no servers running right now/);
  assert.equal(load.notice(...waiting('qwen')), '', 'a free model holds nothing');
  assert.equal(load.notice(...waiting('auto')), '', 'Auto picks its own model');
  // An image needs a vision model: the default, here.
  assert.match(load.notice(waiting('glm')[0], 2, { waitingSince: Date.now() - 4000, needsVision: true }), /data-value="gemma"/);
});

test('the person’s own limit is named, and a reply the gateway gave up on can switch', async () => {
  const own = setup({ ...answer, models: { ...answer.models, glm: { ...answer.models.glm, you: { inFlight: 4, limit: 4 } } } });
  await own.load.refresh();
  const html = own.load.notice(...waiting('glm'));
  assert.match(html, /You have 4 of 4 requests running on glm-name, so this one starts when one of them finishes\./);
  assert.doesNotMatch(html, /switch-waiting/);
  const { load } = setup(answer, { favorites: [], defaultModel: 'glm' });
  await load.refresh();
  const failed = { role: 'assistant', status: 'error', errorMessage: 'The model stayed full, so the gateway stopped waiting. Retry, or pick another model.', settings: { model: 'glm' } };
  const notice = load.notice(failed, 5);
  assert.match(notice, /glm-name stayed full\./);
  assert.match(notice, /data-value="auto">Let Auto choose/);
  assert.doesNotMatch(notice, /role="status"/);
});
