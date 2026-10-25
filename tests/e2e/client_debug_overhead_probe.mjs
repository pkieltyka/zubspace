#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
// Diagnostic measurement, never an acceptance verdict. Alternate the actual
// server-advertised presentation tracer, keeping QA/HUD/sprite rings off.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {repoRoot,freePort,startRealServer,startStaticServer,stopProcess,sleep} from '../../tools/browser_firefox_rtc_smoke.mjs';
import {openChromeClient,startChrome,waitForChrome,waitUntil,assertDataChannelDeliveryAudit,assertSecurityRejectAudit} from './combat_replay.mjs';
import {collectArtifactProvenance,assertArtifactProvenanceMatches} from './artifact_provenance.mjs';
import {openCdpSession} from '../../tools/webrtc_capacity_runner.mjs';
import {assertCleanGateEnvironment,assertMachineHeadroom} from '../../tools/e2e_gate_hygiene.mjs';
import {assertRendererConsoleClean,browserConsoleEvidence} from '../../tools/browser_console_evidence.mjs';

export function summarizeWindow(rows, elapsedMs, cpuSeconds) {
  assert.ok(rows.length > 0 && rows.length <= 8192 && elapsedMs >= 25000 && elapsedMs <= 60000 &&
    Number.isFinite(cpuSeconds) && cpuSeconds >= 0 && rows.every(ms=>Number.isFinite(ms)&&ms>=0),
    'complete bounded measurement window required');
  const gaps = rows.slice().sort((a,b)=>a-b);
  return {frames:rows.length,elapsedMs,fps:rows.length*1000/elapsedMs,
    frameGapP50Ms:gaps[Math.floor((gaps.length-1)*.5)],frameGapP95Ms:gaps[Math.floor((gaps.length-1)*.95)],
    frameGapP99Ms:gaps[Math.floor((gaps.length-1)*.99)],frameGapMaxMs:gaps.at(-1),
    gapsOver50Ms:rows.filter(ms=>ms>50).length,taskMsPerFrame:cpuSeconds*1000/rows.length};
}

