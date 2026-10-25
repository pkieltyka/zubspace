import assert from "node:assert/strict";
import test from "node:test";
import "../js/zubspace_player_telemetry.js";

function fixture() {
  let time = 0, visible = true;
  const packets = [];
  const peer = { connectionState: "connected", getStats: async () => new Map([
    ["transport", { type: "transport", selectedCandidatePairId: "selected" }],
    ["selected", { id: "selected", type: "candidate-pair", currentRoundTripTime: 0.042 }],
    ["old", { id: "old", type: "candidate-pair", nominated: true, state: "succeeded", currentRoundTripTime: 9 }],
  ]) };
  const telemetry = ZubspacePlayerTelemetry.create({ now: () => time, visible: () => visible, peer: () => peer, send: p => packets.push(p) });
  const template = new Uint8Array(54); template[0] = 0x80; template[1] = 1;
  return { telemetry, template, packets, peer, advance: ms => time += ms, visible: value => visible = value };
}

test("visible frame stalls and selected-pair RTT survive the bounded report", async () => {
  const f = fixture();
  f.telemetry.frame(10, true);
  for (let i = 0; i < 100; i++) { f.advance(16); f.telemetry.frame(10, true); }
  f.advance(100); f.telemetry.frame(12, false);
  await f.telemetry.report(f.template);
  const view = new DataView(f.packets[0].buffer);
  assert.equal(view.getUint32(22, true), 101);
  assert.equal(view.getUint32(26, true), 1);
  assert.equal(view.getUint32(30, true), 1_700_000);
  assert.equal(view.getUint32(34, true), 100_000);
  assert.equal(view.getUint32(38, true), 42_000);
  assert.equal(view.getUint32(42, true), 2);
  assert.equal(view.getUint32(46, true), 1);
});

test("hidden time is excluded and missing ICE statistics stay absent", async () => {
  const f = fixture();
  f.telemetry.frame(0, true); f.advance(3000); f.visible(false); f.telemetry.frame(0, true);
  await f.telemetry.report(f.template); assert.equal(f.packets.length, 0);
  f.visible(true); f.telemetry.frame(0, true);
  f.advance(1500); f.telemetry.frame(0, true);
  f.peer.getStats = async () => { throw new Error("stats unavailable"); };
  await f.telemetry.report(f.template);
  const view = new DataView(f.packets[0].buffer);
  assert.equal(view.getUint32(30, true), 1_500_000);
  assert.equal(view.getUint32(38, true), 0xffffffff);
});

test("a peer replaced while getStats is pending cannot emit an old report", async () => {
  let resolve;
  const f = fixture(); f.peer.getStats = () => new Promise(done => resolve = done);
  f.telemetry.frame(0, true); f.advance(1500); f.telemetry.frame(0, true);
  const pending = f.telemetry.report(f.template);
  f.peer.connectionState = "closed"; resolve(new Map()); await pending;
  assert.equal(f.packets.length, 0);
});

test("ambiguous nominated pairs cannot masquerade as a selected ICE RTT", async () => {
  const f = fixture();
  f.peer.getStats = async () => new Map([
    ["a", { type: "candidate-pair", nominated: true, state: "succeeded", currentRoundTripTime: 0.02 }],
    ["b", { type: "candidate-pair", nominated: true, state: "succeeded", currentRoundTripTime: 0.09 }],
  ]);
  f.telemetry.frame(0, true); f.advance(1500); f.telemetry.frame(0, true);
  await f.telemetry.report(f.template);
  assert.equal(new DataView(f.packets[0].buffer).getUint32(38, true), 0xffffffff);
});

