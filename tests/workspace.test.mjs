import test from 'node:test';
import assert from 'node:assert/strict';
import { previewReload } from '../web/workspace.js';

test('reload reloads a running app and otherwise wakes the workspace', () => {
  assert.deepEqual(previewReload({ status: 'running', previewPort: 3000 }), { label: 'Reload preview', wake: false, disabled: false });
  assert.deepEqual(previewReload({ status: 'stopped', previewPort: null }), { label: 'Wake the workspace', wake: true, disabled: false });
  assert.deepEqual(previewReload({ status: 'running', previewPort: null }), { label: 'Start the preview', wake: true, disabled: false });
  assert.equal(previewReload({ status: 'error' }).label, 'Start the workspace again');
});

test('reload waits while the workspace is already waking', () => {
  assert.equal(previewReload({ status: 'starting' }).disabled, true);
  assert.equal(previewReload({ status: 'stopped' }, true).disabled, true);
});
