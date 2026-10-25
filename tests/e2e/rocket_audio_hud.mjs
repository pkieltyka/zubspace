#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {freePort,startRealServer,startStaticServer,stopProcess,sleep,waitForServerListening} from '../../tools/browser_firefox_rtc_smoke.mjs';
import {openChromeClient,startChrome,waitForChrome,waitUntil,exactLiveGameplayScope,browserE2eFailureFooter} from './combat_replay.mjs';
import {createFocusedBrowserEvidence} from './focused_browser_evidence.mjs';

// Observe actual WebAudio calls, forwarding every call with its original args.
// Disconnection is the runtime's audible stop, even when it does not call stop().
export function installAudioWitness() {
  const proto=window.AudioBufferSourceNode.prototype, start=proto.start, stop=proto.stop;
  const connect=proto.connect, disconnect=proto.disconnect;
  const voices=new Map(), events=[], signatures=new WeakMap(); let next=1, dropped=0;
  const signature=buffer=>{
    if(!buffer)return null;if(signatures.has(buffer))return signatures.get(buffer);
    let hash=2166136261;
    for(let c=0;c<buffer.numberOfChannels;c++){
      const data=buffer.getChannelData(c),bytes=new Uint8Array(data.buffer,data.byteOffset,data.byteLength);
      for(const value of bytes)hash=Math.imul(hash^value,16777619)>>>0;
    }
    const value=[buffer.sampleRate,buffer.numberOfChannels,buffer.length,hash].join(':');
    signatures.set(buffer,value);return value;
  };
  window.__rocketExpectedAudio=async rate=>{
    const context=new OfflineAudioContext(1,1,rate),result={};
    for(const name of ['rocket1','rocket2']){
      const response=await fetch('assets/sound/'+name+'.wa2');if(!response.ok)throw Error('original Rocket asset unavailable');
      result[name]=signature(await context.decodeAudioData(await response.arrayBuffer()));
    }
    return result;
  };
  const get=node=>{
    if(!voices.has(node))voices.set(node,{id:next++,connected:false,started:false});
    return voices.get(node);
  };
  const record=(type,node)=>{
    const voice=get(node);events.push({type,id:voice.id,at:Date.now(),contextTime:node.context.currentTime,
      duration:node.buffer?.duration??0,sampleRate:node.buffer?.sampleRate,signature:signature(node.buffer),loop:node.loop,connected:voice.connected,running:node.context.state==='running',gain:voice.gain?.value??1});
    if(events.length>256){events.shift();dropped++;}
    sessionStorage.setItem('zubspace.qa.rocket-audio',JSON.stringify({events,dropped}));
  };
  proto.connect=function(destination,...args){const result=connect.call(this,destination,...args),v=get(this);
    v.connected=true;v.gain=destination?.gain;return result;};
  proto.disconnect=function(...args){const result=disconnect.apply(this,args),v=get(this);v.connected=false;
    if(v.started)record('disconnect',this);return result;};
  proto.start=function(...args){const result=start.apply(this,args),v=get(this);v.started=true;record('start',this);
    this.addEventListener('ended',()=>{v.ended=true;record('ended',this);},{once:true});return result;};
  proto.stop=function(...args){const result=stop.apply(this,args);record('stop',this);return result;};
  window.__rocketAudioWitness=()=>({events:events.slice(),dropped,voices:[...voices].filter(([,v])=>v.started).map(([node,v])=>({
    id:v.id,connected:v.connected,ended:v.ended===true,loop:node.loop,duration:node.buffer?.duration??0,
    sampleRate:node.buffer?.sampleRate,signature:signature(node.buffer),
    gain:v.gain?.value??1,running:node.context.state==='running'}))});
  return true;
}

