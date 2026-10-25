import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { assertServerProcessEvidence, captureServerLaunchEvidence, captureServerProcessEvidence, parseServerProcessEvidence } from "./server_process_evidence.mjs";

function fixture() {
  const fields = Array(22).fill("0");
  fields[0] = "S";
  fields[11] = "200";
  fields[12] = "100";
  fields[19] = "1000";
  return { pid: 42, stat: `42 (server name (worker)) ${fields.join(" ")}`,
    status: "VmRSS:\t2048 kB\nVmHWM:\t4096 kB\nThreads:\t4\n",
    uptime: "12.00 8.00", ticksPerSecond: 100 };
}

test("process lifetime CPU includes both CPU modes and is not capped at one core", () => {
  const evidence = parseServerProcessEvidence(fixture());
  assert.equal(evidence.cpuSeconds, 3);
  assert.equal(evidence.lifetimeSeconds, 2);
  assert.equal(evidence.cpuPercentOfOneCore, 150);
  assert.equal(evidence.rssKiB, 2048);
  assert.equal(evidence.highWaterRssKiB, 4096);
  assert.equal(evidence.threads, 4);
});

test("missing and malformed process measurements fail instead of becoming zero", () => {
  for (const patch of [
    { pid: 43 }, { stat: "42 malformed" }, { ticksPerSecond: 0 },
    { uptime: "10.00" }, { uptime: "unknown" }, { status: "Threads: 1" },
    { status: fixture().status.replace("4096", "100") },
  ]) assert.throws(() => parseServerProcessEvidence({ ...fixture(), ...patch }));
  assert.equal(captureServerProcessEvidence(null).available, false);
});

test("live Linux process capture retains explicit scope and positive RSS", () => {
  const evidence = captureServerProcessEvidence(process.pid);
  assert.equal(evidence.available, true, evidence.error);
  assert.equal(evidence.scope, "server-process-lifetime-including-startup");
  assert.ok(evidence.rssKiB > 0);
});

test("completion audit rejects absent, unavailable, mislabeled and inconsistent resources", () => {
  const evidence = parseServerProcessEvidence(fixture());
  assert.equal(assertServerProcessEvidence(evidence), evidence);
  for (const value of [null, {}, { ...evidence, available: false },
    { ...evidence, scope: "steady-state" }, { ...evidence, rssKiB: 0 },
    { ...evidence, cpuSeconds: -1 }, { ...evidence, cpuPercentOfOneCore: 100 },
    { ...evidence, lifetimeSeconds: Infinity }, { ...evidence, threads: 0 },
    { ...evidence, startTicks: -1 }, { ...evidence, error: "capture failed" },
  ]) assert.throws(() => assertServerProcessEvidence(value));
});

test("early server exit retains its attempted listener while process measurements remain unavailable", async () => {
  const child = spawn(process.execPath, ["-e", "process.exit(2)", "127.0.0.1:42689"], { stdio: "ignore" });
  await once(child, "exit");
  assert.equal(captureServerProcessEvidence(child.pid).available, false);
  assert.deepEqual(captureServerLaunchEvidence(child), {
    pid: child.pid, listenAddress: "127.0.0.1:42689", exitCode: 2, signalCode: null,
  });
});
