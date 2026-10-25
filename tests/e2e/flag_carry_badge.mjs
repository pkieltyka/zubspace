#!/usr/bin/env node

// One real Chrome client in the generated flag-fixture zone claims the flag, holds it through the authored
// goal, and proves what the HUD carry badge does with the pixels the client actually drew. A server-side
// state read, or a screenshot from a renderer that had already stalled, cannot prove that: a frame
// reporting `localCarryingFlag: true` can still show no icon at all. This gate reads the shipped frame, paired with the client's own predicate at each step: the own-carry slot must sit under
// its dark band before the capture while the client reports no carry, over its lit band while the client
// reports the carry, and back under the dark band once the authored win delay clears it with the client
// reporting no carry again. The hold doubles as the goal assertion, and it proves the clear came from the
// goal rather than a death or a relocation by pinning the life/respawn counters and the ballistic coast
// of a ship that touched no key.

import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

import {
  freePort,
  repoRoot,
  startRealServer,
  startStaticServer,
  stopProcess,
  waitForServerListening,
} from "../../tools/browser_firefox_rtc_smoke.mjs";
import { assertMachineHeadroom } from "../../tools/e2e_gate_hygiene.mjs";
import {
  browserE2eFailureFooter,
  driveClientToWorldTarget,
  startChrome,
  waitUntil,
} from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import {
  createFlagTestZoneBundle,
  flagState,
  flagTestZoneRoot,
  openReadyClient,
  tolerant,
} from "./flag_carrier_ride.mjs";

const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const zone = "flag-test";

// The slot rectangle comes from the draw path, not from eyeballing a screenshot:
// `timed_indicator_icon_x(screen_width, frame_width) = screen_width - frame_width` and
// `draw_timed_indicator` puts the sprite top at `y - frame_size.y * 0.5` (`src/game/render/hud/indicators.rs`).
// The own-carry call passes `y = 117.0`, which is unique to that indicator for a player who is not
// spectating, and `self.assets.dropped_flag` is `assets/graphics/dropflag.bm2` - 160x16 of eight-to-ten
// 16x16 frames, drawn at its authored frame size. The client's canvas is 1280x720 because the harness sets
// device metrics to that viewport, so these numbers are screenshot pixels one-for-one.
const clientViewportWidth = 1280;
const carrySlotCentreY = 117.0;
const badgeFramePx = 16;
const badgeRect = {
  x: clientViewportWidth - badgeFramePx,
  y: carrySlotCentreY - badgeFramePx / 2,
  width: badgeFramePx,
  height: badgeFramePx,
};
// The countdown label is drawn right-aligned at the icon's own x (`draw_tinted_scaled(label, x, y - 5.0,
// …, TextAlign::Right, 1.25)`), so any seconds land immediately to its left. `update_flags` holds
// `flag_carry_timer` at 0.0 while there is no authored carry expiry, so this band must stay empty.
const labelRect = { x: badgeRect.x - 26, y: 106, width: 26, height: 14 };

// Brightness is the mean of RGB scaled by 1000, the same convention as
// `magick -format "%[fx:round(mean*1000)]"`. Measured over many frames: 24 with the slot empty (the dark
// HUD panel behind it peaks around (5,5,8)) and 311-316 while carrying.
const darkSlotBrightness = 100;
const litSlotBrightness = 150;

// `tests/fixtures/zones/flag-test/arena.conf` authors Flags=1, FlagWinMode=0 (CarryAll) and
// FlagWinDelayTicks=3600. Objective deadlines advance on the continuum tick counter
// (`crate::continuum::continuum_tick_add`), and a continuum tick is 10 ms - `src/continuum.rs` converts
// with `ticks as f32 / 100.0` - so the authored hold is 36 s, not the 60 s that reading the server's
// `tick_rate = 60` would give. A live claim-to-win interval on those authored values measured 36.000 s.
const authoredHoldMs = (3600 / 100) * 1000;
const holdToleranceMs = 6_000;
const holdBudgetMs = authoredHoldMs + 3 * holdToleranceMs;
// The carrier never touches a key after the claim, so a cleared carry with a moved or respawned ship is
// a death or a relocation, and the goal assertion this half exists for would not have been exercised.
// This is the budget on top of the ballistic coast the claim velocity buys - see the drift guard below.
const maximumParkedDriftPx = 48;

