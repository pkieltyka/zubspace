import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { parseSpawnRegionArguments } from "../../tools/e2e_spawn_region.mjs";
import { repoRoot } from "../../tools/browser_firefox_rtc_smoke.mjs";

const flags = ["--zone", "chaos-zone", "--x", "512", "--y", "512", "--radius", "0"];
const command = [process.execPath, "-e", "process.exit(0)"];
function run(t, { child = command, extraEnv = {}, existingOut = false } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-spawn-runner-unit-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const out = path.join(root, "evidence");
  if (existingOut) fs.mkdirSync(out);
  const env = { ...process.env, ...extraEnv };
  if (!Object.hasOwn(extraEnv, "ZUBSPACE_SERVER_ZONE_ROOT")) delete env.ZUBSPACE_SERVER_ZONE_ROOT;
  const result = spawnSync(process.execPath, ["tools/e2e_spawn_region.mjs", ...flags,
    "--out", out, "--", ...child], { cwd: repoRoot, env, encoding: "utf8" });
  return { result, out };
}

test("explicit spawn arguments preserve executable and literal argument boundaries", () => {
  const parsed = parseSpawnRegionArguments([...flags, "--", "node", "-e", "a; b", "--x"]);
  assert.equal(parsed.zoneId, "chaos-zone");
  assert.deepEqual(parsed.spawnRegion, { x: 512, y: 512, radius: 0 });
  assert.deepEqual(parsed.command, ["node", "-e", "a; b", "--x"]);
  assert.equal(parseSpawnRegionArguments(["--zone", "extreme", ...flags.slice(2), "--", ...command]).zoneId, "extreme");
});

test("ambiguous options, fractional coordinates and missing commands fail before creating a fixture", () => {
  for (const args of [flags, [...flags, "--"], [...flags, "--x", "4", "--", ...command],
    [...flags, "--unknown", "4", "--", ...command],
    [...flags.slice(0, 3), "1.5", ...flags.slice(4), "--", ...command],
    ["--zone", "other", ...flags.slice(2), "--", ...command]]) {
    assert.throws(() => parseSpawnRegionArguments(args));
  }
});

test("child receives the retained exact fixture, with unchanged map and non-Spawn settings", t => {
  const { result, out } = run(t, { child: [process.execPath, "-e", `
    const fs = require('node:fs'), path = require('node:path');
    const settings = fs.readFileSync(path.join(process.env.ZUBSPACE_SERVER_ZONE_ROOT, 'chaos-zone', 'arena.conf'), 'utf8');
    if (process.env.ZUBSPACE_E2E_ARENA !== 'chaos-zone' || process.env.ZUBSPACE_SERVER_ZONE !== 'chaos-zone' ||
        process.env.ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES !== '1' || !settings.includes('Team0-X=-512')) process.exit(8);
  `] });
  assert.equal(result.status, 0, result.stderr);
  const record = JSON.parse(fs.readFileSync(path.join(out, "result.json")));
  assert.equal(record.ok, true); assert.equal(record.diagnosticOnly, true);
  assert.equal(fs.existsSync(record.fixture.root), true);
  const original = path.join(repoRoot, "assets/zones/chaos-zone");
  const retained = path.join(record.fixture.root, "chaos-zone");
  assert.deepEqual(fs.readFileSync(path.join(retained, "bdhadar.lvl")), fs.readFileSync(path.join(original, "bdhadar.lvl")));
  const removeSpawn = text => text.replace(/^(Team[0-3]-(X|Y|Radius))=-?\d+/gm, "$1=<spawn>");
  assert.equal(removeSpawn(fs.readFileSync(path.join(retained, "arena.conf"), "utf8")),
    removeSpawn(fs.readFileSync(path.join(original, "arena.conf"), "utf8")));
});

test("a failing test retains its nonzero exit and fixture evidence", t => {
  const { result, out } = run(t, { child: [process.execPath, "-e", "process.exit(7)"] });
  assert.equal(result.status, 7);
  const record = JSON.parse(fs.readFileSync(path.join(out, "result.json")));
  assert.equal(record.ok, false); assert.equal(record.code, 7);
  assert.equal(fs.existsSync(path.join(record.fixture.root, "chaos-zone/zone.bundle")), true);
});

test("a failed launch is retained as failure rather than a passing zero-work run", t => {
  const { result, out } = run(t, { child: ["/nonexistent/zubspace-test-command"] });
  assert.equal(result.status, 1);
  const record = JSON.parse(fs.readFileSync(path.join(out, "result.json")));
  assert.equal(record.ok, false); assert.match(record.error, /ENOENT/);
});

test("existing output or a pre-existing zone fixture is never silently replaced", t => {
  const existing = run(t, { existingOut: true });
  assert.equal(existing.result.status, 1); assert.match(existing.result.stderr, /already exists/);
  const conflict = run(t, { extraEnv: { ZUBSPACE_SERVER_ZONE_ROOT: "/some/existing/fixture" } });
  assert.equal(conflict.result.status, 1); assert.match(conflict.result.stderr, /existing zone fixture/);
  assert.equal(fs.existsSync(conflict.out), false);
});
