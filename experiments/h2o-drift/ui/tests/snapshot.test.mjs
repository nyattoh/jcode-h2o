// Snapshot import gate: JS schema must match the real Rust serde types (src/monitor.rs, src/ui.rs).
// The same case table is run against the already-built h2o-ui launcher when it exists (parity).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const C = require('../core.js');
const here = dirname(fileURLToPath(import.meta.url));
const uiDir = resolve(here, '..');

const goal = () => ({ purpose: 'Verify', constraints: [], success_conditions: ['ok'], owned_paths: [], unfinished: [] });
const spec = (id) => ({ run_id: 'run-1', branch_id: id, owner_session_id: 'root', session_id: 'worker-a', goal: goal(), context_ids: [], max_corrections: 1 });
const checkpoint = (sequence) => ({
  sequence,
  input: { goal: goal(), observation: { call_id: 'c', tool: 'bash', output: 'PRIVATE-OUTPUT-TEXT', error: 'PRIVATE-ERROR-TEXT' }, context: [{ id: 's', bytes: [7, 7, 7] }] },
  assessment: { signal: 'Aligned', reason: 'fine' },
  decision: 'Continue',
  action: 'Continue',
});
const branch = (id, seqs, receipts, reconcile) => ({ spec: spec(id), checkpoints: seqs.map(checkpoint), receipts, needs_reconciliation: reconcile });
const env = (branches) => ({ schema: 'h2o-snapshot/1', captured_at: '2026-11-12 manual', branches });
const J = (o) => JSON.stringify(o);
const one = (mutate) => { const b = branch('a', [1], [{ sequence: 1, receipt: { acknowledged: true } }], false); mutate(b); return J(env([b])); };
const cp = (mutate) => one((b) => mutate(b.checkpoints[0]));
const nest = (n) => one((b) => { b.spec.goal.extra = 'NEST'; }).replace('"NEST"', '['.repeat(n) + ']'.repeat(n));
const pad = (s, total) => s + ' '.repeat(total - Buffer.byteLength(s));
const many = (n) => Array.from({ length: n }, (_, i) => i);

const VALID = J(env([
  branch('a', [1, 2], [{ sequence: 1, receipt: { acknowledged: true } }, { sequence: 2, receipt: null }], null),
  branch('b', [], [], true),
  branch('c', [7], [{ sequence: 7, receipt: { acknowledged: false } }], false),
]));

