// Menu-only browser regression: no WASM, RTC server, or gameplay fixture.
// Run with: node tests/e2e/controls_layout.mjs
import assert from 'node:assert/strict';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
let browser, ws;
const root = process.cwd();
const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'zubspace-controls-layout-'));
// Use the shipped menu DOM and CSS, with only the menu scripts. Starting the
// game is deliberately excluded so this can measure even very small windows.
const html = fs.readFileSync('index.html', 'utf8').replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi, '');
const probe = `
  (async () => {
    const result = {};
    try {
      localStorage.clear();
      const pending = window.zubspaceMenu.welcomeControls();
      await new Promise(resolve => setTimeout(resolve, 50));
      const dialog = document.querySelector('#controls-dialog');
      const ready = document.querySelector('#controls-ready');
      const bounds = element => {
        const r = element.getBoundingClientRect();
        return { top:r.top, bottom:r.bottom, left:r.left, right:r.right };
      };
      result.viewport = [innerWidth, innerHeight];
      result.dialog = bounds(dialog);
      result.ready = bounds(ready);
      result.scroll = [dialog.scrollHeight, dialog.clientHeight];
      const keyboard = document.querySelector('#controls-keyboard');
      result.keyboardScroll = [keyboard.scrollWidth, keyboard.clientWidth];
      result.actionVisible = !ready.hidden && getComputedStyle(ready.parentElement).display !== 'none';
      document.querySelector('#controls-close').click();
      result.closeAccepted = await pending;
      // Independent acknowledgement exercises the ready button as well.
      const welcome = window.zubspaceCreateControlsWelcome({dialog, button:ready,
        title:document.querySelector('#controls-title'), storage:() => localStorage});
      const second = welcome();
      ready.click();
      result.readyAccepted = await second;
      document.querySelector('#controls-open').click();
      result.helpOpened = dialog.open;
      document.querySelector('#controls-close').click();
      result.helpClosed = !dialog.open;
    } catch(error) { result.error = error.stack; }
    const output = document.createElement('pre');
    output.id = 'layout-result';
    output.textContent = JSON.stringify(result);
    document.body.append(output);
  })();
`;
const server = http.createServer((req, res) => {
  if (req.url === '/') {
    res.setHeader('Content-Type', 'text/html');
    res.end(html.replace('</body>', `<script src="/js/zubspace_menu.js"></script><script>${probe}</script></body>`));
    return;
  }
  const file = path.resolve(root, '.' + new URL(req.url, 'http://localhost').pathname);
  if (!file.startsWith(root + path.sep) || !fs.existsSync(file) || !fs.statSync(file).isFile()) {
    res.writeHead(404).end(); return;
  }
  if (file.endsWith('.css')) res.setHeader('Content-Type', 'text/css');
  if (file.endsWith('.js')) res.setHeader('Content-Type', 'text/javascript');
  res.end(fs.readFileSync(file));
});
try {
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  browser = spawn(process.env.CHROME || 'google-chrome', [
    '--headless', '--no-sandbox', '--disable-gpu', '--no-first-run', '--no-default-browser-check',
    '--class=zubspace-e2e', '--user-data-dir=' + profile, '--remote-debugging-port=0', 'about:blank',
  ], { stdio: 'ignore' });
  const portFile = path.join(profile, 'DevToolsActivePort');
  for (let i = 0; !fs.existsSync(portFile); i++) {
    assert.ok(i < 100, 'Chrome started'); await sleep(50);
  }
  const port = fs.readFileSync(portFile, 'utf8').split('\n')[0];
  const target = await (await fetch('http://127.0.0.1:' + port + '/json/new?about:blank', {method:'PUT'})).json();
  ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise(resolve => ws.addEventListener('open', resolve, {once:true}));
  let id = 0;
  const pending = new Map();
  ws.addEventListener('message', event => {
    const message = JSON.parse(event.data);
    if (pending.has(message.id)) {
      pending.get(message.id)(message); pending.delete(message.id);
    }
  });
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const request = ++id;
    const timer = setTimeout(() => { pending.delete(request); reject(Error('CDP timeout: ' + method)); }, 5000);
    pending.set(request, message => { clearTimeout(timer); message.error ? reject(Error(JSON.stringify(message.error))) : resolve(message.result); });
    ws.send(JSON.stringify({id:request, method, params}));
  });
  for (const [width, height] of [[1512, 982], [1512, 850], [1440, 780], [1280, 720], [1366, 600],
    [1024, 768], [900, 500], [800, 400], [600, 300], [390, 700]]) {
    await send('Emulation.setDeviceMetricsOverride', {width, height, deviceScaleFactor:1, mobile:false});
    await send('Page.navigate', {url:'http://127.0.0.1:' + server.address().port + '/'});
    let result;
    for (let i = 0; !result; i++) {
      assert.ok(i < 100, 'browser probe completed');
      await sleep(50);
      const evaluated = await send('Runtime.evaluate', {expression:"document.querySelector('#layout-result')?.textContent", returnByValue:true});
      if (evaluated.result.value) result = JSON.parse(evaluated.result.value);
    }
    assert.equal(result.error, undefined);
    assert.ok(result.actionVisible && result.closeAccepted && result.readyAccepted && result.helpOpened && result.helpClosed, JSON.stringify(result));
    assert.ok(result.scroll[0] <= result.scroll[1] + 1, 'no vertical scrolling: ' + JSON.stringify(result));
    assert.ok(result.keyboardScroll[0] <= result.keyboardScroll[1] + 1, 'whole keyboard fits: ' + JSON.stringify(result));
    for (const rect of [result.dialog, result.ready]) {
      assert.ok(rect.top >= 0 && rect.bottom <= result.viewport[1] && rect.left >= 0 && rect.right <= result.viewport[0], JSON.stringify(result));
    }
    console.log('PASS', JSON.stringify(result));
    if (width === 1440 && process.env.ZUBSPACE_CONTROLS_SCREENSHOT) {
      // Let the ordinary-help close event finish before starting a new welcome.
      await sleep(50);
      await send('Runtime.evaluate', {expression:`window.zubspaceCreateControlsWelcome({
        dialog:document.querySelector('#controls-dialog'), button:document.querySelector('#controls-ready'),
        title:document.querySelector('#controls-title'), storage:() => localStorage})(); void 0`});
      const screenshot = await send('Page.captureScreenshot', {format:'png'});
      fs.writeFileSync(process.env.ZUBSPACE_CONTROLS_SCREENSHOT, Buffer.from(screenshot.data, 'base64'));
    }
  }
} finally {
  ws?.close();
  if (browser && browser.exitCode === null) {
    browser.kill(); await new Promise(resolve => browser.once('exit', resolve));
  }
  server.closeAllConnections();
  server.close();
  fs.rmSync(profile, { recursive: true, force: true });
}
