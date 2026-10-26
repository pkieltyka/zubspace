import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
const context = vm.createContext({ AbortController, setTimeout, clearTimeout });
vm.runInContext(fs.readFileSync('js/zubspace_menu.js', 'utf8'), context);
const admission = context.zubspaceMenuAdmission;

function roster(players) { return {total_players:players.length,players:players.map(player=>({zone:'chaos-zone',bot:false,...player}))}; }
test('name preview shares trimmed case-insensitive ownership including bots across zones', () => {
  assert.equal(admission.rosterError(roster([{ id: 768, name: 'Ace', bot:true,zone:'extreme' }]), '  aCE '), 1);
  assert.equal(admission.rosterError(roster([{ id: 0, name: 'Other' }]), 'Ace'), null);
  assert.equal(admission.rosterError(roster([]), 'Ace'), null);
  for(const invalid of [null,[],{players:[]},{total_players:4,players:[{name:'Ace'}]}]) assert.equal(admission.rosterError(invalid,'Ace'),null);
});
test('capacity preview uses only the selected zone cap, including bots', () => {
  assert.equal(admission.rosterError(roster([]),'Ace',{total_players:22,max_players:23}),null);
  assert.equal(admission.rosterError(roster([]),'Ace',{total_players:23,max_players:23}),2);
  assert.equal(admission.rosterError({total_players:500,players:[]},'Ace'),null,'global count cannot fill a different zone');
  assert.equal(admission.rosterError(roster([{name:'Ace'}]),'ace',{total_players:23,max_players:23}),1,'name conflict takes priority');
});
test('normal rejection messages distinguish name, population and connection failure', () => {
  assert.equal(admission.message(1), 'That username is currently in use. Try another name.');
  assert.equal(admission.message(2), 'Sorry, the arena is full at this time.');
  assert.equal(admission.message(3), 'Unable to connect. Please try again.');
  assert.equal(admission.message(5), 'Use 1–14 letters, numbers, !, *, - or _, with single spaces between words.');
  for (const code of [0, -1, 6, '1', '4', '5', 'toString', '__proto__']) assert.equal(admission.message(code), null);
});

test('player name normalization matches the shared server vectors without filtering or truncating', () => {
  const policy = context.zubspacePlayerName;
  for (const row of JSON.parse(fs.readFileSync('tests/fixtures/player_names.json', 'utf8'))) {
    assert.equal(policy.normalize(row.raw), row.normalized, JSON.stringify(row.raw));
    assert.equal(policy.valid(policy.normalize(row.raw)), row.valid, JSON.stringify(row.raw));
  }
  assert.equal(admission.rosterError(roster([{name:'Ace!*'}]), '\u200b\t aCE!*\u00a0'), 1);
  const h = history(retry({code:5,name:'Old Name'}));
  assert.equal(admission.takeRetry(h,retry().server,1001).name,'Old Name');
  assert.equal(policy.valid('Ace\n'),false,'validity alone never accepts a final newline');
  const internal='Ace'+' '.repeat(100000)+'Pilot';
  assert.equal(policy.normalize(internal),internal);
  assert.equal(policy.valid(internal),false);
});
test('lookup reads the global roster and selected zone concurrently without cache or authentication', async () => {
  const calls=[];
  context.fetch=async(url,options)=>{calls.push({url,options});return {ok:true,json:async()=>url.endsWith('/players')?roster([{id:1,name:'Ace'}]):{total_players:23,max_players:23}};};
  assert.equal(await admission.check('https://server.example/v1/players','ACE','https://server.example/v1/zones/extreme'),1);
  assert.equal(calls.length,2);assert.equal(calls[0].options.signal,calls[1].options.signal);
  assert.equal(calls[0].options.cache,'no-store');assert.equal(calls[0].options.headers,undefined);
  assert.equal(await admission.check('https://server.example/v1/players','Bo','https://server.example/v1/zones/extreme'),2);
  for(const response of [null,{ok:false},{ok:true,json:async()=>{throw Error('bad JSON');}}]) {
    context.fetch=async()=>{if(!response)throw Error('offline');return response;};
    assert.equal(await admission.check('https://server.example/v1/players','Ace','https://server.example/v1/zones/extreme'),null);
  }
});

