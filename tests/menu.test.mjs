import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";
const scope = vm.createContext({});
vm.runInContext(fs.readFileSync("js/zubspace_menu.js", "utf8"), scope);
const zone = (query) => scope.zubspaceMenuZoneHint(new URLSearchParams(query));
test("public menu defaults to Chaos and preserves explicit hidden zone URLs", () => {
  assert.equal(zone(""), "chaos-zone");
  assert.equal(zone("zone=extreme-games"), "extreme");
  assert.equal(zone("zone=Extreme%20Games&arena=chaos-zone"), "extreme");
  assert.equal(zone("arena=extreme"), "extreme");
  assert.equal(zone("zone=test-zone"), "test-zone");
});
test("menu shows the cross-zone total and rejects unavailable counts", () => {
  const catalog = {total_players:6,zones:[{id:"chaos-zone",total_players:4},{id:"extreme",total_players:2}]};
  assert.deepEqual(JSON.parse(JSON.stringify(scope.zubspaceMenuPopulation(catalog))), {players:6});
  for(const invalid of [null,{}, {...catalog,total_players:-1},{...catalog,total_players:"6"},
    {...catalog,total_players:0.5},{...catalog,total_players:Infinity},{ok:true,zone:"chaos-zone",players:2,bots:4}]) {
    assert.equal(scope.zubspaceMenuPopulation(invalid),null);
  }
  assert.equal(scope.zubspaceMenuPopulation({...catalog,total_players:0}).players,0);
});
test("menu hides unlisted zones while retaining an explicit deep link", () => {
  const zones=[{id:"chaos-zone",listed:true},{id:"extreme",listed:false},{id:"custom",listed:false}];
  assert.deepEqual(JSON.parse(JSON.stringify(scope.zubspaceMenuZoneChoices(zones,"chaos-zone"))),[zones[0]]);
  assert.deepEqual(JSON.parse(JSON.stringify(scope.zubspaceMenuZoneChoices(zones,"extreme"))),zones.slice(0,2));
});

test("desktop-only join detects phones and iPads without rejecting touch laptops or small windows", () => {
  const mobile = scope.zubspaceMenuIsMobileDevice;
  assert.equal(mobile({ userAgent: "iPhone; CPU iPhone OS 18_0 like Mac OS X" }), true);
  assert.equal(mobile({ userAgent: "iPad; CPU OS 18_0 like Mac OS X" }), true);
  assert.equal(mobile({ userAgent: "Linux; Android 15; Tablet" }), true);
  assert.equal(mobile({ userAgentData: { mobile: true } }), true);
  assert.equal(mobile({ userAgent: "Macintosh; Intel Mac OS X", platform: "MacIntel", maxTouchPoints: 5 }), true);
  assert.equal(mobile({ platform: "MacIntel", maxTouchPoints: 0 }), false);
  assert.equal(mobile({ platform: "Win32", maxTouchPoints: 10, userAgentData: { mobile: false } }), false);
  assert.equal(mobile({ platform: "Linux x86_64" }), false);
});

function welcomeFixture(storage = new Map()) {
  const dialog = new EventTarget();
  dialog.dataset = {};
  dialog.open = false;
  dialog.showModal = () => { dialog.open = true; };
  dialog.close = () => { dialog.open = false; dialog.dispatchEvent(new Event('close')); };
  const button = new EventTarget();
  button.hidden = true;
  const title = { textContent: 'Flight keyboard controls', focus() {} };
  const storageAccess = typeof storage === 'function' ? storage : () => ({
    getItem: key => storage.get(key), setItem: (key, value) => storage.set(key, value),
  });
  const welcome = scope.zubspaceCreateControlsWelcome({ dialog, button, title, storage: storageAccess });
  return { dialog, button, title, welcome };
}

test('first play waits for explicit acknowledgement; repeat requests share the pending guide', async () => {
  const storage = new Map();
  const f = welcomeFixture(storage);
  let joined = false;
  const pending = f.welcome();
  pending.then(ready => { joined = ready; });
  assert.equal(f.welcome(), pending);
  assert.equal(f.title.textContent, 'Welcome, player');
  assert.equal(f.dialog.open, true);
  assert.equal(f.button.hidden, false);
  await Promise.resolve();
  assert.equal(joined, false);
  assert.equal(storage.size, 0);
  f.button.dispatchEvent(new Event('click'));
  assert.equal(await pending, true);
  assert.equal(f.dialog.open, false);
  assert.equal(f.button.hidden, true);
  assert.equal(f.title.textContent, 'Flight keyboard controls');
  assert.equal(storage.size, 0, 'OK alone must not mark a failed join as completed');
  assert.equal(f.welcome.needed(), false, 'same-page retries do not repeat the guide');
  f.welcome.markJoined();
  assert.equal(await welcomeFixture(storage).welcome(), true, 'another page skips guide after successful entry');
});

