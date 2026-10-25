#!/usr/bin/env node

// Two real Chrome clients in the generated flag-fixture zone: one flies onto the flag with genuine
// keyboard thrust, becomes the authoritative carrier, and keeps flying; the other must present the
// carried flag riding on the carrier. This closes the clause that earlier ad-hoc sessions could not
// show: a zero-pixel frame pair proved the carrier never moved, so "the flag follows the carrier"
// had never been observed on a session with real carrier motion.

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

import {
  freePort,
  repoRoot,
  sleep,
  startRealServer,
  startStaticServer,
  stopProcess,
  waitForServerListening,
} from "../../tools/browser_firefox_rtc_smoke.mjs";
import { assertMachineHeadroom } from "../../tools/e2e_gate_hygiene.mjs";
import {
  browserE2eFailureFooter,
  driveClientToWorldTarget,
  openChromeClient,
  startChrome,
  waitForChrome,
  waitUntil,
} from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";

const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
// Neither shipped zone authors a flag: Chaos and Extreme deliberately carry count-zero objective
// profiles, and that off-switch is itself test-pinned (`objective_profile_disabled` in `/v1/status`). So
// the flag gates run against a `flag-test` zone assembled at run time from the committed authored
// profile in `tests/fixtures/zones/flag-test/arena.conf` - the shipped Extreme arena settings verbatim,
// which is the complete parseable profile in the repo, plus the objective rules retired with the Trench
// Wars capture - laid over the shipped Chaos map, news and screen-object set. Nothing duplicates a
// shipped binary, no tuning number is invented, and no objective profile reaches a player.
const zone = "flag-test";
const fixtureSettingsPath = "tests/fixtures/zones/flag-test/arena.conf";
const profiledZonePath = "assets/zones/chaos-zone";

const sha256 = (bytes) => crypto.createHash("sha256").update(bytes).digest("hex");

// Assembles the authenticated disk bundle the server will accept: a new id, so the manifest declares
// no `override` and the operator override opt-in stays unnecessary.
//
// It deliberately carries no LVZ screen objects. Carrying the shipped Chaos set was tried first, on
// the theory that the badge gate should see exactly what a Chaos join renders; the gate then refused
// to start because its own-carry slot read brightness 317 before anyone captured a flag, against the
// dark ceiling of about 5 documented in `flag_carry_badge.mjs` and the 311-316 lit band it asserts -
// the Chaos score/kill screen object paints over that top-right 16x16 rect, which makes a screen
// object indistinguishable from the flag badge there. The badge is a HUD draw, so the fixture keeps
// the HUD unobstructed instead.
export function createFlagTestZoneBundle(root) {
  const directory = path.join(root, zone);
  fs.mkdirSync(directory, { recursive: true });
  const map = fs.readFileSync(path.join(repoRoot, profiledZonePath, "bdhadar.lvl"));
  const settings = fs.readFileSync(path.join(repoRoot, fixtureSettingsPath));
  const news = Buffer.from("Flag fixture zone\n");
  const files = new Map([
    ["bdhadar.lvl", map],
    ["arena.conf", settings],
    ["news.txt", news],
  ]);
  for (const [name, bytes] of files) {
    fs.writeFileSync(path.join(directory, name), bytes);
  }
  fs.writeFileSync(
    path.join(directory, "zone.bundle"),
    [
      "version=1",
      `id=${zone}`,
      "display_name=Flag Fixture",
      "map=bdhadar.lvl",
      `map_sha256=${sha256(map)}`,
      "settings=arena.conf",
      `settings_sha256=${sha256(settings)}`,
      "news=news.txt",
      `news_sha256=${sha256(news)}`,
      "",
    ].join("\n"),
  );
  return { root, settingsSha256: sha256(settings), mapSha256: sha256(map) };
}

export const flagTestZoneRoot = path.join(
  "/tmp",
  `zubspace-flag-zone-${process.pid}`,
);

// A carried flag is presented to a peer by attachment, not by a re-published position. The flag sprite is
// drawn at the carrier's interpolated ship position (`src/game/render/ships/network.rs`) and
// `draw_network_flags` skips carried flags (`src/game/render.rs`), so the server's `FlagState::Carried`
// stamp - which `flagView` reports, rewritten only at grant and drop - is deliberately not what a peer sees
// while the flag rides. The gate therefore compares the carrier's position as the *observer presents it*
// (`presentedCarrier*`, published from the same expression the draw path uses) against the carrier's own.
// The stamp is still recorded, as a diagnostic: a peer that lost the stamp is a different bug from a peer
// shown the flag at the wrong ship.
const presentedRideTolerancePx = 24;
const minimumTravelPx = 250;
const minimumRidingSamples = 20;
const rideWindowMs = 12_000;
const frameDirectory = path.join(
  "/tmp",
  `zubspace-flag-carrier-ride-frames-${process.pid}`,
);