test("version 2 reports carry the game channel's queue peak and this window's dropped sends", async () => {
  let sending = { bufferedPeak: 0, dropped: 7 };
  const f = fixture();
  const packets = [];
  let time = 0;
  const telemetry = ZubspacePlayerTelemetry.create({
    now: () => time, visible: () => true, peer: () => f.peer, send: p => packets.push(p),
    transport: () => sending,
  });
  const template = new Uint8Array(62); template[0] = 0x80; template[1] = 2;
  const window = async (peak, dropped) => {
    sending = { bufferedPeak: peak, dropped };
    telemetry.frame(0, true); time += 1500; telemetry.frame(0, true);
    await telemetry.report(template);
    const view = new DataView(packets.at(-1).buffer);
    return [view.getUint32(22, true), view.getUint32(54, true), view.getUint32(58, true)];
  };
  // The first window has no earlier count, so drops before it are not claimed.
  assert.deepEqual(await window(2_400, 7), [1, 2_400, 0]);
  assert.deepEqual(await window(900, 12), [1, 900, 5]);
  assert.deepEqual(await window(64 * 1024 * 1024, 12), [1, 16 * 1024 * 1024, 0]);
});

test("a template of an unknown version or length is never filled", async () => {
  const f = fixture();
  f.telemetry.frame(0, true); f.advance(1500); f.telemetry.frame(0, true);
  const wrong = new Uint8Array(62); wrong[0] = 0x80; wrong[1] = 1;
  await f.telemetry.report(wrong);
  assert.equal(f.packets.length, 0);
});

test("HUD ping samples the selected RTC route once per second and expires stale readings", async () => {
  let time = 0, visible = true, calls = 0, hang = false;
  const peer = { connectionState: 'connected', getStats() {
    calls++;
    if (hang) return new Promise(() => {});
    return Promise.resolve(new Map([
      ['transport', {type:'transport', selectedCandidatePairId:'live'}],
      ['live', {id:'live', type:'candidate-pair', currentRoundTripTime:0.0436}],
      ['old', {id:'old', type:'candidate-pair', nominated:true, state:'succeeded', currentRoundTripTime:9}],
    ]));
  }};
  const ping = ZubspacePlayerTelemetry.createPing({now:()=>time, visible:()=>visible, peer:()=>peer});
  assert.equal(ping(), -1);
  await new Promise(setImmediate);
  assert.equal(ping(), 44);
  for (let i=0;i<100;i++) ping();
  assert.equal(calls, 1);
  time=1000; ping(); await new Promise(setImmediate);
  assert.equal(calls, 2);
  hang=true; time=2000; ping(); await new Promise(setImmediate);
  assert.equal(ping(), 44);
  time=4001; assert.equal(ping(), -1, 'stalled requests cannot leave a stale good reading');
  peer.connectionState='disconnected'; assert.equal(ping(), -1);
  visible=false; assert.equal(ping(), -1);
});

test("HUD ping rejects ambiguous, missing and invalid RTT and isolates getStats failures", async () => {
  for(const rows of [[],
    [{type:'candidate-pair', nominated:true,state:'succeeded',currentRoundTripTime:0.01},
     {type:'candidate-pair', nominated:true,state:'succeeded',currentRoundTripTime:0.02}],
    ...[undefined,NaN,-1,Infinity,11].map(currentRoundTripTime=>[
      {type:'candidate-pair',nominated:true,state:'succeeded',currentRoundTripTime}]),
  ]) {
    const peer={connectionState:'connected',getStats:async()=>new Map(rows.map((row,i)=>[i,row]))};
    const ping=ZubspacePlayerTelemetry.createPing({now:()=>0,visible:()=>true,peer:()=>peer});
    ping(); await new Promise(setImmediate); assert.equal(ping(),-1);
  }
  const peer={connectionState:'connected',getStats(){throw Error('stats unavailable');}};
  const ping=ZubspacePlayerTelemetry.createPing({now:()=>0,visible:()=>true,peer:()=>peer});
  assert.equal(ping(),-1); await new Promise(setImmediate); assert.equal(ping(),-1);
});

