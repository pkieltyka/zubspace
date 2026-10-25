import assert from "node:assert/strict";
import test from "node:test";
import { commandWithCpuSet } from "../tools/cpu_affinity.mjs";

test("default launch is unchanged and isolation preserves literal arguments", () => {
  const args = ["--name", "two words", "$(not-a-shell)"];
  assert.deepEqual(commandWithCpuSet("game", args), {command: "game", args});
  assert.deepEqual(commandWithCpuSet("game", args, "0-1,16-17"), {
    command: "taskset", args: ["--cpu-list", "0-1,16-17", "game", ...args],
  });
});

test("invalid and reversed CPU masks fail before spawning", () => {
  for (const mask of ["", "1-0", "-1", "1,", "1;echo x", "all", "1 2", 1,
    "9007199254740992", "0".repeat(4097)]) {
    assert.throws(() => commandWithCpuSet("game", [], mask), /invalid explicit test CPU set/);
  }
});
