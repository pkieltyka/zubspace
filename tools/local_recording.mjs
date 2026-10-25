#!/usr/bin/env node

// Preserve the standard Make restart lifecycle, with unique local evidence.
// No TOML reimplementation: the real server parses the selected config.
import { createHash } from "node:crypto";
import { spawn, execFileSync } from "node:child_process";
import { createReadStream } from "node:fs";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

export function recordingOptions(args) {
  const result = { config: "etc/server/playtest.toml", directory: ".debug", release: false };
  for (let i = 0; i < args.length; i += 1) {
    if (args[i] === "--release") result.release = true;
    else if (args[i] === "--config" || args[i] === "--directory") {
      const key = args[i].slice(2), value = args[++i];
      if (!value || value.startsWith("--")) throw new Error(`--${key} requires a value`);
      result[key] = value;
    } else throw new Error(`Unknown option: ${args[i]}`);
  }
  return result;
}

export async function fingerprint(filename) {
  try {
    const hash = createHash("sha256");
    let bytes = 0;
    for await (const chunk of createReadStream(filename)) { hash.update(chunk); bytes += chunk.length; }
    return { path: filename, bytes, sha256: hash.digest("hex") };
  } catch (error) {
    if (error.code === "ENOENT") return { path: filename, missing: true };
    throw error;
  }
}

export async function prepareRecording(options, { cwd = process.cwd(), env = process.env, now = new Date() } = {}) {
  const configPath = path.resolve(cwd, options.config);
  // Make's existing recipes expand paths in a shell; reject shell syntax and
  // whitespace instead of promising that arbitrary paths are safely quoted.
  // Fail before any service is stopped when the supplied config is absent.
  await readFile(configPath);
  const root = path.resolve(cwd, options.directory);
  for (const value of [cwd, configPath, root]) {
    if (!/^[a-zA-Z0-9_./:+-]+$/.test(value)) throw new Error("Recording paths must contain only letters, numbers, _ . / : + or -");
  }
  await mkdir(root, { recursive: true, mode: 0o700 });
  const directory = await mkdtemp(path.join(root, `${now.toISOString().replaceAll(":", "-")}-`));
  const serverLog = path.join(directory, "server.log");
  const target = options.release ? "restart-local-release" : "restart-local";
  const makeArgs = ["--no-print-directory", target, `SERVER_CONFIG=${configPath}`,
    `SERVER_LOG=${path.join(directory, "server-console.log")}`, `STATIC_LOG=${path.join(directory, "static.log")}`];
  const childEnv = { ...env, ZUBSPACE_SERVER_LOG_FORMAT: "json", ZUBSPACE_SERVER_LOG_FILE: serverLog };
  const manifest = {
    schema: "zubspace-local-session-v1", launchRequestedAt: now.toISOString(),
    directory, profile: options.release ? "release-client" : "debug-client",
    command: ["make", ...makeArgs], config: await fingerprint(configPath),
    log: { format: "ndjson", path: serverLog, levelOverride: env.ZUBSPACE_SERVER_LOG_LEVEL || env.ZUBSPACE_SERVER_LOG || null,
      targetFilterOverride: env.ZUBSPACE_SERVER_LOG_FILTER ?? null },
    // These flags describe what this launcher does and does not verify.
    acceptanceEvidence: false, readinessVerified: false, startupCommandExitCode: null,
  };
  await writeFile(path.join(directory, "session.json"), `${JSON.stringify(manifest, null, 2)}\n`, { mode: 0o600 });
  return { cwd, directory, makeArgs, childEnv, manifest };
}

export async function finishRecording(recording, code) {
  const { cwd, directory, manifest } = recording;
  const wasm = manifest.profile === "release-client" ? "dist/zubspace.wasm" : "target/wasm32-unknown-unknown/debug/zubspace.wasm";
  const entry = manifest.profile === "release-client" ? "dist/index.html" : "index.html";
  manifest.startupCommandExitCode = code;
  manifest.launchCommandFinishedAt = new Date().toISOString();
  manifest.binaries = await Promise.all(["target/debug/zubspace_server", wasm, entry].map(file => fingerprint(path.join(cwd, file))));
  manifest.configAfterLaunch = await fingerprint(manifest.config.path);
  manifest.configUnchangedDuringLaunch = manifest.config.sha256 === manifest.configAfterLaunch.sha256;
  try {
    manifest.sourceRevision = execFileSync("git", ["rev-parse", "HEAD"], { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
    // Metadata only; never read/hash arbitrary owner working documents.
    manifest.worktreeStatus = execFileSync("git", ["status", "--short", "--untracked-files=no"],
      { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  } catch { manifest.sourceRevision = null; }
  await writeFile(path.join(directory, "session.json"), `${JSON.stringify(manifest, null, 2)}\n`, { mode: 0o600 });
}

async function main() {
  const cwd = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  const recording = await prepareRecording(recordingOptions(process.argv.slice(2)), { cwd });
  console.log(`Recording local playtest in ${recording.directory}`);
  let code = 1;
  try {
    code = await new Promise((resolve, reject) => {
      const child = spawn("make", recording.makeArgs, { cwd, env: recording.childEnv, stdio: "inherit" });
      child.once("error", reject);
      child.once("exit", status => resolve(status ?? 1));
    });
  } finally {
    await finishRecording(recording, code);
  }
  console.log(`Session manifest: ${path.join(recording.directory, "session.json")}`);
  console.log("Launch recorded; use the printed game URL to check readiness. This is not an acceptance run.");
  process.exitCode = code;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