const frameDirectory = path.join(
  "/tmp",
  `zubspace-flag-carry-badge-frames-${process.pid}`,
);

// The screenshot arrives as base64 PNG. Decoding it in the page (Image -> 2D canvas -> getImageData)
// keeps the gate dependency-free; both rects are read from one composited frame so the badge and its
// label band cannot come from two different frames.
function decodeExpression(frame) {
  return `(() => {
    window.__zubBadgePixels = { done: false, error: null, badge: null, label: null };
    const img = new Image();
    img.onload = () => {
      try {
        const canvas = document.createElement("canvas");
        canvas.width = img.width;
        canvas.height = img.height;
        const ctx = canvas.getContext("2d", { willReadFrequently: true });
        ctx.drawImage(img, 0, 0);
        const read = (rect) => Array.from(
          ctx.getImageData(rect.x, rect.y, rect.width, rect.height).data,
        );
        window.__zubBadgePixels = {
          done: true,
          error: null,
          badge: read(${JSON.stringify(badgeRect)}),
          label: read(${JSON.stringify(labelRect)}),
        };
      } catch (error) {
        window.__zubBadgePixels = { done: false, error: String(error), badge: null, label: null };
      }
    };
    img.onerror = () => {
      window.__zubBadgePixels = {
        done: false,
        error: "the client could not decode its own screenshot",
        badge: null,
        label: null,
      };
    };
    img.src = "data:image/png;base64," + ${JSON.stringify(frame)};
    return true;
  })()`;
}

