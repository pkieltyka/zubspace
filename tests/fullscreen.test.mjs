import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';

const html = fs.readFileSync(new URL('../index.html', import.meta.url), 'utf8');
const source = html.slice(html.indexOf('        let fullscreenRequestPending ='), html.indexOf('        function eventTargetIsEditable('));
function fullscreen(document) {
  const warnings = [];
  const context = vm.createContext({ document, console: { warn: (...args) => warnings.push(args) } });
  vm.runInContext(source, context);
  return { toggle: () => vm.runInContext('toggleGameFullscreen()', context), warnings };
}

test('fullscreen includes the entire page and coalesces an in-flight request', async () => {
  let finish, calls = 0;
  const root = { requestFullscreen() { assert.equal(this, root); calls++; return new Promise(resolve => { finish = resolve; }); } };
  const document = { documentElement: root, fullscreenElement: null };
  const controller = fullscreen(document);
  assert.equal(controller.toggle(), 1);
  assert.equal(controller.toggle(), 1);
  assert.equal(calls, 1);
  finish(); await new Promise(setImmediate);
  document.fullscreenElement = root;
  document.exitFullscreen = function () { assert.equal(this, document); calls++; return Promise.resolve(); };
  assert.equal(controller.toggle(), 1);
  assert.equal(calls, 2);
});

test('Safari-prefixed entry and exit can return no Promise', async () => {
  let entered = false, exited = false;
  const root = { webkitRequestFullscreen() { assert.equal(this, root); entered = true; } };
  const document = { documentElement: root, webkitFullscreenElement: null,
    webkitExitFullscreen() { assert.equal(this, document); exited = true; } };
  const controller = fullscreen(document);
  assert.equal(controller.toggle(), 1); assert.equal(entered, true);
  await new Promise(setImmediate);
  document.webkitFullscreenElement = root;
  assert.equal(controller.toggle(), 1); assert.equal(exited, true);
});

test('unsupported fullscreen and synchronous permission denial fail safely', () => {
  assert.equal(fullscreen({ documentElement: {} }).toggle(), 0);
  assert.equal(fullscreen({ documentElement: { requestFullscreen() { throw new Error('denied'); } } }).toggle(), 0);
});

test('an asynchronous refusal is handled and does not prevent a fresh retry', async () => {
  let calls = 0;
  const controller = fullscreen({ documentElement: { requestFullscreen() { calls++; return Promise.reject(new Error('denied')); } } });
  assert.equal(controller.toggle(), 1);
  await new Promise(setImmediate);
  assert.equal(controller.warnings.length, 1);
  assert.equal(controller.toggle(), 1);
  await new Promise(setImmediate);
  assert.equal(calls, 2);
});
