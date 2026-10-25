#!/usr/bin/env node
import { acknowledgeControlsWelcome } from "../../tools/browser_controls_welcome.mjs";
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
// Admission through real form input and ordinary RTC only. SIGKILL is an
// explicit owned-browser crash fixture, never a gameplay packet/state hook.
import assert from "node:assert/strict";
import fs from "node:fs";
import {once} from "node:events";
import {freePort,startRealServer,startStaticServer,stopProcess,waitForServerListening} from "../../tools/browser_firefox_rtc_smoke.mjs";
import {killProcessGroup,rememberOwnedXDisplays} from "../../tools/e2e_gate_hygiene.mjs";
import {openChromeClient,startChrome,waitForChrome,waitUntil,browserE2eFailureFooter} from "./combat_replay.mjs";
import {createFocusedBrowserEvidence} from "./focused_browser_evidence.mjs";
import {browserConsoleEvidence,assertRendererConsoleClean} from "../../tools/browser_console_evidence.mjs";
import {serverLogField,serverLogMessage} from "../../tools/server_log_fields.mjs";

const name="RetryPilot",ship=3,cases=[],extraLogs={},screenshots={};
const exhaustion="Your previous session is still connected. Close the other game and try again.";
const waitText="Waiting for your previous session. Retrying…";
function rows(server,message,offset=0){return server.logs.slice(offset).join("").split("\n").filter(line=>serverLogMessage(line)===message);}
async function form(evaluate,message=null,timeout=20000){
  await waitUntil("editable join form",()=>evaluate(`(() => {
    const input=document.querySelector('#player-name-input');return document.querySelector('#player-name-form')?.classList.contains('visible') &&
      !input.readOnly && ${message?`document.querySelector('#loading-identity-text').textContent===${JSON.stringify(message)}`:"true"};
  })()`),timeout);
}
async function submit(send,evaluate){
  await form(evaluate);
  const point=await evaluate(`(() => {const r=document.querySelector('.ship-option[data-ship="${ship}"]').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`);
  await send("Input.dispatchMouseEvent",{type:"mousePressed",button:"left",clickCount:1,...point});
  await send("Input.dispatchMouseEvent",{type:"mouseReleased",button:"left",clickCount:1,...point});
  await evaluate("document.querySelector('#player-name-input').focus(); document.querySelector('#player-name-input').select(); true");
  await send("Input.insertText",{text:name});
  await send("Input.dispatchKeyEvent",{type:"keyDown",key:"Enter",code:"Enter",windowsVirtualKeyCode:13});
  await send("Input.dispatchKeyEvent",{type:"keyUp",key:"Enter",code:"Enter",windowsVirtualKeyCode:13});
  await acknowledgeControlsWelcome(send, evaluate);
}
async function identity(client){return client.evaluate(`(() => {
  const input=document.querySelector('#player-name-input');
  return {id:window.__zubspaceIdentity?.playerId,name:input?.value ?? window.__zubspaceIdentity?.name,
    ship:window.zubspaceMenu.selectedShip,focused:document.activeElement?.id ?? null,
    selected:!!input && input.selectionStart===0 && input.selectionEnd===input.value.length};
})()`);}
async function waiting(client){
  await waitUntil("visible same-identity retry status",()=>client.evaluate(`
    document.querySelector('#loading-progress-text').textContent===${JSON.stringify(waitText)} &&
    document.querySelector('#loading-progress-text').getClientRects().length>0 &&
    !document.querySelector('#loading-overlay').classList.contains('hidden')`),10000);
}
async function motion(client){
  const before=(await client.metricsState()).uiMetrics;
  assert.equal(before.playerShip,ship);
  assert.equal(before.localGameplayState.gameplayStepReady,true);
  await client.keyDown("ArrowUp");
  try {await waitUntil("genuine thrust remains admitted",async()=>{
    const after=(await client.metricsState()).uiMetrics;
    return Math.hypot(after.playerX-before.playerX,after.playerY-before.playerY)>=8 && after.clientInputSeq!==before.clientInputSeq;
  },3000);} finally {await client.keyUp("ArrowUp");}
  return {playerId:before.selfPlayerId,lifeSeq:before.playerLifeSeq,ship:before.playerShip};
}
async function retain(client,label){
  const console=browserConsoleEvidence(client);assertRendererConsoleClean(console);assert.equal(console.errors,0);
  extraLogs[`${label}-console`]=JSON.stringify(console);
  extraLogs[`${label}-state`]=JSON.stringify(await client.pageState());
}