async function arm(index, enabled, directory, provenanceOptions) {
  assertCleanGateEnvironment('client diagnostic overhead arm');
  assertMachineHeadroom('client diagnostic overhead arm');
  fs.mkdirSync(directory);
  const port=await freePort(), metricsPort=await freePort();
  const server=startRealServer(port,{profile:'release',botCount:15,env:{
    ZUBSPACE_SERVER_CLIENT_DEBUG_TRACE:enabled?'1':'0',ZUBSPACE_SERVER_QA_TRACE_UPLOAD:'0',
    ZUBSPACE_SERVER_BOT_SHIP_TYPES:'1,4,5,8',ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT:'true',
    ZUBSPACE_SERVER_LOG_FORMAT:'json',ZUBSPACE_SERVER_LOG_FILE:path.join(directory,'server.log'),
    ZUBSPACE_SERVER_LOG_FILTER:'zubspace_server::server_tick::bots=debug',
    ZUBSPACE_SERVER_METRICS_LISTEN:`127.0.0.1:${metricsPort}`}});
  const consoleLogs=server.logs;
  // File logging intentionally replaces stdout. Read it only at readiness and
  // audit boundaries; no polling/tailing work runs in the timed frame window.
  Object.defineProperty(server,'logs',{get:()=>[...consoleLogs,
    ...(fs.existsSync(path.join(directory,'server.log'))?[fs.readFileSync(path.join(directory,'server.log'),'utf8')]:[])]});
  const staticServer=await startStaticServer();
  const browser=startChrome(await freePort(),`client-debug-cost-${index}`,{graphicsBackend:process.env.PROBE_GRAPHICS_BACKEND||'vulkan'});
  let client,cdp;
  const url=`http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&name=CostProbe&password=secret`;
  try {
    await waitUntil('file-logged server reports its bound listener',async()=>{
      if(server.child.exitCode!==null||server.child.signalCode!==null)
        throw new Error(`server exited: ${server.logs.join('')}`);
      return server.logs.join('').includes(`\"rtc\":\"http://127.0.0.1:${port}/v1/rtc/offer\"`);
    },10000);
    await waitForChrome(browser);
    client=await openChromeClient(browser,url);await client.enterShip();
    await waitUntil('actual presentation-tracer policy applied',async()=>{
      const stats=await client.evaluate('window.__zubspaceClientDebugStats?.() || null');
      return stats && stats.enabled===enabled;
    },10000);
    const targets=await(await fetch(`http://127.0.0.1:${browser.port}/json/list`)).json();
    const target=targets.find(t=>t.type==='page'&&t.url===url);
    assert.ok(target?.webSocketDebuggerUrl);cdp=await openCdpSession(target.webSocketDebuggerUrl,{timeoutMs:15000});
    await cdp.send('Performance.enable');
    await sleep(10000);
    const graphics=await client.evaluateAwait(`(()=>{const g=document.querySelector('#glcanvas').getContext('webgl2');
      const e=g.getExtension('WEBGL_debug_renderer_info');return {renderer:g.getParameter(e?.UNMASKED_RENDERER_WEBGL||g.RENDERER),
        version:g.getParameter(g.VERSION),antialias:g.getContextAttributes().antialias}})()`);
    assert.ok(!/swiftshader|llvmpipe|softpipe/i.test(graphics.renderer),'hardware measurement must retain an actual adapter');
    const metric=async()=>Object.fromEntries((await cdp.send('Performance.getMetrics')).metrics.map(m=>[m.name,m.value]));
    const startStatus=await(await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`)).json();
    const startProm=await(await fetch(`http://127.0.0.1:${metricsPort}/metrics`)).text();
    const before=await metric();
    await client.evaluate(`(()=>{
      if(window.__zubspaceUiMetrics||window.__zubspaceMetricsTrace||window.__zubspaceRemoteSpriteTrace)
        throw new Error('ordinary measurement unexpectedly enabled expensive QA rings');
      const c=window.__clientDebugCost={started:performance.now(),previous:performance.now(),rows:[],ended:null,overflow:0};
      const step=()=>{const now=performance.now();if(c.rows.length<8192)c.rows.push(now-c.previous);else c.overflow++;
        c.previous=now;if(now-c.started>=30000){c.ended=now;return;}requestAnimationFrame(step);};
      requestAnimationFrame(step);return true;})()`);
    await sleep(30050);
    const after=await metric();
    const measured=await client.evaluate(`(()=>{const c=window.__clientDebugCost;
      return {...c,debug:window.__zubspaceClientDebugStats(),visible:!document.hidden,
        qaPayloads:!!(window.__zubspaceUiMetrics||window.__zubspaceMetricsTrace||window.__zubspaceRemoteSpriteTrace),
        lifecycle:window.__zubspaceLifecycle};})()`);
    assert.ok(measured.ended && measured.visible && !measured.qaPayloads && measured.overflow===0);
    assert.equal(measured.debug.enabled,enabled);
    if(enabled){assert.ok(measured.debug.uploads>=1);assert.equal(measured.debug.failures,0);}
    const endStatus=await(await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`)).json();
    const endProm=await(await fetch(`http://127.0.0.1:${metricsPort}/metrics`)).text();
    const browserConsoleAudit=browserConsoleEvidence(client);
    fs.writeFileSync(path.join(directory,'browser-console.json'),JSON.stringify(browserConsoleAudit,null,2)+'\n');
    assertRendererConsoleClean(browserConsoleAudit);
    const result={index,enabled,diagnosticOnly:true,url,graphics,browserConsoleAudit,
      timing:summarizeWindow(measured.rows,measured.ended-measured.started,after.TaskDuration-before.TaskDuration),
      taskObservationIntervalSeconds:after.Timestamp-before.Timestamp,
      debug:measured.debug,lifecycle:measured.lifecycle,startStatus,endStatus};
    fs.writeFileSync(path.join(directory,'frame-gaps.json'),JSON.stringify(measured));
    fs.writeFileSync(path.join(directory,'start.prom'),startProm);fs.writeFileSync(path.join(directory,'end.prom'),endProm);
    result.securityAudit=assertSecurityRejectAudit(server);
    result.dataChannelDeliveryAudit=await assertDataChannelDeliveryAudit({server,serverPort:port,browserLogs:[browser.logs],
      disconnect:async()=>{await client.evaluate('(window.__zubspaceSendDisconnectForTest(),true)');}});
    assertArtifactProvenanceMatches(collectArtifactProvenance(provenanceOptions),provenanceOptions.initial);
    // Drain console events preceding this cold boundary, after synchronous
    // provenance hashing and cleanup RPCs, outside the timed frame window.
    await client.evaluate('true');
    result.browserConsoleAudit=assertRendererConsoleClean(browserConsoleEvidence(client));
    fs.writeFileSync(path.join(directory,'result.json'),JSON.stringify(result,null,2)+'\n');return result;
  } catch(error) {
    fs.writeFileSync(path.join(directory,'failure.json'),JSON.stringify({error:String(error.stack||error),url},null,2));
    throw error;
  } finally {
    fs.writeFileSync(path.join(directory,'browser-console.json'),JSON.stringify(browserConsoleEvidence(client),null,2)+'\n');
    fs.writeFileSync(path.join(directory,'server-console.log'),consoleLogs.join(''));
    fs.writeFileSync(path.join(directory,'browser.log'),browser.logs.join(''));
    cdp?.close();await client?.close().catch(()=>{});
    await Promise.all([stopProcess(browser.child),stopProcess(server.child)]);
    fs.rmSync(browser.userDataDir,{recursive:true,force:true,maxRetries:10,retryDelay:150});
    await new Promise(resolve=>staticServer.server.close(resolve));
  }
}

async function main(){
  assert.equal(process.env.ZUBSPACE_E2E_STATIC_ROOT,'dist','release bundle is required');
  assert.equal(process.env.ZUBSPACE_SMOKE_LOG_LEVEL,'info','matched INFO baseline required');
  const options={e2eScript:'tests/e2e/client_debug_overhead_probe.mjs',serverBinary:'target/release/zubspace_server',
    wasm:'dist/zubspace.wasm',clientEntry:'dist/index.html',runStartedAt:new Date().toISOString()};
  options.initial=collectArtifactProvenance(options);
  const directory=path.resolve(process.env.PROBE_EVIDENCE_DIR||`.debug/client-debug-overhead-${Date.now()}`);
  fs.mkdirSync(directory,{recursive:true});assert.ok(!fs.existsSync(path.join(directory,'result.json')));
  fs.writeFileSync(path.join(directory,'provenance.json'),JSON.stringify(options.initial,null,2)+'\n');
  const results=[];
  for(const enabled of [false,true,true,false]){
    results.push(await arm(results.length+1,enabled,path.join(directory,`arm-${results.length+1}-${enabled?'on':'off'}`),options));
    console.log(JSON.stringify({arm:results.length,enabled,...results.at(-1).timing}));
  }
  const result={diagnosticOnly:true,kind:'client-presentation-tracer-cost',results,
    limitations:['Four sequential ABBA arms; natural bot combat and scheduling vary.','Frame cadence is capped; CDP task time includes the identical observer and boundary RPCs.',
      'Not key-to-photon latency, a 200-client trace-capacity result or a whole-product acceptance verdict.'],provenance:options.initial};
  fs.writeFileSync(path.join(directory,'result.json'),JSON.stringify(result,null,2)+'\n');
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url))main().catch(error=>{console.error(error);process.exitCode=1;});
