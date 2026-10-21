import test from 'node:test';
import assert from 'node:assert/strict';
import { conversationBusy, mergeProgress, responseVersion, suggestionsPending, renderSignature } from '../web/conversation-state.js';

test('pending checkpoints require recovery while follow-up generation does not block chat', () => {
  assert.equal(conversationBusy({ activeGenerationId: 'g', messages: [] }), true);
  assert.equal(conversationBusy({ messages: [{ role: 'assistant', status: 'pending' }] }), true);
  const completed = { messages: [{ role: 'assistant', status: 'completed', followUpsStatus: 'pending' }] };
  assert.equal(conversationBusy(completed), false);
  assert.equal(suggestionsPending(completed), true);
});

test('a delayed pending checkpoint cannot replace a terminal response with blank content', () => {
  const live = { id: 'chat', messages: [{ id: 'answer', role: 'assistant', status: 'completed', content: 'Finished answer', reasoningContent: 'Actual thinking' }] };
  const old = { id: 'chat', messages: [{ id: 'answer', role: 'assistant', status: 'pending', content: '', reasoningContent: '' }] };
  assert.deepEqual(mergeProgress(old, live).messages, live.messages);
});

test('checkpoint merge preserves each streaming channel without importing another version', () => {
  const current = { id: 'chat', messages: [{ id: 'v1', role: 'assistant', status: 'streaming', content: 'Answer in progress', reasoningContent: 'First thought' }] };
  const saved = { id: 'chat', messages: [{ id: 'v1', role: 'assistant', status: 'pending', content: 'Answer', reasoningContent: 'First thought then another' }] };
  const result = mergeProgress(saved, current).messages[0];
  assert.equal(result.content, 'Answer in progress');
  assert.equal(result.reasoningContent, 'First thought then another');
  const different = { id: 'chat', messages: [{ id: 'v2', role: 'assistant', status: 'pending', content: '' }] };
  assert.deepEqual(mergeProgress(different, current), different);
});

test('inline version selection uses sibling IDs and stays within bounds', () => {
  const message = { id: 'v2', versionIndex: 2, versions: [{ id: 'v1' }, { id: 'v2' }, { id: 'v3' }] };
  assert.equal(responseVersion(message, -1), 'v1');
  assert.equal(responseVersion(message, 1), 'v3');
  assert.equal(responseVersion({ ...message, versionIndex: 1 }, -1), null);
  assert.equal(responseVersion({ ...message, versionIndex: 3 }, 1), null);
});

test('render signature ignores follow-up suggestions but tracks transcript changes', () => {
  const base = { id: 'c', title: 'Chat', messages: [{ id: 'u', role: 'user', content: 'Hi', status: 'completed' }, { id: 'a', role: 'assistant', content: 'Hello', status: 'completed', followUpsStatus: 'pending', followUps: [] }] };
  const suggested = { ...base, messages: [base.messages[0], { ...base.messages[1], followUpsStatus: 'completed', followUps: ['More?'] }] };
  assert.equal(renderSignature(suggested), renderSignature(base));
  assert.notEqual(renderSignature({ ...base, title: 'Renamed' }), renderSignature(base));
  assert.notEqual(renderSignature({ ...base, messages: [base.messages[0], { ...base.messages[1], content: 'Hello there' }] }), renderSignature(base));
  assert.notEqual(renderSignature({ ...base, activeGenerationId: 'g' }), renderSignature(base));
});
