#!/usr/bin/env node

import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function usage() {
  return [
    "usage: node tools/dev_multiplayer.mjs [--http-port PORT] [--signal-bind ADDR] [--zone ZONE] [--transport rtc] [--tick-rate HZ] [--bot-count N] [--server-config PATH] [--root DIR] [--print-commands]",
    "",
    "Defaults:",
    "  --http-port 7000",
    "  --signal-bind 127.0.0.1:9001   (HTTP signaling bind; becomes http://<addr>/v1/rtc/offer)",
    "  --zone chaos-zone",
    "  --transport rtc                (deprecated/rtc-only no-op; only 'rtc' is accepted)",
    "  --tick-rate 60",
    "  --bot-count unset",
    "  --server-config unset",
    "  --root .",
    "",
    "Deprecated aliases: --ws-bind (alias of --signal-bind), env WS_BIND (alias of SIGNAL_BIND).",
  ].join("\n");
}

function parseArgs(argv) {
  const config = {
    httpPort: process.env.HTTP_PORT || "7000",
    // Primary env is SIGNAL_BIND; WS_BIND is a deprecated alias kept for
    // back-compat. This sets the HTTP signaling bind (becomes
    // http://<addr>/v1/rtc/offer), not a WebSocket gameplay transport.
    signalBind: process.env.SIGNAL_BIND || process.env.WS_BIND || "127.0.0.1:9001",
    zone: process.env.ZONE || "chaos-zone",
    transport: process.env.TRANSPORT || "rtc",
    tickRate: process.env.TICK_RATE || "60",
    botCount: process.env.BOT_COUNT || "",
    serverConfig: process.env.SERVER_CONFIG || "",
    root: process.env.ROOT || ".",
    printCommands: false,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--help" || arg === "-h") {
      console.log(usage());
      process.exit(0);
    }
    if (arg === "--print-commands") {
      config.printCommands = true;
      continue;
    }

    const value = argv[index + 1];
    if (!value) {
      throw new Error(`${arg} requires a value\n${usage()}`);
    }
    index += 1;

    if (arg === "--http-port") {
      config.httpPort = value;
    } else if (arg === "--signal-bind" || arg === "--ws-bind") {
      // --ws-bind is a deprecated alias of --signal-bind. Both set the
      // HTTP signaling bind address (used to build http://<addr>/v1/rtc/offer).
      config.signalBind = value;
    } else if (arg === "--zone") {
      config.zone = value;
    } else if (arg === "--transport") {
      // Deprecated/rtc-only: the only accepted value is "rtc"; this flag is
      // a no-op kept for back-compat.
      if (value !== "rtc") {
        throw new Error(`--transport must be rtc\n${usage()}`);
      }
      config.transport = value;
    } else if (arg === "--tick-rate") {
      config.tickRate = value;
    } else if (arg === "--bot-count") {
      config.botCount = value;
    } else if (arg === "--server-config") {
      config.serverConfig = value;
    } else if (arg === "--root") {
      config.root = value;
    } else {
      throw new Error(`unknown option: ${arg}\n${usage()}`);
    }
  }

  return config;
}

function prefixedPipe(stream, prefix, target) {
  stream.setEncoding("utf8");
  stream.on("data", (chunk) => {
    for (const line of chunk.split(/\r?\n/)) {
      if (line.length > 0) {
        target.write(`[${prefix}] ${line}\n`);
      }
    }
  });
}

function spawnChild(label, command, args, env = {}) {
  const child = spawn(command, args, {
    cwd: repoRoot,
    env: { ...process.env, ...env },
    stdio: ["ignore", "pipe", "pipe"],
  });
  prefixedPipe(child.stdout, label, process.stdout);
  prefixedPipe(child.stderr, label, process.stderr);
  return child;
}

function shellQuote(value) {
  if (/^[A-Za-z0-9_./:=+-]+$/.test(value)) {
    return value;
  }
  return `'${value.replaceAll("'", "'\\''")}'`;
}

function commandLine(command, args) {
  return [command, ...args].map(shellQuote).join(" ");
}

let children = [];
let shuttingDown = false;

function stopChildren() {
  if (shuttingDown) {
    return;
  }
  shuttingDown = true;
  for (const child of children) {
    if (!child.killed && child.exitCode === null) {
      child.kill("SIGINT");
    }
  }
}

try {
  const config = parseArgs(process.argv.slice(2));
  const servedRoot = path.resolve(repoRoot, config.root);
  const rtcUrl = `http://${config.signalBind}/v1/rtc/offer`;
  const gameUrl = `http://127.0.0.1:${config.httpPort}/` + (config.zone === "chaos-zone" ? "" : `?zone=${encodeURIComponent(config.zone)}`);

  console.log("starting zubspace local multiplayer dev servers");
  console.log(`open ${gameUrl}`);

  const serverArgs = [
    "run",
    "--bin",
    "zubspace_server",
    "--",
    "--bind",
    config.signalBind,
    "--zone",
    config.zone,
    "--tick-rate",
    config.tickRate,
  ];
  if (config.serverConfig) {
    serverArgs.splice(4, 0, "--config", config.serverConfig);
  }
  if (config.botCount) {
    serverArgs.push("--bot-count", config.botCount);
  }
  const staticArgs = ["tools/dev_static_server.mjs", config.httpPort, servedRoot];

  if (config.printCommands) {
    console.log(commandLine("cargo", serverArgs));
    console.log(`RTC_OFFER_ENDPOINT=${shellQuote(rtcUrl)} ${commandLine("node", staticArgs)}`);
    process.exit(0);
  }

  children = [
    spawnChild("server", "cargo", serverArgs),
    spawnChild("static", "node", staticArgs, { RTC_OFFER_ENDPOINT: rtcUrl }),
  ];

  for (const child of children) {
    child.on("exit", (code, signal) => {
      if (!shuttingDown && code !== 0) {
        console.error(`child exited: ${signal || code}`);
        stopChildren();
        process.exitCode = code || 1;
      }
    });
  }

  process.on("SIGINT", () => {
    stopChildren();
  });
  process.on("SIGTERM", () => {
    stopChildren();
  });
} catch (error) {
  console.error(error.message);
  process.exit(1);
}
