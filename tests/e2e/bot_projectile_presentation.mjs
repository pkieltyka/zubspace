#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {repoRoot,freePort,startRealServer,startStaticServer,stopProcess,waitForServerListening} from '../../tools/browser_firefox_rtc_smoke.mjs';
import {openChromeClient,startChrome,waitForChrome,waitUntil,browserE2eFailureFooter} from './combat_replay.mjs';
import {createFocusedBrowserEvidence} from './focused_browser_evidence.mjs';
import {serverDamageEventsSince} from './bot_combat_evidence.mjs';
import {parseZubspaceEvents,parseServerDeathEvents} from '../../tools/server_events.mjs';
import {botProjectilePresentationEvidence,botProjectileObservationWindow} from './bot_projectile_presentation_evidence.mjs';
import {createBotShotReader} from './spectator_bot_audio.mjs';
import {weaponBirthViewEvidence} from './weapon_birth_view_evidence.mjs';
import {humanProjectileVisibilityEvidence} from './human_projectile_visibility_evidence.mjs';
import {createSpawnRegionFixture} from './fixture_spawn_region.mjs';

// Observe ordinary server-native combat in the unmodified shipped map from
// a declared central spawn. No injected weapons, passive bots, teleporting
// actors or changed collision settings.
async function main() {
  const appRoot=path.resolve(repoRoot,process.env.ZUBSPACE_E2E_STATIC_ROOT||'.');
  const minimumObservationMs=Number(process.env.ZUBSPACE_E2E_PROJECTILE_MIN_OBSERVATION_MS||0);
  assert.ok(Number.isSafeInteger(minimumObservationMs)&&minimumObservationMs>=0&&minimumObservationMs<=60000,
    'projectile observation extension must be0–60000ms within the unchanged90s budget');
  const evidence=createFocusedBrowserEvidence({e2eScript:'tests/e2e/bot_projectile_presentation.mjs',
    wasm:process.env.ZUBSPACE_E2E_STATIC_ROOT?path.relative(repoRoot,path.join(appRoot,'zubspace.wasm')):undefined,
    clientEntry:path.relative(repoRoot,path.join(appRoot,'index.html'))});
  // This is a contact/presentation audit, not a spawn-distribution test.
  // Random spawns can leave its stationary human behind terrain, outside
  // every firing encounter for the entire observation. Declare the origin
  // for each real lifecycle instead; map, bots and balance stay unchanged.
  const spawnFixture=createSpawnRegionFixture({zoneId:'chaos-zone',spawnRegion:{x:512,y:512,radius:0}});
  const staticServer=await startStaticServer(), port=await freePort();
  const server=startRealServer(port,{botCount:15,zoneRoot:spawnFixture.root,env:{ZUBSPACE_SERVER_BOT_SHIP_TYPES:'1,4,5,8',
    ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES:'1',
    ZUBSPACE_SERVER_LOG_FORMAT:'json',ZUBSPACE_SERVER_QA_TRACE_UPLOAD:'1',
    ZUBSPACE_SERVER_LOG_FILTER:'zubspace_server::server_tick::bots=debug',
    ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT:'true',
    ZUBSPACE_SERVER_SIGNALING_ALLOWED_ORIGINS:`http://127.0.0.1:${staticServer.port}`}});
  const graphicsBackend=process.env.ZUBSPACE_E2E_PROJECTILE_GRAPHICS_BACKEND||'swiftshader';
  const graphics={requested:graphicsBackend,renderers:[]};
  const browsers=[startChrome(await freePort(),'terminal-pilot',{graphicsBackend}),startChrome(await freePort(),'terminal-spectator',{graphicsBackend})];
  const clients=[],urls=[],reports={},screenshots={};
  let observationWindows=null,humanVisibility=null;
  let humanStartTick=null,lastHumanAuditAt=0;
  const localDamage=new Map(),captureReads=[];
  const birthReports={};
  const readBotShots=createBotShotReader(server.logs);
  const retained=[];
  const killNotices={pilot:null,spectator:null};
  const capture=()=>({server,serverPort:port,staticServer,urls,
    clients:clients.map((client,i)=>({label:i?'spectator':'pilot',client})),extraScreenshots:screenshots,
    extraLogs:{'spawn-fixture':JSON.stringify(spawnFixture,null,2),
      'presentation-evidence':JSON.stringify(reports,null,2),
      'kill-notices':JSON.stringify(killNotices,null,2),
      'birth-evidence':JSON.stringify(birthReports,null,2),
      'human-visibility':JSON.stringify(humanVisibility,null,2),
      'local-damage':JSON.stringify([...localDamage.values()]),
      'capture-reads':JSON.stringify(captureReads),
      'graphics':JSON.stringify(graphics),
      'observation-windows':JSON.stringify(retained.map(r=>({
        remoteSpriteTrace:[...r.frames.values()].sort((a,b)=>a.at-b.at),packetLog:[...r.packets.values()]}))),
      'pilot-browser':browsers[0].logs,'spectator-browser':browsers[1].logs},
    disconnect:async()=>{for(const c of clients)await c.evaluate('window.__zubspaceSendDisconnectForTest?.(); true').catch(()=>{});}});
  try {
    await waitForServerListening(port,server.child,server.logs);
    await Promise.all(browsers.map(waitForChrome));
    for(let i=0;i<2;i++){
      const url=`http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&name=${i?'TermWatch':'TermPilot'}&password=secret&trace=1&packetlog=1`;
      urls.push(url);const client=await openChromeClient(browsers[i],url);clients.push(client);await client.enterShip();
      const renderer=await client.evaluateAwait(`(()=>{const g=document.querySelector('#glcanvas').getContext('webgl2');
        const e=g.getExtension('WEBGL_debug_renderer_info');if(!e)throw new Error('renderer identity unavailable');
        return g.getParameter(e.UNMASKED_RENDERER_WEBGL)})()`);
      graphics.renderers.push(renderer);
      if(graphicsBackend!=='swiftshader')assert.ok(/AMD|NVIDIA|Intel/.test(renderer)&&
        !/swiftshader|llvmpipe|lavapipe|software/i.test(renderer),'actual hardware game renderer required');
    }
    const observer=clients[1];
    await waitUntil('spectator entry at full live energy',async()=>{
      const m=(await observer.metricsState()).uiMetrics;
      return m.playerAlive&&m.serverPlayerAlive&&m.playerEnergy>=m.playerMaxEnergy&&m.serverPlayerEnergy>=m.playerMaxEnergy&&m.localGameplayState?.authorityPhase==='Live';
    },20000);
    await observer.press('Escape');await observer.press('s');
    await waitUntil('free spectator admitted',async()=>(await observer.metricsState()).uiMetrics.localGameplayState?.observerReady===true,10000);
    const noticeAfterTick=(await observer.metricsState()).uiMetrics.latestServerTick;
    const observationStartAt=Date.now();
    let failure=null;
    for(const _ of clients)retained.push({frames:new Map(),packets:new Map(),cases:new Map(),lastOrdinal:-1,lastPacketAt:0});
    await waitUntil('both views retire exact bot collision groups, including linked and lethal shots',async()=>{
      try{
        await clients[0].press('Control',75);
        const damage=serverDamageEventsSince(server);
        const readStarted=Date.now();
        const snapshots=await Promise.all(clients.map((client,i)=>client.evaluateAwait(`(()=>{
          const since=Math.max(${retained[i].lastPacketAt},${observationStartAt});
          const ordinal=${retained[i].lastOrdinal};
          const packets=[...(window.__zubspacePacketLogFirst||[]),...(window.__zubspacePacketLog||[])].filter(e=>e.at>=since&&e.direction==='received'&&e.bytes?.[0]===125&&(e.bytes?.[2]===1||(e.bytes?.[2]===4&&(e.bytes[105]&4)))&&((e.bytes[27]|(e.bytes[28]<<8))>=768));
          const frames=(window.__zubspaceRemoteSpriteTrace||[]).filter(f=>f.renderOrdinal>ordinal&&f.at>=${observationStartAt}).map(f=>({...f,
            entries:f.entries.map(p=>({id:p.id,sourceSessionId:p.sourceSessionId,lifeSeq:p.lifeSeq,ship:p.ship,drawn:p.drawn,renderedX:p.renderedX,renderedY:p.renderedY,authoritativeX:p.authoritativeX,authoritativeY:p.authoritativeY,displayOrientation:p.displayOrientation,authoritativeOrientation:p.authoritativeOrientation,shipFrame:p.shipFrame})),
            projectiles:{...f.projectiles,entries:f.projectiles.entries.filter(p=>p.sourcePlayerId>=768)}}));
          const m=window.__zubspaceUiMetrics||{};
          return {packetLog:packets,remoteSpriteTrace:frames,notificationMessages:m.notificationMessages,
            names:(m.scoreboardPlayers||[]).map(p=>[p.id,p.name]),killNoticeCount:m.combatKillNotificationCount,
            selfPlayerId:m.selfPlayerId,serverTick:m.latestServerTick,damageTrace:window.__zubspaceDamageTrace||[]};
        })()`)));
        captureReads.push({at:readStarted,elapsedMs:Date.now()-readStarted,frames:snapshots.map(s=>s.remoteSpriteTrace.length)});
        for(const hit of snapshots[0].damageTrace)localDamage.set(`${hit.damageSeq}/${hit.projectileId}`,hit);
        for(let i=0;i<snapshots.length;i++){
          for(const frame of snapshots[i].remoteSpriteTrace)retained[i].frames.set(frame.at,frame);
          for(const packet of snapshots[i].packetLog)retained[i].packets.set(`${packet.at}/${packet.bytes.join(',')}`,packet);
          if(snapshots[i].remoteSpriteTrace.length)retained[i].lastOrdinal=snapshots[i].remoteSpriteTrace.at(-1).renderOrdinal;
          if(snapshots[i].packetLog.length)retained[i].lastPacketAt=Math.max(...snapshots[i].packetLog.map(p=>p.at))-1;
        }
        const deaths=parseServerDeathEvents(server.logs).filter(d=>
          d.serverTick>noticeAfterTick&&d.killerId>=768&&d.victimId>=768&&d.killerId!==d.victimId);
        for(let i=0;i<2;i++){
          const label=i?'spectator':'pilot',snapshot=snapshots[i],names=new Map(snapshot.names);
          if(killNotices[label])continue;
          for(const death of deaths){
            if(!names.has(death.killerId)||!names.has(death.victimId))continue;
            const message=`${names.get(death.victimId)}(${death.killedBounty}) killed by: ${names.get(death.killerId)}`;
            if(snapshot.killNoticeCount>0&&snapshot.notificationMessages?.filter(s=>s===message).length===1){
              killNotices[label]={death,message,count:snapshot.killNoticeCount};break;
            }
          }
        }
        const sampledAt=Date.now();
        const pilotFrames=[...retained[0].frames.values()];
        if(humanStartTick===null&&pilotFrames.length)humanStartTick=(pilotFrames[0].serverTick+35)&0x7fffffff;
        if(humanStartTick!==null&&sampledAt-lastHumanAuditAt>=1000){
          const events=parseZubspaceEvents(server.logs).map(e=>e.event);
          humanVisibility=humanProjectileVisibilityEvidence({playerId:snapshots[0].selfPlayerId,
            startTick:humanStartTick,endTick:(snapshots[0].serverTick-100)&0x7fffffff,
            frames:pilotFrames,damageTrace:[...localDamage.values()],damage,shotCommits:readBotShots(),
            collisionWitnesses:events.filter(e=>e.type==='server_collision_witness'),
            respawns:events.filter(e=>e.type==='server_player_respawned')});
          lastHumanAuditAt=sampledAt;
        }
        const shots=readBotShots();
        for(let i=0;i<2;i++){
          const frames=[...retained[i].frames.values()].sort((a,b)=>a.monotonicMs-b.monotonicMs);
          if(frames.length>=30)birthReports[i?'spectator':'pilot']=weaponBirthViewEvidence({frames,
            packetLog:[...retained[i].packets.values()],shots});
        }
        const prelim=retained.map(r=>botProjectilePresentationEvidence({
          state:botProjectileObservationWindow(r,sampledAt),damage,requireGeometry:false}));
        for(let i=0;i<2;i++){
          for(const item of prelim[i].cases)retained[i].cases.set(item.terminal.eventId,item);
          const cases=[...retained[i].cases.values()];
          reports[i?'spectator':'pilot']={cases:cases.length,ships:[...new Set(cases.map(c=>c.sourceShip))],linked:cases.filter(c=>c.sourceShip===4&&c.maxLinkedDrawn>=2).length,lethal:cases.filter(c=>c.hit.lethal).length,skipped:prelim[i].skipped};
        }
        if(!Object.values(killNotices).every(Boolean)||!Object.values(reports).every(r=>r.cases>=4&&r.ships.length>=2&&r.linked>0&&r.lethal>0)||
          !['pilot','spectator'].every(label=>birthReports[label]?.complete===true))return false;
        observationWindows=retained.map(r=>({remoteSpriteTrace:[...r.frames.values()].sort((a,b)=>a.at-b.at),packetLog:[...r.packets.values()]}));
        const collisionWitnesses=parseZubspaceEvents(server.logs).map(e=>e.event).filter(e=>e.type==='server_collision_witness');
        for(let i=0;i<2;i++)reports[i?'spectator':'pilot']=botProjectilePresentationEvidence({state:observationWindows[i],damage,collisionWitnesses,requireOutsideLegacyCircle:true});
        return Object.values(reports).every(r=>r.complete)&&humanVisibility?.complete&&
          sampledAt-observationStartAt>=minimumObservationMs;
      }catch(error){failure=error;return true;}
    },90000);
    if(failure)throw failure;
    for(let i=0;i<2;i++)screenshots[i?'spectator':'pilot']=await clients[i].screenshot();
    for(const report of Object.values(reports)){assert.ok(report.complete);assert.ok(report.geometryCorrelated);}
    const result=await evidence.succeed({ok:true,totalBots:15,ships:[1,4,5,8],reports,killNotices,birthReports,humanVisibility,graphics,minimumObservationMs,spawnFixture,
      scope:'exact local-human hit draw audit plus drawn bot bullets and victims before exact native collisions; consumed linked groups absent for a complete half-second tail in player and free-spectator views; multiple ship types and lethal hits; both views include a real box contact outside the former point/circle hull plus moving/turning Warbird birth alignment and convergence'},capture());
    console.log(JSON.stringify({ok:result.ok,reports:Object.fromEntries(Object.entries(reports).map(([k,v])=>[k,{cases:v.cases.length,ships:v.shipTypes,linked:v.linkedCases,lethal:v.lethalCases}]))}));
  }catch(error){const artifacts=await evidence.fail(error,capture());console.error(browserE2eFailureFooter(artifacts));throw error;}
  finally{
    for(const c of clients)await c.close().catch(()=>{});
    await Promise.all([...browsers.map(b=>stopProcess(b.child)),stopProcess(server.child)]).catch(()=>{});
    for(const b of browsers)fs.rmSync(b.userDataDir,{recursive:true,force:true,maxRetries:10,retryDelay:150});
    staticServer.server.close();
    fs.rmSync(spawnFixture.root,{recursive:true,force:true});
  }
}
main().catch(error=>{console.error(error.stack||error);process.exitCode=1;});
