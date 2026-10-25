#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {repoRoot,freePort,startRealServer,startStaticServer,stopProcess,sleep,waitForServerListening} from '../../tools/browser_firefox_rtc_smoke.mjs';
import {openChromeClient,startChrome,waitForChrome,waitUntil,browserE2eFailureFooter} from './combat_replay.mjs';
import {createFocusedBrowserEvidence} from './focused_browser_evidence.mjs';
import {approachLiveBotEncounter} from './bot_encounter_staging.mjs';

export function parseBotShots(logs){
  return logs.join('').split('\n').slice(0,-1).flatMap(line=>{
    if(!line.includes('"event":"bot_weapon_committed"'))return [];
    const row=JSON.parse(line);
    const session=/"owner_session_id":(\d+)/.exec(line)?.[1];
    const group=/"shot_group_id":(\d+)/.exec(line)?.[1];
    assert.ok(session&&group,'bot diagnostics require exact u64 source/group identity');
    return [{...row.fields,owner_session_id:session,shot_group_id:group,at:Date.parse(row.timestamp)}];
  });
}

// Repeated presentation audits should parse each native log chunk once.
// Keep a partial last line until its newline arrives; never infer a shot from it.
export function createBotShotReader(logs){
  let cursor=0,pending='';const shots=[];
  return ()=>{
    assert.ok(logs.length>=cursor,'append-only native log evidence required');
    while(cursor<logs.length){
      const text=pending+String(logs[cursor++]||''),end=text.lastIndexOf('\n');
      if(end<0){pending=text;continue;}
      shots.push(...parseBotShots([text.slice(0,end+1)]));pending=text.slice(end+1);
    }
    return shots;
  };
}

