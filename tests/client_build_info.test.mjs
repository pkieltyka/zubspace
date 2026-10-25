import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import test from "node:test";
import { clientBuildInfo, sourceRevision, stampClientBuildInfo } from "../tools/client_build_info.mjs";

const cleanEnv = () => ({ ...process.env, ZUBSPACE_BUILD_COMMIT: "", ZUBSPACE_REQUIRE_BUILD_REVISION: "" });
function fixture(t, git = false) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-build-info-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.writeFileSync(path.join(root, "Cargo.toml"), '[package]\nname = "test"\nversion = "1.2.3"\n');
  if (git) {
    const run = (...args) => execFileSync("git", args, { cwd: root, env: cleanEnv(), stdio: "pipe" });
    run("init", "--quiet");run("config", "user.name", "Build Test");run("config", "user.email", "build@example.invalid");
    run("config", "core.hooksPath", "/dev/null");run("config", "commit.gpgsign", "false");
    run("add", ".");run("commit", "--quiet", "-m", "fixture");
  }
  return root;
}

test("checkout revision and eight-character footer agree with Git", t => {
  const root = fixture(t, true), env = cleanEnv();
  const head = execFileSync("git", ["rev-parse", "HEAD"], { cwd: root, env, encoding: "utf8" }).trim();
  assert.equal(sourceRevision(root, env), head);
  const info = clientBuildInfo(root, env);
  assert.deepEqual(info, { appVersion: "1.2.3", commit: head.slice(0, 8) });
  assert.equal(stampClientBuildInfo('<span id="menu-build">Development build</span>', info), `<span id="menu-build">v1.2.3 · ${head.slice(0, 8)}</span>`);
  assert.equal(sourceRevision(root, { ...env, ZUBSPACE_BUILD_COMMIT: head }), head);
  assert.throws(() => sourceRevision(root, { ...env, ZUBSPACE_BUILD_COMMIT: "a".repeat(40) === head ? "b".repeat(40) : "a".repeat(40) }), /does not match/);
});

test("container/archive revision needs no Git directory and supports full Git hash formats", t => {
  const root = fixture(t);
  for (const revision of ["0123456789abcdef".repeat(2) + "01234567", "0123456789abcdef".repeat(4)]) {
    const env = { ...cleanEnv(), ZUBSPACE_BUILD_COMMIT: revision, ZUBSPACE_REQUIRE_BUILD_REVISION: "1" };
    assert.equal(sourceRevision(root, env), revision);
    assert.equal(clientBuildInfo(root, env).commit, revision.slice(0, 8));
  }
  assert.equal(fs.existsSync(path.join(root, ".git")), false);
});

test("ordinary source archives report unknown honestly; container builds reject missing revision", t => {
  const root = fixture(t), env = cleanEnv();
  assert.equal(sourceRevision(root, env), "nogit");
  assert.throws(() => sourceRevision(root, { ...env, ZUBSPACE_REQUIRE_BUILD_REVISION: "1" }), /Container builds require/);
  for (const revision of ["nogit", "deadbeef", "A".repeat(40), "a".repeat(39), "a".repeat(41), "<script>", "a".repeat(40) + "\n"])
    assert.throws(() => sourceRevision(root, { ...env, ZUBSPACE_BUILD_COMMIT: revision }), /full lowercase hexadecimal/);
});
