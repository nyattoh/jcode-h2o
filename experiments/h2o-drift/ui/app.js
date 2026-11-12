/* H₂O local reception UI — DOM controller. All logic lives in core.js (H2OCore); every dynamic
 * string is written with textContent. The only network call is the explicit snapshot button. */
(function () {
  'use strict';
  const C = globalThis.H2OCore;
  if (!C || typeof document === 'undefined') return;

  const SNAPSHOT_URL = '/api/snapshot';
  const DEMO_DELAY_MS = 400;
  const SHOWN_CHECKPOINTS = 20; // ponytail: per-branch display cap; counts are still exact.
  const $ = (id) => document.getElementById(id);
  const el = (tag, cls, text) => {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text != null) n.textContent = String(text);
    return n;
  };

  const app = $('app');
  const input = $('input');
  const sendBtn = $('send');
  const wide = window.matchMedia('(min-width: 1180px)');
  const narrow = window.matchMedia('(max-width: 859px)');

  let storage = null;
  try { storage = window.localStorage; } catch (e) { storage = null; }
  const store = C.createStore({ storage });
  let state = C.createInitialState();
  let pending = null; // session id waiting for its demo reply
  let composing = false;
  let importSource = '';

  // ---- rendering ---------------------------------------------------------------------------

  function renderHistory() {
    const nav = $('history');
    const sessions = store.list();
    nav.replaceChildren(...sessions.map((s) => {
      const b = el('button', 'history-item');
      b.type = 'button';
      b.dataset.id = s.id;
      b.title = s.title;
      if (s.id === store.activeId) b.setAttribute('aria-current', 'page');
      b.append(el('span', 'truncate', s.title));
      return b;
    }));
    $('history-empty').hidden = sessions.length > 0;
    const note = $('storage-note');
    note.textContent = store.warning || '';
    note.hidden = !store.warning;
  }

  function renderMessages() {
    const s = store.get(store.activeId);
    $('chat-title').textContent = s ? s.title : '新しいチャット';
    $('welcome').hidden = !!s;
    $('messages').replaceChildren(...(s ? s.messages : []).map((m) => {
      const li = el('li', 'msg msg-' + m.role);
      if (m.role === 'demo') {
        const head = el('div', 'msg-head');
        head.append(el('span', 'avatar', 'H₂O'), el('span', 'badge badge-demo truncate', C.DEMO_LABEL));
        li.append(head);
      } else {
        li.append(el('span', 'sr-only', 'あなた：'));
      }
      li.append(el('div', 'bubble', C.messageBody(m)));
      return li;
    }));
    const waiting = !!s && pending === s.id;
    $('typing').hidden = !waiting;
    $('conversation').setAttribute('aria-busy', String(waiting));
  }

  function renderStatus() {
    const dl = $('status-rows');
    const rows = C.statusRows(state, { sessions: store.list().length, persisted: store.persisted });
    dl.replaceChildren(...rows.map((r) => {
      const row = el('div', 'status-row');
      const dd = el('dd', 'truncate', r.value);
      dd.title = r.value;
      row.append(el('dt', 'truncate', r.label), dd, el('span', 'kind kind-' + r.kind, C.KIND_LABELS[r.kind]));
      return row;
    }));
  }

  function chip(value) {
    const kind = value === 'unknown' || value === 'absent' ? 'unknown' : 'imported';
    return el('span', 'kind kind-' + kind, C.STATE_LABELS[value]);
  }

  function kv(parent, label, value, unknown) {
    const row = el('div', 'kv');
    const v = el('span', 'truncate' + (unknown ? ' is-unknown' : ''), value);
    v.title = value;
    row.append(el('span', 'kv-key truncate', label), v);
    parent.append(row);
  }

  function renderSnapshot() {
    const box = $('snapshot');
    const vm = state.snapshot;
    if (!vm) {
      box.replaceChildren(el('p', 'empty', 'まだ取り込んでいません。'));
      return;
    }
    const head = el('div', 'snap-head');
    head.append(el('span', 'kind kind-imported truncate', vm.label));
    const clear = el('button', 'link-btn', '取り込みを解除');
    clear.type = 'button';
    clear.id = 'clear-snapshot';
    head.append(clear);
    const meta = el('div', 'snap-meta');
    kv(meta, '取得元', importSource);
    kv(meta, 'captured_at', vm.capturedAt === null ? '不明（未記載）' : vm.capturedAt, vm.capturedAt === null);
    kv(meta, 'ブランチ', `${vm.branches.length} 件`);
    const cards = vm.branches.map((b) => {
      const card = el('article', 'branch');
      const top = el('div', 'branch-top');
      top.append(el('strong', 'truncate', b.branchId || '（branch_id 空）'), chip(b.needsReconciliation));
      card.append(top, el('p', 'purpose', b.purpose || '（目的 空）'));
      const meta = el('div', 'branch-meta');
      kv(meta, 'run_id', b.runId);
      kv(meta, 'session', b.sessionId);
      kv(meta, 'owner', b.ownerSessionId);
      kv(meta, '修正上限', b.maxCorrections);
      kv(meta, '成功条件', b.successConditions.length ? b.successConditions.join(' / ') : '（なし）');
      kv(meta, '所有パス', b.ownedPaths.length ? b.ownedPaths.join(', ') : '（なし）');
      kv(meta, 'checkpoint', `${b.checkpoints.length} 件・receipt ${b.receipts.length} 件`);
      card.append(meta);
      if (b.checkpoints.length) {
        const list = el('ol', 'checkpoints');
        for (const c of b.checkpoints.slice(0, SHOWN_CHECKPOINTS)) {
          const li = el('li', 'checkpoint');
          const line = el('div', 'cp-line');
          line.append(el('span', 'cp-seq', '#' + c.sequence), el('span', 'cp-signal sig-' + c.signal.toLowerCase(), c.signal),
            el('span', 'truncate', `${c.decision} → ${c.action}${c.actionMessage !== null ? '：' + c.actionMessage : ''}`), chip(c.receiptState));
          const detail = el('div', 'cp-detail truncate', `${c.tool}・出力 ${c.outputBytes} B・エラー ${c.errorBytes === null ? 'なし' : c.errorBytes + ' B'}・文脈 ${c.contextCount} 件 ${c.contextBytes} B（本文は非表示）`);
          const reason = el('div', 'cp-detail truncate', '理由：' + c.reason);
          reason.title = c.reason;
          li.append(line, detail, reason);
          list.append(li);
        }
        card.append(list);
        if (b.checkpoints.length > SHOWN_CHECKPOINTS) card.append(el('p', 'muted', `他 ${b.checkpoints.length - SHOWN_CHECKPOINTS} 件は省略（件数は上記のとおり）`));
      }
      return card;
    });
    box.replaceChildren(head, meta, ...cards);
  }

  function render() {
    renderHistory();
    renderMessages();
    renderStatus();
    renderSnapshot();
    updateSend();
  }

  function updateSend() {
    sendBtn.disabled = !!pending || !input.value.trim();
    sendBtn.setAttribute('aria-label', pending ? 'デモ応答を作成中' : '送信');
  }

  function scrollToEnd() {
    const box = $('conversation');
    box.scrollTop = box.scrollHeight;
  }

  function showComposerError(message) {
    const p = $('composer-error');
    p.textContent = message || '';
    p.hidden = !message;
  }

  function showImport(kind, message) {
    const p = $('import-status');
    p.textContent = message;
    p.className = 'import-status is-' + kind;
    p.hidden = !message;
  }

  // ---- chat --------------------------------------------------------------------------------

  function send() {
    if (pending) return;
    if (!input.value.trim()) return;
    const v = C.validateMessage(input.value);
    if (!v.ok) { showComposerError(v.error); return; }
    showComposerError('');
    const s = store.addUser(v.text);
    input.value = '';
    pending = s.id;
    render();
    scrollToEnd();
    window.setTimeout(() => {
      store.addDemo(s.id, C.demoReply(v.text));
      pending = null;
      render();
      scrollToEnd();
    }, DEMO_DELAY_MS);
  }

  input.addEventListener('compositionstart', () => { composing = true; });
  input.addEventListener('compositionend', () => { composing = false; });
  input.addEventListener('keydown', (e) => {
    if (C.shouldSend(e, composing)) { e.preventDefault(); send(); }
  });
  input.addEventListener('input', () => { showComposerError(''); updateSend(); });
  $('composer').addEventListener('submit', (e) => { e.preventDefault(); send(); });

  $('new-chat').addEventListener('click', () => {
    store.draft();
    showComposerError('');
    closeSidebar();
    render();
    input.focus();
  });

  $('history').addEventListener('click', (e) => {
    const b = e.target.closest('.history-item');
    if (!b || !store.select(b.dataset.id)) return;
    closeSidebar();
    render();
    scrollToEnd();
  });

  $('suggestions').addEventListener('click', (e) => {
    const b = e.target.closest('[data-prompt]');
    if (!b) return;
    input.value = b.dataset.prompt;
    updateSend();
    input.focus();
  });

  // ---- drawers -----------------------------------------------------------------------------

  function setSidebar(open) {
    app.dataset.sidebar = open ? 'open' : 'closed';
    $('open-sidebar').setAttribute('aria-expanded', String(open));
    syncScrim();
  }
  function closeSidebar() { if (narrow.matches) setSidebar(false); }
  function setPanel(open) {
    app.dataset.panel = open ? 'open' : 'closed';
    $('toggle-panel').setAttribute('aria-expanded', String(open));
    syncScrim();
  }
  function syncScrim() {
    $('scrim').hidden = !((narrow.matches && app.dataset.sidebar === 'open') || (!wide.matches && app.dataset.panel === 'open'));
  }
  $('open-sidebar').addEventListener('click', () => setSidebar(true));
  $('close-sidebar').addEventListener('click', () => setSidebar(false));
  $('toggle-panel').addEventListener('click', () => setPanel(app.dataset.panel !== 'open'));
  $('close-panel').addEventListener('click', () => setPanel(false));
  $('scrim').addEventListener('click', () => { setSidebar(false); if (!wide.matches) setPanel(false); });
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    if (narrow.matches && app.dataset.sidebar === 'open') setSidebar(false);
    else if (!wide.matches && app.dataset.panel === 'open') setPanel(false);
  });
  const onResize = () => { setPanel(wide.matches); if (!narrow.matches) setSidebar(false); };
  wide.addEventListener('change', onResize);
  narrow.addEventListener('change', onResize);

  // ---- explicit, read-only snapshot import -------------------------------------------------

  function applyImport(data, source) {
    const r = C.importSnapshot(state, data);
    if (r.error) { showImport('error', r.error); return; }
    state = r.state;
    importSource = source;
    showImport('ok', '読み取り専用で取り込みました。事実・デモ値とは区別して表示します。');
    render();
  }

  $('file').addEventListener('change', async (e) => {
    const f = e.target.files && e.target.files[0];
    e.target.value = '';
    if (!f) return;
    showImport('busy', '読み込み中…');
    try {
      // Read at most limit + 1 bytes so the size check rejects large files without loading them.
      applyImport(await f.slice(0, C.LIMITS.bytes + 1).arrayBuffer(), 'ファイル：' + f.name);
    } catch (err) {
      showImport('error', 'ファイルを読み込めませんでした。現在の表示は変更していません。');
    }
  });

  $('import-paste').addEventListener('click', () => applyImport($('paste').value, '貼り付け'));

  $('fetch-snapshot').addEventListener('click', async () => {
    if (location.protocol !== 'http:' && location.protocol !== 'https:') {
      showImport('error', 'ファイルとして開いているため取得できません。h2o-ui ランチャー経由で開くか、ファイル選択・貼り付けを使ってください。');
      return;
    }
    const btn = $('fetch-snapshot');
    btn.disabled = true;
    showImport('busy', '取得中…');
    try {
      const res = await fetch(SNAPSHOT_URL, { method: 'GET', mode: 'same-origin', credentials: 'same-origin', cache: 'no-store', redirect: 'error' });
      if (res.status === 404) showImport('error', 'ランチャーにスナップショットが指定されていません（--snapshot）。現在の表示は変更していません。');
      else if (!res.ok) showImport('error', `取得できませんでした（HTTP ${res.status}）。現在の表示は変更していません。`);
      else applyImport(await res.arrayBuffer(), 'ランチャー（/api/snapshot）');
    } catch (err) {
      showImport('error', '取得できませんでした。ランチャーが起動しているか確認してください。現在の表示は変更していません。');
    } finally {
      btn.disabled = false;
    }
  });

  $('snapshot').addEventListener('click', (e) => {
    if (!e.target.closest('#clear-snapshot')) return;
    state = Object.assign({}, state, { snapshot: null });
    importSource = '';
    showImport('ok', '取り込みを解除しました。');
    render();
  });

  // ---- start -------------------------------------------------------------------------------

  store.load();
  onResize();
  render();
})();