// One autonomous gunner and one unarmed-by-input human isolate the sound source.
// Never invent a weapon, move actors directly or relax spectator admission.
async function main(){
  const profile=process.env.ZUBSPACE_E2E_SERVER_PROFILE||'debug';
  const appRoot=path.resolve(repoRoot,process.env.ZUBSPACE_E2E_STATIC_ROOT||'.');
  const evidence=createFocusedBrowserEvidence({e2eScript:'tests/e2e/spectator_bot_audio.mjs',
    serverBinary:`target/${profile}/zubspace_server`,
    wasm:process.env.ZUBSPACE_E2E_STATIC_ROOT?path.relative(repoRoot,path.join(appRoot,'zubspace.wasm')):undefined,
    clientEntry:path.relative(repoRoot,path.join(appRoot,'index.html'))});
  const staticServer=await startStaticServer(),port=await freePort();
  const server=startRealServer(port,{profile,botCount:1,env:{ZUBSPACE_SERVER_BOT_SHIP_TYPES:'1',
    ZUBSPACE_SERVER_QA_TRACE_UPLOAD:'1',ZUBSPACE_SERVER_LOG_FORMAT:'json',
    ZUBSPACE_SERVER_LOG_FILTER:'zubspace_server::server_tick::bots=debug'}});
  const browsers=[],clients=[],urls=[],screenshots={};
  let observation={phase:'joining',playback:[],newShots:[]};
  const shots=createBotShotReader(server.logs);
  const capture=()=>({server,serverPort:port,staticServer,urls,
    clients:clients.map((client,index)=>({label:index?'observer':'pilot',client})),extraScreenshots:screenshots,
    extraLogs:{'bot-audio-observation':JSON.stringify(observation,null,2),
      ...Object.fromEntries(browsers.map((browser,index)=>[`${index}-browser`,browser.logs]))},
    disconnect:async()=>{for(const client of clients)await client.evaluate('(window.__zubspaceSendDisconnectForTest(),true)').catch(()=>{});}});
  try{
    await waitForServerListening(port,server.child,server.logs);
    // The bot is assigned to the first human. The later observer never serves
    // as the intended hostile target while it enters the full-energy menu.
    for(const name of ['SoundPilot','SoundObserver']){
      const browser=startChrome(await freePort(),name);browsers.push(browser);await waitForChrome(browser);
      const url=`http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&name=${name}&password=secret&trace=1&packetlog=1`;
      urls.push(url);const client=await openChromeClient(browser,url);clients.push(client);await client.enterShip();
    }
    const [pilot,observer]=clients, state=async()=>(await observer.metricsState()).uiMetrics;
    await waitUntil('full-energy live observer can request spectator',async()=>{
      const m=await state();return m.playerAlive&&m.serverPlayerAlive&&m.playerMaxEnergy>0&&
        m.playerEnergy>=m.playerMaxEnergy&&m.serverPlayerEnergy>=m.playerMaxEnergy&&m.localGameplayState?.authorityPhase==='Live';
    },20000);
    await observer.press('Escape',70);await observer.press('s',70);
    await waitUntil('spectator application admitted',async()=>(await state()).localGameplayState?.observerReady===true,10000);
    for(let i=0;i<4&&(await state()).localGameplayState.spectatingPlayerId!==768;i++){
      await observer.press('Control',70);await sleep(150);
    }
    await waitUntil('genuine-key follow selects the moving bot',async()=>{
      const m=await state(),bot=m.remotePlayers?.find(p=>p.id===768);
      return m.localGameplayState.spectatingPlayerId===768&&bot&&
        Math.hypot(m.playerX-bot.positionX,m.playerY-bot.positionY)<80;
    },10000);
    // Random stock spawns can leave the pilot over 600px from the bot. A
    // continuous turning circle never reaches its 320px fire range. Prepare
    // an actual encounter using normal movement before the original audio
    // observation window; do not manufacture a shot or open observer input.
    observation.phase='preparing-live-encounter';
    observation.encounter=await approachLiveBotEncounter({actor:pilot,server,
      botIds:[768],label:'followed-bot audio pilot',searchPoint:{x:8200,y:8200}});
    await observer.evaluate(`(()=>{
      const proto=window.AudioBufferSourceNode.prototype,original=proto.start,connect=proto.connect;
      window.__botFollowAudio=[];
      proto.connect=function(destination,...args){this.__observedGain=destination?.gain;return connect.call(this,destination,...args);};
      proto.start=function(...args){const result=original.apply(this,args),rows=window.__botFollowAudio;
        rows.push({at:Date.now(),running:this.context.state==='running',gain:this.__observedGain?.value??1,duration:this.buffer?.duration??0});
        if(rows.length>128)rows.shift();return result;};return true;})()`);
    const before=await state(),startedAt=Date.now();
    const firstGroup=shots().reduce((largest,s)=>BigInt(s.shot_group_id)>largest?BigInt(s.shot_group_id):largest,0n);
    Object.assign(observation,{phase:'observing',startedAt,firstGroup:firstGroup.toString()});
    await pilot.keyDown('ArrowUp');await pilot.keyDown('ArrowRight');
    let playback=[],newShots=[];
    try{
      await waitUntil('autonomous bot shot reaches the followed spectator WebAudio context',async()=>{
        const audioRows=await observer.evaluate('window.__botFollowAudio');
        playback=audioRows.filter(row=>row.at>=startedAt&&row.running&&row.gain>0&&row.duration>0);
        newShots=shots().filter(s=>s.owner_id===768&&BigInt(s.shot_group_id)>firstGroup&&s.at>=startedAt);
        Object.assign(observation,{lastObservedAt:Date.now(),audioRows,playback,newShots});
        return playback.length>0&&newShots.length>0;
      },20000);
    }finally{
      await pilot.keyUp('ArrowRight').catch(()=>{});await pilot.keyUp('ArrowUp').catch(()=>{});
    }
    const after=await state(),pilotState=(await pilot.metricsState()).uiMetrics;
    assert.equal(after.localGameplayState.spectatingPlayerId,768);
    assert.equal(after.localGameplayState.observerReady,true);
    assert.equal(after.clientInputSeq,before.clientInputSeq,'spectator audio cannot open input authority');
    assert.equal(pilotState.semanticInputCounts.primary,0,'the human must not supply the observed weapon sound');
    assert.ok(shots().every(s=>s.owner_id===768));
    Object.assign(observation,{phase:'verified',startedAt,firstGroup:firstGroup.toString(),playback,newShots,
      camera:{x:after.playerX,y:after.playerY},bot:after.remotePlayers.find(p=>p.id===768),
      observerInputSequence:after.clientInputSeq,observerReady:true,followedBot:768,humanPrimaryActions:0});
    screenshots.followedBot=await observer.screenshot();
    const result=await evidence.succeed({ok:true,arena:'chaos-zone',profile,followedBotAudio:observation,
      scope:'actual autonomous bot fire and running audible WebAudio while a genuine-key spectator follows that bot'},capture());
    console.log(JSON.stringify({ok:result.ok,shots:newShots.length,playback:playback.length}));
  }catch(error){const artifacts=await evidence.fail(error,capture());console.error(browserE2eFailureFooter(artifacts));throw error;}
  finally{
    for(const client of clients)await client.close().catch(()=>{});
    await Promise.all([...browsers.map(browser=>stopProcess(browser.child)),stopProcess(server.child)]);
    for(const browser of browsers)fs.rmSync(browser.userDataDir,{recursive:true,force:true,maxRetries:10,retryDelay:150});
    await new Promise(resolve=>staticServer.server.close(resolve));
  }
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url))main().catch(error=>{console.error(error.stack||error);process.exitCode=1;});