test("HUD ping discards a sample from a replaced RTC connection", async () => {
  let resolve;
  let current={connectionState:'connected',getStats:()=>new Promise(r=>{resolve=r;})};
  const ping=ZubspacePlayerTelemetry.createPing({now:()=>0,visible:()=>true,peer:()=>current});
  ping(); await new Promise(setImmediate);
  current={connectionState:'connected',getStats:async()=>new Map()};
  resolve(new Map([['pair',{type:'candidate-pair',nominated:true,state:'succeeded',currentRoundTripTime:0.05}]]));
  await new Promise(setImmediate);
  assert.equal(ping(),-1);
});

test("local ping commands get bounded rolling RTT statistics from the HUD sampler", async () => {
  let time=0, seconds=0.04;
  const peer={connectionState:'connected',getStats:async()=>new Map([
    ['pair',{type:'candidate-pair',nominated:true,state:'succeeded',currentRoundTripTime:seconds}],
  ])};
  const ping=ZubspacePlayerTelemetry.createPing({now:()=>time,visible:()=>true,peer:()=>peer});
  assert.equal(ping.stats(),null);
  await new Promise(setImmediate);
  assert.deepEqual(ping.stats(),[40,40,40,40,0]);
  time+=1000;seconds=0.06;ping();await new Promise(setImmediate);
  assert.deepEqual(ping.stats(),[60,50,40,60,20]);
  for(let i=0;i<60;i++){time+=1000;seconds=0.05;ping();await new Promise(setImmediate);}
  assert.deepEqual(ping.stats(),[50,50,50,50,0],'history retains at most sixty samples');
  peer.connectionState='disconnected';assert.equal(ping.stats(),null);
});

test("a hung old peer cannot block ping sampling after reconnect", async () => {
  let current={connectionState:'connected',getStats:()=>new Promise(()=>{})};
  const ping=ZubspacePlayerTelemetry.createPing({now:()=>0,visible:()=>true,peer:()=>current});
  ping(); await new Promise(setImmediate);
  current={connectionState:'connected',getStats:async()=>new Map([
    ['pair',{type:'candidate-pair',nominated:true,state:'succeeded',currentRoundTripTime:0.033}],
  ])};
  assert.equal(ping(),-1);await new Promise(setImmediate);
  assert.equal(ping(),33);
});

test("a stats result delayed by a page pause cannot resurrect stale ping", async () => {
  let time=0, resolve;
  const peer={connectionState:'connected',getStats:()=>new Promise(r=>{resolve=r;})};
  const ping=ZubspacePlayerTelemetry.createPing({now:()=>time,visible:()=>true,peer:()=>peer});
  ping(); await new Promise(setImmediate);
  time=5000;
  resolve(new Map([['pair',{type:'candidate-pair',nominated:true,state:'succeeded',currentRoundTripTime:0.01}]]));
  await new Promise(setImmediate);
  assert.equal(ping(),-1);
  assert.equal(ping.stats(),null);
});

test("page visibility notices carry the published scope in the server's 0x85 framing", () => {
  const scope = new Uint8Array(16);
  const view = new DataView(scope.buffer);
  view.setBigUint64(0, 0x0102030405060708n, true);
  view.setBigUint64(8, 7n, true);
  const hidden = ZubspacePlayerTelemetry.visibilityNotice(scope, true);
  assert.deepEqual([...hidden], [0x85, 1, 8, 7, 6, 5, 4, 3, 2, 1, 7, 0, 0, 0, 0, 0, 0, 0, 1]);
  assert.equal(ZubspacePlayerTelemetry.visibilityNotice(scope, false)[18], 0);
  // No scope yet, or half of one, sends nothing rather than an unscoped notice.
  assert.equal(ZubspacePlayerTelemetry.visibilityNotice(null, true), null);
  assert.equal(ZubspacePlayerTelemetry.visibilityNotice(new Uint8Array(16), true), null);
  assert.equal(ZubspacePlayerTelemetry.visibilityNotice(scope.slice(0, 15), true), null);
  const noStream = scope.slice(); noStream.fill(0, 8);
  assert.equal(ZubspacePlayerTelemetry.visibilityNotice(noStream, true), null);
});
