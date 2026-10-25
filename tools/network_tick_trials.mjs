#!/usr/bin/env node

import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function usage() {
  return [
    "usage: node tools/network_tick_trials.mjs [--tick-rates 40,60] [--arena chaos-zone] [--sample-ms 3000]",
    "",
    "Runs focused two-browser position relay smokes and prints a compact comparison table.",
  ].join("\n");
}

function parseList(value) {
  return value
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}

function parseArgs(argv) {
  const config = {
    tickRates: parseList(process.env.ZUBSPACE_TRIAL_TICK_RATES || "40,60"),
    arena: process.env.ZUBSPACE_TRIAL_ARENA || "chaos-zone",
    sampleMs: process.env.ZUBSPACE_TRIAL_SAMPLE_MS || "3000",
  };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--help" || arg === "-h") {
      console.log(usage());
      process.exit(0);
    }
    const value = argv[index + 1];
    if (!value) {
      throw new Error(`${arg} requires a value\n${usage()}`);
    }
    index += 1;

    if (arg === "--tick-rates") {
      config.tickRates = parseList(value);
    } else if (arg === "--arena") {
      config.arena = value;
    } else if (arg === "--sample-ms") {
      config.sampleMs = value;
    } else {
      throw new Error(`unknown option: ${arg}\n${usage()}`);
    }
  }

  for (const tickRate of config.tickRates) {
    if (!/^[1-9][0-9]*$/.test(tickRate)) {
      throw new Error(`unsupported tick rate ${JSON.stringify(tickRate)}; expected a positive integer`);
    }
  }
  if (!/^[0-9]+$/.test(config.sampleMs)) {
    throw new Error(`unsupported sample ms ${JSON.stringify(config.sampleMs)}; expected an integer`);
  }

  return config;
}

function runTrial({ tickRate, arena, sampleMs }) {
  return new Promise((resolve, reject) => {
    const child = spawn("node", ["tools/browser_real_server_smoke.mjs"], {
      cwd: repoRoot,
      env: {
        ...process.env,
        ZUBSPACE_SMOKE_TICK_RATE: tickRate,
        ZUBSPACE_SMOKE_ARENA: arena,
        ZUBSPACE_SMOKE_STOP_AFTER_POSITION: "1",
        ZUBSPACE_SMOKE_POSITION_SAMPLE_MS: sampleMs,
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", reject);
    child.on("exit", (code, signal) => {
      if (code !== 0) {
        reject(
          new Error(
            `trial failed tickRate=${tickRate} exit=${signal || code}\n${stderr || stdout}`,
          ),
        );
        return;
      }
      try {
        resolve(JSON.parse(stdout));
      } catch (error) {
        reject(
          new Error(
            `trial did not produce JSON tickRate=${tickRate}: ${error.message}\n${stdout}\n${stderr}`,
          ),
        );
      }
    });
  });
}

function average(values) {
  return values.reduce((sum, value) => sum + value, 0) / values.length;
}

function summarize(result) {
  const sentRates = [
    result.alpha.positionTraffic.sentPerSecond,
    result.beta.positionTraffic.sentPerSecond,
  ];
  const receivedRates = [
    result.alpha.positionTraffic.receivedPerSecond,
    result.beta.positionTraffic.receivedPerSecond,
  ];
  return {
    transport: result.transport,
    tickRate: result.tickRate,
    exchangeMs: result.positionExchangeMs,
    elapsedMs: result.positionTrafficElapsedMs,
    avgSentPerSecond: Number(average(sentRates).toFixed(1)),
    avgReceivedPerSecond: Number(average(receivedRates).toFixed(1)),
    alphaSentPerSecond: result.alpha.positionTraffic.sentPerSecond,
    betaSentPerSecond: result.beta.positionTraffic.sentPerSecond,
    alphaReceivedPerSecond: result.alpha.positionTraffic.receivedPerSecond,
    betaReceivedPerSecond: result.beta.positionTraffic.receivedPerSecond,
    avgReceivedP95GapMs: Number(
      average([
        result.alpha.positionTraffic.receivedTimestampGaps?.p95Ms ?? 0,
        result.beta.positionTraffic.receivedTimestampGaps?.p95Ms ?? 0,
      ]).toFixed(1),
    ),
    maxSteadyGapMs: Math.max(
      result.alpha.positionTraffic.receivedTimestampGaps?.maxMs ?? 0,
      result.beta.positionTraffic.receivedTimestampGaps?.maxMs ?? 0,
    ),
    largeGapCount:
      (result.alpha.positionTraffic.receivedTimestampGaps?.largeGapsOver250Ms ?? 0) +
      (result.beta.positionTraffic.receivedTimestampGaps?.largeGapsOver250Ms ?? 0),
  };
}

function printTable(rows) {
  const headers = [
    "transport",
    "tick",
    "exchange_ms",
    "avg_sent/s",
    "avg_recv/s",
    "alpha_sent/s",
    "beta_sent/s",
    "alpha_recv/s",
    "beta_recv/s",
    "p95_gap_ms",
    "max_gap_ms",
    "large_gaps",
  ];
  const data = rows.map((row) => [
    row.transport,
    String(row.tickRate),
    String(row.exchangeMs),
    String(row.avgSentPerSecond),
    String(row.avgReceivedPerSecond),
    String(row.alphaSentPerSecond),
    String(row.betaSentPerSecond),
    String(row.alphaReceivedPerSecond),
    String(row.betaReceivedPerSecond),
    String(row.avgReceivedP95GapMs),
    String(row.maxSteadyGapMs),
    String(row.largeGapCount),
  ]);
  const widths = headers.map((header, index) =>
    Math.max(header.length, ...data.map((row) => row[index].length)),
  );
  const line = (columns) =>
    columns.map((column, index) => column.padStart(widths[index])).join("  ");
  console.log(line(headers));
  console.log(line(widths.map((width) => "-".repeat(width))));
  for (const row of data) {
    console.log(line(row));
  }
}

async function main() {
  const config = parseArgs(process.argv.slice(2));
  const rows = [];
  for (const tickRate of config.tickRates) {
    process.stderr.write(`running RTC trial tickRate=${tickRate}\n`);
    const result = await runTrial({
      tickRate,
      arena: config.arena,
      sampleMs: config.sampleMs,
    });
    rows.push(summarize(result));
  }
  printTable(rows);
  console.log(JSON.stringify({ ok: true, arena: config.arena, sampleMs: Number(config.sampleMs), rows }, null, 2));
}

main().catch((error) => {
  console.error(error.message);
  process.exit(1);
});