// [name, input (string | Uint8Array), accepted]
const CASES = [
  ['valid full envelope', VALID, true],
  ['captured_at omitted', '{"schema":"h2o-snapshot/1","branches":[]}', true],
  ['captured_at null', '{"schema":"h2o-snapshot/1","captured_at":null,"branches":[]}', false],
  ['captured_at number', '{"schema":"h2o-snapshot/1","captured_at":1,"branches":[]}', false],
  ['wrong schema', J({ ...env([]), schema: 'h2o-snapshot/2' }), false],
  ['schema number', '{"schema":1,"branches":[]}', false],
  ['branches missing', '{"schema":"h2o-snapshot/1"}', false],
  ['unknown top-level key', J({ ...env([]), extra: 1 }), false],
  ['unknown branch key', one((b) => { b.surprise = 1; }), false],
  ['unknown receipt-entry key', one((b) => { b.receipts[0].note = 'x'; }), false],
  ['unknown nested goal key is ignored like serde', one((b) => { b.spec.goal.extra = { any: [1, 2] }; }), true],
  ['unknown checkpoint key is ignored like serde', cp((c) => { c.extra = true; }), true],
  ['spec missing fields', J(env([{ spec: { run_id: 'x' }, checkpoints: [], receipts: [], needs_reconciliation: null }])), false],
  ['goal missing unfinished', one((b) => { delete b.spec.goal.unfinished; }), false],
  ['constraints not strings', one((b) => { b.spec.goal.constraints = [1]; }), false],
  ['needs_reconciliation missing', one((b) => { delete b.needs_reconciliation; }), false],
  ['needs_reconciliation string', one((b) => { b.needs_reconciliation = 'maybe'; }), false],
  ['receipt key missing', one((b) => { delete b.receipts[0].receipt; }), false],
  ['acknowledged string', one((b) => { b.receipts[0].receipt = { acknowledged: 'yes' }; }), false],
  ['receipt seq form [true]', one((b) => { b.receipts[0].receipt = [true]; }), true],
  ['duplicate checkpoint sequence', J(env([branch('a', [1, 2, 1], [], false)])), false],
  ['receipt for unlisted sequence', J(env([branch('a', [1], [{ sequence: 9, receipt: { acknowledged: true } }], false)])), false],
  ['repeated receipt for listed sequence', J(env([branch('a', [1], [{ sequence: 1, receipt: null }, { sequence: 1, receipt: { acknowledged: true } }], false)])), true],
  ['observation.error omitted', cp((c) => { delete c.input.observation.error; }), true],
  ['observation.error null', cp((c) => { c.input.observation.error = null; }), true],
  ['observation.error number', cp((c) => { c.input.observation.error = 3; }), false],
  ['observation seq form', cp((c) => { c.input.observation = ['c', 'bash', 'out', null]; }), true],
  ['observation seq form too short', cp((c) => { c.input.observation = ['c', 'bash', 'out']; }), false],
  ['observation seq form too long', cp((c) => { c.input.observation = ['c', 'bash', 'out', null, 1]; }), false],
  ['goal seq form', one((b) => { b.spec.goal = ['p', [], [], [], []]; }), true],
  ['signal unknown variant', cp((c) => { c.assessment.signal = 'Maybe'; }), false],
  ['signal map form with null', cp((c) => { c.assessment.signal = { Misaligned: null }; }), true],
  ['signal map form with value', cp((c) => { c.assessment.signal = { Aligned: 1 }; }), false],
  ['signal map with two keys', cp((c) => { c.assessment.signal = { Aligned: null, Unknown: null }; }), false],
  ['decision Correct', cp((c) => { c.decision = 'Correct'; }), true],
  ['decision invalid', cp((c) => { c.decision = 'Correcting'; }), false],
  ['action Replan struct', cp((c) => { c.action = { Replan: { message: 'refocus' } }; }), true],
  ['action Replan bare string', cp((c) => { c.action = 'Replan'; }), false],
  ['action Replan seq form (serde Value rejects arrays for struct variants)', cp((c) => { c.action = { Replan: ['refocus'] }; }), false],
  ['action Replan missing message', cp((c) => { c.action = { Replan: {} }; }), false],
  ['array envelope', '["h2o-snapshot/1",null,[]]', false],
  ['top-level string', '"h2o-snapshot/1"', false],
  ['array branch', J(env([[spec('a'), [], [], false]])), false],
  ['array spec', one((b) => { b.spec = Object.values(b.spec); }), false],
  ['array checkpoint item', one((b) => { b.checkpoints[0] = Object.values(b.checkpoints[0]); }), false],
  ['array receipt entry', one((b) => { b.receipts = [[1, null]]; }), false],
  ['sequence string', cp((c) => { c.sequence = '1'; }), false],
  ['sequence 1.0 (float source)', one(() => {}).replace('"sequence":1,"input"', '"sequence":1.0,"input"'), false],
  ['sequence 1e0 (float source)', one(() => {}).replace('"sequence":1,"input"', '"sequence":1e0,"input"'), false],
  ['sequence -0', one(() => {}).replace('"sequence":1,"input"', '"sequence":-0,"input"'), false],
  ['sequence negative', cp((c) => { c.sequence = -1; }), false],
  ['sequence u64::MAX', J(env([branch('a', [987654321], [], false)])).replace('987654321', '18446744073709551615'), true],
  ['sequence u64::MAX + 1', J(env([branch('a', [987654321], [], false)])).replace('987654321', '18446744073709551616'), false],
  ['max_corrections u32::MAX', one((b) => { b.spec.max_corrections = 4294967295; }), true],
  ['max_corrections u32::MAX + 1', one((b) => { b.spec.max_corrections = 4294967296; }), false],
  ['context byte 255', cp((c) => { c.input.context[0].bytes = [0, 255]; }), true],
  ['context byte 256', cp((c) => { c.input.context[0].bytes = [256]; }), false],
  ['context bytes as string', cp((c) => { c.input.context[0].bytes = 'AAEC'; }), false],
  ['50 branches', J(env(many(50).map((i) => branch('b' + i, [], [], null)))), true],
  ['51 branches', J(env(many(51).map((i) => branch('b' + i, [], [], null)))), false],
  ['1000 checkpoints', J(env([branch('a', many(1000), [], false)])), true],
  ['1001 checkpoints', J(env([branch('a', many(1001), [], false)])), false],
  ['exactly 1 MiB', pad(J(env([])), 1048576), true],
  ['1 MiB + 1 byte', pad(J(env([])), 1048577), false],
  ['invalid UTF-8 bytes', Uint8Array.from([...Buffer.from('{"schema":"h2o-snapshot/1","captured_at":"'), 0xff, ...Buffer.from('","branches":[]}')]), false],
  ['UTF-8 BOM', Uint8Array.from([0xef, 0xbb, 0xbf, ...Buffer.from('{"schema":"h2o-snapshot/1","branches":[]}')]), false],
  ['lone surrogate escape', '{"schema":"h2o-snapshot/1","captured_at":"\\ud800","branches":[]}', false],
  ['paired surrogate escape', '{"schema":"h2o-snapshot/1","captured_at":"\\ud83d\\ude00","branches":[]}', true],
  ['duplicate key, last wins', '{"schema":"wrong","schema":"h2o-snapshot/1","branches":[]}', true],
  ['nesting depth 127', nest(122), true],
  ['nesting depth 128', nest(123), false],
  ['trailing garbage', '{"schema":"h2o-snapshot/1","branches":[]}x', false],
  ['not JSON', 'not json', false],
  ['empty input', '', false],
];

