#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {repoRoot,freePort,startRealServer,startStaticServer,stopProcess,waitForServerListening,sleep} from '../../tools/browser_firefox_rtc_smoke.mjs';
import {openChromeClient,startChrome,waitForChrome,waitUntil,orientClientForProtocolDirection,browserE2eFailureFooter} from './combat_replay.mjs';
import {createFocusedBrowserEvidence} from './focused_browser_evidence.mjs';
import {createSpawnRegionFixture} from './fixture_spawn_region.mjs';
import {authoredMapGeometry} from './authored_map_geometry.mjs';
import {parseZubspaceEvents,parseServerDamageEvents} from '../../tools/server_events.mjs';
import {serverLogRecord} from '../../tools/server_log_fields.mjs';
import {logLines} from '../../tools/log_chunks.mjs';

// Four real pilots, two authored clear gun lines, unchanged Chaos weapons.
// Only B's real outbound projectile presentation queue is held (debug-only).
function fixture() {
  const zone=path.join(repoRoot,'assets/zones/chaos-zone');
  const manifest=fs.readFileSync(path.join(zone,'zone.bundle'),'utf8');
  const mapName=manifest.match(/^map=(.+)$/m)[1].trim();
  const geometry=authoredMapGeometry(fs.readFileSync(path.join(zone,mapName)));
  const corridors=[];
  for(let y=490;y<=550;y++)for(let x=490;x<=540;x++) {
    const a={x:x*16+8,y:y*16+8},b={x:(x+12)*16+8,y:a.y};
    if(Array.from({length:25},(_,i)=>({x:a.x+i*8,y:a.y})).every(p=>
      geometry.authoredHullIsTraversable(p.x,p.y,20)&&geometry.tileAt(p.x,p.y)!==171)) {
      corridors.push([a,b]);
    }
  }
  const first=corridors[0];
  assert.ok(first,'first authored full-hull gun corridor');
  const second=corridors.find(pair=>Math.abs(pair[0].y-first[0].y)>=640);
  assert.ok(first&&second,'two independent authored full-hull gun corridors');
  const spots=[...first,...second];
  const result=createSpawnRegionFixture({zoneId:'chaos-zone',spawnRegion:{x:(first[0].x-8)/16,y:(first[0].y-8)/16,radius:0}});
  const settingsPath=path.join(result.root,'chaos-zone/arena.conf');
  let settings=fs.readFileSync(settingsPath,'utf8');
  for(let i=0;i<4;i++)for(const [field,value] of Object.entries({X:(spots[i].x-8)/16,Y:(spots[i].y-8)/16})) {
    settings=settings.replace(new RegExp(`^Team${i}-${field}=-?\\d+`,'m'),`Team${i}-${field}=${value>=512?value-1024:value}`);
  }
  fs.writeFileSync(settingsPath,settings);
  const hash=crypto.createHash('sha256').update(settings).digest('hex');
  fs.writeFileSync(path.join(result.root,'chaos-zone/zone.bundle'),manifest.replace(/^settings_sha256=.+$/m,`settings_sha256=${hash}`));
  return {...result,settingsSha256:hash,spots,description:'Unmodified map and balance; four explicit zero-radius Spawn regions on two authored gun lines'};
}

