// Static boundary checks on the shipped assets: CSP-compatible markup, no dynamic HTML, one explicit fetch.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const dir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const read = (f) => readFileSync(resolve(dir, f), 'utf8');
const html = read('index.html');
const css = read('app.css');
const app = read('app.js');
const core = read('core.js');

test('index.html loads only the three relative assets with classic scripts and no inline code/style', () => {
  const refs = [...html.matchAll(/\b(?:src|href)\s*=\s*"([^"]*)"/g)].map((m) => m[1]);
  assert.deepEqual(refs.sort(), ['app.css', 'app.js', 'core.js']);
  assert.equal(/<script(?![^>]*\bsrc=)[^>]*>/i.test(html), false, 'inline script');
  assert.equal(/type\s*=\s*"module"/i.test(html), false, 'module script');
  assert.equal(/<style/i.test(html), false, 'inline style element');
  assert.equal(/\sstyle\s*=/i.test(html), false, 'style attribute');
  assert.equal(/\son[a-z]+\s*=/i.test(html), false, 'inline handler');
  assert.equal(/(?:https?:)?\/\/[a-z]/i.test(html.replace(/xmlns="http:\/\/www\.w3\.org\/2000\/svg"/g, '')), false, 'external URL');
  assert.equal(/url\(|@import|https?:/i.test(css), false, 'external CSS resource');
});

test('UI shows the demo/unconnected banner and unconfirmed target from first paint', () => {
  assert.match(html, /デモ／未接続/);
  assert.match(html, /未確認（書き込み不可）/);
  assert.match(html, /lang="ja"/);
});

test('scripts never build HTML from strings or evaluate code', () => {
  for (const [name, src] of [['app.js', app], ['core.js', core]]) {
    for (const bad of [/innerHTML/, /outerHTML/, /insertAdjacentHTML/, /document\.write/, /\beval\s*\(/, /new\s+Function/, /setTimeout\s*\(\s*['"`]/, /import\s*\(/]) {
      assert.equal(bad.test(src), false, `${name}: ${bad}`);
    }
  }
});

test('the only network call is the explicit same-origin snapshot fetch', () => {
  assert.equal((app.match(/\bfetch\s*\(/g) || []).length, 1);
  assert.match(app, /fetch\(\s*SNAPSHOT_URL/);
  assert.match(app, /const SNAPSHOT_URL = '\/api\/snapshot'/);
  assert.equal(/\bfetch\s*\(|XMLHttpRequest|WebSocket|EventSource|sendBeacon/.test(core), false);
  assert.equal(/XMLHttpRequest|WebSocket|EventSource|sendBeacon/.test(app), false);
});

test('compact status rows truncate; chat prose wraps', () => {
  const rule = (sel) => { const m = css.match(new RegExp('(?:^|\\n)' + sel.replace(/[.]/g, '\\.') + '\\s*\\{([^}]*)\\}')); return m ? m[1] : ''; };
  for (const sel of ['.truncate']) {
    const body = rule(sel);
    for (const decl of ['white-space:nowrap', 'overflow:hidden', 'text-overflow:ellipsis']) assert.ok(body.replace(/\s/g, '').includes(decl), `${sel} ${decl}`);
  }
  assert.ok(rule('.bubble').replace(/\s/g, '').includes('white-space:pre-wrap'));
  assert.match(css, /@media\s*\(max-width:\s*\d+px\)/);
});
