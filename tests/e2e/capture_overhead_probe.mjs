// What does the QA trace ring cost during ordinary play? The question matters twice over: the harness enables the
// recorder on every E2E lane by design, so a harness measurement is not a player measurement; and before this file
// existed, both bench probes appended `trace=1` unconditionally and reported instrumented-build numbers as if they
// described the shipped client. A capture feature that costs frames is a feature somebody has to agree to pay for,
// and that decision needs a price rather than an argument.
//
// This is a measurement, not an acceptance gate - no number here fails a build. It runs the existing cadence probe
// once with the recorder off and once on per repetition, alternating which goes first so a warm-up penalty or a
// drifting machine cannot masquerade as capture cost. It needs the same local servers as the cadence probe
// (static 7000, game 9001) for the arena being probed.
import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

// This file sits two levels below the repo root, so the climb is three dirnames from the module path itself.
const repoRoot = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));
const arena = process.env.PROBE_ARENA || "chaos-zone";
const repetitions = Number(process.env.PROBE_REPETITIONS || 2);

function cadenceFps(qaTrace) {
  const run = spawnSync(process.execPath, [path.join(repoRoot, "tests/e2e/frame_cadence_probe.mjs")], {
    cwd: repoRoot,
    encoding: "utf8",
    env: { ...process.env, PROBE_ARENA: arena, PROBE_QA_TRACE: qaTrace ? "1" : "0",
      // Without the recorder the readiness key never appears, so let that arm fail in seconds rather than half
      // an hour. A blocked verdict that takes 30 minutes to arrive is not a verdict, it is an outage.
      PROBE_ENTER_ATTEMPTS: qaTrace ? (process.env.PROBE_ENTER_ATTEMPTS || "60") : "2" },
  });
  const line = `${run.stdout}`.split("\n").find((candidate) => candidate.startsWith("CADENCE "));
  if (!line) {
    throw new Error(`the cadence probe returned no result with trace=${qaTrace ? "on" : "off"}: `
      + `${(run.stderr || "").slice(0, 300) || "no stderr"}`);
  }
  const report = JSON.parse(line.slice("CADENCE ".length));
  // Refuse to average a number that mislabels itself. This whole file exists because a probe silently ran with
  // the recorder on; a configuration flag that is not echoed back by the measurement is not a flag.
  if (report.qaTrace !== qaTrace) {
    throw new Error(`the probe reported qaTrace=${report.qaTrace} for a run requested with `
      + `trace=${qaTrace ? "on" : "off"}; refusing to compare numbers whose configuration is not what was asked`);
  }
  const windows = report.samples ?? [];
  if (windows.length === 0) throw new Error("the probe reported no cadence windows");
  return windows.reduce((total, value) => total + value, 0) / windows.length;
}

const median = (values) => {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
};

// The recorder-off arm used to be unmeasurable: `uiMetrics`, and with it the `zoneMapReady` key `enterShip` waits
// for, was published only when QA tracing was enabled (measured: 141 metric keys and zoneMapReady=true
// with the flag, zero keys without it). That coupling is gone - readiness now arrives on the always-on lifecycle
// channel, published only when it changes - so an arm that cannot reach gameplay is a failure of this probe, named
// by arm and repetition, rather than a condition to report as a result.
const rows = [];
for (let repetition = 1; repetition <= repetitions; repetition += 1) {
  const order = repetition % 2 === 1 ? [false, true] : [true, false];
  for (const qaTrace of order) {
    try {
      rows.push({ repetition, qaTrace, fps: cadenceFps(qaTrace) });
    } catch (error) {
      throw new Error(`${qaTrace ? "trace-on" : "trace-off"} arm failed to reach gameplay in repetition `
        + `${repetition}: ${String(error?.message ?? error).slice(0, 400)}`);
    }
  }
  console.log(`repetition ${repetition}: ` + rows.slice(-2)
    .map((row) => `${row.qaTrace ? "trace-on" : "trace-off"} ${row.fps.toFixed(2)} fps`).join(", "));
}
const off = median(rows.filter((row) => !row.qaTrace).map((row) => row.fps));
const on = median(rows.filter((row) => row.qaTrace).map((row) => row.fps));
console.log(JSON.stringify({
  kind: "capture-overhead",
  status: "ok",
  arena,
  repetitions,
  medianFpsWithoutQaTrace: Number(off.toFixed(2)),
  medianFpsWithQaTrace: Number(on.toFixed(2)),
  deltaFps: Number((on - off).toFixed(2)),
  deltaPercent: off > 0 ? Number(((on - off) / off * 100).toFixed(2)) : null,
  note: "measurement only; alternating order per repetition; needs the arena's local servers already running",
}));
