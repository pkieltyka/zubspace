#!/usr/bin/env node
import { acknowledgeControlsWelcome } from "../../tools/browser_controls_welcome.mjs";
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
// Real form input and RTC admission. Storage has its own fault-injection
// component; this scenario uses the ordinary HTTP/cache/protocol paths.
import assert from "node:assert/strict";
import fs from "node:fs";
import {once} from "node:events";
import {freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening} from "../../tools/browser_firefox_rtc_smoke.mjs";
import {openChromeClient, startChrome, waitForChrome, waitUntil, browserE2eFailureFooter} from "./combat_replay.mjs";
import {createFocusedBrowserEvidence} from "./focused_browser_evidence.mjs";
import {browserConsoleEvidence, assertRendererConsoleClean} from "../../tools/browser_console_evidence.mjs";
import {serverLogField, serverLogMessage} from "../../tools/server_log_fields.mjs";

const manifest=JSON.parse(fs.readFileSync("target/zubspace-zone-assets.json","utf8"));
const files=manifest.zones["chaos-zone"].filter(name=>/\.(lvl|lvz)$/.test(name));
const expected=files.length, phases=[], extraLogs={};
const initScript="performance.setResourceTimingBufferSize(512);";

async function submit(send,evaluate) {
  await waitUntil("menu prefetch completes before genuine Enter",()=>evaluate(`(() => {
    const input=document.querySelector('#player-name-input'), s=window.__zubspaceZoneLoadingStats;
    return document.querySelector('#player-name-form')?.classList.contains('visible') && !input.readOnly &&
      s?.completed===${manifest.zones["chaos-zone"].length} && s.active===0 && s.failures===0 &&
      performance.getEntriesByType('resource').some(row=>/zubspace\\.wasm/.test(row.name));
  })()`),20000);
  await evaluate("document.querySelector('#player-name-input').focus(); document.querySelector('#player-name-input').select(); true");
  await send("Input.insertText",{text:"CachePilot"});
  await send("Input.dispatchKeyEvent",{type:"keyDown",key:"Enter",code:"Enter",windowsVirtualKeyCode:13});
  await send("Input.dispatchKeyEvent",{type:"keyUp",key:"Enter",code:"Enter",windowsVirtualKeyCode:13});
  await acknowledgeControlsWelcome(send, evaluate);
}

async function ready(client) {
  await waitUntil("reloaded admission reveals playable ship",()=>client.evaluate(`
    window.__zubspaceLifecycle?.inGameplay && window.__zubspaceLifecycle.gameplayStepReady &&
    document.querySelector('#loading-overlay').classList.contains('hidden')`),20000);
}

