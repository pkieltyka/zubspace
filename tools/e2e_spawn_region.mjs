#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { spawn, execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import { createSpawnRegionFixture } from "../tests/e2e/fixture_spawn_region.mjs";
import { repoRoot } from "./browser_firefox_rtc_smoke.mjs";

export function parseSpawnRegionArguments(argv) {
  const separator = argv.indexOf("--");
  if (separator < 0 || separator === argv.length - 1) throw new Error("supply a test command after --");
  const options = {};
  for (let i = 0; i < separator; i += 2) {
    const name = argv[i];
    if (!["--zone", "--x", "--y", "--radius", "--out"].includes(name) ||
        i + 1 >= separator || Object.hasOwn(options, name)) throw new Error("invalid or duplicate spawn option");
    options[name] = argv[i + 1];
  }
  if (!["chaos-zone", "extreme"].includes(options["--zone"])) throw new Error("specify --zone chaos-zone or extreme");
  const integer = name => {
    if (!/^(0|[1-9]\d*)$/.test(options[name] ?? "")) throw new Error(`specify an integer ${name}`);
    return Number(options[name]);
  };
  return { zoneId: options["--zone"],
    spawnRegion: { x: integer("--x"), y: integer("--y"), radius: integer("--radius") },
    out: options["--out"], command: argv.slice(separator + 1) };
}

export async function runSpawnRegionTest(options) {
  if (process.env.ZUBSPACE_SERVER_ZONE_ROOT) throw new Error("spawn wrapper cannot replace an existing zone fixture");
  const evidence = path.resolve(options.out || path.join(repoRoot, ".debug",
    `spawn-region-${new Date().toISOString().replaceAll(/[:.]/g, "-")}-${process.pid}`));
  // Refuse reused evidence and leave shipped assets untouched. The existing
  // fixture validates the complete region against conservative map clearance.
  if (fs.existsSync(evidence)) throw new Error("spawn evidence directory already exists");
  const fixture = createSpawnRegionFixture(options);
  try {
    fs.mkdirSync(path.dirname(evidence), { recursive: true });
    fs.mkdirSync(evidence);
    const retainedRoot = path.join(evidence, "zone-root");
    fs.cpSync(fixture.root, retainedRoot, { recursive: true });
    const record = { diagnosticOnly: true, startedAt: new Date().toISOString(),
      sourceHead: execFileSync("git", ["rev-parse", "HEAD"], { cwd: repoRoot, encoding: "utf8" }).trim(),
      sourceTrackedStatus: execFileSync("git", ["status", "--porcelain", "--untracked-files=no"],
        { cwd: repoRoot, encoding: "utf8" }).trim(),
      description: "Explicit single-zone test spawn; does not prove the production random-spawn policy",
      fixture: { ...fixture, root: retainedRoot }, command: options.command,
      originalRandomSpawnAssetsChanged: false };
    fs.writeFileSync(path.join(evidence, "fixture.json"), `${JSON.stringify(record, null, 2)}\n`);
    console.log(`Declared test spawn: ${options.zoneId} tile(${options.spawnRegion.x},${options.spawnRegion.y}), radius ${options.spawnRegion.radius}; evidence ${evidence}`);
    const child = spawn(options.command[0], options.command.slice(1), { cwd: repoRoot, stdio: "inherit",
      env: { ...process.env, ZUBSPACE_SERVER_ZONE_ROOT: retainedRoot,
        ZUBSPACE_SERVER_ZONE: options.zoneId, ZUBSPACE_E2E_ARENA: options.zoneId,
        ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1" } });
    const forward = signal => child.kill(signal);
    const onInt = () => forward("SIGINT"), onTerm = () => forward("SIGTERM");
    process.on("SIGINT", onInt); process.on("SIGTERM", onTerm);
    try {
      const outcome = await new Promise((resolve, reject) => {
        child.once("error", reject);
        child.once("exit", (code, signal) => resolve({ code, signal }));
      });
      fs.writeFileSync(path.join(evidence, "result.json"), `${JSON.stringify({ ...record,
        endedAt: new Date().toISOString(), ...outcome, ok: outcome.code === 0 }, null, 2)}\n`);
      return outcome.code ?? 1;
    } catch (error) {
      fs.writeFileSync(path.join(evidence, "result.json"), `${JSON.stringify({ ...record,
        endedAt: new Date().toISOString(), ok: false, error: error.message }, null, 2)}\n`);
      throw error;
    } finally {
      process.off("SIGINT", onInt); process.off("SIGTERM", onTerm);
    }
  } finally {
    fs.rmSync(fixture.root, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  runSpawnRegionTest(parseSpawnRegionArguments(process.argv.slice(2)))
    .then(code => { process.exitCode = code; })
    .catch(error => { console.error(error.message); process.exitCode = 1; });
}
