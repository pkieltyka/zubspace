import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

const scripts = new URL("../scripts/", import.meta.url);

function fixture(t, runner) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-acceptance-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const repo = path.join(root, "repo"), bin = path.join(root, "bin"), evidence = path.join(root, "evidence");
  fs.mkdirSync(path.join(repo, "scripts"), { recursive: true });
  fs.mkdirSync(bin);
  for (const name of [runner, "acceptance_tree_guard.sh"])
    fs.copyFileSync(new URL(name, scripts), path.join(repo, "scripts", name));
  fs.writeFileSync(path.join(repo, "source.txt"), "original\n");
  const git = (...args) => {
    const result = spawnSync("git", args, { cwd: repo, encoding: "utf8", timeout: 10_000 });
    assert.equal(result.status, 0, result.stderr);
    return result.stdout.trim();
  };
  git("init", "--quiet");
  git("config", "user.name", "Acceptance Test");
  git("config", "user.email", "acceptance@example.invalid");
  git("config", "core.hooksPath", "/dev/null");
  git("add", "."); git("commit", "--quiet", "-m", "fixture");
  const head = git("rev-parse", "HEAD");
  const fake = `#!/bin/sh
printf 'gate\\n' >> "$ACCEPTANCE_CALLS"
case "$ACCEPTANCE_ACTION" in
  edit) printf 'changed\\n' >> source.txt ;;
  untracked) printf 'new\\n' > unexpected.txt ;;
  commit) printf 'changed\\n' >> source.txt; git add source.txt; git commit --quiet -m changed || exit 90 ;;
  fail) exit 9 ;;
esac
`;
  for (const name of ["cargo", "make"]) fs.writeFileSync(path.join(bin, name), fake, { mode: 0o755 });
  return {
    repo, git, head,
    run(action = "", args = ["--from", "1", "--to", "2"]) {
      const result = spawnSync("bash", [path.join(repo, "scripts", runner), ...args], {
        cwd: repo, encoding: "utf8", timeout: 20_000,
        env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, HYPRLAND_INSTANCE_SIGNATURE: "",
          ACCEPTANCE_ACTION: action, ACCEPTANCE_CALLS: path.join(root, "calls"),
          ZUBSPACE_FROZEN_MATRIX_DIR: evidence, ZUBSPACE_FOCUSED_ACCEPTANCE_DIR: evidence },
      });
      assert.ifError(result.error);
      return { ...result, calls: fs.existsSync(path.join(root, "calls"))
        ? fs.readFileSync(path.join(root, "calls"), "utf8").trim().split("\n").length : 0,
        report: fs.existsSync(path.join(evidence, "result.json"))
          ? JSON.parse(fs.readFileSync(path.join(evidence, "result.json"), "utf8")) : null };
    },
  };
}

for (const runner of ["frozen_matrix.sh", "focused_acceptance.sh"]) {
  const focused = runner.startsWith("focused"), records = focused ? "lanes" : "steps";
  const runCount = focused ? "runLanes" : "runSteps", requested = focused ? "requestedLanes" : "requestedSteps";
  test(`${runner}: a clean successful range retains exact HEAD and actual counts`, t => {
    const f = fixture(t, runner), result = f.run();
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.calls, 2);
    assert.equal(result.report.head, f.head);
    assert.equal(result.report.sourceUnchanged, true);
    assert.equal(result.report.failed, 0);
    assert.equal(result.report[runCount], 2);
    assert.equal(result.report[requested], 2);
    assert.ok(result.report[records].every(row => row.exit === 0 && row.gateExit === 0 && row.sourceUnchanged));
  });
  for (const action of ["edit", "untracked", "commit"]) {
    test(`${runner}: ${action} during a passing gate rejects provenance even with keep-going`, t => {
      const f = fixture(t, runner);
      // Repository-local user preferences must not hide newly created source.
      f.git("config", "status.showUntrackedFiles", "no");
      const result = f.run(action, ["--from", "1", "--to", "2", "--keep-going"]);
      assert.equal(result.status, 1, result.stderr);
      assert.equal(result.calls, 1);
      assert.equal(result.report.sourceUnchanged, false);
      assert.equal(result.report.head, f.head);
      assert.equal(result.report[runCount], 1);
      assert.equal(result.report[requested], 2);
      assert.equal(result.report[records][0].gateExit, 0);
      assert.equal(result.report[records][0].exit, 4);
      if (action === "commit") assert.notEqual(f.git("rev-parse", "HEAD"), f.head);
    });
  }
  test(`${runner}: ordinary failure preserves raw exit and reports only attempted gates`, t => {
    const result = fixture(t, runner).run("fail");
    assert.equal(result.status, 1);
    assert.equal(result.calls, 1);
    assert.equal(result.report.sourceUnchanged, true);
    assert.equal(result.report[runCount], 1);
    assert.equal(result.report[requested], 2);
    assert.equal(result.report[records][0].gateExit, 9);
    assert.equal(result.report[records][0].exit, 9);
  });
  test(`${runner}: dirty startup never invokes a gate`, t => {
    const f = fixture(t, runner);
    fs.appendFileSync(path.join(f.repo, "source.txt"), "dirty\n");
    const result = f.run();
    assert.equal(result.status, 3);
    assert.equal(result.calls, 0);
    assert.equal(result.report, null);
  });
  test(`${runner}: invalid ranges and missing values never invoke a gate`, t => {
    const f = fixture(t, runner);
    for (const args of [["--from", "0"], ["--to", "999"], ["--from", "2", "--to", "1"],
      ["--from", "-1"], ["--to", "no"], ["--from", "9999999999999999999999999999"],
      ["--from", "01"], ["--from"], ["--to"]]) {
      const result = f.run("", args);
      assert.equal(result.status, 2, `${args}: ${result.stderr}`);
      assert.equal(result.calls, 0);
      assert.equal(result.report, null);
    }
  });
}