function retry(overrides = {}) { return { server: 'https://server.example/v1/rtc/offer', code: 1, name: 'Ace',
  playerId: '123456789012', ship: 4, at: 1000, ...overrides }; }
function history(record) { return { state: { other: 7, zubspaceJoinRetry: record },
  replaceState(value) { this.state = value; } }; }
test('one-shot recovery retains name, ship and private identity without storage or URL parameters', () => {
  const h = history(retry()); const result = admission.takeRetry(h, retry().server, 1001);
  assert.equal(result.playerId, '123456789012'); assert.equal(result.ship, 4); assert.equal(result.name, 'Ace');
  assert.equal(h.state.other, 7); assert.equal(h.state.zubspaceJoinRetry, undefined);
  assert.equal(admission.takeRetry(h, retry().server, 1002), null);
});
test('stale, cross-server or malformed recovery cannot drive a form and is consumed', () => {
  for (const change of [{ at: 400000 }, { at: -400000 }, { server: 'https://other.example' },
    { code: 'toString' }, { name: '' }, { name: 'a'.repeat(15) }, { name: '<script>' },
    { ship: 8 }, { ship: -1 }, { playerId: 'not-an-id' }]) {
    const h = history(retry(change));
    assert.equal(admission.takeRetry(h, retry().server, 1001), null);
    assert.equal(h.state.zubspaceJoinRetry, undefined);
  }
});
test('the inline application still parses after async form and retry integration', () => {
  const source = fs.readFileSync('index.html', 'utf8');
  for (const match of source.matchAll(/<script(?:\s[^>]*)?>([\s\S]*?)<\/script>/g)) {
    if (match[1].trim()) new vm.Script(match[1]);
  }
});

test('a stalled metadata request aborts within the existing advisory budget and drains its timer', async () => {
  const previousSet = context.setTimeout, previousClear = context.clearTimeout;
  let cleared = false;
  context.setTimeout = (callback, delay) => { assert.equal(delay, 1500); queueMicrotask(callback); return 123; };
  context.clearTimeout = handle => { assert.equal(handle, 123); cleared = true; };
  context.fetch = (_url, { signal }) => new Promise((_resolve, reject) =>
    signal.addEventListener('abort', () => reject(Error('aborted')), { once: true }));
  try { assert.equal(await admission.check('https://server.example/v1/players', 'Ace'), null); assert(cleared); }
  finally { context.setTimeout = previousSet; context.clearTimeout = previousClear; }
});

const entryHtml = fs.readFileSync('index.html', 'utf8');
test('the rejected splash recording is not requested or played by the entry menu', () => {
  assert.doesNotMatch(entryHtml, /arena-entry-sound|arena-enter\.wav|createArenaEntrySound/);
});
function joinFormFixture({name='Ace',mobile=false,dialog=false,stored=null,admissionError=null}={}) {
  const calls=[],inputActions=[],listeners={},inputListeners={},checkedNames=[],classes=new Set(); let checkResolve;
  const input={value:name,focus(){inputActions.push('focus');},select(){inputActions.push('select');},addEventListener:(key,fn)=>{inputListeners[key]=fn;},removeEventListener:key=>{delete inputListeners[key];}};
  const form={classList:{add:x=>classes.add(x),contains:x=>classes.has(x),remove:x=>classes.delete(x)},
    addEventListener:(key,fn)=>{listeners[key]=fn;},removeEventListener:key=>{delete listeners[key];}};
  const button={},text={dataset:{}};
  const window={zubspaceMenu:{mobile,active:true,welcomeControls:async()=>true},
    zubspaceMenuAdmission:{message:admission.message,recoverStoredIdentity:admission.recoverStoredIdentity,check:(_url,name)=>{checkedNames.push(name);calls.push('check');return new Promise(r=>{checkResolve=r;});}},
    addEventListener:(key,fn)=>{listeners[key]=fn;},removeEventListener:key=>{delete listeners[key];},
    location:{href:'http://127.0.0.1:7000/'},history:{replaceState(){}}};
  const document={querySelector:selector=>selector==='#player-name-form'?form:selector==='#player-name-input'?input:
    selector==='#player-name-submit'?button:selector==='#loading-identity-text'?text:selector==='dialog[open]'&&dialog?{}:null};
  const start=entryHtml.indexOf('        function askForPlayerName('),end=entryHtml.indexOf('        function confirmStoredPlayerJoin(',start);
  assert(start>=0&&end>start);
  const ctx=vm.createContext({window,document,URL,canvas:{focus(){}},sanitizePlayerName:context.zubspacePlayerName.normalize,
    nameIsValid:context.zubspacePlayerName.valid,gameServerUrl:()=>'/v1/players',updateIdentityStorage:x=>x,
    readStoredIdentity:()=>stored,showWelcome:()=>calls.push('join')});
  vm.runInContext(entryHtml.slice(start,end),ctx);
  const identity=ctx.askForPlayerName('123',name,admissionError);
  return {calls,inputActions,input,form,button,text,window,listeners,identity,checkedNames,inputListeners,finish:code=>checkResolve(code),submit:()=>listeners.submit({preventDefault(){}})};
}

