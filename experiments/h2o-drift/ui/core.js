/* H₂O local reception UI — pure, DOM-free logic. Classic script: exposes globalThis.H2OCore
 * (and module.exports under Node). Nothing here touches the network, the DOM or the filesystem. */
(function (root) {
  'use strict';

  const SNAPSHOT_SCHEMA = 'h2o-snapshot/1';
  const STORAGE_KEY = 'h2o-ui:sessions';
  const LIMITS = Object.freeze({ bytes: 1048576, branches: 50, checkpoints: 1000, depth: 127, message: 4000, sessions: 50, title: 40 });
  const IMPORT_LABEL = '取り込み済みスナップショット（読み取り専用）';
  const DEMO_LABEL = 'デモ応答（実モデルではありません）';
  const UNKNOWN = '不明・未接続';
  const KIND_LABELS = Object.freeze({ fact: '事実', demo: 'デモ値', unknown: UNKNOWN, imported: '取り込み（読み取り専用）' });
  const STATE_LABELS = Object.freeze({
    acknowledged: '受領確認あり', not_acknowledged: '受領確認なし', unknown: '不明', absent: '記録なし',
    required: '照合が必要', not_required: '照合不要',
  });

  function createInitialState() {
    return { mode: 'demo', connected: false, target: '未確認（書き込み不可）', writeAuthority: false, snapshot: null };
  }

  // ---- text helpers ------------------------------------------------------------------------

  const wellFormed = (s) => {
    if (typeof s.isWellFormed === 'function') return s.isWellFormed();
    try { encodeURIComponent(s); return true; } catch (e) { return false; }
  };
  const utf8Length = (s) => new TextEncoder().encode(s).length;
  const oneLine = (s) => String(s).replace(/\s+/g, ' ').trim();
  function clip(s, max) {
    const chars = Array.from(s);
    return chars.length > max ? chars.slice(0, max - 1).join('') + '…' : s;
  }
  function hash(s) {
    let h = 0x811c9dc5;
    for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 0x01000193) >>> 0;
    return h;
  }

  // ---- chat --------------------------------------------------------------------------------

  /** Enter sends; Shift/Alt/Ctrl/Meta+Enter and every IME composition signal never send. */
  function shouldSend(e, composing) {
    return e.key === 'Enter' && !e.shiftKey && !e.altKey && !e.ctrlKey && !e.metaKey &&
      !e.isComposing && e.keyCode !== 229 && !composing;
  }

  function validateMessage(text) {
    const t = String(text == null ? '' : text).trim();
    if (!t) return { ok: false, text: '', error: 'メッセージを入力してください。' };
    if (Array.from(t).length > LIMITS.message) return { ok: false, text: t, error: `メッセージが長すぎます（最大 ${LIMITS.message} 文字）。` };
    return { ok: true, text: t, error: null };
  }

  const HINTS = [
    '実際の作業を依頼する場合は、対象のプロジェクトとパスを確認できる接続済みの環境で行ってください。',
    '状態パネルの「不明・未接続」は、実際の値を取得していないことを示します。推測値ではありません。',
    '既存の監視記録は、状態パネルの「スナップショット取り込み」から読み取り専用で確認できます。',
  ];

  /** Deterministic local reply. It never claims to come from a model, Jev or a worker. */
  function demoReply(text) {
    const t = String(text);
    return [
      DEMO_LABEL,
      `受け取ったメッセージ（${Array.from(t).length} 文字）：「${clip(oneLine(t), 60)}」`,
      'この画面は未接続のデモです。実モデル・Jev・Worker には送信していません。',
      '作業対象は未確認のため、ファイルの書き込みやコマンドの実行は行いません。',
      HINTS[hash(t) % HINTS.length],
    ].join('\n');
  }

  /** Demo replies carry their label as the first line; the UI shows it as a badge instead. */
  function messageBody(m) {
    return m.role === 'demo' && m.text.startsWith(DEMO_LABEL + '\n') ? m.text.slice(DEMO_LABEL.length + 1) : m.text;
  }

  const sessionTitle = (text) => clip(oneLine(text), LIMITS.title) || '新しいチャット';

  function validSession(s) {
    return s && typeof s === 'object' && typeof s.id === 'string' && s.id.length <= 100 &&
      typeof s.title === 'string' && Number.isFinite(s.createdAt) && Number.isFinite(s.updatedAt) &&
      Array.isArray(s.messages) &&
      s.messages.every((m) => m && (m.role === 'user' || m.role === 'demo') && typeof m.text === 'string' && Number.isFinite(m.at));
  }

  /** Session history. Persists only to the injected Storage under `h2o-ui:`; any failure keeps it in memory. */
  function createStore(opts) {
    const storage = (opts && opts.storage) || null;
    const now = (opts && opts.now) || Date.now;
    let sessions = [];
    let counter = 0;
    const store = {
      activeId: null,
      persisted: false,
      warning: null,
      load() {
        sessions = [];
        store.activeId = null;
        store.warning = null;
        let raw = null;
        try {
          if (!storage) throw new Error('no storage');
          raw = storage.getItem(STORAGE_KEY);
          store.persisted = true;
        } catch (e) {
          store.persisted = false;
          store.warning = 'ブラウザ保存を利用できません。履歴はこのタブを閉じると消えます。';
          return { persisted: false, warning: store.warning };
        }
        if (raw !== null) {
          let list = null;
          try { list = JSON.parse(raw); } catch (e) { list = null; }
          const valid = Array.isArray(list) ? list.filter(validSession) : [];
          if (!Array.isArray(list) || valid.length !== list.length) store.warning = '保存済みの履歴の一部が破損していたため読み込めませんでした。';
          sessions = valid.slice(0, LIMITS.sessions).map((s) => ({
            id: s.id, title: s.title, createdAt: s.createdAt, updatedAt: s.updatedAt,
            messages: s.messages.map((m) => ({ role: m.role, text: m.text, at: m.at })),
          }));
        }
        return { persisted: store.persisted, warning: store.warning };
      },
      list: () => sessions.slice(),
      get: (id) => sessions.find((s) => s.id === id) || null,
      draft() { store.activeId = null; },
      select(id) {
        if (!store.get(id)) return false;
        store.activeId = id;
        return true;
      },
      addUser(text) {
        const t = now();
        let s = store.get(store.activeId);
        if (!s) {
          s = { id: `s${t}-${++counter}`, title: sessionTitle(text), createdAt: t, updatedAt: t, messages: [] };
          sessions.unshift(s);
          store.activeId = s.id;
        }
        push(s, 'user', text, t);
        return s;
      },
      addDemo(id, text) {
        const s = store.get(id);
        if (s) push(s, 'demo', text, now());
        return s;
      },
    };
    function push(s, role, text, t) {
      s.messages.push({ role, text: String(text), at: t });
      s.updatedAt = t;
      sessions = [s, ...sessions.filter((x) => x !== s)].slice(0, LIMITS.sessions);
      save();
    }
    function save() {
      if (!storage) { store.persisted = false; return; }
      try {
        storage.setItem(STORAGE_KEY, JSON.stringify(sessions));
        store.persisted = true;
      } catch (e) {
        store.persisted = false;
        store.warning = 'ブラウザに保存できませんでした。履歴はこのタブを閉じると消えます。';
      }
    }
    return store;
  }

  // ---- snapshot gate -----------------------------------------------------------------------
  // Mirrors the serde types in src/monitor.rs and the envelope rules in src/ui.rs:
  // derived structs ignore unknown keys and also accept sequence form; Envelope/BranchView/
  // ReceiptView deny unknown keys; spec/checkpoint/receipt entries must be JSON objects.

  class SnapshotError extends Error {}
  const fail = (path, what) => { throw new SnapshotError(`${path || '最上位'}：${what}`); };
  const NON_INTEGER = Object.freeze(Object.create(null)); // serde_json parses 1.0 / 1e0 / -0 as f64
  const isObject = (v) => v !== null && typeof v === 'object' && Object.getPrototypeOf(v) === Object.prototype;
  const has = (o, k) => Object.prototype.hasOwnProperty.call(o, k);

  const STR = { str: true };
  const BOOL = { bool: true };
  const uint = (max) => ({ uint: BigInt(max) });
  const vec = (of) => ({ vec: of });
  const option = (of) => ({ option: of });   // Option<T>: missing or null → None
  const present = (of) => ({ present: of }); // deserialize_with "present": required key, null = unknown
  const omittable = (of) => ({ omittable: of }); // captured_at: missing, or a string (null rejected by ui.rs)
  const struct = (fields, flags) => ({ fields, deny: !!(flags && flags.deny), objectOnly: !!(flags && flags.objectOnly) });
  const enumeration = (variants) => ({ variants }); // externally tagged; null = unit variant

  const GOAL = struct([['purpose', STR], ['constraints', vec(STR)], ['success_conditions', vec(STR)], ['owned_paths', vec(STR)], ['unfinished', vec(STR)]]);
  const SPEC = struct([['run_id', STR], ['branch_id', STR], ['owner_session_id', STR], ['session_id', STR], ['goal', GOAL], ['context_ids', vec(STR)], ['max_corrections', uint('4294967295')]], { objectOnly: true });
  const OBSERVATION = struct([['call_id', STR], ['tool', STR], ['output', STR], ['error', option(STR)]]);
  const SOURCE = struct([['id', STR], ['bytes', vec(uint(255))]]);
  const INPUT = struct([['goal', GOAL], ['observation', OBSERVATION], ['context', vec(SOURCE)]]);
  const SIGNAL = enumeration({ Aligned: null, Misaligned: null, Unknown: null });
  const ASSESSMENT = struct([['signal', SIGNAL], ['reason', STR]]);
  const DECISION = enumeration({ Continue: null, Correct: null, StopBranch: null, Hold: null });
  const ACTION = enumeration({ Continue: null, Replan: struct([['message', STR]]), Hold: null, StopBranch: null });
  const U64 = uint('18446744073709551615');
  const CHECKPOINT = struct([['sequence', U64], ['input', INPUT], ['assessment', ASSESSMENT], ['decision', DECISION], ['action', ACTION]], { objectOnly: true });
  const RECEIPT = struct([['acknowledged', BOOL]]);
  const RECEIPT_ENTRY = struct([['sequence', U64], ['receipt', present(RECEIPT)]], { deny: true, objectOnly: true });
  const BRANCH = struct([['spec', SPEC], ['checkpoints', vec(CHECKPOINT)], ['receipts', vec(RECEIPT_ENTRY)], ['needs_reconciliation', present(BOOL)]], { deny: true, objectOnly: true });
  const ENVELOPE = struct([['schema', STR], ['captured_at', omittable(STR)], ['branches', vec(BRANCH)]], { deny: true, objectOnly: true });

  /** Type-checks `v` against `t` and returns a normalised copy (structs as objects, enums as {variant, value}). */
  function decode(t, v, path) {
    if (t.str) return typeof v === 'string' ? v : fail(path, '文字列が必要です');
    if (t.bool) return typeof v === 'boolean' ? v : fail(path, '真偽値が必要です');
    if (t.uint !== undefined) {
      const ok = (typeof v === 'number' && Number.isSafeInteger(v) && v >= 0 && !Object.is(v, -0)) || typeof v === 'bigint';
      if (!ok || BigInt(v) < 0n || BigInt(v) > t.uint) fail(path, `0〜${t.uint} の整数が必要です`);
      return BigInt(v);
    }
    if (t.vec) return Array.isArray(v) ? v.map((x, i) => decode(t.vec, x, `${path}[${i}]`)) : fail(path, '配列が必要です');
    if (t.option || t.present) return v === null ? null : decode(t.option || t.present, v, path);
    if (t.omittable) return decode(t.omittable, v, path);
    if (t.variants) return decodeEnum(t, v, path);
    return decodeStruct(t, v, path);
  }

  function decodeStruct(t, v, path) {
    const out = {};
    const at = (name) => (path ? `${path}.${name}` : name);
    if (Array.isArray(v) && !t.objectOnly) {
      if (v.length !== t.fields.length) fail(path, `要素数は ${t.fields.length} 個である必要があります`);
      t.fields.forEach(([name, ft], i) => { out[name] = decode(ft, v[i], at(name)); });
      return out;
    }
    if (!isObject(v)) fail(path, 'オブジェクトが必要です');
    if (t.deny) for (const k of Object.keys(v)) if (!t.fields.some(([name]) => name === k)) fail(at(k), '未知の項目です');
    for (const [name, ft] of t.fields) {
      if (has(v, name)) out[name] = decode(ft, v[name], at(name));
      else if (ft.option || ft.omittable) out[name] = null;
      else fail(at(name), '必須項目がありません');
    }
    return out;
  }

  function decodeEnum(t, v, path) {
    let name;
    let value;
    let unitForm = false;
    if (typeof v === 'string') { name = v; unitForm = true; }
    else if (isObject(v) && Object.keys(v).length === 1) { name = Object.keys(v)[0]; value = v[name]; }
    else fail(path, '列挙値（文字列または 1 項目のオブジェクト）が必要です');
    if (!has(t.variants, name)) fail(path, `未知の値「${clip(name, 40)}」です`);
    const body = t.variants[name];
    if (body === null) {
      if (!unitForm && value !== null) fail(path, `${name} に値は指定できません`);
      return { variant: name, value: null };
    }
    if (unitForm || !isObject(value)) fail(path, `${name} にはオブジェクトの値が必要です`);
    return { variant: name, value: decodeStruct(body, value, `${path}.${name}`) };
  }

  /** JSON.parse reviver: rejects lone surrogates (serde_json does) and keeps exact integer text. */
  function reviver(key, value, context) {
    if (!wellFormed(key) || (typeof value === 'string' && !wellFormed(value))) throw new SnapshotError('対になっていないサロゲート文字を含みます');
    if (typeof value !== 'number') return value;
    const source = context && typeof context.source === 'string' ? context.source : null;
    if (Object.is(value, -0) || (source !== null && /[.eE]/.test(source))) return NON_INTEGER;
    if (Number.isSafeInteger(value)) return value;
    // ponytail: without JSON.parse source access (older browsers) unsafe integers are rejected, which is stricter than Rust.
    return source !== null && /^-?\d+$/.test(source) ? BigInt(source) : NON_INTEGER;
  }

  function depth(doc) {
    let max = 0;
    const stack = [[doc, 1]];
    while (stack.length) {
      const [v, d] = stack.pop();
      if (v === null || typeof v !== 'object') continue;
      if (d > max) max = d;
      for (const x of Array.isArray(v) ? v : Object.values(v)) stack.push([x, d + 1]);
    }
    return max;
  }

  function readText(input) {
    if (typeof input === 'string') {
      if (input.length > LIMITS.bytes || utf8Length(input) > LIMITS.bytes) fail('', 'サイズが 1 MiB を超えています');
      if (!wellFormed(input)) fail('', 'UTF-8 として正しくありません');
      return input;
    }
    if (input instanceof ArrayBuffer || ArrayBuffer.isView(input)) {
      const bytes = input instanceof ArrayBuffer ? new Uint8Array(input) : new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
      if (bytes.byteLength > LIMITS.bytes) fail('', 'サイズが 1 MiB を超えています');
      try { return new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes); } catch (e) { return fail('', 'UTF-8 として正しくありません'); }
    }
    return fail('', 'JSON テキストまたはファイルのみ取り込めます');
  }

  function parseSnapshot(input) {
    const text = readText(input);
    let raw;
    try { raw = JSON.parse(text, reviver); } catch (e) {
      if (e instanceof SnapshotError) throw e;
      fail('', 'JSON として解析できません');
    }
    if (depth(raw) > LIMITS.depth) fail('', `入れ子が深すぎます（最大 ${LIMITS.depth} 段）`);
    const doc = decode(ENVELOPE, raw, '');
    if (doc.schema !== SNAPSHOT_SCHEMA) fail('schema', `${SNAPSHOT_SCHEMA} である必要があります`);
    if (doc.branches.length > LIMITS.branches) fail('branches', `${LIMITS.branches} 件までです`);
    doc.branches.forEach((b, i) => {
      if (b.checkpoints.length > LIMITS.checkpoints) fail(`branches[${i}].checkpoints`, `${LIMITS.checkpoints} 件までです`);
      const listed = new Set();
      for (const c of b.checkpoints) {
        if (listed.has(c.sequence)) fail(`branches[${i}]`, `checkpoint の sequence ${c.sequence} が重複しています`);
        listed.add(c.sequence);
      }
      for (const r of b.receipts) if (!listed.has(r.sequence)) fail(`branches[${i}]`, `receipt が存在しない sequence ${r.sequence} を参照しています`);
    });
    return doc;
  }

  const receiptState = (receipt) => (receipt === null ? 'unknown' : receipt.acknowledged ? 'acknowledged' : 'not_acknowledged');

  /** Builds the display model. Observation output/error text and context bytes are reduced to sizes. */
  function view(doc) {
    return {
      label: IMPORT_LABEL,
      schema: doc.schema,
      capturedAt: doc.captured_at,
      branches: doc.branches.map((b) => {
        const receipts = b.receipts.map((r) => ({ sequence: String(r.sequence), state: receiptState(r.receipt) }));
        const bySequence = new Map();
        for (const r of receipts) bySequence.set(r.sequence, (bySequence.get(r.sequence) || new Set()).add(r.state));
        const g = b.spec.goal;
        return {
          runId: b.spec.run_id, branchId: b.spec.branch_id, ownerSessionId: b.spec.owner_session_id, sessionId: b.spec.session_id,
          purpose: g.purpose, constraints: g.constraints, successConditions: g.success_conditions, ownedPaths: g.owned_paths, unfinished: g.unfinished,
          contextIds: b.spec.context_ids, maxCorrections: String(b.spec.max_corrections),
          needsReconciliation: b.needs_reconciliation === null ? 'unknown' : b.needs_reconciliation ? 'required' : 'not_required',
          receipts,
          checkpoints: b.checkpoints.map((c) => {
            const o = c.input.observation;
            const states = bySequence.get(String(c.sequence));
            return {
              sequence: String(c.sequence),
              signal: c.assessment.signal.variant, reason: c.assessment.reason,
              decision: c.decision.variant,
              action: c.action.variant, actionMessage: c.action.value ? c.action.value.message : null,
              tool: o.tool, callId: o.call_id,
              outputBytes: utf8Length(o.output), errorBytes: o.error === null ? null : utf8Length(o.error),
              contextCount: c.input.context.length,
              contextBytes: c.input.context.reduce((n, s) => n + s.bytes.length, 0),
              receiptState: !states ? 'absent' : states.size === 1 ? [...states][0] : 'unknown',
            };
          }),
        };
      }),
    };
  }

  /** Validates an explicitly imported snapshot (string, ArrayBuffer or byte view); throws a Japanese Error. */
  function normalizeSnapshot(input) {
    try {
      return view(parseSnapshot(input));
    } catch (e) {
      const detail = e instanceof SnapshotError ? e.message : '形式を確認してください';
      throw new Error(`スナップショットを読み込めません（${detail}）。現在の表示は変更していません。`);
    }
  }

  /** Pure import transition: on any error the given state object is returned unchanged. */
  function importSnapshot(state, input) {
    try {
      return { state: Object.assign({}, state, { snapshot: normalizeSnapshot(input) }), error: null };
    } catch (e) {
      return { state, error: e.message };
    }
  }

  // ---- status model ------------------------------------------------------------------------

  function statusRows(state, info) {
    const sessions = (info && info.sessions) || 0;
    const persisted = !!(info && info.persisted);
    return [
      { label: '接続', value: state.connected ? '接続済み' : '未接続', kind: 'fact' },
      { label: '作業対象', value: state.target, kind: 'fact' },
      { label: '書き込み・実行', value: state.writeAuthority ? '許可あり' : '許可なし', kind: 'fact' },
      { label: '会話履歴', value: `${sessions} 件・${persisted ? 'このブラウザに保存' : 'このタブのみ（保存不可）'}`, kind: 'fact' },
      { label: 'スナップショット', value: state.snapshot ? `${state.snapshot.branches.length} ブランチ` : '未取り込み', kind: state.snapshot ? 'imported' : 'fact' },
      { label: '応答', value: 'ローカル定型文（決定的）', kind: 'demo' },
      { label: '応答時間', value: '約 0.4 秒（擬似）', kind: 'demo' },
      { label: 'メモリ', value: UNKNOWN, kind: 'unknown' },
      { label: 'Worker', value: UNKNOWN, kind: 'unknown' },
      { label: '目的監視', value: UNKNOWN, kind: 'unknown' },
    ];
  }

  const api = {
    SNAPSHOT_SCHEMA, STORAGE_KEY, LIMITS, IMPORT_LABEL, DEMO_LABEL, KIND_LABELS, STATE_LABELS,
    createInitialState, createStore, shouldSend, validateMessage, demoReply, messageBody, sessionTitle,
    normalizeSnapshot, importSnapshot, statusRows,
  };
  root.H2OCore = api;
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
})(globalThis);