const accepts = (input) => { try { C.normalizeSnapshot(input); return true; } catch (e) { return false; } };

test('JS snapshot gate accepts/rejects exactly as the Rust serde types do', () => {
  const wrong = CASES.filter(([, input, ok]) => accepts(input) !== ok).map(([name, , ok]) => `${name}: expected ${ok ? 'accept' : 'reject'}`);
  assert.deepEqual(wrong, []);
});

test('rejections carry a Japanese message', () => {
  for (const [name, input, ok] of CASES) {
    if (ok) continue;
    assert.throws(() => C.normalizeSnapshot(input), (e) => /スナップショット/.test(e.message), name);
  }
});

test('view model keeps unknown/uncertain states and is labelled read-only import', () => {
  const vm = C.normalizeSnapshot(VALID);
  assert.equal(vm.label, '取り込み済みスナップショット（読み取り専用）');
  assert.equal(vm.capturedAt, '2026-11-12 manual');
  assert.deepEqual(vm.branches.map((b) => b.needsReconciliation), ['unknown', 'required', 'not_required']);
  assert.deepEqual(vm.branches[0].receipts.map((r) => [r.sequence, r.state]), [['1', 'acknowledged'], ['2', 'unknown']]);
  assert.equal(vm.branches[2].receipts[0].state, 'not_acknowledged');
  assert.deepEqual(vm.branches[0].checkpoints.map((c) => c.receiptState), ['acknowledged', 'unknown']);
  assert.equal(C.normalizeSnapshot('{"schema":"h2o-snapshot/1","branches":[]}').capturedAt, null);
  const absent = C.normalizeSnapshot(J(env([branch('a', [3], [], false)])));
  assert.equal(absent.branches[0].checkpoints[0].receiptState, 'absent');
  assert.equal(C.STATE_LABELS.unknown, '不明');
});

