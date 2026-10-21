import test from 'node:test';
import assert from 'node:assert/strict';
import { generationValues } from '../web/generation.js';

const model = { defaults: { systemPrompt: 'Workspace instructions', temperature: 0.4, topP: 0.8, maxTokens: 4096, reasoningEffort: 'high' }, allowedOverrides: ['systemPrompt', 'temperature', 'topP', 'maxTokens', 'reasoningEffort'] };
test('new accounts and appearance-only preferences inherit model defaults', () => {
  assert.deepEqual(generationValues(model), model.defaults);
  assert.deepEqual(generationValues(model, { density: 'compact', context: false }), model.defaults);
});
test('explicit personal values override defaults and conversation values take precedence', () => {
  assert.deepEqual(generationValues(model, { systemPrompt: '', temperature: 0 }, { maxTokens: 512 }), { ...model.defaults, systemPrompt: '', temperature: 0, maxTokens: 512 });
});
test('model locks take precedence over both user and conversation overrides', () => {
  assert.equal(generationValues({ ...model, lockedParameters: ['temperature'] }, { temperature: 1 }, { temperature: 2 }).temperature, 0.4);
  assert.equal(generationValues({ ...model, allowedOverrides: ['temperature'] }, { maxTokens: 512 }, { maxTokens: 1024 }).maxTokens, 4096);
});