async function main(){
  const evidence=createFocusedBrowserEvidence({e2eScript:'tests/e2e/rocket_audio_hud.mjs'});
  const port=await freePort(),staticServer=await startStaticServer();
  const server=startRealServer(port,{botCount:1,env:{ZUBSPACE_SERVER_BOT_SHIP_TYPES:'1',
    ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ:'1',ZUBSPACE_HIDDEN_PRIZE_RNG_SEED:'20'}});
  const browser=startChrome(await freePort(),'rocket-audio-hud');let actor;
  const url=`http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&name=AudioPilot&password=secret&trace=1&packetlog=1`;
  const screenshots={},checks=[],hud=[];let playback,menuPlayback;
  const state=async()=>(await actor.metricsState()).uiMetrics;
  const audio=()=>actor.evaluate('window.__rocketAudioWitness()');
  const ignitionVoice=v=>!v.loop&&Math.abs(v.duration-1.485306)<0.002;
  const sustainVoice=v=>v.loop&&Math.abs(v.duration-0.466893)<0.002;
  const rocketVoice=v=>ignitionVoice(v)||sustainVoice(v);
  const capture=()=>({server,serverPort:port,staticServer,urls:[url],clients:actor?[{label:'pilot',client:actor}]:[],
    extraScreenshots:screenshots,extraLogs:{'rocket-audio':JSON.stringify({checks,playback,menuPlayback}),
      'hud-states':JSON.stringify(hud),'browser':browser.logs},
    disconnect:async()=>{await actor?.evaluate('window.__zubspaceSendDisconnectForTest?.(); true').catch(()=>{});}});
  async function live(){await waitUntil('exact live player scope',async()=>exactLiveGameplayScope(await state()),15000);}
  async function fullEnergy(){await waitUntil('full live energy for menu role change',async()=>{const m=await state();return exactLiveGameplayScope(m)
    &&m.playerMaxEnergy>0&&m.playerEnergy>=m.playerMaxEnergy&&m.serverPlayerEnergy>=m.playerMaxEnergy;},20000);}
  async function snapshot(label){
    await waitUntil(`${label}: splash transition fully transparent`,()=>actor.evaluate(`(()=>{
      const overlay=document.querySelector('#loading-overlay');if(!overlay)return true;
      const style=getComputedStyle(overlay);return style.display==='none'||style.visibility==='hidden'||Number(style.opacity)===0;
    })()`),5000);
    const m=await state();assert.equal(m.localGameplayState.debugMode,false);
    assert.equal(m.hudLineCount,0,'normal HUD must not draw diagnostic ship/frequency/bounty rows');
    hud.push({label,metrics:m});screenshots[label]=await actor.screenshot();return m;}
  async function activate(label){const before=await state(),start=Date.now(),old=await audio();
    await actor.press('2',60);let active,voice;
    await waitUntil(`${label}: real Rocket activation and running loop`,async()=>{active=await state();const a=await audio();
      const newVoices=a.voices.filter(v=>ignitionVoice(v)&&!old.voices.some(p=>p.id===v.id));
      voice=newVoices.find(v=>v.connected&&v.running&&v.gain>0);
      return voice&&active.playerRocketTimer>0&&active.playerRockets===before.playerRockets-1
        &&active.lastSelfServerRocketLifeSeq===active.playerLifeSeq&&active.pendingRocketActivationSequence==null;},5000);
    assert.equal(active.playerLifeSeq,before.playerLifeSeq);assert(active.playerRocketTimer>8);
    const expected=await actor.evaluateAwait(`window.__rocketExpectedAudio(${voice.sampleRate})`);
    assert.equal(voice.signature,expected.rocket1,'ignition must use the original rocket1 waveform');
    const check={label,start,voiceId:voice.id,oldVoiceIds:old.voices.map(v=>v.id),expected,before,active};checks.push(check);return check;}
  async function stopped(check){await waitUntil(`${check.label}: both Rocket phases become inaudible`,async()=>{
    const a=await audio(),voices=a.voices.filter(v=>rocketVoice(v)&&!check.oldVoiceIds.includes(v.id));
    return voices.length>0&&voices.every(v=>v.ended||!v.connected);
  },3000);const a=await audio();assert.equal(a.dropped,0);
    const starts=a.events.filter(v=>v.type==='start'&&rocketVoice(v)&&!check.oldVoiceIds.includes(v.id));
    assert.equal(starts.filter(ignitionVoice).length,1,'reconciliation cannot replay ignition');
    assert(starts.filter(sustainVoice).length<=1,'sustain uses one continuing loop');
    for(const voice of starts)assert.equal(voice.signature,ignitionVoice(voice)?check.expected.rocket1:check.expected.rocket2);
    const latest=starts.find(sustainVoice)||starts.find(ignitionVoice);
    check.stop=a.events.find(v=>v.id===latest.id&&['disconnect','stop','ended'].includes(v.type));assert(check.stop);
    check.phaseStarts=starts;check.after=await state();}
  try{
    await waitForServerListening(port,server.child,server.logs);await waitForChrome(browser);
    actor=await openChromeClient(browser,url);await actor.enterShip();await live();
    await actor.evaluate(`(${installAudioWitness.toString()})()`);
    await snapshot('player-hud');
    const expiry=await activate('natural-expiry');await sleep(2200);
    const long=await audio(),ignition=long.voices.find(v=>v.id===expiry.voiceId),during=await state();
    const voice=long.voices.find(v=>sustainVoice(v)&&!expiry.oldVoiceIds.includes(v.id));
    assert(ignition.ended||!ignition.connected,'ignition must not repeat');
    assert(voice&&voice.connected&&voice.running&&voice.gain>0);assert(during.playerRocketTimer>5);
    assert.equal(voice.signature,expiry.expected.rocket2,'sustained thrust must use the original rocket2 waveform');
    const ignitionStart=long.events.find(v=>v.id===ignition.id&&v.type==='start');
    const sustainStart=long.events.find(v=>v.id===voice.id&&v.type==='start');
    const handoffGapSeconds=sustainStart.contextTime-(ignitionStart.contextTime+ignitionStart.duration);
    assert(handoffGapSeconds>=-0.025&&handoffGapSeconds<=0.08,`Rocket phase handoff gap ${handoffGapSeconds}s`);
    assert.equal(long.events.filter(v=>v.id===ignition.id&&v.type==='start').length,1);
    assert.equal(long.events.filter(v=>v.id===voice.id&&v.type==='start').length,1);
    expiry.beyondClip={at:Date.now(),ignition,voice,handoffGapSeconds,timer:during.playerRocketTimer,snapshotId:during.latestSnapshotId};
    assert(during.latestSnapshotId!==expiry.active.latestSnapshotId);
    await waitUntil('natural Rocket effect expires',async()=>(await state()).playerRocketTimer<=0,12000);
    expiry.inactiveObservedAt=Date.now();await stopped(expiry);
    assert(expiry.stop.at-expiry.inactiveObservedAt<=500);

    await fullEnergy();const ship=await activate('ship-change');
    await actor.press('Escape',60);await actor.press('2',60);
    await waitUntil('Javelin replacement life',async()=>{const m=await state();return exactLiveGameplayScope(m)
      &&m.playerShip===1&&m.playerLifeSeq>ship.active.playerLifeSeq;},15000);await stopped(ship);

    await fullEnergy();const spectate=await activate('spectator-change');
    await actor.press('Escape',60);await actor.press('s',60);
    await waitUntil('spectator application ready',async()=>(await state()).localGameplayState.observerReady===true,10000);
    await stopped(spectate);await snapshot('spectator-free-hud');
    await actor.press('Control',70);
    await waitUntil('passive Warbird followed through normal controls',async()=>{
      const m=await state(),target=m.remotePlayers?.find(p=>p.id===768);
      return m.localGameplayState.spectatingPlayerId===768&&target&&Math.hypot(m.playerX-target.positionX,m.playerY-target.positionY)<80;
    },10000);await snapshot('spectator-follow-hud');
    await actor.press('ArrowRight',150);
    await waitUntil('pan detaches to free spectator',async()=>(await state()).localGameplayState.spectatingPlayerId==null,5000);

    await actor.press('Escape',60);await actor.press('1',60);await live();
    await fullEnergy();const leave=await activate('leave-to-menu');
    await actor.press('Escape',60);await actor.press('q',60);
    await waitUntil('normal quit returns to join form',()=>actor.evaluate("document.querySelector('#player-name-form')?.classList.contains('visible')"),15000);
    menuPlayback=await actor.evaluate("JSON.parse(sessionStorage.getItem('zubspace.qa.rocket-audio'))");
    assert.equal(menuPlayback.dropped,0);leave.stop=menuPlayback.events.find(v=>v.id===leave.voiceId&&v.type==='disconnect');
    assert(leave.stop,'active loop must disconnect before the old document is unloaded');
    assert(leave.stop.at>=leave.start);assert(leave.stop.at-leave.start<5000);
    playback=menuPlayback;
    // Renew an ordinary live session for final connection/QA/cleanup evidence.
    await actor.evaluate("document.querySelector('#player-name-submit').focus(); true");await actor.press('Space',60,false);
    await actor.enterShip();await live();await snapshot('rejoined-player-hud');
    const result=await evidence.succeed({ok:true,arena:'chaos-zone',rocketAudio:checks,hudCaptures:hud.map(v=>v.label),
      scope:'original rocket1 ignition once followed by original rocket2 sustain, independently decoded waveform identities and bounded audio-clock handoff, natural expiry/ship/spectator/menu stops, no confirmation restart; normal player/free/follow HUD screenshots; native tests separately cover death/scope cuts; passive bot is a camera fixture, not combat proof'},capture());
    console.log(JSON.stringify({ok:result.ok,rocketCases:checks.length,hudCaptures:hud.map(v=>v.label)}));
  }catch(error){const artifacts=await evidence.fail(error,capture());console.error(browserE2eFailureFooter(artifacts));throw error;}
  finally{await actor?.close().catch(()=>{});await Promise.all([stopProcess(browser.child),stopProcess(server.child)]);
    fs.rmSync(browser.userDataDir,{recursive:true,force:true,maxRetries:10,retryDelay:150});await new Promise(resolve=>staticServer.server.close(resolve));}
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url))main().catch(error=>{console.error(error.stack||error);process.exitCode=1;});
