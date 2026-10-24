import test from 'node:test';
import assert from 'node:assert/strict';
import { workOf, stats, gist, latest, duration, changeOf } from '../web/turn.js';

const t = s => new Date(Date.UTC(2026, 9, 19, 12, 0, s)).toISOString();
const reasoning = 'Plan seven files. Then write them.Nothing vendored, so load three.js from a CDN.The script was deferred, so ours ran first.';
const a = 'Plan seven files. Then write them.'.length, b = a + 'Nothing vendored, so load three.js from a CDN.'.length;
const message = {
  role: 'assistant', status: 'completed', reasoningContent: reasoning,
  activity: [
    { id: 'thought-1', kind: 'thought', from: 0, to: a, durationMs: 12000 },
    { id: 'c1', tool: 'list_files', summary: 'Listed .', status: 'completed', startedAt: t(0), durationMs: 100 },
    { id: 'c2', tool: 'list_files', summary: 'Listed vendor', status: 'error', detail: "can't cd to vendor", startedAt: t(1), durationMs: 100 },
    { id: 'c3', tool: 'run_command', summary: 'Ran find', status: 'error', startedAt: t(2), durationMs: 100 },
    { id: 'thought-2', kind: 'thought', from: a, to: b, durationMs: 4000 },
    { id: 'c4', tool: 'write_file', summary: 'Wrote index.html', path: 'index.html', added: 38, status: 'completed', check: 'error', startedAt: t(10), durationMs: 200 },
    { id: 'thought-3', kind: 'thought', from: b, to: reasoning.length, durationMs: 9000 },
    { id: 'c5', tool: 'edit_file', summary: 'Edited index.html', path: 'index.html', added: 1, removed: 1, status: 'completed', check: 'ok', startedAt: t(20), durationMs: 200 },
    { id: 'checkpoint-ab12', kind: 'checkpoint', sha: 'ab12cd34ef56', summary: 'Checkpoint 1', stat: '1 file changed', status: 'completed' },
    { id: 'note-1', kind: 'note', summary: 'make the mane darker', status: 'completed' },
  ],
};

test('thoughts sit before the step they led to, in phases', () => {
  const work = workOf(message);
  assert.deepEqual(work.rows.map(r => [r.type, r.phase]), [
    ['thought', 'Explore'], ['step', 'Explore'], ['deadend', 'Explore'], ['thought', 'Build'], ['problem', 'Build'], ['checkpoint', 'Build'], ['note', 'Build'],
  ]);
  assert.equal(work.rows[0].text, 'Plan seven files. Then write them.');
});

test('failed exploration folds into a dead end, and a broken preview with its fix into a problem', () => {
  const work = workOf(message);
  assert.equal(work.rows[2].tries.length, 2);
  const problem = work.rows[4];
  assert.equal(problem.start.e.id, 'c4');
  assert.equal(problem.fix.e.id, 'c5');
  assert.equal(problem.middle[0].text, 'The script was deferred, so ours ran first.');
});

test('stats add up files, lines, thinking time and checks', () => {
  const s = stats(workOf(message));
  assert.deepEqual(s.files, [{ path: 'index.html', added: 39, removed: 1 }]);
  assert.equal(s.thoughtMs, 25000);
  assert.equal(s.check, 'ok');
  assert.deepEqual([s.steps, s.deadEnds, s.fixes, s.checkpoints.length], [5, 1, 1, 1]);
  assert.equal(s.span, 20200);
});

test('an older reply shows its one block of reasoning as the first thought', () => {
  const work = workOf({ role: 'assistant', reasoningContent: 'Look first.', activity: [{ id: 'x', tool: 'read_file', summary: 'Read a', status: 'completed' }] });
  assert.deepEqual(work.rows.map(r => r.type), ['thought', 'step']);
});

test('gists and latest thoughts are short and plain', () => {
  assert.equal(gist('**Stance** must not slip. The hip follows an arcsine.'), 'Stance must not slip.');
  assert.ok(gist('x'.repeat(400)).length <= 160);
  assert.equal(latest('One. Two. Three.', 9), 'Three.');
  assert.deepEqual([duration(400), duration(12000), duration(100000)], ['0.4s', '12s', '1m 40s']);
});

test('cluster work goes through its own stages, not the cluster\'s name', () => {
  const k8s = { service: 'kubernetes', connection: 'Lab cluster' };
  const steps = [
    { id: 'k1', tool: 'lab__list', action: 'list', readOnly: true, status: 'completed', ...k8s },
    { id: 'k2', tool: 'lab__apply', action: 'apply', readOnly: false, status: 'awaiting', ...k8s },
  ];
  assert.deepEqual(workOf({ role: 'assistant', activity: steps }).rows.map(r => r.phase), ['Inspect', 'Approve']);
  steps[1].status = 'completed';
  steps.push({ id: 'k3', tool: 'lab__get', action: 'get', readOnly: true, status: 'completed', ...k8s });
  assert.deepEqual(workOf({ role: 'assistant', activity: steps }).rows.map(r => r.phase), ['Inspect', 'Apply', 'Verify']);
  const slurm = { service: 'slurm', connection: 'Campus HPC' };
  const job = [
    { id: 's1', action: 'cluster', readOnly: true, status: 'completed', ...slurm },
    { id: 's2', action: 'submit', readOnly: false, status: 'completed', ...slurm },
    { id: 's3', action: 'job', readOnly: true, status: 'completed', ...slurm },
  ];
  assert.deepEqual(workOf({ role: 'assistant', activity: job }).rows.map(r => r.phase), ['Inspect', 'Submit', 'Watch']);
});

test('a change waiting for approval says what it does', () => {
  const yaml = 'manifest:\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: hello\n  namespace: default\nspec:\n  replicas: 1\n  template:\n    spec:\n      containers:\n      - name: web\n        image: nginx:1.27\n        ports:\n        - containerPort: 80\n---\nkind: Service\nmetadata:\n  name: hello\nspec:\n  ports:\n  - port: 80';
  const applied = changeOf({ service: 'kubernetes', action: 'apply', detail: yaml });
  assert.deepEqual(applied.items.map(i => [i.what, i.where]), [['Deployment/hello', 'default'], ['Service/hello', undefined]]);
  assert.deepEqual(applied.items[0].facts, [['Image', 'nginx:1.27'], ['Replicas', '1'], ['Port', '80']]);
  assert.ok(applied.text.startsWith('apiVersion'));
  const json = changeOf({ service: 'kubernetes', action: 'apply', detail: 'manifest: {"kind":"ConfigMap","metadata":{"name":"cfg","namespace":"lab"}}' });
  assert.deepEqual(json.items.map(i => [i.what, i.where]), [['ConfigMap/cfg', 'lab']]);
  const job = changeOf({ service: 'slurm', action: 'submit', detail: 'cpus: 2\nmemory_mb: 4096\nname: heat-bench\npartition: long\nscript:\n#!/bin/bash\nsrun ./heat\ntime_limit_minutes: 90' });
  assert.equal(job.items[0].what, 'heat-bench');
  assert.deepEqual(job.items[0].facts, [['Partition', 'long'], ['Time limit', '1 h 30 min'], ['CPUs', '2'], ['Memory', '4 GB']]);
  assert.equal(job.text, '#!/bin/bash\nsrun ./heat');
  assert.equal(changeOf({ service: 'mcp', action: 'send', detail: 'to: x' }), null);
});