function summarizeRgba(bytes) {
  let total = 0;
  let hash = 0x811c9dc5;
  for (const byte of bytes) {
    hash ^= byte;
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  for (let i = 0; i < bytes.length; i += 4) {
    total += bytes[i] + bytes[i + 1] + bytes[i + 2];
  }
  const pixels = bytes.length / 4;
  return {
    brightness: Math.round((1000 * total) / (pixels * 3 * 255)),
    signature: hash.toString(16).padStart(8, "0"),
  };
}

async function frameStats(client, label) {
  const frame = await tolerant(`${label} screenshot`, () => client.screenshot(), 12);
  fs.writeFileSync(path.join(frameDirectory, `${label}.png`), Buffer.from(frame, "base64"));
  await tolerant(`${label} decode`, () => client.evaluate(decodeExpression(frame)), 12);
  let boxes = null;
  await waitUntil(
    `${label} pixels`,
    async () => {
      const state = await tolerant(`${label} read`, () => client.evaluate("window.__zubBadgePixels"), 8);
      if (state?.error) {
        throw new Error(`${label}: ${state.error}`);
      }
      if (!state?.done) {
        throw new Error(`${label}: decode still pending`);
      }
      boxes = state;
      return true;
    },
    20_000,
  );
  return { badge: summarizeRgba(boxes.badge), label: summarizeRgba(boxes.label) };
}

async function metricsOf(client) {
  const state = await tolerant(`${client.label} metrics`, () => client.metricsState(), 16);
  return state.uiMetrics ?? state;
}

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }
  assertMachineHeadroom("flag carry badge gate", {});

  const evidence = createFocusedBrowserEvidence({
    e2eScript: "tests/e2e/flag_carry_badge.mjs",
  });
  const serverPort = await freePort();
  const chromePort = await freePort();
  const staticServer = await startStaticServer();
  const flagZone = createFlagTestZoneBundle(flagTestZoneRoot);
  // No bots: the hold must end in the authored goal, and a bot kill would turn it into a death drop.
  const server = startRealServer(serverPort, { zone, zoneRoot: flagZone.root, botCount: 0, enableObjectives: true });
  const chrome = startChrome(chromePort, "flag-badge-carrier");
  let carrier = null;
  let carrierUrl = null;
  const capture = () => ({
    server,
    serverPort,
    staticServer,
    urls: [carrierUrl],
    clients: [{ label: "carrier", client: carrier }],
    extraLogs: { "carrier-chrome": chrome.logs },
    // The delivery audit asserts an ingress drain, and a drain is disconnect-cleanup evidence, so this
    // is how the audit gets to close the gate: the session ends, then the queues are read.
    disconnect: async () => {
      await carrier?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {});
    },
  });

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    carrierUrl =
      `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}` +
      `&password=secret&arena=${encodeURIComponent(zone)}&trace=1&name=badgecarrier`;
    carrier = await openReadyClient(chrome, carrierUrl);
    fs.mkdirSync(frameDirectory, { recursive: true });

    const start = await flagState(carrier);
    if (start.self == null) {
      throw new Error(`carrier has no scoped player id: ${JSON.stringify(start)}`);
    }
    if (start.carrying) {
      throw new Error(`carrier started already carrying: ${JSON.stringify(start)}`);
    }
    const before = await frameStats(carrier, "00-before-capture");
    console.log(`flag-badge stage: baseline slot ${JSON.stringify(before.badge)}`);

    // Claim where the flag actually sits, re-reading the live seat each attempt: a rejected or revoked
    // grant re-seats the flag somewhere new, which is why this loop keeps the ride gate's shape.
    const seat = start.flag ?? { x: start.x, y: start.y };
    const approachOffsets = [
      { x: 0, y: 0 },
      { x: 96, y: 0 },
      { x: -96, y: 0 },
      { x: 0, y: 96 },
    ];
    let claim = null;
    let claimedAt = null;
    for (const offset of approachOffsets) {
      const live = (await flagState(carrier)).flag ?? seat;
      const target = { x: live.x + offset.x, y: live.y + offset.y };
      console.log(`flag-badge stage: driving the carrier onto ${JSON.stringify(target)}`);
      // The fixture seats the flag ~630px from the join spawn and the helper caps desired speed at
      // 42px/s, so one approach pass needs well over the 20s a zero-distance route used to need.
      // Arrival must be braked, not merely proximate: the ship coasts indefinitely once the keys are
      // released, so claiming while still moving turns the parked-hold drift assertion into a 600px
      // coast and destroys the premise that only the authored goal can end the carry.
      await driveClientToWorldTarget(carrier, target, 45_000, true, 20);
      try {
        await waitUntil(
          "carrier presents itself as the flag carrier",
          async () => {
            const state = await flagState(carrier);
            if (!state.carrying) {
              throw new Error(`carrier not holding the flag: ${JSON.stringify(state)}`);
            }
            claim = state;
            claimedAt = Date.now();
            return true;
          },
          10_000,
        );
        break;
      } catch (error) {
        console.log(`flag-badge stage: no claim at ${JSON.stringify(target)}: ${error.message}`);
      }
    }
    if (!claim) {
      throw new Error(
        `carrier never became the flag carrier; final view ${JSON.stringify(await flagState(carrier))}`,
      );
    }

    const atCapture = await frameStats(carrier, "01-at-capture");

    // The client's own carry predicate and the pixels it drew have to describe the same moment as
    // tightly as the harness can bind them. A state read alone is the mistake this gate exists to
    // catch - a frame reporting `localCarryingFlag: true` once got reviewed as showing no icon - and
    // a brightness number alone could come from a renderer that stopped compositing.
    const captureState = await flagState(carrier);
    const parkedMetrics = await metricsOf(carrier);
    const parked = {
      x: claim.x,
      y: claim.y,
      lifeSeq: parkedMetrics.playerLifeSeq,
      respawnSeq: parkedMetrics.playerRespawnSeq,
      coastPxPerS: Math.hypot(parkedMetrics.playerVelocityX ?? 0, parkedMetrics.playerVelocityY ?? 0),
    };
    console.log(`flag-badge stage: claimed at ${JSON.stringify(claim.x)},${claim.y}; slot ${JSON.stringify(atCapture.badge)}`);

    // Hold with no further input: the ship parks, and the only thing that can end the carry is the
    // authored win delay.
    let clearedAfterMs = null;
    await waitUntil(
      "the authored win delay clears the carry",
      async () => {
        const state = await flagState(carrier);
        if (state.carrying) {
          throw new Error(`still carrying ${Date.now() - claimedAt}ms after the claim`);
        }
        clearedAfterMs = Date.now() - claimedAt;
        return true;
      },
      holdBudgetMs,
    );

    const after = await flagState(carrier);
    const afterMetrics = await metricsOf(carrier);
    const cleared = await frameStats(carrier, "02-after-goal");
    const clearedState = await flagState(carrier);
    const driftPx = Math.round(Math.hypot(after.x - parked.x, after.y - parked.y));

    // The win reset parks the flag in Waiting and respawns it after FlagResetDelayTicks=600, which is
    // 6 s on the 100 t/s continuum clock. Seeing it back on the map, unowned, is the client-side face of
    // `objective flag win committed`.
    let resetFlag = null;
    await waitUntil(
      "the goal-reset flag respawns on the map unowned",
      async () => {
        const state = await flagState(carrier);
        // An absent flag parses as an object of nulls, so only real coordinates count as a respawn.
        if (state.carrying || state.flag?.x == null || state.flag?.y == null) {
          throw new Error(`flag not respawned yet: ${JSON.stringify(state.flag)}`);
        }
        resetFlag = state.flag;
        return true;
      },
      25_000,
    );
    const afterReset = await frameStats(carrier, "03-after-respawn");

    const failures = [];
    if (before.badge.brightness >= darkSlotBrightness) {
      failures.push(
        `the own-carry slot was not dark before the capture: ${JSON.stringify(before.badge)}`,
      );
    }
    if (atCapture.badge.brightness <= litSlotBrightness) {
      failures.push(
        `the own-carry slot did not light on capture (need > ${litSlotBrightness}): ` +
          `${JSON.stringify(atCapture.badge)} while ${JSON.stringify({ carrying: claim.carrying })}`,
      );
    }
    if (!captureState.carrying) {
      failures.push(
        "the frame that lit the own-carry slot came from a client that no longer reported the carry: " +
          JSON.stringify({ badge: atCapture.badge, carrying: captureState.carrying }),
      );
    }
    // Countdown digits land in the band left of the sprite, so an empty label is asserted with the
    // same dark band: any authored expiry would paint digits over it. Pixel equality against the
    // pre-capture frame is not usable here - both bands sit over the scrolling world, and the
    // ballistic coast below lets that backdrop move during the hold.
    if (atCapture.label.brightness >= darkSlotBrightness) {
      failures.push(
        "the carry badge drew a countdown label although no authored carry expiry exists " +
          `(need < ${darkSlotBrightness}): ${JSON.stringify(atCapture.label)}`,
      );
    }
    if (afterMetrics.playerLifeSeq !== parked.lifeSeq || afterMetrics.playerRespawnSeq !== parked.respawnSeq) {
      failures.push(
        `the carry ended on a respawn, not the goal: ${JSON.stringify({ parked, after: afterMetrics })}`,
      );
    }
    // Ballistic, not spatial: the movement model has no drag, so a carrier that touches no key coasts
    // at its claim velocity for the whole hold. A braked arrival still leaves a couple of px/s, which
    // over the 36s authored hold is tens of px - physically indistinguishable from sitting still. What
    // the guard has to refuse is a relocation: a death, a correction snap-back or a wormhole move
    // travels hundreds of px and turns the life/respawn counters, which the check above pins anyway.
    // `maximumParkedDriftPx` stays as the budget on top of the coast, so a stationary claim must still
    // stay put, and the lower bound below refuses a ship that stopped dead mid-coast.
    const coastPx = parked.coastPxPerS * (clearedAfterMs / 1000);
    const parkedBudgetPx = maximumParkedDriftPx + coastPx;
    if (driftPx > parkedBudgetPx) {
      failures.push(
        `the parked carrier moved ${driftPx}px before the carry ended, more than the ${Math.round(coastPx)}px ` +
          `its ${parked.coastPxPerS.toFixed(2)}px/s claim velocity coasts over ` +
          `${Math.round(clearedAfterMs / 1000)}s plus the ${maximumParkedDriftPx}px budget, so the clear ` +
          "is not the authored goal path this half asserts",
      );
    }
    if (coastPx > 2 * maximumParkedDriftPx && driftPx < coastPx / 2) {
      failures.push(
        `the parked carrier stopped dead after claiming at ${parked.coastPxPerS.toFixed(2)}px/s: ` +
          `${driftPx}px of drift against a ${Math.round(coastPx)}px ballistic coast means something ` +
          "corrected or relocated the ship during the hold",
      );
    }
    if (clearedAfterMs < authoredHoldMs - holdToleranceMs) {
      failures.push(
        `the carry cleared ${clearedAfterMs}ms after the claim, earlier than the authored ` +
          `${authoredHoldMs}ms win delay minus ${holdToleranceMs}ms of tolerance`,
      );
    }
    // Cleared means the sprite is back under the dark band *and* the client stopped reporting the
    // carry. The old assertion also demanded the slot's pixels match the pre-capture frame, which
    // silently asserted that nothing in the world behind that HUD corner had moved - true only while
    // the carrier sat motionless on the spawn point, and false as soon as the hold includes a coast.
    if (cleared.badge.brightness >= darkSlotBrightness || clearedState.carrying) {
      failures.push(
        "the own-carry slot did not clear after the goal: " +
          JSON.stringify({ badge: cleared.badge, carrying: clearedState.carrying }),
      );
    }
    if (afterReset.badge.brightness >= darkSlotBrightness) {
      failures.push(
        `the own-carry slot lit again with no carrier: ${JSON.stringify(afterReset.badge)}`,
      );
    }
    if (resetFlag?.owner !== 65535) {
      failures.push(`the respawned flag came back owned: ${JSON.stringify(resetFlag)}`);
    }
    if (failures.length > 0) {
      throw new Error(
        `${failures.join("\n")}\nframes: ${frameDirectory}\nmeasurements: ` +
          JSON.stringify({ before, atCapture, cleared, afterReset, clearedAfterMs, driftPx, parked }),
      );
    }

    const result = await evidence.succeed(
      {
        ok: true,
        transport: "rtc",
        zone,
        carrierId: claim.self,
        badgeRect,
        labelRect,
        before,
        atCapture,
        cleared,
        afterReset,
        clearedAfterMs,
        authoredHoldMs,
        parkedDriftPx: driftPx,
        parkedCoastPx: Math.round(coastPx),
        claimCoastPxPerS: Number(parked.coastPxPerS.toFixed(2)),
        resetFlag,
        frameDirectory,
      },
      capture(),
    );
    console.log(
      `flag carry badge gate passed: slot brightness ${before.badge.brightness} -> ` +
        `${atCapture.badge.brightness} -> ${cleared.badge.brightness} (pre-capture signature ` +
        `${before.badge.signature} restored), carry cleared ${clearedAfterMs}ms after the claim against ` +
        `the authored ${authoredHoldMs}ms win delay with the ship drifting ${driftPx}px on the ` +
        `${Math.round(parked.coastPxPerS * (clearedAfterMs / 1000))}px coast its ` +
        `${parked.coastPxPerS.toFixed(2)}px/s claim velocity buys, flag respawned unowned at ` +
        `${resetFlag.x},${resetFlag.y}; frames in ${frameDirectory}\n` +
        `${JSON.stringify(result, null, 2)}`,
    );
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    error.message = `${error.message}\n${browserE2eFailureFooter(artifacts)}\n\nzubspace_server logs:\n${server.logs.join("").slice(-8_000)}`;
    throw error;
  } finally {
    await carrier?.close().catch(() => {});
    await Promise.all([stopProcess(chrome.child), stopProcess(server.child)]).catch(() => {});
    fs.rmSync(chrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
    fs.rmSync(flagTestZoneRoot, { recursive: true, force: true });
  }
}

const isMain = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;

if (isMain) {
  main().catch((error) => {
    console.error(error.stack || error.message);
    process.exit(1);
  });
}

export { main };
