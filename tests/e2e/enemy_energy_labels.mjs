#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import { withFixtureSpawnRegion } from "./fixture_spawn_region.mjs";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { repoRoot, freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { parseServerDamageEvents } from "../../tools/server_events.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, waitForPlayersReady,
  driveClientToWorldTarget, brakeClientVelocity, orientClientForProtocolDirection, browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";

const appRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/enemy_energy_labels.mjs",
    wasm: process.env.ZUBSPACE_E2E_STATIC_ROOT ? path.relative(repoRoot, path.join(appRoot, "zubspace.wasm")) : undefined,
    clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) });
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-energy-labels-"));
  const zone = path.join(root, "chaos-zone"); fs.cpSync("assets/zones/chaos-zone", zone, { recursive: true });
  const original = fs.readFileSync(path.join(zone, "arena.conf"), "utf8");
  let section = "";
  const spawnRegion = { x: 512, y: 512, radius: 0 };
  const settings = withFixtureSpawnRegion(original.split("\n").map(line => {
    if (line.startsWith("[")) section = line;
    if (/^Initial(Bounty|Recharge)=/.test(line)) return `${line.split("=")[0]}=0`;
    if (section === "[PrizeWeight]" && /^\w+=\d+$/.test(line)) return `${line.split("=")[0]}=0`;
    return line;
  }).join("\n"), spawnRegion).toString("utf8");
  const hash = data => crypto.createHash("sha256").update(data).digest("hex");
  fs.writeFileSync(path.join(zone, "arena.conf"), settings);
  const manifest = path.join(zone, "zone.bundle");
  fs.writeFileSync(manifest, fs.readFileSync(manifest, "utf8").replace(/^settings_sha256=.+$/m, `settings_sha256=${hash(settings)}`).split("\n").filter(line => !line.startsWith("lvz=")).join("\n"));
  const fixture = { description: "Explicit central radius0 fixture spawn; Chaos map/weapons; zero starting bounty/recharge/prizes; no LVZ overlays; one stationary passive life-1 bot; damage and weapon consumption remain enabled",
    maxedSpawnLoadout: false, spawnRegion, originalSettingsSha256: hash(original), settingsSha256: hash(settings) };
  const port = await freePort(), staticServer = await startStaticServer();
  const server = startRealServer(port, { botCount: 1, zoneRoot: root, env: {
    ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT: "false", ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1", ZUBSPACE_SERVER_TEST_STATIONARY_BOT_STATIC_LIFE_SEQ: "1",
    ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ: "1",
  } });
  const browsers = await Promise.all(["actor", "ordinary", "debug"].map(async label => startChrome(await freePort(), `energy-label-${label}`)));
  const base = `http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&trace=1&packetlog=1&password=secret`;
  const urls = [`${base}&name=energyactor`, `${base}&name=ordinary`, `${base}&name=debugger&debug=31337`];
  const clients = [], stages = [], screenshots = {};
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: ["actor", "ordinary", "debug"][i], client })),
    extraScreenshots: screenshots, extraLogs: { stages: JSON.stringify(stages, null, 2), fixture: JSON.stringify(fixture, null, 2), "fixture-settings": settings,
      ...Object.fromEntries(browsers.map((b, i) => [`browser-${i}`, b.logs])) },
    disconnect: async () => { await Promise.all(clients.map(c => c.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); },
  });
  const metrics = async client => (await client.metricsState()).uiMetrics;
  try {
    await waitForServerListening(port, server.child, server.logs);
    const serverStatus = await (await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`)).json();
    assert.equal(serverStatus.maxed_spawn_loadout, false, "declared acquisition/exhaustion fixture must use reference starting loadouts");
    await Promise.all(browsers.map(waitForChrome));
    for (let i = 0; i < browsers.length; i++) { clients.push(await openChromeClient(browsers[i], urls[i])); await clients[i].enterShip(); }
    const [actor, ordinary, debug] = clients;
    await waitForPlayersReady(actor, ordinary, server);
    const initial = await metrics(actor), actorId = initial.selfPlayerId, actorLife = initial.playerLifeSeq;
    for (const observer of [ordinary, debug]) {
      assert.ok((await driveClientToWorldTarget(observer, { x: 8120, y: 8200 }, 15_000, true, 6)).reached);
      assert.ok((await brakeClientVelocity(observer, 6000)).braked);
    }
    assert.equal((await metrics(ordinary)).localGameplayState.debugMode, false);
    assert.equal((await metrics(debug)).localGameplayState.debugMode, true);
    let bot;
    await waitUntil("stationary bot is drawn", async () => {
      bot = await actor.evaluate("window.__zubspaceRemoteSpriteTrace?.at(-1)?.entries.find(p => p.id === 768 && p.drawn)");
      return bot?.lifeSeq === 1;
    }, 5000);

    async function pixelEvidence(client, targetId, color, expected, label, own = false) {
      const m = await metrics(client);
      const entry = own ? { screenX: m.screenWidth/2, screenY: m.screenHeight/2, drawn: true }
        : await client.evaluate(`window.__zubspaceRemoteSpriteTrace.at(-1).entries.find(p => p.id === ${targetId})`);
      assert.ok(entry?.drawn, `${label}: actor must actually be drawn`);
      // Tallfont uses 8x12 cells, scaled to an 11-pixel line height.
      // Test each digit cell rather than imposing a glyph-dependent count of
      // only the brightest pixels: the shipped font includes shaded pixels.
      const glyphWidth = 8 * 11 / 12;
      const labelLeft = entry.screenX - 3 * glyphWidth / 2;
      const rect = { x: Math.floor(entry.screenX-28), y: Math.floor(entry.screenY-55), width: 56, height: 14 };
      const png = await client.screenshot(); screenshots[label] = png;
      await client.evaluate(`(() => { window.__energyPixels = null; const img = new Image();
        img.onload = () => { const c = document.createElement('canvas'); c.width=img.width; c.height=img.height;
          const ctx=c.getContext('2d'); ctx.drawImage(img,0,0); const r=${JSON.stringify(rect)};
          const data=ctx.getImageData(r.x,r.y,r.width,r.height).data; let yellow=0,red=0;
          const digits={yellow:[0,0,0],red:[0,0,0]};
          for(let i=0;i<data.length;i+=4){const [r,g,b]=data.slice(i,i+3);
            const x=${rect.x}+(i/4)%${rect.width};
            const digit=Math.floor((x+.5-${labelLeft})/${glyphWidth});
            // Tints are yellow (255,225,95), red (255,90,70).
            // Ratios include shaded atlas pixels and raster rounding while
            // excluding neutral stars, white names, and the dark background.
            const isYellow=r>=50 && g/r>.82 && g/r<.95 && b/r>.25 && b/r<.50;
            const isRed=r>=50 && g/r>.25 && g/r<.45 && b/r>.18 && b/r<.38;
            if(isYellow){yellow++;if(digit>=0&&digit<3)digits.yellow[digit]++;}
            if(isRed){red++;if(digit>=0&&digit<3)digits.red[digit]++;}}
          window.__energyPixels={yellow,red,digits,width:img.width,height:img.height}; };
        img.onerror=()=>{window.__energyPixels={error:'PNG decode failed'}};
        img.src='data:image/png;base64,${png}'; return true; })()`);
      let pixels;
      await waitUntil(`${label}: decode retained screenshot`, async () => {
        pixels = await client.evaluate("window.__energyPixels"); return Boolean(pixels);
      }, 3000);
      assert.ok(!pixels.error);
      assert.ok(expected ? pixels.digits[color].every(count => count >= 2) && pixels[color === "yellow" ? "red" : "yellow"] === 0 : pixels.yellow+pixels.red <= 2,
        `${label}: actual label pixels diverge: ${JSON.stringify({rect,pixels,expected,color})}`);
      stages.push({ label, targetId, own, expected, color, rect, pixels, entry });
    }
    async function views(id, color, phase) {
      await pixelEvidence(ordinary, id, color, false, `${phase}-ordinary`);
      await pixelEvidence(debug, id, color, true, `${phase}-debug`);
    }
    const a = await metrics(actor);
    assert.ok((await orientClientForProtocolDirection(actor, bot.authoritativeX-a.playerX, bot.authoritativeY-a.playerY, 6000)).oriented);
    for (let hit = 1; hit <= 4; hit++) {
      await waitUntil("gun cooldown before precise bot hit", async () => (await metrics(actor)).localGameplayState.bulletCooldown <= 0, 3000);
      const count = parseServerDamageEvents(server.logs).filter(e => e.victimId === 768 && e.shooterId === actorId).length;
      await actor.press("Control", 55);
      let damage;
      await waitUntil(`real shot ${hit} damages bot`, () => {
        damage = parseServerDamageEvents(server.logs).filter(e => e.victimId === 768 && e.shooterId === actorId)[count];
        return damage;
      }, 5000);
      assert.equal(damage.damage, 400); assert.equal(damage.energyAfter, 1700-hit*400); assert.equal(damage.lethal, false);
      await waitUntil("bot energy presentation reaches all observers", async () => {
        const states = await Promise.all([ordinary, debug].map(metrics));
        return states.every(m => m.remotePlayers.find(p => p.id === 768)?.energy === damage.energyAfter);
      }, 5000);
      if (hit === 3) await views(768, "yellow", "bot-yellow");
      if (hit === 4) await views(768, "red", "bot-red");
    }
    // Fire north into empty space, away from the observer and stationary bot.
    assert.ok((await orientClientForProtocolDirection(actor, 0, -1, 6000)).oriented);
    for (const [color, threshold] of [["yellow", 700], ["red", 330]]) {
      await actor.keyDown("Control");
      try {
        await waitUntil(`genuine firing spends own energy into ${color}`, async () => (await metrics(actor)).serverPlayerEnergy < threshold, 10_000);
      } finally { await actor.keyUp("Control"); }
      await waitUntil("human low-energy state reaches both observers", async () => {
        const states = await Promise.all([ordinary, debug].map(metrics));
        return states.every(m => { const e=m.remotePlayers.find(p => p.id === actorId)?.energy; return e < (color === "yellow" ? 850 : 425); });
      }, 5000);
      await pixelEvidence(actor, actorId, color, true, `self-${color}`, true);
      await views(actorId, color, `human-${color}`);
    }
    // The ordinary player list must not provide a second route to enemy
    // energy. Retain both rendered lists for review alongside native policy.
    for (const [label, observer] of [["ordinary", ordinary], ["debug", debug]]) {
      await observer.keyDown("Space");
      await waitUntil("player list opens", async () => (await metrics(observer)).scoreboardVisible, 3000);
      screenshots[`player-list-${label}`] = await observer.screenshot();
      await observer.keyUp("Space");
      await waitUntil("player list closes", async () => !(await metrics(observer)).scoreboardVisible, 3000);
    }
    for (const observer of [ordinary, debug]) {
      await observer.press("Escape"); await observer.press("s");
      await waitUntil("Escape-S enters spectator", async () => (await metrics(observer)).localGameplayState.spectatorMode, 5000);
    }
    await views(actorId, "red", "spectator-free-human-red");
    await views(768, "red", "spectator-free-bot-red");
    for (const observer of [ordinary, debug]) {
      await observer.press("Control");
      await waitUntil("spectator follows the human", async () => (await metrics(observer)).localGameplayState.spectatingPlayerId === actorId, 5000);
    }
    await views(actorId, "red", "spectator-follow-human-red");
    await views(768, "red", "spectator-follow-bot-red");
    assert.equal((await metrics(actor)).playerLifeSeq, actorLife);
    for (const client of clients) {
      const state = await client.pageState(); assert.equal(state.uiMetrics.localGameplayState.rtcRejoinCount, 0);
      assert.ok(!state.networkLog.some(e => e.level === "error"));
      assert.ok(!client.events.some(e => ["error", "exception"].includes(e.type)));
    }
    console.log(JSON.stringify(await evidence.succeed({ ok: true, fixture,
      coverage: "real weapon damage/spend; screenshot pixels for human/bot yellow/red labels in ordinary/debug play, free/follow spectator; own labels retained", stages }, capture()), null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture()); error.message += `\n${browserE2eFailureFooter(artifacts)}`; throw error;
  } finally {
    await Promise.all(clients.map(c => c.close().catch(() => {})));
    await Promise.all([...browsers.map(b => stopProcess(b.child)), stopProcess(server.child)]).catch(() => {});
    for (const b of browsers) fs.rmSync(b.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close(); fs.rmSync(root, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error.stack || error); process.exit(1); });
