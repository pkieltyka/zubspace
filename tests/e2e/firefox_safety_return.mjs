#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import assert from 'node:assert/strict';
import {freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening,
  startGeckodriver, waitForGeckodriver, createSession, openClient, deleteSession,
  enterShip, metricsState, firefoxArtifactClient, holdFirefoxKeys, releaseFirefoxKeys,
  screenshot, execute} from '../../tools/browser_firefox_rtc_smoke.mjs';
import {returnExtremeFirefoxPilotToSafety} from './bot_authority_firefox.mjs';
import {navigateAuthoredTarget} from './authored_navigation.mjs';
import {selectPlayerShip, playerShipReady} from './player_ship_identity.mjs';
import {waitUntil, browserE2eFailureFooter} from './combat_replay.mjs';
import {createFocusedBrowserEvidence} from './focused_browser_evidence.mjs';

// Same real Firefox adapter and safety-return function as aged authority.
// No bots, age wait or combat claim: this isolates the retained route geometry
// and Lancaster handling through ordinary keys and unmodified Extreme assets.
async function main() {
  assert.equal(process.env.ZUBSPACE_E2E_ARENA, 'extreme');
  const evidence = createFocusedBrowserEvidence({e2eScript:'tests/e2e/firefox_safety_return.mjs'});
  const port = await freePort(), staticServer = await startStaticServer();
  const server = startRealServer(port, {zone:'extreme', botCount:0});
  const driver = startGeckodriver(await freePort(), 'safety-return');
  const url = `http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=extreme&name=SafetyReturn&password=secret&trace=1&packetlog=1`;
  let actor, report;
  const capture = () => ({server, serverPort:port, staticServer, urls:[url],
    clients:actor ? [{label:'firefox',client:actor}] : [],
    extraLogs:{browser:driver.logs, route:JSON.stringify(report ?? null)},
    disconnect:async()=>{await actor?.evaluate('window.__zubspaceSendDisconnectForTest?.()');}});
  try {
    await waitForServerListening(port, server.child, server.logs);
    const serverStatus = await (await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`)).json();
    assert.equal(serverStatus.botPlayers,0);
    assert.equal(serverStatus.zone,'extreme');
    assert.equal(serverStatus.maxed_spawn_loadout,true);
    await waitForGeckodriver(driver); await createSession(driver);
    const artifacts = firefoxArtifactClient(driver);
    actor = {...artifacts, metricsState:()=>metricsState(driver),
      evaluate:expression=>execute(driver, `return ${expression};`),
      screenshot:()=>screenshot(driver),
      press:(key,ms=45)=>holdFirefoxKeys(driver,[key==='Escape'?'\uE00C':key],ms),
      releaseKeys:()=>releaseFirefoxKeys(driver)};
    await openClient(driver,url); await enterShip(driver);
    const read = async()=>(await actor.metricsState()).uiMetrics;
    await waitUntil('fresh Firefox maximum-entry authority', async()=>playerShipReady(await read()),15000);
    const beforeSelection = await read();
    const selection = await selectPlayerShip({read, press:actor.press, waitUntil,
      ship:6, fromShip:beforeSelection.playerShip, label:'retained Lancaster handling'});
    const stage = await navigateAuthoredTarget(actor,{x:7464,y:8536},server,
      {timeoutMs:90000,arrivalRadiusPx:8,maximumDesiredSpeed:42,desiredSpeedScale:0.25});
    const before = await read();
    assert.equal(before.inSafeZone,false);
    assert.equal(before.playerShip,6);
    assert.equal(before.localGameplayState.baseThrustAcceleration,310);
    assert.equal(before.localGameplayState.baseTopSpeed,350);
    const returned = await returnExtremeFirefoxPilotToSafety(actor);
    const after = await read();
    assert.equal(returned,true);
    assert.equal(after.playerLifeSeq,before.playerLifeSeq);
    assert.equal(after.inSafeZone,true);
    assert.ok(Math.hypot(after.playerVelocityX,after.playerVelocityY)<=3.5);
    assert.equal(after.selfCorrectionCount,0);
    const routes = actor.rendezvousTrace || [];
    assert.ok(routes.length>=2 && routes.every(route=>route.status==='complete'));
    assert.ok(routes.slice(1).some(route=>route.plan.routes[0].drivePoints.length>=2),
      'real return must exercise the retained corner, not a single unobstructed shortcut');
    report = {serverStatus,selection,stage,before,after,routes};
    console.log(JSON.stringify(await evidence.succeed({ok:true,diagnosticOnly:true,report,
      scope:'fresh real Firefox/Lancaster return over retained Extreme corner geometry; original3px leg/rest criteria, unchanged90s setup and20s final approach; not aged/combat/whole-matrix proof'},capture()),null,2));
  } catch(error) {
    const artifacts = await evidence.fail(error,capture());
    console.error(browserE2eFailureFooter(artifacts)); throw error;
  } finally {
    await deleteSession(driver).catch(()=>{});
    await Promise.all([stopProcess(driver.child),stopProcess(server.child)]).catch(()=>{});
    staticServer.server.close();
  }
}
main().catch(error=>{console.error(error.stack||error);process.exitCode=1;});
