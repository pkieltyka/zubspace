import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, mkdir, readFile, writeFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { recordingOptions, prepareRecording, finishRecording, fingerprint } from "../tools/local_recording.mjs";

async function fixture(t) {
  const cwd = await mkdtemp(path.join(os.tmpdir(), "zubspace-recording-test-"));
  t.after(() => rm(cwd, { recursive: true, force: true }));
  await writeFile(path.join(cwd, "server.toml"), '[server]\nlog_level = "info"\nsignaling_auth_token = "config-secret"\n');
  return { cwd, options: { config: "server.toml", directory: ".debug", release: false } };
}

test("recordings remain distinct at the same timestamp and never truncate an earlier log", async t => {
  const { cwd, options } = await fixture(t);
  const settings = { cwd, env: {}, now: new Date("2000-01-01T12:00:00.123Z") };
  const first = await prepareRecording(options, settings);
  await writeFile(first.childEnv.ZUBSPACE_SERVER_LOG_FILE, '{"event":"original"}\n');
  const second = await prepareRecording(options, settings);
  assert.notEqual(first.directory, second.directory);
  assert.ok(first.directory.startsWith(path.join(cwd, ".debug", "2000-01-01T12-00-00.123Z-")));
  assert.equal(await readFile(first.childEnv.ZUBSPACE_SERVER_LOG_FILE, "utf8"), '{"event":"original"}\n');
  assert.equal(first.childEnv.ZUBSPACE_SERVER_LOG_FORMAT, "json");
  assert.ok(first.makeArgs.includes(`SERVER_LOG=${path.join(first.directory, "server-console.log")}`));
  assert.notEqual(first.childEnv.ZUBSPACE_SERVER_LOG_FILE, path.join(first.directory, "server-console.log"));
});

test("custom root, existing server settings and release lifecycle survive without copying secrets into the manifest", async t => {
  const { cwd, options } = await fixture(t);
  const env = { ZUBSPACE_SERVER_LOG_LEVEL: "debug", ZUBSPACE_SERVER_SIGNALING_AUTH_TOKEN: "env-secret",
    ZUBSPACE_SERVER_BOT_SHIP_TYPES: "1,4,5,8", ZUBSPACE_SERVER_LOG_FILE: "/tmp/old-file", ZUBSPACE_SERVER_LOG_FILTER: "" };
  const result = await prepareRecording({ ...options, directory: "recordings", release: true }, { cwd, env });
  assert.ok(result.directory.startsWith(path.join(cwd, "recordings")));
  assert.ok(result.makeArgs.includes("restart-local-release"));
  assert.equal(result.childEnv.ZUBSPACE_SERVER_SIGNALING_AUTH_TOKEN, "env-secret");
  assert.equal(result.childEnv.ZUBSPACE_SERVER_BOT_SHIP_TYPES, "1,4,5,8");
  assert.equal(result.manifest.log.levelOverride, "debug");
  assert.equal(result.childEnv.ZUBSPACE_SERVER_LOG_FILTER, "");
  assert.equal(result.manifest.log.targetFilterOverride, "");
  const serialized = await readFile(path.join(result.directory, "session.json"), "utf8");
  assert.ok(!serialized.includes("env-secret") && !serialized.includes("config-secret"));
});

test("failed launches retain their nonzero result, missing binaries and changed configuration honestly", async t => {
  const { cwd, options } = await fixture(t);
  const recording = await prepareRecording(options, { cwd, env: {} });
  await writeFile(path.join(cwd, "server.toml"), '[server]\nlog_level = "debug"\n');
  await finishRecording(recording, 17);
  const manifest = JSON.parse(await readFile(path.join(recording.directory, "session.json"), "utf8"));
  assert.equal(manifest.startupCommandExitCode, 17);
  assert.equal(manifest.readinessVerified, false);
  assert.equal(manifest.acceptanceEvidence, false);
  assert.equal(manifest.configUnchangedDuringLaunch, false);
  assert.ok(manifest.binaries.every(binary => binary.missing));
});

test("artifact identity is the actual file content, and does not claim source provenance from a revision alone", async t => {
  const { cwd, options } = await fixture(t);
  await mkdir(path.join(cwd, "target/debug"), { recursive: true });
  await writeFile(path.join(cwd, "target/debug/zubspace_server"), "abc");
  const recording = await prepareRecording(options, { cwd, env: {} });
  await finishRecording(recording, 0);
  assert.equal(recording.manifest.binaries[0].sha256, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
  assert.equal(recording.manifest.configUnchangedDuringLaunch, true);
  assert.equal(recording.manifest.sourceRevision, null);
  assert.equal(recording.manifest.readinessVerified, false);
  assert.deepEqual(await fingerprint(path.join(cwd, "missing")), { path: path.join(cwd, "missing"), missing: true });
});

test("invalid config or shell-active paths fail before calling the restart lifecycle", async t => {
  const { cwd, options } = await fixture(t);
  for (const directory of ["with space", "semi;colon", "`id`", "$(id)", "quote'", "double\""]) {
    await assert.rejects(prepareRecording({ ...options, directory }, { cwd, env: {} }), /Recording paths/);
  }
  await assert.rejects(prepareRecording({ ...options, config: "absent.toml" }, { cwd, env: {} }), /ENOENT/);
  for (const args of [["--bogus"], ["--directory"], ["--config", "--release"]]) assert.throws(() => recordingOptions(args));
});

test("the actual launcher preserves a failed make exit and writes its manifest without starting real services", async t => {
  const { cwd } = await fixture(t);
  const bin = path.join(cwd, "bin");
  await mkdir(bin);
  await writeFile(path.join(bin, "make"), '#!/bin/sh\nprintf \'%s\\n\' \'{"event":"stub-launch"}\' > "$ZUBSPACE_SERVER_LOG_FILE"\nexit 17\n', { mode: 0o700 });
  const execute = promisify(execFile);
  const launcher = fileURLToPath(new URL("../tools/local_recording.mjs", import.meta.url));
  await assert.rejects(execute(process.execPath, [launcher, "--config", path.join(cwd, "server.toml"), "--directory", path.join(cwd, "captures")], {
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, ZUBSPACE_SERVER_SIGNALING_AUTH_TOKEN: "never-record-this" }, timeout: 10_000,
  }), error => {
    assert.equal(error.code, 17);
    assert.match(error.stdout, /Session manifest:/);
    return true;
  });
  const { readdir } = await import("node:fs/promises");
  const [session] = await readdir(path.join(cwd, "captures"));
  const directory = path.join(cwd, "captures", session);
  const raw = await readFile(path.join(directory, "session.json"), "utf8");
  assert.ok(!raw.includes("never-record-this"));
  assert.equal(JSON.parse(raw).startupCommandExitCode, 17);
  assert.deepEqual(JSON.parse((await readFile(path.join(directory, "server.log"), "utf8")).trim()), { event: "stub-launch" });
});