async function main() {
  const evidence=createFocusedBrowserEvidence({e2eScript:'tests/e2e/slow_client_isolation.mjs'});
  const spawn=fixture(), holdFile=path.join(spawn.root,'hold-projectiles');
  const staticServer=await startStaticServer(),port=await freePort();
  const server=startRealServer(port,{botCount:0,zoneRoot:spawn.root,env:{
    ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES:'1', ZUBSPACE_SERVER_LOG_FORMAT:'json',
    ZUBSPACE_SERVER_BOT_COUNT:'0', ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT:'true',
    ZUBSPACE_SERVER_QA_TRACE_UPLOAD:'1',ZUBSPACE_SERVER_LOG_LEVEL:'debug',
    ZUBSPACE_SERVER_TEST_PRESENTATION_HOLD_PLAYER:'IsoB',
    ZUBSPACE_SERVER_TEST_PRESENTATION_HOLD_FILE:holdFile,
    ZUBSPACE_SERVER_SIGNALING_ALLOWED_ORIGINS:`http://127.0.0.1:${staticServer.port}`,
  }});
  const browsers=[],clients=[],urls=[],shots=[],screenshots={};
  let held=null,control=null,released=null,heldStartedAt=null,releaseAt=null;
  const events=type=>parseZubspaceEvents(server.logs).map(row=>row.event).filter(event=>event.type===type);
  const damage=()=>parseServerDamageEvents(server.logs);
  const capture=()=>({server,serverPort:port,staticServer,urls,
    clients:clients.map((client,i)=>({label:`pilot-${i}`,client})),extraScreenshots:screenshots,
    extraLogs:{fixture:JSON.stringify(spawn),isolation:JSON.stringify({control,held,shots,released,heldStartedAt,releaseAt}),
      ...Object.fromEntries(browsers.map((b,i)=>[`browser-${i}`,b.logs]))},
    disconnect:async()=>{for(const client of clients)await client.evaluate('window.__zubspaceSendDisconnectForTest?.(); true').catch(()=>{});}});
  const committed=(hit)=>[...logLines(server.logs)].map(line=>serverLogRecord(line)).find(row=>
    row.message==='server projectile hit'&&row.fields?.projectile_id===hit.projectileId)?.fields;
  const witness=(hit)=>events('server_collision_witness').find(w=>w.projectile_id===hit.projectileId&&w.target_id===hit.victimId);
  try {
    await waitForServerListening(port,server.child,server.logs);
    for(let i=0;i<4;i++) {
      const browser=startChrome(await freePort(),`isolation-${i}`);browsers.push(browser);await waitForChrome(browser);
      const url=`http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&name=Iso${'ABCD'[i]}&password=secret&trace=1&packetlog=1`;
      urls.push(url);clients.push(await openChromeClient(browser,url));
    }
    const [a,b,c,d]=clients;
    const initial=await Promise.all(clients.map(async client=>(await client.metricsState()).uiMetrics));
    const ids=initial.map(m=>m.selfPlayerId),lives=initial.map(m=>m.playerLifeSeq);
    for(let i=0;i<4;i++) {
      assert.equal(ids[i],i,'fixture binds each genuine admission to its declared Spawn region');
      assert.ok(Math.hypot(initial[i].playerX-spawn.spots[i].x,initial[i].playerY-spawn.spots[i].y)<4,'exact stationary spawn');
    }
    await orientClientForProtocolDirection(a,1,0,5000);
    await orientClientForProtocolDirection(c,1,0,5000);

    async function shot(shooter,victim,sourceIndex,targetIndex) {
      const offset=damage().length;
      await shooter.press('Control',35);
      let hit;
      await waitUntil('exact genuine-key healthy bullet damage',async()=>{
        hit=damage().slice(offset).find(h=>h.shooterId===ids[sourceIndex]&&h.victimId===ids[targetIndex]);return !!hit;
      },5000);
      assert.equal(damage().slice(offset).filter(h=>h.shooterId===hit.shooterId&&h.victimId===hit.victimId).length,1);
      assert.equal(hit.shooterLifeSeq,lives[sourceIndex]);assert.equal(hit.victimLifeSeq,lives[targetIndex]);
      assert.equal(hit.damage,400,'unchanged Chaos L3 bullet damage');
      const geometry=witness(hit),commit=committed(hit);
      assert.ok(geometry&&commit,'collision and actual commit witnesses');
      const eligibleTick=Math.max(geometry.collision_tick,geometry.harmful_eligibility.projectile_harmful_open_tick??geometry.collision_tick);
      assert.ok(commit.committed_tick-eligibleTick<=4,'commit within two 60Hz server steps after presentation eligibility');
      const perspectives=[];
      for(const client of [shooter,victim]) {
        await waitUntil('matching browser damage receipt',async()=>{
          const state=await client.metricsState();return state.damageTrace.some(t=>t.damageSeq===hit.damageSeq&&t.projectileId===hit.projectileId&&t.victimId===hit.victimId);
        },5000);
        // The compact metrics reader intentionally omits the render ring.
        // Read only this exact shot, not a full page-state dump during flight.
        const frames=await client.evaluate(`(() => {
          const receipt=(window.__zubspaceDamageTrace||[]).find(t=>t.damageSeq===${hit.damageSeq}&&t.projectileId===${hit.projectileId});
          return (window.__zubspaceRemoteSpriteTrace||[]).filter(f=>f.at<=receipt.at)
            .flatMap(frame=>(frame.projectiles?.entries||[])
              .filter(p=>p.drawn&&p.sourcePlayerId===${hit.shooterId}&&p.sourceLifeSeq===${hit.shooterLifeSeq}&&String(p.shotGroupId)===${JSON.stringify(String(geometry.shot_group_id))})
              .map(p=>({at:frame.at,x:p.x,y:p.y,authorityProjectileId:p.authorityProjectileId})));
        })()`);
        assert.ok(frames.length>=2,'both perspectives submitted multiple projectile draws before damage');
        assert.ok(Math.max(...frames.map(p=>p.x))-Math.min(...frames.map(p=>p.x))>5,'healthy projectile moved while drawn');
        perspectives.push({drawnFrames:frames.length,frames});
      }
      return {hit,geometry,commit,perspectives};
    }
    control=await shot(c,d,2,3);
    await waitUntil('healthy victim recharged before measured window',async()=>{
      const m=(await d.metricsState()).uiMetrics;return m.serverPlayerEnergy>=m.playerMaxEnergy;
    },20000);
    fs.writeFileSync(holdFile,'armed');
    await a.press('Control',35);
    await waitUntil('real queued projectile waits for B presentation',async()=>{
      held=events('server_collision_witness').find(w=>w.owner_id===ids[0]&&w.target_id===ids[1]&&!w.harmful_eligibility.harmful);
      return !!held&&server.logs.join('').includes('QA projectile presentation hold started');
    },5000);
    heldStartedAt=Date.now();
    assert.equal(held.owner_life_seq,lives[0]);assert.equal(held.target_life_seq,lives[1]);
    for(let i=0;i<2;i++) {
      const result=await shot(c,d,2,3);shots.push(result);
      assert.equal(damage().filter(h=>h.victimId===ids[1]).length,0,'B cannot take unseen damage while hold remains armed');
      assert.ok(!committed({projectileId:held.projectile_id}),'blocked transaction did not commit');
    }
    if(Date.now()-heldStartedAt<2000)await sleep(2000-(Date.now()-heldStartedAt));
    assert.equal(damage().filter(h=>h.victimId===ids[1]).length,0);
    const liveBefore=await Promise.all(clients.map(async client=>(await client.metricsState()).uiMetrics));
    for(let i=0;i<4;i++)assert.equal(liveBefore[i].playerLifeSeq,lives[i],'no setup/lifecycle replacement during isolation window');
    releaseAt=Date.now();fs.unlinkSync(holdFile);
    await waitUntil('released exact held collision commits once',async()=>{
      released=damage().find(h=>h.projectileId===held.projectile_id&&h.victimId===ids[1]);return !!released;
    },5000);
    await sleep(500);
    assert.equal(damage().filter(h=>h.projectileId===held.projectile_id&&h.victimId===ids[1]).length,1);
    assert.equal(released.damage,400);
    for(const client of [a,b])await waitUntil('held shot receipt after normal presentation release',async()=>
      (await client.metricsState()).damageTrace.some(t=>t.projectileId===released.projectileId&&t.damageSeq===released.damageSeq),5000);
    const delayedPresentationDrawFrames=await b.evaluate(`(() => {
      const receipt=(window.__zubspaceDamageTrace||[]).find(t=>t.damageSeq===${released.damageSeq}&&t.projectileId===${released.projectileId});
      return (window.__zubspaceRemoteSpriteTrace||[]).filter(f=>f.at<=receipt.at&&
        f.projectiles.overflow===0&&f.projectiles.entries.some(p=>p.drawn&&p.sourcePlayerId===${ids[0]}&&
          p.sourceLifeSeq===${lives[0]}&&String(p.shotGroupId)===${JSON.stringify(String(held.shot_group_id))})).length;
    })()`);
    assert.ok(delayedPresentationDrawFrames>=2,'released victim drew the held threat before damage');
    const status=await(await fetch(`http://127.0.0.1:${port}/v1/zones/chaos-zone/status`)).json();
    assert.ok(status.damage_independent_commits_total>=2,'server counted independent commits during an actual older wait');
    const result=await evidence.succeed({ok:true,arena:'chaos-zone',browserPair:'four-chrome',
      scenario:'stalled presentation in one fight, independent genuine-key combat in another',
      control,held,shots,released,delayedPresentationDrawFrames,holdMilliseconds:releaseAt-heldStartedAt,
      scheduler:{readinessWaits:status.damage_readiness_waits_total,independentCommits:status.damage_independent_commits_total}},capture());
    console.log(JSON.stringify({ok:true,artifactDirectory:process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR??null,shots:shots.length,holdMilliseconds:releaseAt-heldStartedAt}));
  } catch(error) {
    fs.rmSync(holdFile,{force:true});
    const artifacts=await evidence.fail(error,capture());
    throw new Error(`${error.message}\n${browserE2eFailureFooter(artifacts)}`,{cause:error});
  } finally {
    fs.rmSync(holdFile,{force:true});
    await Promise.allSettled(clients.map(client=>client.close()));
    await Promise.all([...browsers.map(b=>stopProcess(b.child)),stopProcess(server.child)]);
    staticServer.server.close();
    for(const browser of browsers)fs.rmSync(browser.userDataDir,{recursive:true,force:true,maxRetries:10,retryDelay:150});
    fs.rmSync(spawn.root,{recursive:true,force:true});
  }
}
main().catch(error=>{console.error(error);process.exitCode=1;});
