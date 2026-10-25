#!/usr/bin/env node

// Browser-free integration: actual game metrics -> actual container -> PromQL.
// All listeners, container names and volumes are isolated from owner services.
import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { fingerprint } from "./local_recording.mjs";

const exec = promisify(execFile);
const cwd = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const root = path.join(cwd, ".debug");
await mkdir(root, { recursive: true, mode: 0o700 });
const evidence = await mkdtemp(path.join(root, "observability-check-"));
const project = `zubspace-prom-check-${process.pid}`;
const logs = [];
const result = { schema: "zubspace-local-observability-check-v1", startedAt: new Date().toISOString(),
  evidence, project, browserTest: false, status: "running" };
let server;
let compose;

async function run(command, args) {
  try {
    const output = await exec(command, args, { cwd, timeout: 30_000, maxBuffer: 2_000_000 });
    logs.push({ command: [command, ...args], ...output });
    return output.stdout;
  } catch (error) {
    logs.push({ command: [command, ...args], stdout: error.stdout, stderr: error.stderr, error: error.message });
    throw error;
  }
}

async function poll(description, probe) {
  const deadline = Date.now() + 20_000;
  let lastError;
  while (Date.now() < deadline) {
    try { const value = await probe(); if (value) return value; } catch (error) { lastError = error.message; }
    await new Promise(resolve => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out: ${description}${lastError ? ` (${lastError})` : ""}`);
}

async function text(url) {
  const response = await fetch(url, { signal: AbortSignal.timeout(2000) });
  if (!response.ok) throw new Error(`HTTP ${response.status} from ${url}`);
  return response.text();
}

async function stopFixture() {
  if (!server?.pid || server.exitCode !== null || server.signalCode !== null) return;
  const exited = new Promise(resolve => server.once("exit", resolve));
  server.kill("SIGTERM");
  const timeout = setTimeout(() => server.kill("SIGKILL"), 3000);
  try { await exited; } finally { clearTimeout(timeout); }
}

try {
  const leases = [];
  try {
    for (let i = 0; i < 3; i += 1) {
      const listener = net.createServer();
      await new Promise((resolve, reject) => { listener.once("error", reject); listener.listen(0, "127.0.0.1", resolve); });
      leases.push(listener);
    }
    const [gamePort, metricsPort, promPort] = leases.map(listener => listener.address().port);
    result.ports = { gamePort, metricsPort, promPort };
  } finally {
    await Promise.all(leases.map(listener => new Promise(resolve => listener.close(resolve))));
  }
  const { gamePort, metricsPort, promPort } = result.ports;
  const base = `http://127.0.0.1:${promPort}`;
  const scrapeConfig = path.join(evidence, "prometheus.yml");
  // JSON is valid YAML. These overrides leave the checked-in local config intact.
  await writeFile(scrapeConfig, JSON.stringify({
    global: { scrape_interval: "1s", scrape_timeout: "500ms" },
    scrape_configs: [{ job_name: "zubspace", static_configs: [{ targets: [`127.0.0.1:${metricsPort}`] }] }],
  }, null, 2));
  const override = path.join(evidence, "compose.override.json");
  await writeFile(override, JSON.stringify({ services: { prometheus: {
    container_name: project, restart: "no",
    command: ["--config.file=/etc/prometheus/prometheus.yml", "--storage.tsdb.path=/prometheus", `--web.listen-address=127.0.0.1:${promPort}`],
    volumes: [`${scrapeConfig}:/etc/prometheus/prometheus.yml:ro`],
  } } }, null, 2));
  compose = args => run("docker", ["compose", "--project-name", project, "--file", "etc/prom/compose.yml", "--file", override, ...args]);
  const serverLog = path.join(evidence, "server.log");
  const serverArgs = ["--config", "etc/server/playtest.toml", "--bind", `127.0.0.1:${gamePort}`,
    "--metrics-listen", `127.0.0.1:${metricsPort}`, "--bot-count", "0", "--log-format", "json", "--log-level", "debug", "--log-file", serverLog];
  result.server = await fingerprint(path.join(cwd, "target/debug/zubspace_server"));
  result.configs = await Promise.all(["etc/server/playtest.toml", "etc/prom/compose.yml", "etc/prom/prometheus.yml"].map(file => fingerprint(path.join(cwd, file))));
  server = spawn("target/debug/zubspace_server", serverArgs, { cwd, stdio: ["ignore", "pipe", "pipe"] });
  server.on("error", error => logs.push({ serverError: error.message }));
  let consoleOutput = "";
  for (const stream of [server.stdout, server.stderr]) stream.on("data", chunk => { consoleOutput = (consoleOutput + chunk).slice(-1_000_000); });
  result.fixturePid = server.pid;
  await poll("isolated game listener", () => text(`http://127.0.0.1:${gamePort}/v1/status`));
  const advertisedStatus=JSON.parse(await text(`http://127.0.0.1:${gamePort}/v1/status`));
  assert.equal(advertisedStatus.client_debug_trace,true);
  result.clientDebugAdvertised=true;
  await compose(["up", "-d", "prometheus"]);
  await poll("Prometheus readiness", () => text(`${base}/-/ready`));
  const query = async (expression, time) => {
    const params = new URLSearchParams({ query: expression, ...(time ? { time: String(time) } : {}) });
    const response = JSON.parse(await text(`${base}/api/v1/query?${params}`));
    assert.equal(response.status, "success");
    return response.data.result;
  };
  result.scrapeUp = await poll("real game scrape", async () => {
    const samples = await query('up{job="zubspace"}');
    return samples.length === 1 && samples[0].value[1] === "1" ? samples : null;
  });
  result.historyTime = Date.now() / 1000;
  result.beforeRestart = await query('zubspace_uptime_seconds{job="zubspace"}', result.historyTime);
  assert.equal(result.beforeRestart.length, 1);
  await compose(["down"]); // Deliberately keep the test project's data volume.
  await compose(["up", "-d", "prometheus"]);
  await poll("Prometheus after restart", () => text(`${base}/-/ready`));
  result.afterRestart = await query('zubspace_uptime_seconds{job="zubspace"}', result.historyTime);
  assert.deepEqual(result.afterRestart, result.beforeRestart, "historic PromQL sample must survive container removal/recreation");
  result.persistedHistory = true;
  await stopFixture();
  result.scrapeDown = await poll("failed target is reported down", async () => {
    const samples = await query('up{job="zubspace"}');
    return samples.length === 1 && samples[0].value[1] === "0" ? samples : null;
  });
  const lines = (await readFile(serverLog, "utf8")).trim().split("\n").map(line => JSON.parse(line));
  const startup = lines.find(line => line.fields?.message === "zubspace server started");
  assert.ok(startup);
  assert.equal(startup.fields.log_level, "debug");
  assert.equal(startup.fields.log_filter, "rtc=info,webrtc=info");
  result.effectiveLogging = { level: startup.fields.log_level, filter: startup.fields.log_filter };
  for (const line of lines) {
    assert.match(line.timestamp, /T\d\d:\d\d:\d\d\.\d{3,}Z$/);
    assert.ok(["TRACE", "DEBUG", "INFO", "WARN", "ERROR"].includes(line.level));
    assert.equal(typeof line.fields, "object");
  }
  result.structuredLogLines = lines.length;
  await writeFile(path.join(evidence, "server-console.log"), consoleOutput);
  result.status = "passed";
} catch (error) {
  result.status = "failed";
  result.error = error.stack;
  process.exitCode = 1;
} finally {
  await stopFixture();
  if (compose) {
    try { await compose(["logs", "--no-color", "prometheus"]); } catch { /* Keep original verdict. */ }
    try {
      await compose(["down", "--volumes"]);
      const containers = await run("docker", ["ps", "--all", "--format", "{{.Names}}"]);
      const volumes = await run("docker", ["volume", "ls", "--format", "{{.Name}}"]);
      assert.ok(!containers.trim().split("\n").includes(project), "test container must be gone");
      assert.ok(!volumes.trim().split("\n").includes(`${project}_data`), "test volume must be gone");
      assert.ok(!server?.pid || server.exitCode !== null || server.signalCode !== null, "owned server must be reaped");
      result.cleanup = "passed";
    }
    catch (error) { result.cleanup = error.message; result.status = "failed"; process.exitCode = 1; }
  }
  result.finishedAt = new Date().toISOString();
  await writeFile(path.join(evidence, "commands.json"), `${JSON.stringify(logs, null, 2)}\n`);
  await writeFile(path.join(evidence, "result.json"), `${JSON.stringify(result, null, 2)}\n`);
  console.log(`${result.status}: ${path.join(evidence, "result.json")}`);
}
