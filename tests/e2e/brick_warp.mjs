#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { repoRoot, freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening, sleep } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitForPlayersReady, waitUntil,
  driveClientToWorldTarget, browserE2eFailureFooter } from "./combat_replay.mjs";
import { parseZubspaceEvents } from "../../tools/server_events.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { createWeaponLifecycleFixture } from "./weapon_lifecycle_fixture.mjs";
import { logLines } from "../../tools/log_chunks.mjs";
import { serverLogField } from "../../tools/server_log_fields.mjs";

const appRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
const key = p => `${p.x},${p.y}`;
function tiles(line) {
  assert.ok(line.x1 === line.x2 || line.y1 === line.y2, "authoritative line must be cardinal");
  const result = [];
  for (let x = line.x1; x <= line.x2; x++) for (let y = line.y1; y <= line.y2; y++) result.push({ x, y });
  return result;
}
async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/brick_warp.mjs",
    wasm: process.env.ZUBSPACE_E2E_STATIC_ROOT ? path.relative(repoRoot, path.join(appRoot, "zubspace.wasm")) : undefined,
    clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) });
  // A declared authored region keeps this geometry scenario in its clear
  // patch. Stock random FFA spawn coverage stays in the ordinary combat lanes.
  const fixture = createWeaponLifecycleFixture({ spawnRegion: { x: 512, y: 512, radius: 2 } }), port = await freePort();
  const staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
  const server = startRealServer(port, { botCount: 0, zoneRoot: fixture.root, env: {
    ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1", ZUBSPACE_SERVER_LOG_FORMAT: "json",
    ZUBSPACE_SERVER_LOG_FILTER: "zubspace_server::objectives::bricks=debug" } });
  const browsers = [], clients = [], urls = [], stages = [], screenshots = {};
  const metrics = async c => (await c.metricsState()).uiMetrics;
  const events = () => parseZubspaceEvents(server.logs).map(e => e.event);
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: ["owner", "peer", "observer", "late"][i], client })),
    extraScreenshots: screenshots, extraLogs: { fixture: JSON.stringify(fixture, null, 2), stages: JSON.stringify(stages, null, 2),
      ...Object.fromEntries(browsers.map((b, i) => [`browser-${i}`, b.logs])) },
    disconnect: async () => { await Promise.all(clients.map(c => c.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); } });
  async function join(name) {
    const browser = startChrome(await freePort(), `brick-${name}`); browsers.push(browser);
    await waitForChrome(browser);
    const url = `http://127.0.0.1:${staticServer.port}/?arena=chaos-zone&password=secret&trace=1&packetlog=1&name=${name}`;
    urls.push(url);
    const client = await openChromeClient(browser, url); clients.push(client);
    await client.enterShip(); return client;
  }
  async function lines(c) {
    const packets = await c.evaluate(`[...(window.__zubspacePacketLogFirst || []), ...(window.__zubspacePacketLog || [])]
      .filter(e => e.direction === 'received' && e.bytes?.length === 23 && e.bytes[0] === 0 && e.bytes[1] === 3 && e.bytes[6] === 33).map(e => e.bytes)`);
    const found = new Map();
    for (const bytes of packets) {
      const p = Buffer.from(bytes).subarray(6);
      const line = { x1: p.readUInt16LE(1), y1: p.readUInt16LE(3), x2: p.readUInt16LE(5), y2: p.readUInt16LE(7),
        team: p.readUInt16LE(9), id: p.readUInt16LE(11), tick: p.readUInt32LE(13) };
      found.set(`${line.id}/${line.tick}`, line);
    }
    return [...found.values()];
  }
  async function linePresent(c, line, friendly) {
    const m = await metrics(c), entries = m.localGameplayState.brickTiles;
    assert.equal(m.localGameplayState.brickTilesOverflow, 0);
    return tiles(line).every(tile => entries.some(b => key(b) === key(tile) && b.team === line.team && b.friendly === friendly));
  }
  function sourceWitness(line) {
    let entry;
    for (const text of logLines(server.logs)) {
      if (text.includes("server_brick_deployed") && serverLogField(text, "event") === "server_brick_deployed" &&
          serverLogField(text, "brick_id") === line.id && serverLogField(text, "server_tick") === line.tick) {
        entry = text; break;
      }
    }
    assert(entry, "every observed wall must bind to its actual server source/geometry diagnostic");
    const fields = Object.fromEntries(["brick_id", "source_x", "source_y", "source_direction", "request_x", "request_y",
      "x1", "y1", "x2", "y2", "tile_count", "configured_span", "server_tick", "expires_at_tick"]
      .map(key => [key, serverLogField(entry, key)]));
    for (const key of ["x1", "y1", "x2", "y2"]) assert.equal(fields[key], line[key]);
    assert.equal(fields.tile_count, tiles(line).length);
    assert.equal(fields.configured_span, 10);
    assert.equal(fields.expires_at_tick, line.tick + 12000);
    const axis = Math.floor(((fields.source_direction + 5) % 40) / 10);
    assert.equal(line.x1 === line.x2, axis % 2 === 1, "wire axis must match the accepted source heading");
    return fields;
  }
  async function full(c) {
    await waitUntil("full live energy", async () => { const m = await metrics(c); return m.playerAlive && m.gameplayStepReady && m.playerEnergy >= m.playerMaxEnergy && m.serverPlayerEnergy >= m.playerMaxEnergy; }, 20_000);
  }
  async function align(c, target) {
    for (let attempt = 0; attempt < 40; attempt++) {
      const m = await metrics(c), direction = m.playerProtocolDirection;
      if (direction === target) return;
      let distance = (target - direction + 40) % 40;
      if (distance >= 20) distance -= 40;
      await c.press(distance >= 0 ? "ArrowRight" : "ArrowLeft", Math.abs(distance) <= 2 ? 24 : 60);
      await sleep(30);
    }
    assert.fail(`genuine turns did not reach direction ${target}`);
  }
  try {
    await waitForServerListening(port, server.child, server.logs);
    const owner = await join("brickowner"), peer = await join("brickpeer"), observer = await join("brickwatch");
    await waitForPlayersReady(owner, peer, server);
    await full(observer); await observer.press("Escape"); await observer.press("s");
    await waitUntil("free spectator ready", async () => (await metrics(observer)).localGameplayState.observerReady, 8000);
    const before = await metrics(owner);
    const overlapSetup = await driveClientToWorldTarget(peer,
      { x: before.playerX, y: before.playerY + 14 }, 12_000, true, 3, null, 100);
    assert.ok(overlapSetup.reached, "genuine flight must put the hostile hull behind the stationary owner");
    stages.push({ label: "initial-hostile-overlap-setup", overlapSetup });
    const peerBefore = await metrics(peer);
    await owner.press("4", 60);
    let horizontal;
    await waitUntil("ten-tile horizontal line is installed for owner and spectator", async () => {
      horizontal = (await lines(owner))[0];
      return horizontal && await linePresent(owner, horizontal, true) && await linePresent(observer, horizontal, false);
    }, 6000);
    assert.equal(horizontal.y1, horizontal.y2); assert.equal(tiles(horizontal).length, 10);
    assert.equal((horizontal.x2 - horizontal.x1 + 1) * 16, 160);
    let overlap;
    await waitUntil("hostile overlap commits the server relocation", async () => {
      overlap = events().find(e => e.type === "brick_overlap_relocation_committed" && e.player_id === peerBefore.selfPlayerId);
      const m = await metrics(peer);
      return overlap && Math.hypot(m.playerX - overlap.position_x, m.playerY - overlap.position_y) < 2 && m.gameplayStepReady;
    }, 8000);
    const afterOverlap = await metrics(peer);
    assert.equal(afterOverlap.playerLifeSeq, peerBefore.playerLifeSeq);
    assert.equal(afterOverlap.playerMaxEnergy, peerBefore.playerMaxEnergy);
    assert.equal(afterOverlap.serverPlayerEnergy, peerBefore.serverPlayerEnergy);
    assert.equal(afterOverlap.localGameplayState.gunLevel, peerBefore.localGameplayState.gunLevel);
    assert.equal(afterOverlap.playerFrequency, peerBefore.playerFrequency);
    const ownerAfter = await metrics(owner);
    assert.ok(Math.hypot(ownerAfter.playerX - before.playerX, ownerAfter.playerY - before.playerY) < 1, "friendly brick cannot relocate owner");
    screenshots["horizontal-friendly"] = await owner.screenshot();
    screenshots["horizontal-enemy"] = await observer.screenshot();
    stages.push({ label: "horizontal-overlap", horizontal, source: sourceWitness(horizontal), overlap, peerBefore, afterOverlap });
    await sleep(1700);
    const inventory = (await metrics(owner)).localGameplayState.bricks;
    await owner.press("4", 60); await sleep(300);
    assert.equal((await metrics(owner)).localGameplayState.bricks, inventory, "rejected occupied center must restore predicted inventory");
    assert.equal((await lines(owner)).length, 1, "rejected center cannot replace the original line");
    assert.ok(await linePresent(owner, horizontal, true));
    for (const destination of [{ x: 8200, y: 8248 }, { x: 8440, y: 8320 }]) {
      const route = await driveClientToWorldTarget(owner, destination, 20_000, true, 6, null, 160);
      assert.equal(route.reached, true, "real flight must cross the friendly wall and reach the open deployment site");
    }
    await align(owner, 10);
    await owner.press("4", 60);
    let vertical;
    await waitUntil("second cardinal line renders exactly", async () => {
      vertical = (await lines(owner)).find(line => line.id !== horizontal.id);
      return vertical && await linePresent(owner, vertical, true) && await linePresent(observer, vertical, false);
    }, 6000);
    assert.equal(vertical.x1, vertical.x2); assert.equal(tiles(vertical).length, 10);
    screenshots.vertical = await owner.screenshot();
    stages.push({ label: "vertical", vertical, source: sourceWitness(vertical) });
    const clippedRoute = await driveClientToWorldTarget(owner, { x: 8384, y: 8184 }, 20_000, true, 4, null, 160);
    assert.equal(clippedRoute.reached, true);
    await align(owner, 10);
    await owner.press("4", 60);
    let clipped;
    await waitUntil("nonempty safety tiles terminate the third line equally in both clients", async () => {
      clipped = (await lines(owner)).find(line => line.id !== horizontal.id && line.id !== vertical.id);
      return clipped && await linePresent(owner, clipped, true) && await linePresent(observer, clipped, false);
    }, 6000);
    assert.equal(clipped.x1, clipped.x2);
    assert.equal(clipped.y1, 508, "safety row 507 must remain outside the wall");
    assert.equal(tiles(clipped).length, 9);
    screenshots["safety-clipped"] = await observer.screenshot();
    stages.push({ label: "safety-clipped", clipped, source: sourceWitness(clipped) });
    const late = await join("bricklate");
    await waitUntil("late join replays all lines with original timestamps", async () => {
      const replay = await lines(late);
      return [horizontal, vertical, clipped].every(line => replay.some(r => JSON.stringify(r) === JSON.stringify(line)))
        && (await Promise.all([horizontal, vertical, clipped].map(line => linePresent(late, line, false)))).every(Boolean);
    }, 12_000);
    const replayed = (await metrics(late)).localGameplayState.brickTiles;
    assert.ok(replayed.some(b => b.age >= 2 && b.ttl < 118), "late replay must preserve age rather than restart 120 seconds");
    stages.push({ label: "late-replay", lines: await lines(late), tiles: replayed });
    // Warbird has three authored bricks. Use the newly joined real pilot's
    // inventory for the turning case; do not restock or change loadouts.
    await full(late);
    const latePose = await metrics(late);
    for (const target of [{ x: 8000, y: latePose.playerY }, { x: 8000, y: 8352 }]) {
      const route = await driveClientToWorldTarget(late, target, 20_000, true, 4, null, 160);
      assert.ok(route.reached, "late pilot flies around the hostile wall's end");
      stages.push({ label: "turning-case-route", target, route });
    }
    const turningRoute = await driveClientToWorldTarget(late, { x: 8128, y: 8352 }, 20_000, true, 4, null, 160);
    assert.ok(turningRoute.reached, "genuine flight reaches the unobstructed turning-drop site");
    await align(late, 4);
    const beforeTurning = (await metrics(late)).playerProtocolDirection;
    await late.holdThenChord("ArrowRight", "4", 30, 60);
    const afterTurning = (await metrics(late)).playerProtocolDirection;
    assert.notEqual(afterTurning, beforeTurning, "the fourth wall is dropped during a genuine turn");
    let turning;
    await waitUntil("turning drop installs one complete cardinal wall", async () => {
      turning = (await lines(owner)).find(line => ![horizontal.id, vertical.id, clipped.id].includes(line.id));
      return turning && await linePresent(owner, turning, false) && await linePresent(late, turning, true) && await linePresent(observer, turning, false);
    }, 6000);
    assert.equal(tiles(turning).length, 10);
    stages.push({ label: "turning", turning, source: sourceWitness(turning), beforeTurning, afterTurning, turningRoute });
    screenshots.turning = await observer.screenshot();
    await full(peer);
    await peer.press("0", 60);
    await waitUntil("normal zero places a portal", async () => (await metrics(peer)).localGameplayState.portalActive, 5000);
    const portalInventory = (await metrics(peer)).localGameplayState.portals;
    const warps = [];
    for (let i = 0; i < 4; i++) {
      await full(peer);
      const offset = events().length;
      await peer.holdThenChord("Shift", "0", 30, 60);
      let terminal;
      await waitUntil(`random central warp ${i}`, async () => {
        terminal = events().slice(offset).find(e => e.type === "server_portal_warp_terminal" && e.player_id === peerBefore.selfPlayerId);
        const m = await metrics(peer);
        return terminal?.outcome === 1 && Math.hypot(m.playerX - terminal.position_x, m.playerY - terminal.position_y) < 2 && m.gameplayStepReady;
      }, 6000);
      assert.equal(terminal.used_portal, false); assert.equal(terminal.energy, 1);
      // Four connections: reference population span 266, offset 379, jitter
      // -9..10 produces tile centers 370..652 on each axis, independent of FFA freq.
      for (const coordinate of [terminal.position_x, terminal.position_y]) assert.ok(coordinate >= 370 * 16 + 8 && coordinate <= 652 * 16 + 8);
      const m = await metrics(peer);
      assert.equal(m.playerLifeSeq, peerBefore.playerLifeSeq);
      assert.equal(m.localGameplayState.portals, portalInventory);
      if (i === 0) assert.equal(m.localGameplayState.portalActive, true, "forced random warp must preserve the separate placed portal");
      warps.push(terminal);
    }
    assert.equal(new Set(warps.map(w => `${w.position_x},${w.position_y}`)).size, 4);
    stages.push({ label: "four-central-warps", warps });
    console.log(JSON.stringify({ stage: "brick-aging", horizontalTick: horizontal.tick, verticalTick: vertical.tick, warps: warps.length }));
    const expired = async (c, line) => {
      const m = await metrics(c);
      return m.latestSnapshotServerTick >= line.tick + 12000 && tiles(line).every(t => !m.localGameplayState.brickTiles.some(b => key(t) === key(b)));
    };
    for (const [label, line] of [["horizontal", horizontal], ["vertical", vertical], ["clipped", clipped], ["turning", turning]]) {
      const deadline = line.tick + 12000;
      await waitUntil(`${label}: observe the original deadline approaching`, async () =>
        (await metrics(observer)).latestSnapshotServerTick >= deadline - 200, 130_000);
      const near = await Promise.all(clients.map(metrics));
      for (const m of near) {
        assert.ok(m.latestSnapshotServerTick < deadline, "near-deadline witness must precede expiry");
        assert.ok(tiles(line).every(t => m.localGameplayState.brickTiles.some(b => key(t) === key(b))),
          `${label}: the complete line must still exist before its deadline`);
      }
      stages.push({ label: `${label}-before-expiry`, deadline, witnesses: near.map(m => ({
        playerId: m.selfPlayerId, tick: m.latestSnapshotServerTick,
        tiles: m.localGameplayState.brickTiles.filter(b => tiles(line).some(t => key(t) === key(b))),
      })) });
      await waitUntil(`${label}: all clients retire the original line`, async () =>
        (await Promise.all(clients.map(c => expired(c, line)))).every(Boolean), 8000);
      const after = await Promise.all(clients.map(metrics));
      assert.ok(after.every(m => m.latestSnapshotServerTick <= deadline + 100), "expiry cannot drift more than one second");
      stages.push({ label: `${label}-expired`, deadline,
        witnesses: after.map(m => ({ playerId: m.selfPlayerId, tick: m.latestSnapshotServerTick, tiles: m.localGameplayState.bricksActive })) });
      console.log(JSON.stringify({ stage: "brick-expired", label, deadline }));
    }
    assert.ok((await Promise.all(clients.map(metrics))).every(m => m.localGameplayState.bricksActive === 0));
    screenshots.expired = await observer.screenshot();
    console.log(JSON.stringify(await evidence.succeed({ ok: true, fixture, horizontal, vertical, clipped, turning, overlap, warps,
      cardinalTileSpan: 10, pixelExtent: 160, lateReplayPreservesAge: true, originalLifetimeTicks: 12000,
      occupiedCenterRejected: true, friendlyHullPassage: true,
      scope: "cardinal brick geometry/rendering, hostile overlap, rejected occupied center, friendly hull passage, late replay, real authored expiry and four genuine central random warps; projectile/fragment collision is separate" }, capture()), null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture()); console.error(browserE2eFailureFooter(artifacts)); throw error;
  } finally {
    await Promise.all(clients.map(c => c.close().catch(() => {})));
    await Promise.all([...browsers.map(b => stopProcess(b.child)), stopProcess(server.child)]).catch(() => {});
    for (const browser of browsers) fs.rmSync(browser.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close(); fs.rmSync(fixture.root, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error.stack || error); process.exit(1); });
