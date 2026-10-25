#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

import {
  createSession,
  deleteSession,
  enterShip,
  execute,
  freezeQaTrace,
  firefoxArtifactClient,
  freePort,
  holdFirefoxKeys,
  metricsState as firefoxMetricsState,
  openClient,
  pageState,
  repoRoot,
  sleep,
  startGeckodriver,
  startRealServer,
  startStaticServer,
  stopProcess,
  waitForGeckodriver,
  waitForServerListening,
  waitUntil,
} from "../../tools/browser_firefox_rtc_smoke.mjs";
import {
  assertDataChannelDeliveryAudit,
  assertSecurityRejectAudit,
  runChaosRouteReplanGate,
  runWormholeAuthorityGate,
  runC2sInputRecoverySoak,
  saveBrowserE2eArtifacts,
  waitForLivePlayerScopes,
} from "./combat_replay.mjs";
import { longTaskWitnessInstallScript, longTaskWitnessReadScript } from "./long_task_witness.mjs";
import { assertRendererConsoleClean } from "../../tools/browser_console_evidence.mjs";
import {
  assertArtifactProvenanceMatches,
  collectArtifactProvenance,
} from "./artifact_provenance.mjs";
import { collectArtifactProvenanceAsync } from "./artifact_provenance_async.mjs";
import { validateArtifactPathReferences } from "./artifact_paths.mjs";
import { createChaosRouteReplanZone, createWormholeE2eZone } from "./wormhole_fixture.mjs";
import { recoveryMembershipReady } from "./recovery_membership.mjs";

const scriptPath = "tests/e2e/firefox_input_recovery.mjs";

function firefoxClient(driver) {
  const capture = firefoxArtifactClient(driver);
  return {
    events: capture.events,
    consoleEvidence: capture.consoleEvidence,
    freezeQaTrace: (options) => freezeQaTrace(driver, options),
    evaluate(expression) {
      return execute(driver, `return (${expression});`);
    },
    holdKeys(keys, durationMs) {
      return holdFirefoxKeys(driver, keys, durationMs);
    },
    hold(key, durationMs) {
      return holdFirefoxKeys(driver, [key], durationMs);
    },
    press(key, durationMs = 60) {
      return holdFirefoxKeys(driver, [key], durationMs);
    },
    pageState() {
      return pageState(driver);
    },
    metricsState() {
      return firefoxMetricsState(driver);
    },
  };
}

async function saveArtifacts({ artifactDir, server, staticServer, urls, alpha, beta, provenance }) {
  return saveBrowserE2eArtifacts({
    artifactDir,
    server,
    staticServer,
    urls,
    clients: [
      { label: "alpha-firefox", client: firefoxClient(alpha) },
      { label: "beta-firefox", client: firefoxClient(beta) },
    ],
    extraLogs: {
      "alpha-geckodriver": alpha.logs,
      "beta-geckodriver": beta.logs,
    },
    provenance,
  });
}

