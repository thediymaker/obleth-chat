import test from 'node:test';
import assert from 'node:assert/strict';
import { temporaryToggle, temporaryNote, chatMenuItems, leftTemporary } from '../web/temporary.js';
import { ApiClient } from '../web/client.js';

test('the new-chat toggle says whether the next chat is temporary', () => {
  assert.match(temporaryToggle(false), /data-action="toggle-temporary" aria-pressed="false"/);
  assert.match(temporaryToggle(true), /class="temporary-toggle active" data-action="toggle-temporary" aria-pressed="true"/);
  assert.match(temporaryNote(), /deleted when you leave it/);
  assert.match(temporaryNote(false), /won’t be in your chats/);
});

test('a temporary chat can only be renamed or deleted from its menu', () => {
  const actions = c => chatMenuItems(c).map(([, , action]) => action);
  assert.deepEqual(actions({ temporary: true }), ['rename-chat', 'delete-chat']);
  assert.deepEqual(actions({ temporary: false, pinned: true }), ['rename-chat', 'pin-chat', 'archive-chat', 'share-chat', 'export-chat', 'delete-chat']);
  assert.equal(chatMenuItems({ pinned: true })[1][1], 'Unpin');
});

test('opening anything other than the temporary chat leaves it', () => {
  assert.equal(leftTemporary(null, { page: 'history', id: '' }), null);
  assert.equal(leftTemporary('t1', { page: 'chat', id: 't1' }), null);
  assert.equal(leftTemporary('t1', { page: 'chat', id: 'other' }), 't1');
  assert.equal(leftTemporary('t1', { page: 'chat', id: '' }), 't1');
  assert.equal(leftTemporary('t1', { page: 'settings', id: '' }), 't1');
});

test('leaving sends a request that outlives the page and never throws', async () => {
  let captured;
  const client = new ApiClient({ fetcher: async (path, options) => { captured = { path, options }; throw new TypeError('offline'); } });
  client.csrfToken = 'csrf-test';
  await client.send('/api/conversations/t1/leave');
  assert.equal(captured.path, '/api/conversations/t1/leave');
  assert.equal(captured.options.method, 'POST');
  assert.equal(captured.options.keepalive, true);
  assert.equal(captured.options.credentials, 'same-origin');
  assert.equal(captured.options.headers['X-CSRF-Token'], 'csrf-test');
  assert.throws(() => client.send('https://elsewhere.test/api/x'), /this origin/);
});
