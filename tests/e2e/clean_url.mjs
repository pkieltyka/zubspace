#!/usr/bin/env node
import { acknowledgeControlsWelcome } from "../../tools/browser_controls_welcome.mjs";
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import fs from "node:fs";
import { freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";

async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/clean_url.mjs" });
  const port = await freePort();
  const server = startRealServer(port, { env: { ZUBSPACE_SERVER_SIM_OBSERVE: "1" } });
  let staticServer = null;
  let browser = null;
  let url = null;
  const screenshots = {};
  let actor = null;
  const capture = () => ({ server, serverPort: port, staticServer, urls: [url],
    clients: [{ label: "actor", client: actor }], extraScreenshots: screenshots,
    extraLogs: { browser: browser?.logs || [], startup: JSON.stringify({
      gamePort: port, staticPort: staticServer?.port ?? null, url,
    }) },
    disconnect: async () => { await actor?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}); },
  });
  const noQuery = async () => {
    const state = await actor.evaluate("({ query: location.search, arena: window.__zubspaceArena, metrics: window.__zubspaceUiMetrics != null })");
    if (state.query || state.arena !== "chaos-zone" || state.metrics) throw new Error(`clean entry acquired overrides/diagnostics: ${JSON.stringify(state)}`);
  };
  const live = async () => {
    await waitUntil("clean URL joins an authoritative life", async () =>
      actor.evaluate("window.__zubspaceLifecycle?.inGameplay === true"), 20_000);
    await waitUntil("same-origin status reports a joined and snapshot-ACKed player", async () => {
      const status = await (await fetchGameStatusResponse(`${url}v1/status`)).json();
      return status.service === "zubspace-server" && status.joinedPlayers === 1 && status.snapshot_acked_clients === 1;
    }, 10_000);
    // Snapshot ACK is transport evidence, not application readiness. Wait
    // for fresh admitted input before the test sends a physical flight key.
    const offset = server.logs.join("").length;
    await waitUntil("clean player admits neutral input after bootstrap", () =>
      server.logs.join("").slice(offset).includes("human authority input trace"), 15_000);
    await noQuery();
  };
  const enterFromMenu = async (remembered) => openChromeClient(browser, url, {
    beforeReady: async ({ send, evaluate }) => {
      await waitUntil("clean join screen is editable", () => evaluate('document.querySelector("#player-name-form")?.classList.contains("visible")'), 15_000);
      const name = await evaluate('document.querySelector("#player-name-input").value');
      if (remembered && name !== "cleanpilot") throw new Error(`refresh lost remembered name: ${name}`);
      if (!remembered) {
        if (name !== "") throw new Error("fresh profile unexpectedly generated a player name");
        await send("Input.insertText", { text: "cleanpilot" });
      }
      const shot = await send("Page.captureScreenshot", { format: "png", fromSurface: true });
      screenshots[remembered ? "remembered-clean-menu" : "fresh-clean-menu"] = shot.data;
      await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
      await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
      await acknowledgeControlsWelcome(send, evaluate);
    },
  });
  try {
    // freePort releases its probe socket. Own the game listener before an
    // ephemeral static/browser listener can take that same available port.
    await waitForServerListening(port, server.child, server.logs);
    staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
    browser = startChrome(await freePort(), "clean-url");
    url = `http://127.0.0.1:${staticServer.port}/`;
    await waitForChrome(browser);
    actor = await enterFromMenu(false);
    await live();
    const offset = server.logs.join("").length;
    await actor.press("ArrowUp", 450);
    await waitUntil("genuine flight input reaches authority through clean URL", () =>
      /human authority input trace[^\n]*input_bits=1\b/.test(server.logs.join("").slice(offset)), 5000);
    await actor.press("Escape", 60);
    await actor.press("q", 60);
    await waitUntil("leave returns to the remembered name with no menu parameter", () => actor.evaluate(`
      location.search === "" && document.querySelector("#player-name-form")?.classList.contains("visible") &&
      document.querySelector("#player-name-input")?.value === "cleanpilot"`), 15_000);
    await waitUntil("leave cleans up the old RTC session", async () => (await (await fetchGameStatusResponse(`${url}v1/status`)).json()).rtc_sessions === 0, 10_000);
    screenshots["left-to-clean-menu"] = await actor.screenshot();
    await actor.press("Enter", 60, false);
    await live();
    // A normal navigation to the same clean URL reloads the app in the same
    // profile, exercising refresh and remembered-name confirmation.
    await actor.close();
    actor = await enterFromMenu(true);
    await live();
    screenshots["clean-gameplay"] = await actor.screenshot();
    const result = await evidence.succeed({ ok: true, url, noQueryParameters: true,
      defaultArena: "chaos-zone", manualName: "cleanpilot", leaveRejoin: true,
      refreshRememberedName: true, ordinaryDiagnosticsDisabled: true }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    error.stack += `\n${browserE2eFailureFooter(artifacts)}`;
    throw error;
  } finally {
    await actor?.close().catch(() => {});
    await Promise.all([stopProcess(browser?.child), stopProcess(server.child)]).catch(() => {});
    if (browser) fs.rmSync(browser.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer?.server.close();
  }
}
main().catch(error => { console.error(error.stack || error); process.exit(1); });