async function main() {
  for (const required of [
    "target/debug/zubspace_server",
    "target/wasm32-unknown-unknown/debug/zubspace.wasm",
  ]) {
    if (!fs.existsSync(path.join(repoRoot, required))) {
      throw new Error(`missing ${required}; build the server and WASM client first`);
    }
  }

  const wormholeGate = process.env.ZUBSPACE_E2E_WORMHOLE_AUTHORITY === "1";
  const chaosRouteGate = process.env.ZUBSPACE_E2E_CHAOS_ROUTE_REPLAN === "1";
  const wormholeFixture = wormholeGate ? createWormholeE2eZone() : null;
  const chaosRouteFixture = chaosRouteGate ? createChaosRouteReplanZone() : null;
  const fixture = wormholeFixture || chaosRouteFixture;
  const arena = fixture?.arena || "chaos-zone";
  if (fixture) {
    process.env.ZUBSPACE_SERVER_ZONE_ROOT = fixture.root;
    process.env.ZUBSPACE_SERVER_ZONE = arena;
  }
  const runStartedAt = new Date().toISOString();
  const provenanceOptions = {
    e2eScript: scriptPath,
    additionalFiles: [
      "tests/e2e/combat_replay.mjs",
      "tests/e2e/recovery_membership.mjs",
      ...(fixture ? [
        "tests/e2e/wormhole_fixture.mjs",
        "tests/e2e/chaos_route_replanner.mjs",
        "tests/e2e/chaos_safe_route.mjs",
        "tests/e2e/authored_map_geometry.mjs",
      ] : []),
      "tools/browser_firefox_rtc_smoke.mjs",
    ],
    runStartedAt,
  };
  const provenance = collectArtifactProvenance(provenanceOptions);
  process.env.ZUBSPACE_SERVER_SIM_OBSERVE = "1";
  const serverPort = await freePort();
  const alphaDriverPort = await freePort();
  const betaDriverPort = await freePort();
  const staticServer = await startStaticServer();
  const server = startRealServer(serverPort, fixture ? { botCount: 0 } : {});
  const alpha = startGeckodriver(alphaDriverPort, "alpha-firefox-recovery");
  const beta = startGeckodriver(betaDriverPort, "beta-firefox-recovery");
  let urls = [];
  let result = null;

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    if (!server.logs.join("").includes("movement_snapback=true")) {
      throw new Error("Firefox recovery server did not start with snap-back enabled");
    }
    await Promise.all([waitForGeckodriver(alpha), waitForGeckodriver(beta)]);
    await Promise.all([createSession(alpha), createSession(beta)]);

    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const pageBase = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}&password=secret&arena=${encodeURIComponent(arena)}&packetlog=1&trace=1`;
    urls = fixture
      ? [
          `${pageBase}&name=ffholealpha`,
          `${pageBase}&name=ffwormholebeta`,
        ]
      : [
          `${pageBase}&name=ffrecoveralpha&c2s_input_impairment=armed`,
          `${pageBase}&name=ffrecoverbeta&repel_delivery_impairment=armed&repel_receipt_impairment=armed`,
        ];
    await Promise.all([openClient(alpha, urls[0]), openClient(beta, urls[1])]);
    await Promise.all([enterShip(alpha), enterShip(beta)]);
    if (wormholeGate) {
      // The Chrome arm gets the shared witness from its own page bootstrap; the WebDriver path has
      // no equivalent, so the Firefox arm's first switch-window run honestly reported
      // `installed: false`. Installing here still brackets the whole relocation, because the gate's
      // own window opens after this point. Firefox publishes no `longtask` entry type at all, so this
      // arm also turns on the starvation probe: without it a Firefox frame gap has no attribution of
      // any kind, which is how a 643 ms Firefox freeze ended up reported beside zero long tasks.
      await firefoxClient(alpha).evaluate(longTaskWitnessInstallScript({ starvationProbe: true }));
    }

    // Random spawns can be outside one another's AOI. The recovery soak's
    // existing real-key approach establishes mutual exact-life membership
    // before enabling impairment; do not demand it before either pilot flies.
    await waitForLivePlayerScopes(firefoxClient(alpha), firefoxClient(beta));
    const initialStates = await Promise.all([firefoxMetricsState(alpha), firefoxMetricsState(beta)]);
    const identities = { alphaId: initialStates[0].uiMetrics.selfPlayerId,
      betaId: initialStates[1].uiMetrics.selfPlayerId };
    if (fixture) {
      const expected = initialStates.map(state => ({
        id: state.uiMetrics.selfPlayerId, lifeSeq: state.uiMetrics.playerLifeSeq,
      }));
      await waitUntil("both Firefox fixture players have exact-life authoritative membership", async () =>
        recoveryMembershipReady(await Promise.all([
          firefoxMetricsState(alpha), firefoxMetricsState(beta),
        ]), expected), 30_000);
    }

    const recovery = fixture
      ? null
      : await runC2sInputRecoverySoak({
          actor: firefoxClient(alpha),
          peer: firefoxClient(beta),
          server,
          serverPort,
          playerId: identities.alphaId,
          participantIds: [identities.alphaId, identities.betaId],
        });
    const wormholeAuthority = wormholeGate
      ? await runWormholeAuthorityGate({
          actor: firefoxClient(alpha),
          server,
          playerId: identities.alphaId,
          fixture: wormholeFixture,
          // Same reported-not-asserted switch window as the Chrome lane: the merge is a client cost
          // and both engines have to be able to show it.
          frameWitness: {
            read: async () => JSON.parse(await firefoxClient(alpha).evaluate(longTaskWitnessReadScript())),
          },
      })
      : null;
    const chaosRouteReplan = chaosRouteGate
      ? await runChaosRouteReplanGate({
          actor: firefoxClient(alpha),
          server,
          playerId: identities.alphaId,
          fixture: chaosRouteFixture,
        })
      : null;
    const deliveryAudit = await assertDataChannelDeliveryAudit({
      server,
      serverPort,
      browserLogs: [alpha.logs, beta.logs],
      disconnect: async () => {
        await Promise.all([
          execute(alpha, "window.__zubspaceSendDisconnectForTest?.(); return true;"),
          execute(beta, "window.__zubspaceSendDisconnectForTest?.(); return true;"),
        ]);
      },
    });
    if (deliveryAudit.movementSnapbacks.total !== 0) {
      throw new Error(`honest Firefox recovery produced snap-backs: ${JSON.stringify(deliveryAudit)}`);
    }
    const securityAudit = assertSecurityRejectAudit(server, 0, []);
    const currentProvenance = await collectArtifactProvenanceAsync(provenanceOptions);
    assertArtifactProvenanceMatches(provenance, currentProvenance);
    for (const driver of [alpha, beta]) {
      assertRendererConsoleClean(firefoxClient(driver).consoleEvidence());
    }

    result = {
      ok: true,
      browser: "firefox",
      headed: process.env.ZUBSPACE_FIREFOX_HEADLESS === "0",
      transport: "rtc",
      arena,
      // A redirected static root drives a different client than the repo default (the shipped
      // `dist/` bundle instead of the debug WASM the repo-root `index.html` points at), so the
      // artifact must never be readable without saying which one it drove.
      staticBundle: {
        appRoot: path.relative(repoRoot, staticServer.appRoot) || ".",
        overridden: path.resolve(staticServer.appRoot) !== repoRoot,
      },
      diagnosticOnly: path.resolve(staticServer.appRoot) !== repoRoot,
      snapbackEnabled: true,
      identities,
      recovery,
      wormholeAuthority,
      chaosRouteReplan,
      deliveryAudit,
      securityAudit,
      provenance,
    };

    const artifactDir = process.env.ZUBSPACE_E2E_FIREFOX_SUCCESS_ARTIFACT_DIR;
    if (artifactDir) {
      const successArtifacts = await saveArtifacts({
        artifactDir,
        server,
        staticServer,
        urls,
        alpha,
        beta,
        provenance,
      });
      result.successArtifacts = successArtifacts;
      const resultPath = path.join(artifactDir, "result.json");
      fs.writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`);
      validateArtifactPathReferences(result, resultPath);
    }
    console.log(JSON.stringify(result, null, 2));
    return result;
  } catch (error) {
    const failureDir = process.env.ZUBSPACE_E2E_FIREFOX_FAILURE_ARTIFACT_DIR ||
      `/tmp/zubspace-firefox-input-recovery-failure-${Date.now()}`;
    let artifacts = null;
    try {
      artifacts = await saveArtifacts({
        artifactDir: failureDir,
        server,
        staticServer,
        urls,
        alpha,
        beta,
        provenance,
      });
    } catch (artifactError) {
      artifacts = { artifactError: artifactError.message };
    }
    throw new Error(`${error.message}; Firefox recovery artifacts: ${JSON.stringify({
      failureDir,
      artifacts,
    })}`);
  } finally {
    await Promise.all([deleteSession(alpha), deleteSession(beta)]).catch(() => {});
    await Promise.all([
      stopProcess(alpha.child),
      stopProcess(beta.child),
      stopProcess(server.child),
    ]).catch(() => {});
    staticServer.server.close();
    wormholeFixture?.cleanup();
    chaosRouteFixture?.cleanup();
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