test('name form prompts only empty names automatically, while explicit admission errors remain editable', async()=>{
  for (const name of ['Ace', ' Ace!* ', 'Old Name']) {
    const f=joinFormFixture({name});
    assert.deepEqual(f.inputActions,[]); assert.equal(f.input.value,name);
  }
  for (const name of ['', '\t \u00a0\u200b']) {
    const f=joinFormFixture({name});
    assert.deepEqual(f.inputActions,['focus','select']); assert.equal(f.button.disabled,true);
  }
  const rejected=joinFormFixture({name:'Ace',admissionError:1});
  assert.deepEqual(rejected.inputActions,['focus','select']);
  for (const name of ['', 'Ace']) {
    assert.deepEqual(joinFormFixture({name,mobile:true,admissionError:1}).inputActions,[]);
  }
  const returning=joinFormFixture();
  const pending=returning.submit(); await new Promise(setImmediate); returning.finish(1); await pending;
  assert.deepEqual(returning.inputActions,['focus','select']);
});

test('actual name form trims edges on submission without silently rewriting invalid or overlong names', async()=>{
  for (const name of ['Ace  Pilot','x.y','x$y','x(y)','a'.repeat(15),'Ace\u200bPilot']) {
    const f=joinFormFixture({name});
    assert.equal(f.button.disabled,true); assert.equal(f.input.value,name);
    assert.equal(f.text.textContent,admission.message(5));
    await f.submit(); assert.deepEqual(f.calls,[]);
    f.input.value='\u200b\t Ace!* \u00a0'; f.inputListeners.input({type:'input'});
    assert.equal(f.button.disabled,false);
    const pending=f.submit(); await new Promise(setImmediate); assert.equal(f.input.value,'Ace!*'); assert.deepEqual(f.checkedNames,['Ace!*']);
    f.finish(null); await pending; assert.equal((await f.identity).name,'Ace!*');
  }
  const name=' '.repeat(50)+'ABCDEFGHIJKL!*'+' '.repeat(50);
  const f=joinFormFixture({name}); assert.equal(f.button.disabled,false);
  const pending=f.submit(); await new Promise(setImmediate); f.finish(null); await pending;
  assert.equal((await f.identity).name,'ABCDEFGHIJKL!*');
  assert.match(entryHtml, /id="player-name-input"[^>]*maxlength="128"/);
});

