#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {repoRoot,freePort,startRealServer,startStaticServer,stopProcess,sleep,waitForServerListening} from '../../tools/browser_firefox_rtc_smoke.mjs';
import {openChromeClient,startChrome,waitForChrome,waitUntil,browserE2eFailureFooter} from './combat_replay.mjs';
import {createFocusedBrowserEvidence} from './focused_browser_evidence.mjs';
import {parseBotShots} from './spectator_bot_audio.mjs';
import {weaponBirthViewEvidence} from './weapon_birth_view_evidence.mjs';
import {createBotSpawnFixture} from './bot_spawn_fixture.mjs';

// One ordinary Warbird pursues a real, genuinely keyboard-driven pilot. No
// synthetic actor state, weapon packets, or altered combat/clock parameters.
async function main(){
  const profile=process.env.ZUBSPACE_E2E_SERVER_PROFILE||'debug';
  // This fragment needs actual nearby shots, not randomized-spawn coverage.
  // Change only the declared spawn region; combat tuning remains authored.
  const spawnFixture=createBotSpawnFixture('chaos-zone','central');
  const appRoot=path.resolve(repoRoot,process.env.ZUBSPACE_E2E_STATIC_ROOT||'.');
  const evidence=createFocusedBrowserEvidence({e2eScript:'tests/e2e/weapon_birth_view.mjs',
    serverBinary:`target/${profile}/zubspace_server`,
    wasm:process.env.ZUBSPACE_E2E_STATIC_ROOT?path.relative(repoRoot,path.join(appRoot,'zubspace.wasm')):undefined,
    clientEntry:path.relative(repoRoot,path.join(appRoot,'index.html'))});
  const port=await freePort(),staticServer=await startStaticServer();
  const server=startRealServer(port,{profile,botCount:1,zoneRoot:spawnFixture.root,
    env:{ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES:'1',ZUBSPACE_SERVER_BOT_SHIP_TYPES:'1',
    ZUBSPACE_SERVER_QA_TRACE_UPLOAD:'1',ZUBSPACE_SERVER_LOG_FORMAT:'json',
    ZUBSPACE_SERVER_LOG_FILTER:'zubspace_server::server_tick::bots=debug'}});
  const browser=startChrome(await freePort(),'weapon-birth-view');
  let client,report=null,stats=null;
  const urls=[],screenshots={},frames=new Map(),packets=new Map();
  const observation=()=>({frames:[...frames.values()].sort((a,b)=>a.monotonicMs-b.monotonicMs),
    packetLog:[...packets.values()],shots:parseBotShots(server.logs)});
  const capture=()=>({server,serverPort:port,staticServer,urls,
    clients:client?[{label:'pilot',client}]:[],extraScreenshots:screenshots,
    extraLogs:{'browser':browser.logs,'birth-observation':JSON.stringify(observation()),
      'birth-spawn-fixture':JSON.stringify(spawnFixture.evidence),
      'birth-report':JSON.stringify({report,stats})},
    disconnect:async()=>{await client?.evaluate('(window.__zubspaceSendDisconnectForTest(),true)').catch(()=>{});}});
  const collect=async()=>{
    const data=await client.evaluate(`({frames:(window.__zubspaceRemoteSpriteTrace||[]).slice(-120),
      packets:[...(window.__zubspacePacketLogFirst||[]),...(window.__zubspacePacketLog||[])].filter(p=>p.direction==='received'&&p.bytes?.[0]===125),
      stats:window.__zubspaceUiMetrics?.combatViewTimeline?.weaponView})`);
    for(const f of data.frames)frames.set(f.renderOrdinal,f);
    for(const p of data.packets)packets.set(`${p.at}/${p.bytes.join(',')}`,p);
    assert.ok(frames.size<6000&&packets.size<12000,'bounded observation window');stats=data.stats;
  };
  try{
    await waitForServerListening(port,server.child,server.logs);await waitForChrome(browser);
    const url=`http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&name=BirthPilot&password=secret&trace=1&packetlog=1`;
    urls.push(url);client=await openChromeClient(browser,url);await client.enterShip();
    await waitUntil('current observer combat clock ready',async()=>
      (await client.metricsState()).uiMetrics.combatViewTimeline?.weaponReady===true,15000);
    await client.keyDown('ArrowUp');await client.keyDown('ArrowRight');
    try{
      const start=Date.now();
      await waitUntil('moving/turning bot fresh births and terminal tails',async()=>{
        await collect();
        if(Date.now()-start<2000)return false;
        report=weaponBirthViewEvidence(observation());return report.complete;
      },35000);
    }finally{await client.keyUp('ArrowRight');await client.keyUp('ArrowUp');}
    await sleep(650);await collect();report=weaponBirthViewEvidence(observation());
    assert.equal(report.complete,true);assert.ok(stats.waitingFrames>0&&stats.historicalFrames>0&&stats.currentFrames>0);
    assert.equal(stats.effectDrops,0,'this one-bot fragment must not overflow effects');
    screenshots.completed=await client.screenshot();
    const result=await evidence.succeed({ok:true,profile,totalBots:1,ships:[1],report,stats,
      spawnFixture:spawnFixture.evidence,
      scope:report.scope},capture());
    console.log(JSON.stringify({ok:result.ok,cases:report.cases.length,exactMovingBirths:report.exactMovingBirths,
      headingBins:report.headingBins,terminalProofs:report.terminalProofs.length,stats}));
  }catch(error){const artifacts=await evidence.fail(error,capture());console.error(browserE2eFailureFooter(artifacts));throw error;}
  finally{
    await client?.close().catch(()=>{});
    await Promise.all([stopProcess(browser.child),stopProcess(server.child)]);
    fs.rmSync(browser.userDataDir,{recursive:true,force:true,maxRetries:10,retryDelay:150});
    fs.rmSync(spawnFixture.root,{recursive:true,force:true});
    await new Promise(resolve=>staticServer.server.close(resolve));
  }
}
main().catch(error=>{console.error(error.stack||error);process.exitCode=1;});