test('closing welcome cancels entry without remembering acknowledgement; Play can retry', async () => {
  const storage = new Map();
  const f = welcomeFixture(storage);
  const first = f.welcome();
  f.dialog.close();
  assert.equal(await first, false);
  assert.equal(storage.size, 0);
  const retry = f.welcome();
  assert.equal(f.dialog.open, true);
  f.button.dispatchEvent(new Event('click'));
  assert.equal(await retry, true);
});

test('blocked localStorage does not prevent entry or repeat play on the same page', async () => {
  const f = welcomeFixture(() => { throw Error('storage blocked'); });
  const pending = f.welcome();
  f.button.dispatchEvent(new Event('click'));
  assert.equal(await pending, true);
  f.welcome.markJoined();
  assert.equal(await f.welcome(), true);
  assert.equal(f.dialog.open, false);
});

test('Play submission gates actual admission and restores a cancelled form', async () => {
  const html = fs.readFileSync('index.html', 'utf8');
  let ready, submit, admissionCalls = 0;
  const form = { classList: { add() {}, contains() { return true; } },
    addEventListener(type, callback) { if (type === 'submit') submit = callback; } };
  const input = { value: '', addEventListener() {}, readOnly: false };
  const button = { disabled: false, focus() {} };
  const text = { dataset: {} };
  const elements = { '#player-name-form': form, '#player-name-input': input,
    '#player-name-submit': button, '#loading-identity-text': text };
  const gate = new Promise(resolve => { ready = resolve; });
  const context = vm.createContext({
    document: { querySelector: selector => elements[selector] ?? null },
    sanitizePlayerName: value => value.trim(), nameIsValid: value => !!value,
    gameServerUrl: path => path,
    window: { addEventListener() {}, zubspaceMenu: { active: true, mobile: false, welcomeControls: () => gate },
      zubspaceMenuAdmission: { message() {}, check() { admissionCalls++; } } },
  });
  vm.runInContext(html.slice(html.indexOf('        function askForPlayerName('),
    html.indexOf('        function confirmStoredPlayerJoin(')), context);
  context.askForPlayerName('123456789012', 'Ace');
  const submission = submit({ preventDefault() {} });
  assert.equal(admissionCalls, 0);
  assert.equal(button.disabled, true);
  assert.equal(input.readOnly, true);
  ready(false);
  await submission;
  assert.equal(admissionCalls, 0, 'no admission or runtime join on dismissal');
  assert.equal(button.disabled, false);
  assert.equal(input.readOnly, false);
  context.window.zubspaceMenu.mobile = true;
  await submit({ preventDefault() {} });
  assert.equal(admissionCalls, 0, 'mobile restriction still holds');
});

test('failed join and ordinary help do not persist first-play completion', async () => {
  const storage = new Map();
  const f = welcomeFixture(storage);
  f.welcome.markJoined();
  assert.equal(storage.size, 0, 'entering without acknowledging does not mark the guide seen');
  const pending = f.welcome();
  f.button.dispatchEvent(new Event('click'));
  await pending;
  assert.equal(storage.size, 0);
  const refreshed = welcomeFixture(storage);
  const retry = refreshed.welcome();
  assert.equal(refreshed.dialog.open, true, 'refresh after unsuccessful entry still offers welcome');
  refreshed.dialog.close();
  assert.equal(await retry, false);
});

test('first-play persistence is triggered by gameplay readiness rather than loading or admission', () => {
  const html = fs.readFileSync('index.html', 'utf8');
  const start = html.indexOf('          importObject.env.zub_lifecycle_state =');
  const end = html.indexOf('          importObject.env.zub_ui_metrics =', start);
  assert(start >= 0 && end > start);
  let marks=0, updates=0, state={arenaAssigned:true,inGameplay:false};
  const context=vm.createContext({
    importObject:{env:{}}, decoder:{decode:()=>JSON.stringify(state)},heapBytes:()=>null,
    window:{zubspaceMenu:{welcomeControls:{markJoined(){marks++;}}}},
    updateJoinProgress(){updates++;},console,
  });
  vm.runInContext(html.slice(start,end),context);
  context.importObject.env.zub_lifecycle_state(0,0);
  assert.equal(marks,0);
  state.inGameplay=true;
  context.importObject.env.zub_lifecycle_state(0,0);
  assert.equal(marks,1);
  assert.equal(updates,2);
});
