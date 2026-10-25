#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {freePort,startRealServer,startStaticServer,stopProcess,waitForServerListening} from '../../tools/browser_firefox_rtc_smoke.mjs';
import {openChromeClient,startChrome,waitForChrome,waitForPlayersReady,rendezvousPlayers,
  orientClientForProtocolDirection,waitUntil,browserE2eFailureFooter} from './combat_replay.mjs';
import {loadRendezvousMap} from './player_rendezvous.mjs';
import {prepareKeyboardLifecycleOrigin} from './keyboard_lifecycle_origin.mjs';
import {createFocusedBrowserEvidence} from './focused_browser_evidence.mjs';
async function main(){
  const evidence=createFocusedBrowserEvidence({e2eScript:'tests/e2e/keyboard_origin_recovery.mjs'});
  const {geometry}=loadRendezvousMap('chaos-zone',[new URL('../../assets/zones',import.meta.url).pathname]);
  const port=await freePort(),server=startRealServer(port,{botCount:0});
  const browsers=[],clients=[],urls=[],observations={};let staticServer;
  const capture=()=>({server,serverPort:port,staticServer,urls,
    clients:clients.map((client,i)=>({label:i?'witness':'pilot',client})),
    extraLogs:{observations:JSON.stringify(observations),...Object.fromEntries(browsers.map((b,i)=>[`browser-${i}`,b.logs]))},
    disconnect:async()=>{await Promise.all(clients.map(c=>c.evaluate('window.__zubspaceSendDisconnectForTest?.();true').catch(()=>{})));}});
  try{
    await waitForServerListening(port,server.child,server.logs);staticServer=await startStaticServer();
    for(const name of ['OriginPilot','OriginWitness']){
      const b=startChrome(await freePort(),name);browsers.push(b);await waitForChrome(b);
      const url=`http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&name=${name}&trace=1&packetlog=1`;
      urls.push(url);const c=await openChromeClient(b,url);clients.push(c);await c.enterShip();
    }
    const [pilot,witness]=clients;await waitForPlayersReady(pilot,witness,server);
    observations.encounter=await rendezvousPlayers(pilot,witness,server,
      {meetingPoint:{x:7784,y:8984},stageRadiusPx:20});
    await witness.press('Escape');await witness.press('s');
    const aim=await orientClientForProtocolDirection(pilot,0,-1);assert(aim.oriented);
    const before=(await pilot.metricsState()).uiMetrics;observations.before=before;
    await pilot.hold('ArrowUp',150);
    await waitUntil('ordinary coasting reaches the retained narrow doorway margin',async()=>{
      const m=(await pilot.metricsState()).uiMetrics;
      if(m.playerLifeSeq!==before.playerLifeSeq||Math.hypot(m.playerX-7784,m.playerY-8984)>64)
        throw new Error('doorway staging left its bounded genuine-motion setup');
      if(!geometry.authoredHullIsTraversable(m.playerX,m.playerY,20)){
        observations.edge={x:m.playerX,y:m.playerY,velocityX:m.playerVelocityX,velocityY:m.playerVelocityY,
          lifeSeq:m.playerLifeSeq,authored14:geometry.authoredHullIsTraversable(m.playerX,m.playerY,14),
          authored20:false};return true;
      }return false;
    },4000);
    observations.recovery=await prepareKeyboardLifecycleOrigin({actor:pilot,server,
      playerId:before.selfPlayerId,initialLifeSeq:before.playerLifeSeq,geometry});
    assert(observations.recovery.warps.length>=1,'the real edge must exercise authoritative random Warp');
    assert.equal(observations.recovery.final.lifeSeq,before.playerLifeSeq);
    assert.equal(observations.recovery.final.energy,observations.recovery.final.maxEnergy);
    assert.equal(observations.recovery.final.serverEnergy,observations.recovery.final.maxEnergy);
    assert(geometry.authoredHullIsTraversable(observations.recovery.final.x,observations.recovery.final.y,20));
    const result=await evidence.succeed({ok:true,observations,
      scope:'genuine-key doorway approach, ordinary authoritative Warp receipt, same life, full energy and20px route margin'},capture());
    console.log(JSON.stringify({ok:result.ok,edge:observations.edge,recovery:observations.recovery}));
  }catch(error){const artifacts=await evidence.fail(error,capture());error.stack=`${error.stack||String(error)}\n${browserE2eFailureFooter(artifacts)}`;throw error;}
  finally{await Promise.all(clients.map(c=>c.close().catch(()=>{})));
    await Promise.all([...browsers.map(b=>stopProcess(b.child)),stopProcess(server.child)]).catch(()=>{});
    if(staticServer)await new Promise(resolve=>staticServer.server.close(resolve));
    for(const b of browsers)fs.rmSync(b.userDataDir,{recursive:true,force:true,maxRetries:10,retryDelay:150});}
}
main().catch(error=>{console.error(error.stack||error);process.exitCode=1;});
