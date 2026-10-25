#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";

import fs from "node:fs";
import path from "node:path";

import {
  freePort,
  repoRoot,
  sleep,
  startRealServer,
  startStaticServer,
  stopProcess,
  waitForServerListening,
} from "../../tools/browser_firefox_rtc_smoke.mjs";
import {
  browserE2eFailureFooter,
  openChromeClient,
  startChrome,
  waitForChrome,
  waitForPlayersReady, rendezvousPlayers,
  waitUntil,
} from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { captureMenuFlight, assertMenuFlightIsolation } from "./menu_flight_evidence.mjs";

const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
const applicationRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
const wasmPath = process.env.ZUBSPACE_E2E_STATIC_ROOT
  ? path.relative(repoRoot, path.join(applicationRoot, "zubspace.wasm"))
  : "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const clientEntry = path.relative(repoRoot, path.join(applicationRoot, "index.html"));
const e2eScript = "tests/e2e/menu_shortcuts.mjs";

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }
  const evidence = createFocusedBrowserEvidence({ e2eScript, wasm: wasmPath, clientEntry });

  const serverPort = await freePort();
  const alphaPort = await freePort();
  const betaPort = await freePort();
  const staticServer = await startStaticServer();
  const server = startRealServer(serverPort);
  const alphaChrome = startChrome(alphaPort, "menu-shortcuts-alpha");
  const betaChrome = startChrome(betaPort, "menu-shortcuts-beta");
  const screenshots = {};
  const flightEvidence = {};
  let alpha = null;
  let beta = null;
  let alphaUrl = null;
  let betaUrl = null;
  const capture = () => ({
    server, serverPort, staticServer, urls: [alphaUrl, betaUrl],
    clients: [{ label: "alpha", client: alpha }, { label: "beta", client: beta }],
    extraScreenshots: screenshots,
    extraLogs: { "alpha-chrome": alphaChrome.logs, "beta-chrome": betaChrome.logs,
      "menu-flight-evidence": JSON.stringify(flightEvidence) },
    // The delivery audit asserts an ingress drain, which is disconnect-cleanup evidence,
    // so it closes both sessions itself after the live-page artifact capture. Each client
    // is guarded so one already-dead session cannot make cleanup throw.
    disconnect: async () => {
      await Promise.all([
        alpha?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
        beta?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
      ]);
    },
  });

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    const policy = await (await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`)).json();
    if (policy.free_for_all !== true) throw new Error("product menu gate requires the default FFA server policy");
    await Promise.all([waitForChrome(alphaChrome), waitForChrome(betaChrome)]);
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const pageBase =
      `http://127.0.0.1:${staticServer.port}/index.html` +
      `?rtc=${encodeURIComponent(rtcUrl)}` +
      `&password=secret&arena=${encodeURIComponent(arena)}&packetlog=1&trace=1`;
    alphaUrl = `${pageBase}&name=menualpha`;
    betaUrl = `${pageBase}&name=menubeta&debug=31337`;
    [alpha, beta] = await Promise.all([
      openChromeClient(alphaChrome, alphaUrl),
      openChromeClient(betaChrome, betaUrl),
    ]);
    await Promise.all([alpha.enterShip(), beta.enterShip()]);
    await rendezvousPlayers(alpha, beta, server, { stageRadiusPx: 128 });
    await waitForPlayersReady(alpha, beta, server);

    // Product cloak feedback: genuine numbered keys activate the normal
    // gameplay path. Captures are visual evidence; state alone is not opacity proof.
    const appearance = async () => (await alpha.metricsState()).uiMetrics.localGameplayState.abilities;
    const alphaId = (await alpha.metricsState()).uiMetrics.selfPlayerId;
    const originalShip = (await beta.metricsState()).uiMetrics.remoteActorPresentation
      .find((entry) => entry.id === alphaId)?.ship;
    if (!Number.isInteger(originalShip) || originalShip < 0 || originalShip > 7) {
      throw new Error("appearance check cannot identify the original ship");
    }
    const selectShip = async (number) => {
      await waitUntil("full energy before appearance ship change", async () => {
        const state = (await alpha.metricsState()).uiMetrics;
        return state.playerEnergy >= state.playerMaxEnergy &&
          state.localGameplayState.lastSelfServerEnergy >= state.playerMaxEnergy;
      }, 20_000);
      const oldLife = (await alpha.metricsState()).uiMetrics.playerLifeSeq;
      await alpha.press("Escape", 60);
      await alpha.press(String(number), 60);
      await waitUntil("appearance ship change completes its authority receipt", async () => {
        const state = (await alpha.metricsState()).uiMetrics;
        return state.playerLifeSeq !== oldLife && state.localGameplayState.authorityPhase === "Live";
      }, 12_000);
      // Ship selection chooses another authoritative random spawn. Establish
      // the appearance witness with real flight before testing visibility.
      await rendezvousPlayers(alpha, beta, server, { stageRadiusPx: 128 });
      await waitForPlayersReady(alpha, beta, server);
    };
    // Shark is explicitly cloak-capable in Chaos; the initial combat ship need not be.
    if (originalShip !== 7) await selectShip(8);
    const capabilities = await appearance();
    if (!capabilities.cloakCapable || !capabilities.stealthCapable) {
      throw new Error("appearance fixture requires Shark cloak and stealth capabilities");
    }
    screenshots["ship-normal"] = await alpha.screenshot();
    await alpha.press("7", 60);
    await waitUntil("cloak activates", async () => (await appearance()).cloak === true, 3000);
    screenshots["ship-cloaked"] = await alpha.screenshot();
    await alpha.press("6", 60);
    await waitUntil("stealth and cloak activate together", async () => {
      const state = await appearance(); return state.cloak === true && state.stealth === true;
    }, 3000);
    screenshots["ship-cloak-and-stealth"] = await alpha.screenshot();
    await alpha.press("7", 60);
    await waitUntil("stealth alone preserves the uncloaked state", async () => {
      const state = await appearance(); return state.cloak === false && state.stealth === true;
    }, 3000);
    screenshots["ship-stealth-only"] = await alpha.screenshot();
    await alpha.press("6", 60);
    await waitUntil("appearance toggles restore", async () => {
      const state = await appearance(); return state.cloak === false && state.stealth === false;
    }, 3000);

    if (originalShip !== 7) await selectShip(originalShip + 1);

    // Review the actual expanded bitmap-font columns at desktop and wrapped widths.
    // The owner's held-Space binding owns a separate input layer.
    for (const width of [1280, 640, 320]) {
      await alpha.setViewport(width, 720);
      await alpha.keyDown("Space");
      await waitUntil("player list is visible", async () => (await alpha.pageState()).uiMetrics.scoreboardVisible, 3000);
      screenshots[`player-list-${width}`] = await alpha.screenshot();
      await alpha.keyUp("Space");
      await waitUntil("player list is hidden", async () => !(await alpha.pageState()).uiMetrics.scoreboardVisible, 3000);
    }
    await alpha.setViewport(1280, 720);
    await alpha.press("F2", 60);
    if ((await alpha.pageState()).uiMetrics.scoreboardVisible) throw new Error("retired F2 binding opened the list");
    const listBefore = (await alpha.metricsState()).uiMetrics;
    await alpha.keyDown("Space");
    try {
      await waitUntil("held-list flight evidence begins in its active layer", async () =>
        (await alpha.metricsState()).uiMetrics.scoreboardVisible === true, 3000);
      flightEvidence.list = { before: await captureMenuFlight(alpha) };
      for (const key of ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Control", "q", "1"]) await alpha.press(key, 250);
      const held = (await alpha.metricsState()).uiMetrics;
      if (!held.scoreboardVisible || JSON.stringify(held.semanticInputCounts) !== JSON.stringify(listBefore.semanticInputCounts)) throw new Error("held player list leaked gameplay actions");
      flightEvidence.list.after = await captureMenuFlight(alpha, flightEvidence.list.before.frame.renderOrdinal);
      flightEvidence.list.verdict = assertMenuFlightIsolation(flightEvidence.list.before, flightEvidence.list.after, "list");
    } finally { await alpha.keyUp("Space"); }
    await waitUntil("Space release restores game layer", async () => !(await alpha.metricsState()).uiMetrics.scoreboardVisible, 3000);

    // A real tab switch must release the held key even without a physical
    // key-up reaching the game tab. The browser bridge is the release witness.
    await alpha.keyDown("Space");
    await waitUntil("list opens before focus loss", async () => (await alpha.metricsState()).uiMetrics.scoreboardVisible, 3000);
    const releasesBefore = (await alpha.keyState()).gameKeyBridge.forcedReleaseKeys;
    await alpha.cycleFocus(300);
    await waitUntil("focus loss releases held list", async () => {
      const state = await alpha.keyState();
      return !state.uiMetrics.scoreboardVisible
        && state.gameKeyBridge.forcedReleaseKeys > releasesBefore
        && !state.gameKeyBridge.trackedCodes.includes("Space");
    }, 3000);
    await alpha.keyUp("Space");

    const abilityState = async () => (await alpha.pageState()).uiMetrics.localGameplayState.abilities;
    const initialAbilities = await abilityState();
    await alpha.press("-", 60);
    await waitUntil("minus toggles acquired multifire", async () =>
      (await abilityState()).multifire === (initialAbilities.multifireCapable ? !initialAbilities.multifire : initialAbilities.multifire), 3000);
    await alpha.press("Delete", 60);
    await waitUntil("Delete toggles the same multifire state back", async () =>
      (await abilityState()).multifire === initialAbilities.multifire, 3000);

    await alpha.press("q", 60);
    await waitUntil("Q dispatches Repel exactly once", async () => {
      const count = (await alpha.metricsState()).uiMetrics.semanticInputCounts.repel;
      if (count > 1) throw new Error(`Q dispatched Repel more than once: ${count}`);
      return count === 1;
    }, 5000);
    const repelCount = (await alpha.metricsState()).uiMetrics.semanticInputCounts.repel;
    await alpha.press("Escape", 60);
    const before = await alpha.pageState();
    const menuState = state => state.uiMetrics?.localGameplayState;
    if (!menuState(before).menuVisible || menuState(before).debugMode) throw new Error("ordinary menu/debug defaults are wrong");
    for (const closer of ["k", "Escape", "mouse"]) {
      await alpha.press("k", 60);
      await waitUntil("Esc K opens controls", () => alpha.evaluate('document.querySelector("#controls-dialog").open'), 3000);
      screenshots[`game-controls-${closer}`] = await alpha.screenshot();
      if (closer === "mouse") {
        const point = await alpha.evaluate('(() => { const r = document.querySelector("#controls-close").getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; })()');
        await alpha.clickAt(point.x, point.y);
      } else await alpha.press(closer, 60, false);
      await waitUntil("guide closes only one layer", async () =>
        !await alpha.evaluate('document.querySelector("#controls-dialog").open') && menuState(await alpha.pageState()).menuVisible, 3000);
    }
    await alpha.press("F9", 60);
    const debugPeer = (await beta.pageState()).uiMetrics;
    if (!debugPeer.localGameplayState.debugMode || debugPeer.hudLineCount <= before.uiMetrics.hudLineCount) throw new Error("explicit debug mode did not enable diagnostics");

    flightEvidence.menu = { before: await captureMenuFlight(alpha) };
    for (const key of ["Control", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "z", "x", "F3", "F8", "9", "0", "-", "`", "Delete"]) {
      await alpha.press(key, 300);
    }
    flightEvidence.menu.after = await captureMenuFlight(alpha, flightEvidence.menu.before.frame.renderOrdinal);
    flightEvidence.menu.verdict = assertMenuFlightIsolation(flightEvidence.menu.before, flightEvidence.menu.after, "menu");
    const blocked = await alpha.pageState();
    if (JSON.stringify(blocked.uiMetrics.semanticInputCounts) !== JSON.stringify(before.uiMetrics.semanticInputCounts)) {
      throw new Error("menu leaked weapon actions");
    }
    if (blocked.uiMetrics.chatActive !== false) throw new Error("menu typing opened chat");
    if (blocked.uiMetrics.playerEnergy < before.uiMetrics.playerEnergy) throw new Error("menu leaked held fire");
    await alpha.press("Escape", 60);
    // Measure menu-flight isolation before Rocket changes the inherited motion.
    // The menu must block thrust, but must preserve existing inertial flight.
    // Dispatch aliases through real keys; counters measure dispatch independently
    // of each ability's inventory/cooldown acceptance.
    const numbersBefore = (await alpha.pageState()).uiMetrics.semanticInputCounts;
    for (const key of ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"]) {
      await alpha.press(key, 60);
      await waitUntil(`authority ready after number ${key}`, async () =>
        (await alpha.pageState()).uiMetrics.localGameplayState?.authorityPhase === "Live", 15_000);
    }
    await alpha.holdThenChord("Shift", "0", 30, 60);
    // Authority can be Live while combat-view readiness holds one-shot
    // actions for a later fixed step. Observe their dispatch before sampling;
    // the exact-once assertion still rejects duplicate or missing actions.
    const expectedNumberActions = ["burst", "rocket", "decoy", "brick", "thor", "cloak", "antiwarp", "warp", "portal"];
    await waitUntil("number aliases dispatch exactly once after combat readiness", async () => {
      const counts = (await alpha.metricsState()).uiMetrics.semanticInputCounts;
      for (const name of expectedNumberActions) {
        if (counts[name] - numbersBefore[name] > 1) throw new Error(`number alias dispatched ${name} more than once`);
      }
      return expectedNumberActions.every(name => counts[name] - numbersBefore[name] === 1);
    }, 15_000);
    // Restore energy-draining toggles before measuring menu isolation.
    for (const key of ["6", "7", "8", "9"]) await alpha.press(key, 60);
    await alpha.press("F8", 60);
    const originalFrequency = before.uiMetrics.playerFrequency;
    const targetFrequency = originalFrequency === 0 ? 1 : 0;
    const chatCounts = (await alpha.pageState()).uiMetrics.semanticInputCounts;
    for (const frequency of [targetFrequency, originalFrequency]) {
      await alpha.press("Enter", 60);
      await alpha.typeChatText(`=${frequency}`);
      await alpha.press("Enter", 60);
      await sleep(250);
      const state = await alpha.pageState();
      if (state.uiMetrics.playerFrequency !== originalFrequency || menuState(state).authorityPhase !== "Live") throw new Error("FFA allowed a frequency change or stalled authority");
    }
    // Identical messages sent twice remain two messages, each displayed once.
    for (let i = 1; i <= 2; i++) {
      await alpha.press("Enter", 60);
      await alpha.typeChatText("hello pilots");
      await alpha.press("Enter", 60);
      await waitUntil("one named server echo per message in both clients", async () => {
        const states = await Promise.all([alpha.pageState(), beta.pageState()]);
        return states.every(state => menuState(state).chatMessages.filter(text => text === "menualpha: hello pilots").length === i);
      }, 5000);
    }
    const chat = menuState(await alpha.pageState());
    if (chat.chatMessages.some(text => text.startsWith("You:")) || chat.chatColors.at(-1) !== "Blue") throw new Error("incorrect self echo or public chat color");
    screenshots["game-chat"] = await alpha.screenshot();
    if (JSON.stringify((await alpha.pageState()).uiMetrics.semanticInputCounts) !== JSON.stringify(chatCounts)) throw new Error("chat numbers leaked gameplay actions");
    // Real messages fill the compact view. A narrow desktop viewport exercises
    // wrapping while the captures retain both the input and history edges.
    await alpha.setViewport(640, 720);
    for (let i = 0; i < 10; i++) {
      const message = `Chat ${i} arrows fly and control fires bullets and tab drops bombs with numbers safe in chat`;
      await alpha.press("Enter", 60);
      await alpha.typeChatText(message);
      await alpha.press("Enter", 60);
      await waitUntil(`wrapped chat ${i} echoed to both clients`, async () => {
        const states = await Promise.all([alpha.pageState(), beta.pageState()]);
        return states.every(state => menuState(state).chatMessages.includes(`menualpha: ${message}`));
      }, 5000);
      await sleep(260);
    }
    screenshots["game-chat-compact-wrapped"] = await alpha.screenshot();
    await alpha.press("Escape", 60);
    if (!menuState(await alpha.pageState()).menuVisible) throw new Error("chat history menu did not open");
    screenshots["game-chat-expanded-wrapped"] = await alpha.screenshot();
    await alpha.press("Escape", 60);
    await alpha.press("Enter", 60);
    await alpha.typeChatText("unsent input with 1234567890");
    screenshots["game-chat-input-padding"] = await alpha.screenshot();
    await alpha.press("Escape", 60);
    if (JSON.stringify((await alpha.pageState()).uiMetrics.semanticInputCounts) !== JSON.stringify(chatCounts)) throw new Error("wrapped chat or cancelled input leaked gameplay actions");
    await alpha.press("Escape", 60);
    await alpha.press("q", 60);
    await waitUntil("Q returns to the Join screen", async () => alpha.evaluate(`
      !new URL(location.href).searchParams.has("menu") &&
      !new URL(location.href).searchParams.has("name") &&
      document.querySelector("#player-name-form")?.classList.contains("visible")
    `), 15_000);
    await waitUntil("departed player removed from peer roster", async () => {
      const peer = await beta.pageState();
      return !peer.uiMetrics.remoteActorPresentation.some((entry) => entry.id === before.uiMetrics.selfPlayerId);
    }, 15_000);
    await waitUntil("departed player notice uses their handle", async () =>
      (await beta.metricsState()).uiMetrics.notificationMessages.includes("Player menualpha left."), 5000);
    await sleep(1200);
    const stayedInMenu = await alpha.evaluate(`document.querySelector("#player-name-form").classList.contains("visible")`);
    if (!stayedInMenu) throw new Error("quit automatically rejoined");
    // A stale auto-join name in the URL must not override an edited name.
    await alpha.evaluate(`document.querySelector("#player-name-input").focus(); document.querySelector("#player-name-input").select(); true`);
    await alpha.insertText("menurejoined");
    // Use genuine keyboard input on the ordinary Join form.
    await alpha.evaluate(`document.querySelector("#player-name-submit").focus(); true`);
    await alpha.press("Space", 60, false);
    await waitUntil("manual Join reconnects", async () => {
      const state = await alpha.pageState();
      return state.uiMetrics?.browserNetworkOwned === true && state.uiMetrics?.localGameplayState?.zoneMapReady === true;
    }, 30_000);
    await alpha.enterShip();
    await rendezvousPlayers(alpha, beta, server, { stageRadiusPx: 128 });
    await waitForPlayersReady(alpha, beta, server);
    await waitUntil("edited name admitted by server", async () => server.logs.join("").includes("name=menurejoined"), 10_000);
    if (!await alpha.evaluate(`JSON.parse(localStorage.getItem("zubspace.identity.v1")).name === "menurejoined" && !new URL(location.href).searchParams.has("name")`)) throw new Error("edited identity was not retained");
    const result = await evidence.succeed({
      ok: true, transport: "rtc", arena, serverPolicy: "free-for-all",
      gameplayQRepelDispatches: repelCount,
      menuWeaponActionsSuppressed: true, menuFlightIsolation: flightEvidence.menu?.verdict,
      numberAliasesDispatchedOnce: true, chatNumbersIsolated: true,
      removedF8Inert: true, frequencyChangesBlocked: { originalFrequency, targetFrequency },
      controlsGuideLayers: ["K", "Escape", "mouse"], namedChatEchoOnce: true, repeatedMessagesPreserved: true,
      chatPresentationCaptures: { viewport: [640, 720], wrappedMessages: 10, compact: true, expanded: true, inputPadding: true },
      departedRosterRemoved: true, stayedInMenu,
      multifireAlias: { capabilityAtJoin: initialAbilities.multifireCapable, stateRestored: true },
      playerList: { held: true, releaseCloses: true, focusLossCloses: true, flightAndActionsIsolated: true, inertialFlight: flightEvidence.list?.verdict, widths: [1280, 640, 320] },
      manualRejoin: true, editedNameAdmitted: "menurejoined",
    }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    const originalStack = error.stack || error.message;
    const footer = browserE2eFailureFooter(artifacts);
    error.message = `${error.message}\n${footer}`;
    error.stack = `${originalStack}\n${footer}`;
    throw error;
  } finally {
    await alpha?.close().catch(() => {});
    await beta?.close().catch(() => {});
    await Promise.all([
      stopProcess(alphaChrome.child),
      stopProcess(betaChrome.child),
      stopProcess(server.child),
    ]).catch(() => {});
    fs.rmSync(alphaChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    fs.rmSync(betaChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}

main().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
