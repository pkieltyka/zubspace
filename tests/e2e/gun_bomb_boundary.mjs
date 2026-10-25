#!/usr/bin/env node
import assert from 'node:assert/strict';
import {
  createSession, deleteSession, firefoxArtifactClient, firefoxKeyDown, firefoxKeyUp,
  freePort, holdFirefoxKeys, metricsState, openClient, sleep, startGeckodriver,
  startRealServer, startStaticServer, stopProcess, waitForGeckodriver, waitForServerListening,
} from '../../tools/browser_firefox_rtc_smoke.mjs';
import { logLines } from '../../tools/log_chunks.mjs';
import { serverLogRecord } from '../../tools/server_log_fields.mjs';
import { parseZubspaceEvents } from '../../tools/server_events.mjs';
import {
  brakeClientVelocity, browserE2eFailureFooter, driveClientToWorldTarget, exactLiveGameplayScope,
  orientClientForProtocolDirection, waitUntil,
} from './combat_replay.mjs';
import { createFocusedBrowserEvidence } from './focused_browser_evidence.mjs';
import { createWeaponLifecycleFixture } from './weapon_lifecycle_fixture.mjs';

// Firefox's ordinary fixed-step keyboard lane produces both23 and24/25-tick
// gun intervals. Hold Tab during the gun's cooldown so the client, rather than
// a guessed DevTools delay, chooses its first legal bomb step. Every attempt
// must be accepted without rejection/correction; only an exact23-tick pair
// establishes the regression witness. No clock, packet or game-state injection.
async function main() {
  const evidence = createFocusedBrowserEvidence({e2eScript:'tests/e2e/gun_bomb_boundary.mjs'});
  const fixture = createWeaponLifecycleFixture({spawnRegion:{x:512,y:512,radius:2}});
  const port = await freePort();
  const server = startRealServer(port,{botCount:0,zoneRoot:fixture.root,env:{
    ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES:'1',ZUBSPACE_SERVER_LOG_FORMAT:'json',
    ZUBSPACE_SERVER_SIM_OBSERVE:'1',
  }});
  const driver = startGeckodriver(await freePort(),'gun-bomb-boundary');
  let staticServer,actor,url,scope; const attempts = []; let witness = null;
  const state = async () => (await actor.metricsState()).uiMetrics;
  const identity = m => JSON.stringify([m.localGameplayState?.authorityScopeServerSessionId,
    m.localGameplayState?.authorityScopeArenaStreamId,m.selfPlayerId,m.playerLifeSeq]);
  const read = async () => {
    const m = await state();
    assert(exactLiveGameplayScope(m),'same live authoritative player required');
    assert.equal(identity(m),scope,'the encounter must retain its exact session/arena/life');
    assert.equal(m.playerShip,0,'this is the production Warbird boundary');
    assert.equal(m.selfCorrectionCount,0,'a weapon must not cause a movement correction');
    return m;
  };
  const capture = () => ({server,serverPort:port,staticServer,urls:url?[url]:[],
    clients:actor?[{label:'pilot',client:actor}]:[],
    extraLogs:{fixture:JSON.stringify(fixture),attempts:JSON.stringify(attempts),'browser':driver.logs},
    disconnect:async()=>{await actor?.evaluate('window.__zubspaceSendDisconnectForTest?.() ?? true').catch(()=>{});}});
  const weaponOutcomes = chunks => [...logLines(chunks)].filter(line=>line.includes('server weapon '))
    .map(line=>serverLogRecord(line).fields).filter(f=>f&&
      ['server weapon accepted','server weapon rejected'].includes(f.message));
  try {
    await waitForServerListening(port,server.child,server.logs);
    staticServer = await startStaticServer({rtcOfferUrl:`http://127.0.0.1:${port}/v1/rtc/offer`});
    await waitForGeckodriver(driver); await createSession(driver);
    url=`http://127.0.0.1:${staticServer.port}/?arena=chaos-zone&name=BoundaryPilot&trace=1&packetlog=1`;
    await openClient(driver,url);
    // The shared Firefox helper maps movement/modifier names. Tab uses the
    // standard WebDriver private-use value, as in the spectator browser gate.
    const keyValue=key=>key==='Tab'?'\uE004':key;
    actor={...firefoxArtifactClient(driver),metricsState:()=>metricsState(driver),
      keyDown:key=>firefoxKeyDown(driver,keyValue(key)),keyUp:key=>firefoxKeyUp(driver,keyValue(key)),
      press:(key,ms)=>holdFirefoxKeys(driver,[keyValue(key)],ms),
      hold:(key,ms)=>holdFirefoxKeys(driver,[keyValue(key)],ms)};
    await waitUntil('initial live Warbird authority',async()=>exactLiveGameplayScope(await state()),60000);
    scope=identity(await state()); const playerId=(await read()).selfPlayerId;
    const deadline=Date.now()+180000;
    const budget=maximum=>{
      const remaining=deadline-Date.now();
      assert(remaining>0,'the3-minute candidate window ended before an exact witness');
      return Math.min(maximum,remaining);
    };
    for(let attempt=0;attempt<24&&!witness;attempt++) {
      const stopped=await brakeClientVelocity(actor,budget(8000));assert(stopped.braked);
      const parked=await read();
      if(Math.hypot(parked.playerX-8200,parked.playerY-8200)>48) {
        const staged=await driveClientToWorldTarget(actor,{x:8200,y:8200},budget(15000),true,8,null,80);
        assert(staged.reached,'recoil drift must stay inside the declared clear patch');
      }
      const aimed=await orientClientForProtocolDirection(actor,1,0);assert(aimed.oriented);
      await waitUntil('full native/client energy and expired bomb cooldown',async()=>{
        const m=await read();return m.combatViewTimeline?.weaponReady===true&&!m.inSafeZone&&
          m.playerEnergy>=m.playerMaxEnergy&&m.serverPlayerEnergy>=m.playerMaxEnergy&&m.bombCooldown<=0;
      },budget(30000));
      // Vary the phase through genuine waiting only; all attempts and intervals
      // are retained, and any rejection fails immediately rather than retrying.
      await sleep((attempt%7)*13);
      const offset=server.logs.length;
      const entry={attempt:attempt+1,before:await read()};attempts.push(entry);
      await actor.press('Control',35);
      await actor.hold('Tab',850);
      await waitUntil('one native gun and bomb outcome',async()=>{
        const outcomes=weaponOutcomes(server.logs.slice(offset)).filter(f=>f.player_id===playerId);
        return outcomes.some(f=>f.message==='server weapon rejected')||outcomes.length>=2;
      },budget(5000));
      entry.outcomes=weaponOutcomes(server.logs.slice(offset)).filter(f=>f.player_id===playerId);
      assert(entry.outcomes.every(f=>f.message==='server weapon accepted'),JSON.stringify(entry.outcomes));
      assert.equal(entry.outcomes.length,2,'one actual gun followed by one actual bomb');
      const [gun,bomb]=entry.outcomes;
      assert([1,2].includes(gun.accepted_weapon_type));assert([3,4].includes(bomb.accepted_weapon_type));
      entry.gap100HzTicks=(bomb.server_tick-gun.server_tick)>>>0;
      assert(entry.gap100HzTicks>=23,'no shot may bypass the reference deadline');
      const spawned=parseZubspaceEvents(server.logs.slice(offset)).map(e=>e.event)
        .filter(e=>e.type==='server_projectile_spawned'&&e.owner_id===playerId);
      entry.projectiles=[gun,bomb].map(shot=>{
        const p=spawned.find(e=>e.server_tick===shot.server_tick&&e.weapon===shot.accepted_weapon);
        assert(p,'accepted weapon needs its actual native projectile');
        assert.equal(p.owner_life_seq,entry.before.playerLifeSeq);return p;
      });
      const braked=await brakeClientVelocity(actor,budget(8000));assert(braked.braked);
      await sleep(1200);entry.after=await read();
      assert.notEqual(entry.after.latestSnapshotId,entry.before.latestSnapshotId,'authority must continue after recoil');
      if(entry.gap100HzTicks===23)witness=entry;
    }
    assert(witness,'no exact23-tick native gun-to-bomb pair captured within24 genuine-key attempts');
    const result=await evidence.succeed({ok:true,browser:'firefox',arena:'chaos-zone',attempts,witness,
      scope:'exact same-life human Warbird gun-to-bomb23-tick admission and continued authority without correction; native rejected-recoil proof remains separate'},capture());
    console.log(JSON.stringify({ok:result.ok,attempts:attempts.length,gap100HzTicks:witness.gap100HzTicks}));
  } catch(error) {
    const artifacts=await evidence.fail(error,capture());error.stack=`${error.stack||String(error)}\n${browserE2eFailureFooter(artifacts)}`;throw error;
  } finally {
    await deleteSession(driver).catch(()=>{});
    await Promise.all([stopProcess(driver.child),stopProcess(server.child)]).catch(()=>{});
    if(staticServer)await new Promise(resolve=>staticServer.server.close(resolve));
    // Retain the declared fixture beside its recorded hash/recipe for cold review.
  }
}
main().catch(error=>{console.error(error.stack||error);process.exitCode=1;});