function parseFlagView(view) {
  if (typeof view !== "string") {
    return null;
  }
  const fields = Object.fromEntries(
    view.split(",").map((part) => {
      const [key, value] = part.split(":");
      return [key, value];
    }),
  );
  const number = (key) => (fields[key] === undefined ? null : Number(fields[key]));
  return {
    id: number("id"),
    x: number("x"),
    y: number("y"),
    owner: number("owner"),
    dropped: fields.dropped === "true",
    carriedBy: number("carriedBy"),
  };
}

// Retries around metric reads and screenshots. This gate used to justify them with a "sustained ~1.7 fps"
// figure derived by dividing metric publications by wall time; that number was wrong, and the real cause was
// a boot-phase main-thread stall (102,903 ms, then 1,916 ms, now 581 ms - see `docs/e2e-diagnostics.md`).
// The retries stay because this zone is pixel-bound under software GL and a screenshot is the largest command
// a lane sends, but a stalled page is now *named* in the failure message rather than absorbed silently.
export async function tolerant(label, run, attempts = 40) {
  let lastError = null;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      return await run();
    } catch (error) {
      lastError = error;
      await sleep(250);
    }
  }
  throw new Error(`${label} still failing after ${attempts} attempts: ${lastError?.message}`);
}

export async function flagState(client) {
  const state = await tolerant(`${client.label} metrics`, () => client.metricsState(), 24);
  const metrics = state.uiMetrics ?? state;
  const gameplay = metrics.localGameplayState || {};
  return {
    self: metrics.selfPlayerId ?? null,
    x: metrics.playerX ?? null,
    y: metrics.playerY ?? null,
    carrying: gameplay.localCarryingFlag === true,
    flag: parseFlagView(gameplay.flagView),
    presented:
      gameplay.presentedCarrierId == null
        ? null
        : {
            id: gameplay.presentedCarrierId,
            x: gameplay.presentedCarrierX,
            y: gameplay.presentedCarrierY,
          },
  };
}

