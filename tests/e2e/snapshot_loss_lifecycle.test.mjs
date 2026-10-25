import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

// Run the actual caller and visibility assertion with a bounded rolling trace.
// Virtual flight consumes35s, as in the retained cef1ca0 failure. This verifies
// observation ordering, not browser delivery or the game's lifecycle behavior.
async function replay({ hidden = true, life = 2, loss = true, snapshotDeath = true,
  reliableAt = 10_000, flightMs = 35_000 } = {}) {
  let now = 1_000, rendezvousCalls = 0;
  const reads = [], waits = [];
  const death = { at: 1_100, playerRespawnSeq: 1, playerRespawnTimer: 1.2,
    playerEnergy: 0, playerAlive: false, serverPlayerAlive: false };
  const remote = { id: 0, lifeSeq: life, displayX: 8_200, displayY: 8_200, serverEnergy: 1_700 };
  const shooter = {
    pageState: async () => ({ uiMetrics: { remotePlayers: now >= 3_000 ? [remote] : [] } }),
    lifecycleState: async () => {
      reads.push({ kind: "remote", at: now });
      return { lifecycleTrace: [
        ...(hidden && now < 7_000 ? [{ at: 1_100, remotePlayers: [] }] : []),
        ...(now >= 3_000 ? [{ at: now, remotePlayers: [remote] }] : []),
      ] };
    },
  };
  const victim = { pageState: async () => {
    reads.push({ kind: "victim", at: now });
    return {
      uiMetrics: { playerRespawnSeq: now >= 3_000 ? 1 : 0,
        latestSnapshotServerTick: 300, playerAlive: true, serverPlayerAlive: true,
        playerEnergy: 1_700, playerMaxEnergy: 1_700 },
      networkStats: { deathDeliveryPacketsDropped: loss ? 1 : 0, deathDeliveryLastDroppedReliableId: 37 },
      metricsTrace: snapshotDeath && now < 20_000 ? [death] : [],
      deathTrace: now >= reliableAt ? [{ at: reliableAt, reliableId: 37, killerId: 1, killedId: 0 }] : [],
    };
  } };
  const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
  const section = (start, end) => source.slice(source.indexOf(start), source.indexOf(end, source.indexOf(start)));
  const ctx = vm.createContext({ Date: { now: () => now }, Number, Promise,
    waitUntil: async (label, predicate, timeout) => {
      waits.push({ label, timeout, at: now }); const deadline = now + timeout;
      while (now <= deadline) { if (await predicate()) return; now += 100; }
      throw Error(`timed out waiting for ${label}`);
    },
    rendezvousPlayers: async () => { if (++rendezvousCalls > 1) now += flightMs; return { calls: rendezvousCalls }; },
    waitForRespawnCycle: async () => { assert(now < 7_000, "retain the death before its rolling trace expires"); now = Math.max(now, 3_000); },
    assertDeadInputSuppressed: async () => ({ probeStartedAt: 1_100 }),
    parseServerRespawnEvents: () => [{ playerId: 0, lifeSeq: 2, serverTick: 300 }],
    fireUntilDamage: async options => {
      if (options.requireLethal) { now = 1_400; await options.onServerDamageEvent(); }
      return { event: { victimLifeSeq: options.requireLethal ? 1 : 2 }, nextSinceCount: 1, attempts: 1 };
    },
  });
  vm.runInContext(section("function deathMatches(", "function selfDeathEvents("), ctx);
  vm.runInContext(section("async function assertRemoteDeathRespawnVisibility(", "async function fireUntilDamage("), ctx);
  vm.runInContext(section("async function runLossySnapshotDamageCycle(", "async function runMovementCorrectionRecoveryGate("), ctx);
  const result = await ctx.runLossySnapshotDamageCycle({ cycle: 1, shooter, victim, server: { logs: "" }, shooterId: 1, victimId: 0, sinceCount: 0 });
  return { result, reads, waits };
}

test("lossy respawn retains death evidence before a long flight and checks the new exact life afterward", async () => {
  const { result, reads, waits } = await replay();
  assert.equal(result.remoteVisibility.hiddenAt, 1_100);
  assert.equal(result.remoteVisibility.reappearedLifeSeq, 2);
  assert.equal(result.death.lifecycleObservedAt, 1_100);
  assert.equal(result.death.firstReliableDeathAt, 10_000, "refresh the eventual reliable retry after flight");
  assert.equal(result.lifecycleDelivery.processedBeforeReliableResend, true);
  assert.equal(result.postRespawnDamage.victimLifeSeq, 2);
  assert(reads.some(read => read.kind === "victim" && read.at < 3_000));
  assert(waits.some(wait => wait.label.includes("disappears") && wait.timeout === 6_000));
  assert(waits.some(wait => wait.label.includes("reappears") && wait.timeout === 12_000));
  assert(waits.some(wait => wait.label.includes("respawn recovery") && wait.timeout === 12_000));
});

test("short-flight lifecycle evidence is preserved too", async () => {
  const { result } = await replay({ flightMs: 1_000 });
  assert.equal(result.death.lifecycleObservedAt, 1_100);
  assert.equal(result.remoteVisibility.reappearedLifeSeq, 2);
});

test("a missing remote disappearance still fails", async () => {
  await assert.rejects(replay({ hidden: false }), /remote victim disappears/);
});

test("a wrong post-respawn generation still fails", async () => {
  await assert.rejects(replay({ life: 3 }), /remote victim reappears/);
});

test("absence of the forced reliable-copy omission still fails", async () => {
  await assert.rejects(replay({ loss: false }), /forced reliable death-copy loss/);
});

test("absence of snapshot-driven death still fails", async () => {
  await assert.rejects(replay({ snapshotDeath: false }), /snapshot lifecycle death evidence/);
});

test("a reliable death arriving before the dead-input probe cannot masquerade as snapshot recovery", async () => {
  await assert.rejects(replay({ reliableAt: 1_050 }), /snapshot lifecycle before reliable death resend/);
});
