#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import fs from "node:fs";
import { freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";

// Measures the real challenge/echo path without enabling WASM HUD diagnostics.
// Sustained-degradation enforcement has a separate, unfinished acceptance lane.
async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/connection_quality.mjs" });
  const port = await freePort();
  const metricsPort = await freePort();
  const staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
  const server = startRealServer(port, { env: {
    ZUBSPACE_SERVER_LOG_FORMAT: "json",
    ZUBSPACE_SERVER_METRICS_LISTEN: `127.0.0.1:${metricsPort}`,
  } });
  const browsers = [startChrome(await freePort(), "quality-normal"), startChrome(await freePort(), "quality-slow")];
  const root = `http://127.0.0.1:${staticServer.port}/`;
  const urls = [root, `${root}?network_profile=slow&network_profile_seed=271828`];
  const clients = [];
  const readings = [];
  let metricsText = "";
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: i === 0 ? "normal" : "slow", client })),
    extraLogs: { "quality-readings": JSON.stringify(readings, null, 2), "prometheus-quality": metricsText, "normal-browser": browsers[0].logs, "slow-browser": browsers[1].logs },
    disconnect: async () => { await Promise.all(clients.map(client => client.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); },
  });
  try {
    await waitForServerListening(port, server.child, server.logs);
    await Promise.all(browsers.map(waitForChrome));
    for (let i = 0; i < browsers.length; i++) {
      clients.push(await openChromeClient(browsers[i], urls[i], {
        beforeReady: async ({ send, evaluate }) => {
          await waitUntil("ordinary editable name screen", () => evaluate('document.querySelector("#player-name-form")?.classList.contains("visible")'), 20_000);
          await send("Input.insertText", { text: i === 0 ? "qualitynormal" : "qualityslow" });
          await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
          await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
        },
      }));
    }
    await waitUntil("both ordinary clients receive authoritative snapshots", async () => {
      const status = await (await fetchGameStatusResponse(`${root}v1/status`)).json();
      return status.joinedPlayers === 2 && status.snapshot_acked_clients === 2;
    }, 20_000);
    await Promise.all(clients.map(client => client.press("ArrowUp", 350)));
    let quality = null;
    await waitUntil("both real probe streams have sufficient mature outcomes", async () => {
      quality = (await (await fetchGameStatusResponse(`${root}v1/status`)).json()).connection_quality;
      readings.push({ at: Date.now(), ...quality });
      return quality?.measured_sessions === 2 && quality.samples >= 40 && quality.echoes_received >= 40;
    }, 45_000);
    if (quality.max_session_average_rtt_ms < 100 || quality.max_session_average_rtt_ms >= 600) {
      throw new Error(`slow profile RTT does not reflect the real delayed path: ${JSON.stringify(quality)}`);
    }
    if (quality.sustained_degraded_sessions !== 0) {
      throw new Error(`measurement-only lane has unexpected policy state: ${JSON.stringify(quality)}`);
    }
    metricsText = await (await fetch(`http://127.0.0.1:${metricsPort}/metrics`)).text();
    if (!metricsText.includes("zubspace_connection_quality_measured_sessions 2\n")
      || !metricsText.includes("# TYPE zubspace_connection_quality_samples gauge\n")
      || /(?:player_id|session_id|player_name)=/.test(metricsText)) {
      throw new Error("quality Prometheus gauges missing or have unbounded identity labels");
    }
    const completeLines = server.logs.join("").split("\n");
    completeLines.pop();
    const windows = completeLines.filter(Boolean).map(line => JSON.parse(line))
      .filter(event => event.fields?.message === "server connection quality window");
    if (new Set(windows.filter(event => event.fields.samples > 0).map(event => event.fields.session_id)).size !== 2
      || windows.some(event => !/\.\d{3,}Z$/.test(event.timestamp) || event.level !== "INFO"
        || typeof event.fields.round_trip_probe_loss !== "number"
        || (event.fields.samples > 0 && typeof event.fields.average_rtt_ms !== "number"))) {
      throw new Error("per-session quality windows lack precise structured log evidence");
    }
    const ordinary = await Promise.all(clients.map(client => client.evaluate(`({
      diagnosticsDisabled: window.__zubspaceUiMetrics == null,
      inGameplay: window.__zubspaceLifecycle?.inGameplay === true,
      stats: window.__zubspaceNetworkStats,
    })`)));
    if (ordinary.some(state => !state.diagnosticsDisabled || !state.inGameplay)) throw new Error("quality sampling requires ordinary live clients without diagnostic HUD payloads");
    const slow = ordinary[1].stats;
    if (slow.networkProfile !== "slow" || slow.networkProfileS2cDelaySamples < 20 || slow.networkProfileC2sDelaySamples < 20
      || slow.networkProfileS2cDelayTotalMs / slow.networkProfileS2cDelaySamples < 50
      || slow.networkProfileC2sDelayTotalMs / slow.networkProfileC2sDelaySamples < 50) {
      throw new Error(`requested delay was not actually realized: ${JSON.stringify(slow)}`);
    }
    const errors = clients.flatMap(client => client.events).filter(event => ["error", "exception"].includes(event.type));
    if (errors.length) throw new Error(`browser errors: ${JSON.stringify(errors)}`);
    const result = await evidence.succeed({ ok: true, transport: "rtc", quality,
      structuredQualityWindows: windows.length, prometheusQualityGauges: true,
      ordinaryDiagnosticsDisabled: true, realizedSlowProfile: slow,
      coverage: "real server probes and scoped WASM echoes; sufficient mature samples with realized delay; enforcement not covered" }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    const originalStack = error.stack || error.message;
    const footer = browserE2eFailureFooter(artifacts);
    error.message += `\n${footer}`;
    error.stack = `${originalStack}\n${footer}`;
    throw error;
  } finally {
    await Promise.all(clients.map(client => client.close().catch(() => {})));
    await Promise.all([...browsers.map(browser => stopProcess(browser.child)), stopProcess(server.child)]).catch(() => {});
    for (const browser of browsers) fs.rmSync(browser.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}
main().catch(error => { console.error(error.stack || error); process.exit(1); });