test('view model never carries observation output/error text or context bytes', () => {
  const vm = C.normalizeSnapshot(VALID);
  const text = JSON.stringify(vm);
  assert.equal(text.includes('PRIVATE'), false);
  const keys = [];
  (function walk(v) { if (v && typeof v === 'object') for (const [k, x] of Object.entries(v)) { keys.push(k); walk(x); } })(vm);
  for (const k of ['output', 'error', 'bytes', 'context']) assert.equal(keys.includes(k), false, k);
  const c = vm.branches[0].checkpoints[0];
  assert.equal(c.outputBytes, Buffer.byteLength('PRIVATE-OUTPUT-TEXT'));
  assert.equal(c.errorBytes, Buffer.byteLength('PRIVATE-ERROR-TEXT'));
  assert.equal(c.contextCount, 1);
  assert.equal(c.contextBytes, 3);
  assert.equal(c.tool, 'bash');
  const noErr = C.normalizeSnapshot(cp((x) => { x.input.observation.error = null; }));
  assert.equal(noErr.branches[0].checkpoints[0].errorBytes, null);
});

test('hostile strings stay inert data; u64 sequences survive as exact text', () => {
  const vm = C.normalizeSnapshot(one((b) => { b.spec.goal.purpose = '<img src=x onerror=alert(1)>'; }));
  assert.equal(vm.branches[0].purpose, '<img src=x onerror=alert(1)>');
  const big = C.normalizeSnapshot(J(env([branch('a', [987654321], [], false)])).replace('987654321', '18446744073709551615'));
  assert.equal(big.branches[0].checkpoints[0].sequence, '18446744073709551615');
});

test('action/signal enum forms are summarised', () => {
  const vm = C.normalizeSnapshot(cp((c) => { c.action = { Replan: { message: 'refocus' } }; c.assessment.signal = { Misaligned: null }; }));
  const c = vm.branches[0].checkpoints[0];
  assert.equal(c.action, 'Replan');
  assert.equal(c.actionMessage, 'refocus');
  assert.equal(c.signal, 'Misaligned');
});

test('invalid import leaves the current state unchanged', () => {
  const s0 = C.createInitialState();
  const r1 = C.importSnapshot(s0, VALID);
  assert.equal(r1.error, null);
  assert.equal(r1.state.snapshot.branches.length, 3);
  assert.equal(s0.snapshot, null);
  const r2 = C.importSnapshot(r1.state, '{"schema":"h2o-snapshot/2","branches":[]}');
  assert.match(r2.error, /スナップショット/);
  assert.equal(r2.state, r1.state);
  assert.equal(r2.state.snapshot.branches.length, 3);
});

// Parity with the accepted Rust gate (c6c728c80) using the existing build; skipped if absent.
const exe = process.env.H2O_UI_BIN || resolve(uiDir, '../../../target/h2o-drift/debug', process.platform === 'win32' ? 'h2o-ui.exe' : 'h2o-ui');

function rustAccepts(file) {
  return new Promise((done, fail) => {
    const child = spawn(exe, ['--port', '0', '--assets', uiDir, '--snapshot', file], { stdio: ['ignore', 'pipe', 'pipe'] });
    const timer = setTimeout(() => { child.kill(); fail(new Error('launcher timeout')); }, 10000);
    child.stdout.on('data', (d) => { if (/^http:\/\/127\.0\.0\.1:\d+\//.test(String(d))) { clearTimeout(timer); child.kill(); done(true); } });
    child.on('exit', (code) => { clearTimeout(timer); if (code === 2) done(false); });
    child.on('error', (e) => { clearTimeout(timer); fail(e); });
  });
}

test('Rust launcher gate agrees with the same case table', { skip: !existsSync(exe) && `launcher not built: ${exe}` }, async () => {
  const dir = mkdtempSync(join(tmpdir(), 'h2o-ui-parity-'));
  try {
    const wrong = [];
    for (const [i, [name, input, ok]] of CASES.entries()) {
      const file = join(dir, `case-${i}.json`);
      writeFileSync(file, typeof input === 'string' ? Buffer.from(input, 'utf8') : input);
      if ((await rustAccepts(file)) !== ok) wrong.push(`${name}: Rust did not ${ok ? 'accept' : 'reject'}`);
    }
    assert.deepEqual(wrong, []);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