async function sample(client,label,cacheHit) {
  await ready(client);
  const before=(await client.metricsState()).uiMetrics;
  await client.keyDown("ArrowUp");
  try {await waitUntil("real thrust works immediately after reveal",async()=>{
    const after=(await client.metricsState()).uiMetrics;
    return Math.hypot(after.playerX-before.playerX,after.playerY-before.playerY)>=8 && after.clientInputSeq!==before.clientInputSeq;
  },3000);} finally {await client.keyUp("ArrowUp");}
  const state=await client.evaluateAwait(`(async()=>{
    await window.__zubspaceZoneCache.flush();
    return {timeOrigin:performance.timeOrigin, cache:{...window.__zubspaceZoneCache.stats},
      loading:{...window.__zubspaceZoneLoadingStats}, timing:{...window.__zubspaceJoinTiming},
      lifecycle:{...window.__zubspaceLifecycle}, resources:performance.getEntriesByType('resource').map(row=>({
        name:row.name,startTime:row.startTime,responseEnd:row.responseEnd,duration:row.duration,transferSize:row.transferSize}))};
  })()`);
  const zone=state.resources.filter(row=>row.name.includes('/assets/zones/'));
  assert.equal(zone.filter(row=>row.name.includes('/extreme/')).length,0,"Chaos must not fetch Extreme");
  const downloads=zone.filter(row=>/\.(lvl|lvz)(?:[?#]|$)/.test(row.name));
  if(cacheHit){
    assert.equal(downloads.length,0,"verified files must come from binary cache after reload/restart");
    assert.equal(state.loading.cacheHits,expected);
  } else {
    assert.equal(downloads.length,expected);
    assert(downloads.some((a,i)=>downloads.some((b,j)=>i!==j && a.startTime<b.responseEnd && b.startTime<a.responseEnd)),"selected-zone HTTP requests must overlap");
    assert(state.cache.httpHits>=expected,"first admission reuses the HTTP copies");
  }
  assert(state.loading.peak<=6);
  assert.equal(state.cache.writes,expected,"persist all files only after validated publication");
  assert.equal(state.cache.storageErrors,0);
  const consoleEvidence=browserConsoleEvidence(client);
  assertRendererConsoleClean(consoleEvidence);
  assert.equal(consoleEvidence.errors,0,JSON.stringify(consoleEvidence));
  phases.push({label,...state,consoleAudit:{...consoleEvidence,events:undefined}});
  // Earlier documents disappear on F5/clean restart. Retain their complete
  // observations and console, in addition to final-client correlated QA capture.
  extraLogs[`${label}-state`]=JSON.stringify(await client.pageState());
  extraLogs[`${label}-console`]=JSON.stringify(consoleEvidence);
  return state;
}

async function main(){
  const evidence=createFocusedBrowserEvidence({e2eScript:"tests/e2e/zone_cache_join.mjs"});
  const port=await freePort(),staticServer=await startStaticServer(),browserPort=await freePort(),metricsPort=await freePort();
  const server=startRealServer(port,{botCount:0,env:{ZUBSPACE_SERVER_LOG_FORMAT:"json",ZUBSPACE_SERVER_QA_TRACE_UPLOAD:"1",ZUBSPACE_SERVER_METRICS_LISTEN:`127.0.0.1:${metricsPort}`}});
  const browsers=[];let client,send;
  const url=`http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&menu=1&password=secret&packetlog=1&trace=1`;
  const capture=()=>({server,serverPort:port,staticServer,urls:[url],clients:client?[{label:"cache-pilot",client}]:[],
    extraLogs:{...extraLogs,phases:JSON.stringify(phases),...Object.fromEntries(browsers.map((b,i)=>[`chrome-${i}`,b.logs]))},
    disconnect:async()=>{await client?.press("Escape",60);await client?.press("q",60);}});
  try {
    await waitForServerListening(port,server.child,server.logs);
    let browser=startChrome(browserPort,"zone-cache-join");browsers.push(browser);await waitForChrome(browser);
    const open=async()=>openChromeClient(browser,url,{initScript,beforeReady:async context=>{send=context.send;await submit(context.send,context.evaluate);}});
    client=await open();
    const first=await sample(client,"first-http",false);
    await send("Page.reload",{ignoreCache:true});
    await waitUntil("F5 creates a new document",()=>client.evaluate(`performance.timeOrigin!==${first.timeOrigin}`),10000);
    await submit(send,expression=>client.evaluate(expression));
    await sample(client,"f5-binary",true);

    // Browser.close performs a normal profile shutdown, unlike SIGKILL. Keep
    // the same profile and origin, and assert the browser actually exited.
    const exited=once(browser.child,"exit");
    await send("Browser.close").catch(error=>{if(!/WebSocket closed/.test(error.message))throw error;});
    const [code,signal]=await Promise.race([exited,new Promise((_,reject)=>setTimeout(()=>reject(Error("clean browser exit timed out")),10000))]);
    assert.equal(code,0);assert.equal(signal,null);client=null;
    // A clean profile exit does not promise an application-level Leave: ICE
    // may remain Disconnected until its ordinary failure timeout. This case
    // measures persistent cache reuse, independently of the immediate-rejoin
    // retry contract. Never send Page.close to an already closed WebSocket.
    const closedAt=Date.now();
    await waitUntil("old browser session expires before cache-only restart case",async()=> (await(await fetch(`http://127.0.0.1:${port}/v1/players`)).json()).total_players===0,35000);
    extraLogs["clean-exit-retirement"]=JSON.stringify({elapsedMs:Date.now()-closedAt,
      scope:"normal Browser.close; no explicit game Leave; immediate-rejoin coverage remains separate"});
    const retiredQuality=(await(await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`)).json()).connection_quality;
    assert.equal(retiredQuality.forced_total,0,"a closed browser must not manufacture a reconnect quality hold");
    extraLogs["retired-quality"]=JSON.stringify(retiredQuality);
    browser=startChrome(browserPort,"zone-cache-join");browsers.push(browser);await waitForChrome(browser);
    client=await open();await sample(client,"clean-restart-binary",true);
    await waitUntil("all three held-file manifests resolve",()=>server.logs.join("").split("\n").filter(line=>serverLogMessage(line)==="server zone_file_manifest_resolved").length===3,10000);
    const logs=server.logs.join("").split("\n").filter(Boolean);
    assert.equal(logs.filter(line=>serverLogMessage(line)==="server zone_file_transfer_complete").length,0);
    const resolved=logs.filter(line=>serverLogMessage(line)==="server zone_file_manifest_resolved").map(line=>({
      held:serverLogField(line,"held_files"),sent:serverLogField(line,"sent_files"),elapsedMs:serverLogField(line,"elapsed_ms")}));
    for(const row of resolved){assert.equal(row.held,expected);assert.equal(row.sent,0);}
    const metrics=await(await fetch(`http://127.0.0.1:${metricsPort}/metrics`)).text();extraLogs.prometheus=metrics;
    assert.match(metrics,new RegExp(`^zubspace_zone_files_held_total ${expected*3}$`,"m"));
    assert.match(metrics,/^zubspace_zone_files_sent_total 0$/m);
    const result=await evidence.succeed({ok:true,expectedFiles:expected,phases,resolved,
      scope:"Chrome ordinary HTTP reuse, genuine F5, clean Browser.close/restart; no public latency or combat claim"},capture());
    console.log(JSON.stringify(result));
  } catch(error){const artifacts=await evidence.fail(error,capture());error.message+=`\n${browserE2eFailureFooter(artifacts)}`;throw error;}
  finally {
    await client?.close().catch(()=>{});
    for(const b of browsers)await stopProcess(b.child);
    await stopProcess(server.child);staticServer.server.close();
    for(const directory of new Set(browsers.map(b=>b.userDataDir)))fs.rmSync(directory,{recursive:true,force:true,maxRetries:10,retryDelay:150});
  }
}
main().catch(error=>{console.error(error.stack||error);process.exitCode=1;});
