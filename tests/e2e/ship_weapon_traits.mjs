#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { repoRoot, freePort, sleep, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, waitForPlayersReady,
  driveClientToWorldTarget, brakeClientVelocity, waitForCombatViewWeaponReady,
  parseServerDamageEvents, assertSingleBrowserDamageTrace, browserE2eFailureFooter } from "./combat_replay.mjs";
import { parseRealtimeWeaponEvent } from "../../tools/realtime_event_evidence.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { createWeaponLifecycleFixture } from "./weapon_lifecycle_fixture.mjs";

const appRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
const names = ["Warbird", "Javelin", "Spider", "Leviathan", "Terrier", "Weasel", "Lancaster", "Shark"];
async function main() {
  process.env.ZUBSPACE_HIDDEN_PRIZE_RNG_SEED = "20";
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/ship_weapon_traits.mjs",
    wasm: process.env.ZUBSPACE_E2E_STATIC_ROOT ? path.relative(repoRoot, path.join(appRoot, "zubspace.wasm")) : undefined,
    clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) });
  // This private mechanic lane has fixed northbound wall/EMP stages. Its
  // origin is declared rather than inherited from shipping random spawns.
  const fixture = createWeaponLifecycleFixture({ spawnRegion: { x: 512, y: 512, radius: 0 }, clearBounds: { minX: 488, maxX: 544, minY: 488, maxY: 544 }, solidRows: [505,523] });
  const port = await freePort(), staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
  const server = startRealServer(port, { botCount: 0, zoneRoot: fixture.root, env: {
    ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1", ZUBSPACE_SERVER_LOG_FORMAT: "json", ZUBSPACE_SERVER_QA_TRACE_UPLOAD: "1",
    ZUBSPACE_SERVER_LOG_FILTER: "zubspace_server::server_tick=debug,zubspace_server::server_damage=debug,zubspace_server::arena::positions=debug",
    ZUBSPACE_SERVER_SIGNALING_ALLOWED_ORIGINS: `http://127.0.0.1:${staticServer.port}` } });
  const browsers = [], clients = [], urls = [], stages = [], screenshots = {}, phaseQa = [];
  let failureDiagnostics = null;
  const metrics = async c => (await c.metricsState()).uiMetrics;
  const damage = () => parseServerDamageEvents(server.logs);
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: i ? "witness" : "owner", client })),
    extraScreenshots: screenshots, extraLogs: { fixture: JSON.stringify(fixture, null, 2), stages: JSON.stringify(stages, null, 2),
      "phase-qa": JSON.stringify(phaseQa), "failure-diagnostics": JSON.stringify(failureDiagnostics),
      ...Object.fromEntries(browsers.map((b, i) => [`browser-${i}`, b.logs])) },
    disconnect: async () => { await Promise.all(clients.map(c => c.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); } });
  async function join(name) {
    const b = startChrome(await freePort(), `ship-trait-${name}`); browsers.push(b); await waitForChrome(b);
    const url = `http://127.0.0.1:${staticServer.port}/?arena=chaos-zone&password=secret&trace=1&packetlog=1&name=${name}`;
    urls.push(url); const c = await openChromeClient(b, url); clients.push(c); await c.enterShip(); return c;
  }
  async function ready(c, label) {
    await waitUntil(`${label}: full energy/current life`, async () => {
      const m = await metrics(c), l = m.localGameplayState;
      return m.gameplayStepReady && !l.networkPrizeBaselinePending && l.prizeAuthorityLifeSeq === m.playerLifeSeq
        && m.playerEnergy >= m.playerMaxEnergy && m.serverPlayerEnergy >= m.playerMaxEnergy && l.bombCooldown <= 0;
    }, 20_000);
    await waitForCombatViewWeaponReady(c, label);
  }
  async function select(c, key) { await c.press("Escape"); await c.press(key); }
  async function packets(c, kind) {
    return c.evaluate(`[...(window.__zubspacePacketLogFirst || []), ...(window.__zubspacePacketLog || [])]
      .filter(e => e.direction === 'received' && e.bytes?.[0] === 125 && e.bytes[1] === 2 && e.bytes[2] === ${kind}).map(e => e.bytes)`);
  }
  async function weapons(c) {
    const unique = new Map();
    for (const p of await packets(c, 1)) { const w = parseRealtimeWeaponEvent(Buffer.from(p)); if (w) unique.set(w.eventId, w); }
    return [...unique.values()];
  }
  async function states(c, group) {
    const unique = new Map();
    for (const bytes of await packets(c, 4)) {
      const p = Buffer.from(bytes); assert.ok([122,124].includes(p.length));
      assert.equal(p.length,122+(p[105]&8?2:0));
      if (p.readBigUInt64LE(41).toString() !== group) continue;
      const s = { eventId:p.readUInt32LE(19), tick:p.readUInt32LE(23), ownerId:p.readUInt16LE(27),
        session:p.readBigUInt64LE(29).toString(), life:p.readUInt32LE(37), id:p.readUInt32LE(74), index:p[82],
        weapon:p.readUInt16LE(83), x:p.readFloatLE(85), y:p.readFloatLE(89), vx:p.readFloatLE(93), vy:p.readFloatLE(97), terminal:Boolean(p[105]&4) };
      unique.set(s.eventId,s);
    }
    return [...unique.values()];
  }
  async function empProof(owner, victim, ownerId, victimId, offset) {
    let hit;
    await waitUntil("Weasel damages hostile pilot with an EMP deadline", () => {
      hit = damage().slice(offset).find(e => e.shooterId === ownerId && e.victimId === victimId && [3,4].includes(e.weapon&31));
      return hit != null && hit.empUntilTick > hit.serverTick;
    }, 6000);
    assert.equal(hit.empUntilTick-hit.serverTick, Math.floor(400*hit.unprotectedDamage/750));
    await waitUntil("victim applies the exact EMP damage/private state", async () => {
      const m = await metrics(victim);
      return m.lastSelfDamageServerSeq === hit.damageSeq && m.serverPlayerEnergy === hit.energyAfter
        && m.latestSnapshotServerTick >= hit.serverTick;
    }, 1500);
    const samples = [];
    await waitUntil("EMP pauses actual server recharge for half a second", async () => {
      const m = await metrics(victim);
      assert.ok(m.latestSnapshotServerTick < hit.empUntilTick, "observe recharge inside EMP deadline");
      assert.equal(m.serverPlayerEnergy,hit.energyAfter);
      samples.push({ tick:m.latestSnapshotServerTick, serverEnergy:m.serverPlayerEnergy, predicted:m.playerPredictedEnergy,
        x:m.playerX,y:m.playerY,vx:m.playerVelocityX,vy:m.playerVelocityY,orientation:m.playerOrientation });
      return samples.at(-1).tick-samples[0].tick >= 50;
    }, 1500);
    const before = await metrics(victim);
    await victim.press("ArrowRight",150);
    const turned = await metrics(victim);
    const turn = ((turned.playerOrientation-before.playerOrientation+1)%1);
    assert.ok(turn >= .05 && turn < .25,"EMP preserves ordinary rotation instead of Shutdown rotation");
    await victim.press("ArrowUp",100);
    const moved = await metrics(victim);
    assert.ok(Math.hypot(moved.playerVelocityX,moved.playerVelocityY)>10,"EMP preserves thrust");
    await victim.press("ArrowDown",100);
    const known = new Set((await weapons(owner)).map(w => w.eventId));
    await victim.press("Control",45);
    let counterfire;
    await waitUntil("EMP target can spend energy on an accepted shot", async () => {
      counterfire=(await weapons(owner)).find(w=>w.source===victimId&&!known.has(w.eventId)&&[1,2].includes(w.weapon&31));
      return counterfire;
    },1500);
    assert.ok(counterfire.serverTick < hit.empUntilTick,"the accepted shot must occur during EMP");
    await Promise.all(clients.map((c,i)=>assertSingleBrowserDamageTrace(c,hit,`Weasel EMP client ${i}`)));
    phaseQa.push(await victim.freezeQaTrace({upload:true}));
    await waitUntil("EMP ends and recharge resumes",async()=>{
      const m=await metrics(victim);
      return m.latestSnapshotServerTick>hit.empUntilTick+30&&m.serverPlayerEnergy>hit.energyAfter;
    },7000);
    stages.push({label:"Weasel EMP recharge/movement/fire",hit,samples,before,turned,moved,counterfire});
  }
  try {
    await waitForServerListening(port,server.child,server.logs);
    const owner=await join("traitowner"), witness=await join("traitwitness");
    await waitForPlayersReady(owner,witness,server);
    const ownerId=(await metrics(owner)).selfPlayerId,witnessId=(await metrics(witness)).selfPlayerId;
    await ready(witness,"witness initial baseline");await select(witness,"s");
    await waitUntil("free witness admitted",async()=>(await metrics(witness)).localGameplayState.observerReady,8000);
    for(let ship=0;ship<8;ship++) {
      const name=names[ship],level=ship===3?3:2,count=ship===5?0:8;
      await ready(owner,`${name} selection`);
      const previousLife=(await metrics(owner)).playerLifeSeq;
      if(ship!==0)await select(owner,String(ship+1));
      await waitUntil(`${name} exact ship baseline`,async()=>{
        const m=await metrics(owner),w=await metrics(witness);
        return m.gameplayStepReady&&m.localGameplayState.prizeAuthorityLifeSeq===m.playerLifeSeq
          &&(ship===0||m.playerLifeSeq>previousLife)&&w.remoteActorPresentation.some(p=>p.id===ownerId&&p.ship===ship);
      },12000);
      assert.equal((await metrics(owner)).playerProtocolDirection,0);
      if(ship===5) {
        await select(witness,"1");await ready(witness,"EMP victim player life");
        assert.ok((await driveClientToWorldTarget(witness,{x:8252,y:8136},12000,true,3,null,160)).reached);
        assert.ok((await brakeClientVelocity(witness,6000)).braked);
        await waitForCombatViewWeaponReady(witness,"EMP victim geometry");
        await waitForCombatViewWeaponReady(owner,"EMP firing geometry");
        const victim=await metrics(witness),shooter=await metrics(owner);
        assert.equal(victim.inSafeZone,false,"EMP victim must remain outside map safety");
        // Chaos ProximityDistance=3, raw level=1; the reference trigger
        // scale is 18 pixels (1.125 tiles), so BombSafety covers 72 pixels.
        assert.ok(Math.hypot(victim.playerX-shooter.playerX,victim.playerY-shooter.playerY)>72,
          "normal proximity BombSafety must permit firing before the victim is approached");
      }
      const known=new Set((await weapons(witness)).map(w=>w.eventId)),offset=damage().length;
      await owner.press("Tab",45);
      let source;
      await waitUntil(`${name} one accepted bomb`,async()=>{
        source=(await weapons(witness)).find(w=>w.source===ownerId&&!known.has(w.eventId)&&[3,4].includes(w.weapon&31));return source;
      },5000);
      assert.equal(source.ship,ship);assert.equal((source.weapon>>5&3)+1,level);
      assert.equal((source.weapon>>8&3)+1,level);assert.equal(source.weapon>>10&31,count);
      assert.ok(source.weapon&128,"maximum loadout owns bouncing shrapnel capability");
      let bounced=null;
      if(ship===6)await waitUntil("Lancaster reflects from north wall before later explosion",async()=>{
        bounced=(await metrics(witness)).remoteProjectileSources.find(p=>p.playerId===ownerId&&p.shotGroupId===source.shotGroupId&&p.kind==="bomb"&&p.velocityY>0);
        return bounced;
      },3000);
      if(ship===5)await empProof(owner,witness,ownerId,witnessId,offset);
      let terminal,children,all;
      await waitUntil(`${name} authoritative parent terminal and child fan`,async()=>{
        all=await states(witness,source.shotGroupId);
        terminal=all.find(s=>s.terminal&&[3,4].includes(s.weapon&31));
        children=all.filter(s=>!s.terminal&&(s.weapon&31)===15);
        return terminal&&children.length===count;
      },6000);
      assert.equal(new Set(children.map(s=>s.id)).size,count);
      assert.ok(children.every(s=>(s.weapon>>5&3)+1===level&&s.ownerId===ownerId&&s.session===source.sourceSessionId&&s.life===source.sourceLifeSeq));
      if(ship!==5) {
        assert.ok(ship===6?terminal.y<8368&&terminal.y>8350:terminal.y>=8096&&terminal.y<8112,"terminal must occur on the expected wall face");
        await waitUntil(`${name} fragments separate from explosion`,async()=>(await metrics(witness)).latestSnapshotServerTick>=terminal.tick+40,2000);
        const live=await Promise.all(clients.map(metrics));
        assert.ok(live.every(m=>m.remoteProjectileSources.some(p=>p.playerId===ownerId&&p.kind==="shrapnel"&&p.shotGroupId===source.shotGroupId)),"both owner and witness render bound fragments");
      }
      [screenshots[`${name.toLowerCase()}-owner`],screenshots[`${name.toLowerCase()}-witness`]]=await Promise.all(clients.map(c=>c.screenshot()));
      stages.push({label:`${name} bomb/fan`,ship,source,terminal,children,bounced});
      if(ship===5) {await ready(witness,"EMP recovery to spectator");await select(witness,"s");await waitUntil("witness spectator restored",async()=>(await metrics(witness)).localGameplayState.observerReady,8000);}
      console.log(JSON.stringify({ship:name,bombLevel:level,fragments:count,bounced:Boolean(bounced),emp:ship===5}));
    }
    console.log(JSON.stringify(await evidence.succeed({ok:true,fixture,ships:8,
      scope:"genuine all-eight Chaos bombs, source/child power and count, local/remote fragment screenshots, Lancaster single bounce and Weasel EMP pause with ordinary movement/fire; artwork pixels audited separately"},capture()),null,2));
  }catch(error){
    // Read the actual server ring/status after the verdict, while the peer
    // remains live. The generic artifact exporter freezes browser traces but
    // its INFO-log fallback cannot explain a retained/deferred projectile.
    failureDiagnostics = { originalError: error.message,
      status: await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`, { signal: AbortSignal.timeout(3000) }).then(r => r.json())
        .catch(e => ({ unavailable: e.message })),
      joinedTrace: clients.length ? await clients.at(-1).freezeQaTrace({ upload: true })
        .catch(e => ({ unavailable: e.message })) : { unavailable: "no live client" } };
    const artifacts=await evidence.fail(error,capture());console.error(browserE2eFailureFooter(artifacts));throw error;
  }
  finally{await Promise.all(clients.map(c=>c.close().catch(()=>{})));await Promise.all([...browsers.map(b=>stopProcess(b.child)),stopProcess(server.child)]).catch(()=>{});
    for(const b of browsers)fs.rmSync(b.userDataDir,{recursive:true,force:true,maxRetries:10,retryDelay:150});staticServer.server.close();fs.rmSync(fixture.root,{recursive:true,force:true});}
}
main().catch(error=>{console.error(error.stack||error);process.exit(1);});