// Opening a client is the same story with a bigger budget: the first `Runtime.evaluate` can land in
// the middle of the authored map decode. Retry the open, and do one client at a time so a single
// decode holds the renderer.
export async function openReadyClient(chrome, url, attempts = 6) {
  let lastError = null;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      const client = await openChromeClient(chrome, url);
      await client.enterShip();
      return client;
    } catch (error) {
      lastError = error;
      await sleep(2_000);
    }
  }
  throw new Error(
    `${chrome.label} never became an in-ship client after ${attempts} attempts: ` +
      `${lastError?.message}`,
  );
}

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }
  assertMachineHeadroom("flag carrier ride gate", {});

  const evidence = createFocusedBrowserEvidence({
    e2eScript: "tests/e2e/flag_carrier_ride.mjs",
  });
  const serverPort = await freePort();
  const carrierPort = await freePort();
  const observerPort = await freePort();
  const staticServer = await startStaticServer();
  const flagZone = createFlagTestZoneBundle(flagTestZoneRoot);
  // No bots: this gate measures flag presentation, and a bot kill would replace the carrier mid-ride.
  const server = startRealServer(serverPort, { zone, zoneRoot: flagZone.root, botCount: 0, enableObjectives: true });
  const carrierChrome = startChrome(carrierPort, "flag-ride-carrier");
  const observerChrome = startChrome(observerPort, "flag-ride-observer");
  let carrier = null;
  let observer = null;
  let carrierUrl = null;
  let observerUrl = null;
  const ride = [];
  const capture = () => ({
    server,
    serverPort,
    staticServer,
    urls: [carrierUrl, observerUrl],
    clients: [
      { label: "carrier", client: carrier },
      { label: "observer", client: observer },
    ],
    extraLogs: {
      "carrier-chrome": carrierChrome.logs,
      "observer-chrome": observerChrome.logs,
    },
    // The delivery audit asserts an ingress drain, and a drain is disconnect-cleanup evidence, so both
    // sessions must hand themselves over before the queues get read.
    disconnect: async () => {
      await Promise.all([
        carrier?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
        observer?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
      ]);
    },
  });

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    // No `packetlog=1`: in the authored zone the client-side packet log keeps the renderer busy
    // enough that five-second DevTools commands stop landing, and the ad-hoc probe that reads this
    // same page successfully never requested the packet log.
    const pageBase =
      `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}` +
      `&password=secret&arena=${encodeURIComponent(zone)}&trace=1`;
    carrierUrl = `${pageBase}&name=ridercarrier`;
    observerUrl = `${pageBase}&name=rideobserver`;
    carrier = await openReadyClient(carrierChrome, carrierUrl);
    observer = await openReadyClient(observerChrome, observerUrl);
    // `waitForPlayersReady` issues one-shot DevTools evaluates with a short per-command deadline,
    // which the authored map decode can blow. Poll the same fact through `waitUntil`, which absorbs
    // the individual timeouts.
    console.log("flag-ride stage: clients opened and in ship");
    await waitUntil(
      "both clients report a scoped player id",
      async () => {
        const [a, b] = await Promise.all([flagState(carrier), flagState(observer)]);
        if (a.self == null || b.self == null || a.self === b.self) {
          throw new Error(`not ready: ${JSON.stringify({ carrier: a, observer: b })}`);
        }
        return true;
      },
      90_000,
    );
    console.log("flag-ride stage: both players scoped");

    const [carrierStart, observerStart] = await Promise.all([
      flagState(carrier),
      flagState(observer),
    ]);
    const carrierId = carrierStart.self;
    const observerId = observerStart.self;
    if (carrierId == null || observerId == null || carrierId === observerId) {
      throw new Error(`missing distinct player ids: ${JSON.stringify({ carrierId, observerId })}`);
    }

    // Claim the flag where it actually sits. The authored base safe zone revokes a grant made inside
    // a base in about three seconds, so the approach tries the seat itself and then points beside it,
    // re-reading the seat each time because a revocation re-seats the flag somewhere new.
    const seat = carrierStart.flag ?? { x: carrierStart.x, y: carrierStart.y };
    const approachOffsets = [
      { x: 0, y: 0 },
      { x: 96, y: 0 },
      { x: -96, y: 0 },
      { x: 0, y: 96 },
    ];
    let claimState = null;
    for (const offset of approachOffsets) {
      const live = (await flagState(carrier)).flag ?? seat;
      const target = { x: live.x + offset.x, y: live.y + offset.y };
      console.log(`flag-ride stage: driving the carrier onto ${JSON.stringify(target)}`);
      // The fixture seats the flag ~630px from the join spawn and the helper caps desired speed at
      // 42px/s, so one approach pass needs well over the 20s a zero-distance route used to need.
      await driveClientToWorldTarget(carrier, target, 45_000, false, 20);
      try {
        await waitUntil(
          "carrier presents itself as the flag carrier",
          async () => {
            const state = await flagState(carrier);
            if (!state.carrying) {
              throw new Error(`carrier not holding the flag: ${JSON.stringify(state)}`);
            }
            claimState = state;
            return true;
          },
          10_000,
        );
        break;
      } catch (error) {
        console.log(`flag-ride stage: no claim at ${JSON.stringify(target)}: ${error.message}`);
      }
    }
    if (!claimState) {
      throw new Error(
        `carrier never became the flag carrier; final view ${JSON.stringify(await flagState(carrier))}`,
      );
    }

    console.log("flag-ride stage: carrier holds the flag; capturing frames");
    fs.mkdirSync(frameDirectory, { recursive: true });
    const claimedAt = await flagState(carrier);
    const framesBefore = {
      carrier: Buffer.from(await tolerant("carrier frame", () => carrier.screenshot(), 8), "base64"),
      observer: Buffer.from(await tolerant("observer frame", () => observer.screenshot(), 8), "base64"),
    };
    fs.writeFileSync(path.join(frameDirectory, "carrier-at-claim.png"), framesBefore.carrier);
    fs.writeFileSync(path.join(frameDirectory, "observer-at-claim.png"), framesBefore.observer);

    // The ride: real key holds keep the ship moving while the observer samples its own presentation.
    const rideTarget = { x: claimedAt.x - 520, y: claimedAt.y - 320 };
    const drive = driveClientToWorldTarget(carrier, rideTarget, rideWindowMs + 8_000);
    const rideStartedAt = Date.now();
    let lastCarrierState = claimedAt;
    while (Date.now() - rideStartedAt < rideWindowMs) {
      const [carrierState, observerState] = await Promise.all([
        flagState(carrier),
        flagState(observer),
      ]);
      lastCarrierState = carrierState;
      ride.push({
        at: Date.now() - rideStartedAt,
        carrier: { x: carrierState.x, y: carrierState.y, carrying: carrierState.carrying },
        observerFlag: observerState.flag,
        observerPresented: observerState.presented,
        // Diagnostic only: the stamp is not what the observer draws while the flag is carried.
        stampGapPx:
          observerState.flag && observerState.flag.carriedBy === carrierId
            ? Math.round(
                Math.hypot(
                  observerState.flag.x - carrierState.x,
                  observerState.flag.y - carrierState.y,
                ),
              )
            : null,
        // The ride itself: where the observer presents the carrier, which is where it draws the flag.
        presentedGapPx:
          observerState.presented?.id === carrierId && carrierState.x != null
            ? Math.round(
                Math.hypot(
                  observerState.presented.x - carrierState.x,
                  observerState.presented.y - carrierState.y,
                ),
              )
            : null,
      });
      await sleep(250);
    }
    await drive;

    const framesAfter = {
      carrier: Buffer.from(await tolerant("carrier frame", () => carrier.screenshot(), 8), "base64"),
      observer: Buffer.from(await tolerant("observer frame", () => observer.screenshot(), 8), "base64"),
    };
    fs.writeFileSync(path.join(frameDirectory, "carrier-after-ride.png"), framesAfter.carrier);
    fs.writeFileSync(path.join(frameDirectory, "observer-after-ride.png"), framesAfter.observer);

    const travelPx = Math.round(
      Math.hypot(
        (lastCarrierState.x ?? 0) - claimedAt.x,
        (lastCarrierState.y ?? 0) - claimedAt.y,
      ),
    );
    const ridingSamples = ride.filter(
      (sample) =>
        sample.presentedGapPx !== null &&
        sample.presentedGapPx <= presentedRideTolerancePx &&
        sample.observerFlag?.carriedBy === carrierId,
    );
    const wrongShipSamples = ride.filter(
      (sample) =>
        sample.observerFlag?.carriedBy === carrierId &&
        sample.observerPresented?.id != null &&
        sample.observerPresented.id !== carrierId,
    );
    const carrierFramesChanged =
      Buffer.compare(framesBefore.carrier, framesAfter.carrier) !== 0;
    const observerFramesChanged =
      Buffer.compare(framesBefore.observer, framesAfter.observer) !== 0;

    if (travelPx < minimumTravelPx) {
      throw new Error(
        `carrier did not travel far enough to prove a ride: ${travelPx}px < ${minimumTravelPx}px; ` +
          JSON.stringify({ claimedAt, last: ride.at(-1) }),
      );
    }
    const presentedGaps = ride
      .map((sample) => sample.presentedGapPx)
      .filter((gap) => gap !== null);
    const stampGaps = ride
      .map((sample) => sample.stampGapPx)
      .filter((gap) => gap !== null);
    if (ridingSamples.length < minimumRidingSamples) {
      throw new Error(
        `observer presented the carrier within ${presentedRideTolerancePx}px in only ` +
          `${ridingSamples.length} of ${ride.length} samples (need ${minimumRidingSamples}); ` +
          `presented gaps: ${JSON.stringify(presentedGaps)}`,
      );
    }
    if (wrongShipSamples.length > 0) {
      throw new Error(
        `observer attached the flag to a ship that is not the carrier in ${wrongShipSamples.length} ` +
          `of ${ride.length} samples: ${JSON.stringify(wrongShipSamples.slice(0, 4))}`,
      );
    }
    if (!carrierFramesChanged || !observerFramesChanged) {
      throw new Error(
        "a client's before/after frames were byte-identical across the ride, which is the " +
          `frozen-frame signature this gate replaced: ${JSON.stringify({
            carrierFramesChanged,
            observerFramesChanged,
            frameDirectory,
          })}`,
      );
    }

    const result = await evidence.succeed(
      {
        ok: true,
        transport: "rtc",
        browserPair: "chrome-chrome",
        zone,
        carrierId,
        observerId,
        claimedAt,
        travelPx,
        ridingSamples: ridingSamples.length,
        samples: ride.length,
        presentedRideTolerancePx,
        maximumPresentedGapPx: presentedGaps.length ? Math.max(...presentedGaps) : null,
        medianPresentedGapPx: presentedGaps.length
          ? presentedGaps.sort((a, b) => a - b)[Math.floor(presentedGaps.length / 2)]
          : null,
        // Diagnostic: the grant-time stamp, which is not what the observer draws while carried.
        maximumStampGapPx: stampGaps.length ? Math.max(...stampGaps) : null,
        wrongShipSamples: wrongShipSamples.length,
        carrierFramesChanged,
        observerFramesChanged,
        frameDirectory,
        ride,
      },
      capture(),
    );
    console.log(
      `flag carrier ride gate passed: carrier ${carrierId} travelled ${travelPx}px, observer ` +
        `presented the carrier within ${presentedRideTolerancePx}px in ${ridingSamples.length}/` +
        `${ride.length} samples (maximum presented gap ` +
        `${presentedGaps.length ? Math.max(...presentedGaps) : "n/a"}px, stamp gap reached ` +
        `${stampGaps.length ? Math.max(...stampGaps) : "n/a"}px), frames in ` +
        `${frameDirectory}\n${JSON.stringify(result, null, 2)}`,
    );
  } finally {
    await carrier?.close().catch(() => {});
    await observer?.close().catch(() => {});
    await Promise.all([
      stopProcess(carrierChrome.child),
      stopProcess(observerChrome.child),
      stopProcess(server.child),
    ]).catch(() => {});
    fs.rmSync(carrierChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    fs.rmSync(observerChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
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
