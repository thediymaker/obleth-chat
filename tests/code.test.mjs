import test from 'node:test';
import assert from 'node:assert/strict';
import { diffRows, conflictRegions } from '../web/code.js';

test('a diff numbers its lines by the new file', () => {
  const rows = diffRows('diff --git a/p.py b/p.py\nindex 1..2 100644\n--- a/p.py\n+++ b/p.py\n@@ -1,3 +1,3 @@ def f():\n a\n-b\n+c\n d\n');
  assert.deepEqual(rows.map(r => [r.kind, r.n, r.text]), [['hunk', undefined, 'line 1 · def f():'], ['ctx', 1, 'a'], ['del', '', 'b'], ['add', 2, 'c'], ['ctx', 3, 'd']]);
});

test('a conflict splits into each side, up to a limit', () => {
  const text = 'x\n<<<<<<< HEAD\nmine\n=======\ntheirs\nmore\n>>>>>>> project\ny\n<<<<<<< HEAD\n||||||| base\nold\n=======\n>>>>>>> project\n';
  assert.deepEqual(conflictRegions(text), [{ line: 2, mine: ['mine'], theirs: ['theirs', 'more'] }, { line: 9, mine: [], theirs: [] }]);
  assert.equal(conflictRegions(text, 1).length, 1);
});
