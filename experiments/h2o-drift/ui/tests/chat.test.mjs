// Chat/session behaviour, keyboard decision, demo replies and the status model.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const C = require('../core.js');

function memoryStorage() {
  const data = new Map();
  return { data, getItem: (k) => (data.has(k) ? data.get(k) : null), setItem: (k, v) => data.set(k, String(v)), removeItem: (k) => data.delete(k) };
}
const clock = () => { let t = 1000; return () => ++t; };

test('history persists under the h2o-ui: prefix and reloads into a new store', () => {
  const storage = memoryStorage();
  const a = C.createStore({ storage, now: clock() });
  a.load();
  const s = a.addUser('最初の質問です');
  a.addDemo(s.id, C.demoReply('最初の質問です'));
  assert.deepEqual([...storage.data.keys()], ['h2o-ui:sessions']);
  const b = C.createStore({ storage, now: clock() });
  const loaded = b.load();
  assert.equal(loaded.persisted, true);
  assert.equal(b.list().length, 1);
  assert.equal(b.list()[0].title, '最初の質問です');
  assert.deepEqual(b.list()[0].messages.map((m) => m.role), ['user', 'demo']);
});

test('storage failures are tolerated and reported; chat still works in memory', () => {
  const broken = { getItem() { throw new Error('SecurityError'); }, setItem() { throw new Error('QuotaExceeded'); } };
  for (const storage of [broken, null]) {
    const st = C.createStore({ storage, now: clock() });
    const r = st.load();
    assert.equal(r.persisted, false);
    assert.match(r.warning, /保存/);
    const s = st.addUser('こんにちは');
    assert.equal(st.get(s.id).messages.length, 1);
    assert.equal(st.persisted, false);
  }
});

test('corrupt or foreign stored history is discarded without throwing', () => {
  for (const raw of ['{not json', '{"a":1}', '[{"id":1}]', JSON.stringify([{ id: 'x', title: 't', createdAt: 1, messages: [{ role: 'model', text: 'fake live answer' }] }])]) {
    const storage = memoryStorage();
    storage.setItem('h2o-ui:sessions', raw);
    const st = C.createStore({ storage, now: clock() });
    const r = st.load();
    assert.equal(st.list().length, 0, raw);
    assert.match(r.warning || '', /破損|読み込めません/, raw);
  }
});

test('new chat drafts, first send creates a session, selection switches, replies go to their own session', () => {
  const st = C.createStore({ storage: memoryStorage(), now: clock() });
  st.load();
  assert.equal(st.activeId, null);
  const a = st.addUser('A の質問');
  assert.equal(st.activeId, a.id);
  st.draft();
  assert.equal(st.activeId, null);
  const b = st.addUser('B の質問');
  assert.notEqual(a.id, b.id);
  assert.deepEqual(st.list().map((s) => s.id), [b.id, a.id]);
  assert.equal(st.select(a.id), true);
  assert.equal(st.select('missing'), false);
  assert.equal(st.activeId, a.id);
  st.addDemo(b.id, 'B への返答');
  assert.equal(st.get(a.id).messages.length, 1);
  assert.equal(st.get(b.id).messages[1].role, 'demo');
  assert.equal(st.list()[0].id, b.id);
});

test('session titles are trimmed single-line summaries', () => {
  const st = C.createStore({ storage: null, now: clock() });
  const s = st.addUser('  一行目\n二行目 ' + 'あ'.repeat(80));
  assert.equal(s.title.includes('\n'), false);
  assert.ok([...s.title].length <= 40);
  assert.ok(s.title.startsWith('一行目 二行目'));
});

test('Enter sends; Shift+Enter, modifiers and any IME composition never send', () => {
  const base = { key: 'Enter', shiftKey: false, altKey: false, ctrlKey: false, metaKey: false, isComposing: false, keyCode: 13 };
  assert.equal(C.shouldSend(base, false), true);
  assert.equal(C.shouldSend({ ...base, shiftKey: true }, false), false);
  assert.equal(C.shouldSend({ ...base, isComposing: true }, false), false);
  assert.equal(C.shouldSend({ ...base, keyCode: 229 }, false), false);
  assert.equal(C.shouldSend(base, true), false, 'compositionstart..end tracked by controller');
  assert.equal(C.shouldSend({ ...base, altKey: true }, false), false);
  assert.equal(C.shouldSend({ ...base, key: 'a' }, false), false);
});

test('message validation rejects empty and overlong input in Japanese', () => {
  assert.equal(C.validateMessage('  \n ').ok, false);
  assert.match(C.validateMessage('').error, /入力/);
  const long = C.validateMessage('x'.repeat(C.LIMITS.message + 1));
  assert.equal(long.ok, false);
  assert.match(long.error, /長すぎ/);
  assert.deepEqual(C.validateMessage('  hi  '), { ok: true, text: 'hi', error: null });
});

test('demo reply is deterministic, labelled, and denies connection and write authority', () => {
  const r = C.demoReply('ファイルを書き換えて');
  assert.equal(r, C.demoReply('ファイルを書き換えて'));
  assert.ok(r.startsWith('デモ応答（実モデルではありません）'));
  assert.match(r, /未接続/);
  assert.match(r, /作業対象は未確認/);
  assert.match(r, /書き込み.*実行.*行いません/);
  assert.notEqual(C.demoReply('a'), C.demoReply('b'));
});

test('default state is demo/unconnected with an unconfirmed, non-writable target', () => {
  const s = C.createInitialState();
  assert.equal(s.mode, 'demo');
  assert.equal(s.connected, false);
  assert.equal(s.target, '未確認（書き込み不可）');
  assert.equal(s.writeAuthority, false);
  assert.equal(s.snapshot, null);
});

test('status rows distinguish 事実 / デモ値 / 不明・未接続 and imported read-only data', () => {
  assert.deepEqual(C.KIND_LABELS, { fact: '事実', demo: 'デモ値', unknown: '不明・未接続', imported: '取り込み（読み取り専用）' });
  const rows = C.statusRows(C.createInitialState(), { sessions: 2, persisted: true });
  const by = Object.fromEntries(rows.map((r) => [r.label, r]));
  assert.deepEqual([by['接続'].kind, by['接続'].value], ['fact', '未接続']);
  assert.deepEqual([by['作業対象'].kind, by['作業対象'].value], ['fact', '未確認（書き込み不可）']);
  for (const k of ['メモリ', 'Worker', '目的監視']) assert.deepEqual([by[k].kind, by[k].value], ['unknown', '不明・未接続'], k);
  assert.ok(rows.some((r) => r.kind === 'demo'));
  assert.equal(by['スナップショット'].kind, 'fact');
  for (const r of rows) assert.ok(r.kind in C.KIND_LABELS);
  const imported = C.importSnapshot(C.createInitialState(), '{"schema":"h2o-snapshot/1","branches":[]}').state;
  const after = Object.fromEntries(C.statusRows(imported, { sessions: 0, persisted: false }).map((r) => [r.label, r]));
  assert.equal(after['スナップショット'].kind, 'imported');
  assert.deepEqual([after['目的監視'].kind, after['目的監視'].value], ['unknown', '不明・未接続'], 'import is not a live monitor');
  assert.match(after['会話履歴'].value, /このタブのみ/);
});