test('legacy invalid names return an editable form and valid stored/URL names retain normalized identity', async()=>{
  const start=entryHtml.indexOf('        async function resolvePlayerIdentity(');
  const end=entryHtml.indexOf('        function installZubspaceAudioUnlockBridge(',start);
  for (const source of ['url','stored']) for (const raw of ['Old  Name','x.y','mr rogers','mr_rogers','mr-rogers','\u200b Ace!* \u00a0']) {
    const forms=[],joins=[],stored={playerId:'123456789012',name:raw};
    const scope=vm.createContext({window:{zubspaceMenu:{mobile:false},zubspaceMenuAdmission:{takeRetry:()=>null}},
      readStoredIdentity:()=>stored,queryParam:key=>key==='name'&&source==='url'?raw:null,
      validPlayerId:/^[0-9]{12}$/,randomPlayerId:()=>{throw Error('must retain identity');},
      gameServerUrl:()=>'/v1/rtc/offer',sanitizePlayerName:context.zubspacePlayerName.normalize,
      nameIsValid:context.zubspacePlayerName.valid,askForPlayerName:(...args)=>{forms.push(args);return 'editable';},
      updateIdentityStorage:value=>value,showWelcome:()=>joins.push('url'),canvas:{focus(){}},
      confirmStoredPlayerJoin:value=>{joins.push(value);return value;},Date});
    vm.runInContext(entryHtml.slice(start,end),scope); await scope.resolvePlayerIdentity();
    if (context.zubspacePlayerName.valid(context.zubspacePlayerName.normalize(raw))) { assert.equal(forms.length,0);assert.equal(joins.length,1); }
    else { assert.deepEqual(forms,[[stored.playerId,raw,5]]); assert.equal(joins.length,0); }
  }
});
test('actual join form checks metadata and ignores duplicate submissions without an entry cue', async()=>{
  const f=joinFormFixture(); const pending=f.submit(); await new Promise(setImmediate); assert.deepEqual(f.calls,['check']);
  await f.submit(); assert.deepEqual(f.calls,['check']);
  f.finish(null); await pending; assert.equal((await f.identity).name,'Ace'); assert.deepEqual(f.calls,['check','join']);
});
test('invalid names, mobile and modal interactions remain blocked; rejection retains retry', async()=>{
  for(const options of [{name:''},{mobile:true},{dialog:true}]) {
    const f=joinFormFixture(options); await f.submit(); assert.deepEqual(f.calls,[]);
  }
  const f=joinFormFixture(); const first=f.submit(); await new Promise(setImmediate); f.finish(1); await first;
  assert.deepEqual(f.calls,['check']); assert(f.form.classList.contains('visible'));
  const second=f.submit(); await new Promise(setImmediate); f.finish(null); await second; await f.identity;
  assert.deepEqual(f.calls,['check','check','join']);
});


test('release packaging does not require or copy the rejected entry effect', () => {
  assert.doesNotMatch(fs.readFileSync('tools/build_wasm_release.mjs','utf8'), /arena-enter\.wav|entrySoundPath/);
  assert.doesNotMatch(fs.readFileSync('tools/release_bundle_check.mjs','utf8'), /arena-enter\.wav/);
});

test('an advisory name collision can reach authoritative recovery only for the same stored name and identity', async () => {
  assert.equal(admission.recoverStoredIdentity(1, {name:' Ace ',playerId:'123'}, 'ACE', '123'), true);
  for (const [code, stored, name, id] of [
    [1,null,'Ace','123'],[1,{name:'Ace',playerId:'456'},'Ace','123'],
    [1,{name:'Other',playerId:'123'},'Ace','123'],[2,{name:'Ace',playerId:'123'},'Ace','123'],
  ]) assert.equal(admission.recoverStoredIdentity(code, stored, name, id), false);
  const fixture=joinFormFixture({stored:{name:'Ace',playerId:'123'}});
  const pending=fixture.submit(); await new Promise(setImmediate); fixture.finish(1);await pending;
  assert.equal((await fixture.identity).playerId,'123');
  assert.deepEqual(fixture.calls,['check','join']);
});
test('identity retry exhaustion returns a useful one-shot menu error while retaining the identity', () => {
  assert.match(admission.message(4), /previous session.*connected/);
  const result=admission.takeRetry(history(retry({code:4})),retry().server,1001);
  assert.equal(result.playerId,'123456789012');assert.equal(result.code,4);
  const source=fs.readFileSync('src/game/network/browser.rs','utf8');
  const branch=source.slice(source.indexOf('} else if packet.reason == "ZUBSPACE_ID_IN_USE"'), source.indexOf('} else if packet.reason.starts_with("ZUBSPACE_ID_INVALID")'));
  assert.match(branch, /identity_login_retry\s*\.rejected/);
  assert.ok(!branch.includes('reset_browser_player_identity'));
});
