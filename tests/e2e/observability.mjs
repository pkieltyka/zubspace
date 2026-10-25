#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import fs from "node:fs";
import { freePort, startRealServer, startStaticServer, stopProcess } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { startChrome, waitForChrome, openChromeClient, waitUntil } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";

async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/observability.mjs" });
  const serverPort = await freePort(), metricsPort = await freePort();
  const staticServer = await startStaticServer();
  const previousMetrics = process.env.ZUBSPACE_SERVER_METRICS_LISTEN;
  const previousFormat = process.env.ZUBSPACE_SERVER_LOG_FORMAT;
  process.env.ZUBSPACE_SERVER_METRICS_LISTEN = `127.0.0.1:${metricsPort}`;
  process.env.ZUBSPACE_SERVER_LOG_FORMAT = "json";
  const server = startRealServer(serverPort);
  if (previousMetrics === undefined) delete process.env.ZUBSPACE_SERVER_METRICS_LISTEN;
  else process.env.ZUBSPACE_SERVER_METRICS_LISTEN = previousMetrics;
  if (previousFormat === undefined) delete process.env.ZUBSPACE_SERVER_LOG_FORMAT;
  else process.env.ZUBSPACE_SERVER_LOG_FORMAT = previousFormat;
  const alphaChrome = startChrome(await freePort(), "observability-alpha");
  const betaChrome = startChrome(await freePort(), "observability-beta");
  let alpha, beta;
  const base = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(`http://127.0.0.1:${serverPort}/v1/rtc/offer`)}&arena=chaos-zone`;
  const urls = [`${base}&name=metricsalpha`, `${base}&name=metricsbeta`];
  const capture = () => ({ server, serverPort, staticServer, urls,
    clients: [{ label: "alpha", client: alpha }, { label: "beta", client: beta }],
    extraLogs: { "alpha-chrome": alphaChrome.logs, "beta-chrome": betaChrome.logs },
    disconnect: async () => { for (const client of [alpha, beta]) await client?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}); },
  });
  let exposition;
  try {
    await waitUntil("JSON logging server HTTP readiness", async () => { try { return (await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`)).ok; } catch { return false; } }, 15_000);
    await Promise.all([waitForChrome(alphaChrome), waitForChrome(betaChrome)]);
    [alpha, beta] = await Promise.all([openChromeClient(alphaChrome, urls[0]), openChromeClient(betaChrome, urls[1])]);
    await Promise.all([alpha.enterShip(), beta.enterShip()]);
    // Ordinary URLs deliberately never request the expensive QA HUD metrics.
    await waitUntil("ordinary client lifecycle readiness", async () => await alpha.evaluate("window.__zubspaceLifecycle?.inGameplay === true") && await beta.evaluate("window.__zubspaceLifecycle?.inGameplay === true"), 20_000);
    await alpha.press("ArrowUp", 600);
    const metric = (text, name) => Number(text.match(new RegExp(`^${name} ([0-9.e+-]+)$`, "m"))?.[1] || 0);
    await waitUntil("real visible browser reports and snapshot ACK timing", async () => {
      exposition = await (await fetch(`http://127.0.0.1:${metricsPort}/metrics`)).text();
      return metric(exposition, "zubspace_browser_reports_total") >= 2
        && metric(exposition, "zubspace_browser_frames_total") > 100
        && metric(exposition, "zubspace_snapshot_ack_round_trip_seconds_count") > 0;
    }, 25_000);
    if (!exposition.includes("# TYPE zubspace_tick_work_seconds histogram") || /player_id=|session_id=|name=/.test(exposition)) throw new Error("invalid or high cardinality exposition");
    const publicMetrics = await fetch(`http://127.0.0.1:${serverPort}/metrics`);
    if (publicMetrics.status !== 404) throw new Error("metrics leaked onto public signaling listener");
    if (await alpha.evaluate("Boolean(window.__zubspaceUiMetrics)")) throw new Error("production telemetry enabled expensive HUD diagnostic payload");
    const logs = server.logs.join("").trim().split("\n").map(line => JSON.parse(line));
    if (logs.length === 0 || logs.some(event => !/\.\d{3,}Z$/.test(event.timestamp) || !["ERROR", "WARN", "INFO", "DEBUG", "TRACE"].includes(event.level) || !event.target || !event.fields)) throw new Error("JSON logs lack precise UTC timestamp, severity, target or fields");
    if (process.env.ZUBSPACE_SMOKE_LOG_LEVEL === "info" && logs.some(event => ["DEBUG", "TRACE"].includes(event.level))) throw new Error("info severity filter leaked verbose logs");
    const result = await evidence.succeed({ ok: true, ordinaryUrl: true, structuredLogs: true, preciseUtcTimestamps: true,
      acceptedBrowserReports: metric(exposition, "zubspace_browser_reports_total"),
      visibleFrames: metric(exposition, "zubspace_browser_frames_total"),
      snapshotAckSamples: metric(exposition, "zubspace_snapshot_ack_round_trip_seconds_count"),
      iceSamples: metric(exposition, "zubspace_browser_ice_round_trip_seconds_count"),
      metricsExposition: exposition, publicMetricsStatus: publicMetrics.status }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) { const retained = await evidence.fail(error, capture()); console.error(retained); throw error; }
  finally {
    await alpha?.close().catch(() => {}); await beta?.close().catch(() => {});
    await Promise.all([stopProcess(alphaChrome.child), stopProcess(betaChrome.child), stopProcess(server.child)]);
    for (const browser of [alphaChrome, betaChrome]) fs.rmSync(browser.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}
main().catch(error => { console.error(error.stack || error); process.exit(1); });