async function main(){
  const evidence=createFocusedBrowserEvidence({e2eScript:"tests/e2e/identity_rejoin.mjs"});
  const port=await freePort(),browserPort=await freePort(),staticServer=await startStaticServer();
  const server=startRealServer(port,{botCount:0,env:{ZUBSPACE_SERVER_LOG_FORMAT:"json",ZUBSPACE_SERVER_QA_TRACE_UPLOAD:"1"}});
  const browsers=[];let owner,duplicate,replacement;
  const url=`http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&menu=1&password=secret&packetlog=1&trace=1`;
  const roster=async()=>{const r=await(await fetch(`http://127.0.0.1:${port}/v1/players`)).json();assert.equal(r.total_players,r.players.length);return r.players;};
  const status=async()=>(await(await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`)).json());
  const capture=()=>({server,serverPort:port,staticServer,urls:[url],clients:[{label:"owner",client:owner},{label:"duplicate",client:duplicate},{label:"replacement",client:replacement}].filter(row=>row.client),
    extraScreenshots:screenshots,extraLogs:{...extraLogs,cases:JSON.stringify(cases),...Object.fromEntries(browsers.map((b,i)=>[`chrome-${i}`,b.logs]))},
    disconnect:async()=>{const current=replacement||owner;await current?.press("Escape",60);await current?.press("q",60);}});
  try {
    await waitForServerListening(port,server.child,server.logs);
    let browser=startChrome(browserPort,"identity-rejoin");browsers.push(browser);await waitForChrome(browser);
    let ownerSend;
    const open=options=>openChromeClient(browser,url,{beforeReady:({send,evaluate})=>{
      if(!ownerSend)ownerSend=send;
      return submit(send,evaluate);
    },...options});
    owner=await open();const original=await identity(owner);const incumbent=await motion(owner);
    assert.match(original.id,/^\d{12}$/);assert.deepEqual(await roster(),[{id:incumbent.playerId,name,zone:"chaos-zone",bot:false}]);
    const duplicateOffset=server.logs.length;
    // Separate visible windows share the actual profile identity. A hidden
    // tab suspends rAF, invalidating both the retry deadline measurement and
    // the claim that its incumbent is a healthy, responsive game client.
    const duplicateTarget=await ownerSend("Target.createTarget",{url:"about:blank",newWindow:true,width:1280,height:720});
    duplicate=await open({waitForReveal:false,targetId:duplicateTarget.targetId});await waiting(duplicate);
    assert.equal((await identity(duplicate)).id,original.id);
    assert.deepEqual(await roster(),[{id:incumbent.playerId,name,zone:"chaos-zone",bot:false}]);
    assert.deepEqual(await motion(owner),incumbent,"duplicate login must not replace incumbent life/session");
    await duplicate.focus();
    for(const client of [owner,duplicate])assert.equal(await client.evaluate("document.visibilityState"),"visible");
    const rejectedAt=Date.now();await retain(duplicate,"duplicate-wait");screenshots.duplicateWait=await duplicate.screenshot();
    await form(expression=>duplicate.evaluate(expression),exhaustion,50000);
    const exhausted=await identity(duplicate);
    for(const client of [owner,duplicate])assert.equal(await client.evaluate("document.visibilityState"),"visible");
    assert.deepEqual(exhausted,{...original,ship,focused:"player-name-input",selected:true});
    assert.deepEqual(await roster(),[{id:incumbent.playerId,name,zone:"chaos-zone",bot:false}]);
    assert.deepEqual(await motion(owner),incumbent);
    const rejections=rows(server,"server login rejected",duplicateOffset);
    assert(rejections.length>=10,"real bounded retry loop must run before exhaustion");
    assert(rejections.every(line=>serverLogField(line,"reason")==="identity_in_use"));
    cases.push({kind:"healthy-incumbent-not-evicted-and-duplicate-exhausts",rejections:rejections.length,
      observedWaitMs:Date.now()-rejectedAt,preservedIdentity:exhausted.id,preservedShip:exhausted.ship,incumbent});
    extraLogs["duplicate-exhausted-identity"]=JSON.stringify(exhausted);
    await duplicate.close();duplicate=null;
    await waitUntil("rejected transport cleans up before crash fixture",async()=> (await status()).rtc_sessions===1,10000);
    await retain(owner,"owner-before-crash");

    const crashOffset=server.logs.length,crashedAt=Date.now(),exited=once(browser.child,"exit");
    rememberOwnedXDisplays(browser.child);
    assert.equal(killProcessGroup(browser.child,"SIGKILL"),true);
    let exitTimer;
    const [exitCode,signal]=await Promise.race([exited,new Promise((_,reject)=>{exitTimer=setTimeout(()=>reject(Error("owned browser SIGKILL did not exit")),5000);})]);
    clearTimeout(exitTimer);assert.equal(signal,"SIGKILL");owner=null;
    await stopProcess(browser.child); // removes only previously captured owned X display files
    assert.deepEqual(await roster(),[{id:incumbent.playerId,name,zone:"chaos-zone",bot:false}],"crash must really leave the old server session pending");
    browser=startChrome(browserPort,"identity-rejoin");browsers.push(browser);await waitForChrome(browser);
    replacement=await open({waitForReveal:false});await waiting(replacement);
    assert.equal((await identity(replacement)).id,original.id,"unclean profile restart must retain its original identity");
    screenshots.crashWait=await replacement.screenshot();
    extraLogs["restart-wait-state"]=JSON.stringify(await replacement.pageState());
    await waitUntil("same connection's automatic retry becomes playable",()=>replacement.evaluate(`
      window.__zubspaceLifecycle?.inGameplay && window.__zubspaceLifecycle.gameplayStepReady &&
      document.querySelector('#loading-overlay').classList.contains('hidden')`),50000);
    const resumed=await motion(replacement),currentIdentity=await identity(replacement);
    assert.notEqual(resumed.playerId,incumbent.playerId);
    assert.equal(currentIdentity.id,original.id);assert.equal(currentIdentity.name,name);
    assert.deepEqual(await roster(),[{id:resumed.playerId,name,zone:"chaos-zone",bot:false}]);
    const finalStatus=await status();assert.equal(finalStatus.connection_quality.forced_total,0);
    const crashRejections=rows(server,"server login rejected",crashOffset);
    assert(crashRejections.length>=1);assert(crashRejections.every(line=>serverLogField(line,"reason")==="identity_in_use"));
    assert.equal(rows(server,"server rtc player connected",crashOffset).length,1,"recovery must reuse its RTC connection");
    assert.equal(rows(server,"server arena joined",crashOffset).length,1);
    cases.push({kind:"unclean-profile-restart-recovers-without-identity-reset",exitCode,signal,
      elapsedSinceKillMs:Date.now()-crashedAt,rejections:crashRejections.length,originalPlayerId:incumbent.playerId,
      resumedPlayerId:resumed.playerId,preservedIdentity:currentIdentity.id,preservedShip:resumed.ship,quality:finalStatus.connection_quality});
    const result=await evidence.succeed({ok:true,cases,scope:"Chrome live duplicate/exhaustion plus SIGKILL profile restart and same-connection retry; no authenticated takeover"},capture());
    console.log(JSON.stringify({ok:result.ok,cases,artifactDir:result.successArtifacts.absoluteArtifactDir}));
  } catch(error){const artifacts=await evidence.fail(error,capture());error.message+=`\n${browserE2eFailureFooter(artifacts)}`;throw error;}
  finally {
    for(const client of [owner,duplicate,replacement])await client?.close().catch(()=>{});
    for(const browser of browsers)await stopProcess(browser.child);
    await stopProcess(server.child);staticServer.server.close();
    for(const directory of new Set(browsers.map(b=>b.userDataDir)))fs.rmSync(directory,{recursive:true,force:true,maxRetries:10,retryDelay:150});
  }
}
main().catch(error=>{console.error(error.stack||error);process.exitCode=1;});
