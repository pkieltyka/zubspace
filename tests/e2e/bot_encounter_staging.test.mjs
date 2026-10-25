import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { botEncounterStageState } from "./bot_encounter_staging.mjs";

const pilot = (targets = []) => ({ playerX: 8200, playerY: 8200,
  playerAlive: true, serverPlayerAlive: true, playerLifeSeq: 1, inSafeZone: false,
  remotePlayers: targets });
const bot = (id, x, y = 8200) => ({ id, lifeSeq: 1, energy: 1700, positionX: x, positionY: y });

test("AOI membership alone cannot establish an on-screen bot encounter", () => {
  const m = { ...pilot([bot(768, 7199, 8040)]), playerX: 6680, playerY: 7128 };
  assert.equal(botEncounterStageState(m, [768], { requireEveryBot: true }).ready, false);
  assert(botEncounterStageState(m, [768]).targets[0].distance > 1000);
});

test("the sprite stage requires every distinct expected live bot, not only a nearby one", () => {
  const m = pilot([bot(768, 8300), bot(769, 9000)]);
  assert.equal(botEncounterStageState(m, [768, 769]).ready, true);
  assert.equal(botEncounterStageState(m, [768, 769], { requireEveryBot: true }).ready, false);
  m.remotePlayers[1] = bot(769, 8100);
  assert.equal(botEncounterStageState(m, [768, 769], { requireEveryBot: true }).ready, true);
  m.remotePlayers.pop();
  assert.equal(botEncounterStageState(m, [768, 769], { requireEveryBot: true }).ready, false);
});

test("dead or non-finite candidates and non-live pilots cannot satisfy encounter readiness", () => {
  for (const candidate of [{ ...bot(768, 8300), energy: 0 }, bot(768, NaN),
    { ...bot(768, 8300), lifeSeq: null }]) {
    assert.equal(botEncounterStageState(pilot([candidate]), [768]).ready, false);
  }
  for (const change of [{ playerAlive: false }, { serverPlayerAlive: false }, { playerX: NaN }]) {
    assert.equal(botEncounterStageState({ ...pilot([bot(768, 8300)]), ...change }, [768]).ready, false);
  }
  const safe = { ...pilot([bot(768, 8300)]), inSafeZone: true };
  assert.equal(botEncounterStageState(safe, [768]).ready, false);
  assert.equal(botEncounterStageState(safe, [768], { allowSafeSource: true }).ready, true);
});

// Exercise the actual coordinator with a virtual navigator. These are setup
// contract tests, not real browser movement or combat acceptance evidence.
async function replay({ fail = null, retireLife = false, exceedBudget = false } = {}) {
  let now = 0, calls = 0;
  const m = pilot([bot(768, 9500)]);
  const actor = { metricsState: async () => ({ uiMetrics: structuredClone(m) }) };
  const source = fs.readFileSync(new URL("./bot_encounter_staging.mjs", import.meta.url), "utf8");
  const ctx = vm.createContext({ Number, Set, Math, Error, Date: { now: () => now },
    exactLiveGameplayScope: metrics => metrics.playerAlive === true && metrics.serverPlayerAlive === true,
    waitUntil: async (_label, predicate, timeout) => {
      for (let i = 0; i < 5; i++) {
        if (await predicate()) return;
        now += 10;
        if (now > timeout) throw new Error("virtual wait expired");
      }
      throw new Error("virtual loop did not converge");
    },
    navigateAuthoredTarget: async (_actor, _destination, _server, options) => {
      calls++;
      if (retireLife && calls === 1) {
        m.playerLifeSeq++;
        throw new Error("authored navigation crossed a lifecycle boundary");
      }
      if (fail) throw new Error(fail);
      now += exceedBudget ? 1100 : 50;
      m.playerX = 9300;
      assert.equal(await options.complete(), true);
      return { keysOnly: true, status: "complete" };
    },
  });
  vm.runInContext(source.slice(source.indexOf("export function")).replaceAll("export ", ""), ctx);
  let result, error;
  try { result = await ctx.approachLiveBotEncounter({ actor, botIds: [768], label: "virtual", timeoutMs: 1000 }); }
  catch (e) { error = e; }
  return { result, error, calls };
}

test("encounter approach can reacquire a legitimately replaced life within one bounded setup", async () => {
  const { result, error, calls } = await replay({ retireLife: true });
  assert.ifError(error); assert.equal(calls, 2); assert.equal(result.approaches.length, 1);
});

test("a still-live route error or late convergence remains a failed encounter setup", async () => {
  const blocked = await replay({ fail: "genuine authored navigation did not reach its planned leg" });
  assert.equal(blocked.result, undefined); assert.match(blocked.error.message, /planned leg/);
  const late = await replay({ exceedBudget: true });
  assert.equal(late.result, undefined); assert.match(late.error.message, /bounded encounter approach budget/);
});

async function replayRadarPreparation({ missingRadar = false, readiness = "radar" } = {}) {
  // Retained focused17 state: both bots are genuinely live and advertised;
  // they need not both fit the later sprite stage's300px preparation radius.
  const m = { ...pilot([bot(768, 9247, 8965), bot(769, 7806, 8073)]),
    playerX: 9127.59, playerY: 9068.44,
    authoritativeSnapshotActors: [{ id: 768, lifeSeq: 1 }, { id: 769, lifeSeq: 1 }],
    radarPlayerIds: missingRadar ? [768] : [768, 769] };
  const source = fs.readFileSync(new URL("./bot_encounter_staging.mjs", import.meta.url), "utf8");
  let calls = 0;
  const ctx = vm.createContext({ Number, Set, Math, Error, Date,
    exactLiveGameplayScope: () => true,
    waitUntil: async (_label, predicate) => {
      if (!await predicate()) throw new Error("radar preparation did not become ready");
    },
    navigateAuthoredTarget: async (_actor, _destination, _server, options) => {
      calls++;
      if (!missingRadar) throw new Error("unexpected flight despite complete radar evidence");
      assert.equal(await options.complete(), false, "a missing radar member cannot satisfy preparation");
      m.radarPlayerIds.push(769);
      assert.equal(await options.complete(), true);
      return { keysOnly: true, status: "complete" };
    },
  });
  vm.runInContext(source.slice(source.indexOf("export function")).replaceAll("export ", ""), ctx);
  const actor = { metricsState: async () => ({ uiMetrics: structuredClone(m) }) };
  const result = await ctx.approachLiveBotEncounter({ actor, botIds: [768, 769],
    requireEveryBot: true, allowSafeSource: true, readiness, label: "captured radar" });
  return { result, calls };
}

test("actual radar preparation accepts captured complete membership without forcing sprite geometry", async () => {
  assert.equal((await replayRadarPreparation()).calls, 0);
});

test("radar preparation still requires every actual radar member and sprite preparation keeps its distance", async () => {
  assert.equal((await replayRadarPreparation({ missingRadar: true })).calls, 1);
  await assert.rejects(replayRadarPreparation({ readiness: "sprite" }), /unexpected flight/);
});

test("radar readiness refuses missing or wrong-life snapshots and non-live sources", () => {
  const m = { ...pilot([bot(768, 9247, 8965), bot(769, 7806, 8073)]),
    authoritativeSnapshotActors: [{ id: 768, lifeSeq: 1 }, { id: 769, lifeSeq: 1 }],
    radarPlayerIds: [768, 769] };
  const options = { readiness: "radar", requireEveryBot: true };
  assert.equal(botEncounterStageState(m, [768, 769], options).ready, true);
  for (const patch of [
    { authoritativeSnapshotActors: [{ id: 768, lifeSeq: 1 }] },
    { authoritativeSnapshotActors: [{ id: 768, lifeSeq: 1 }, { id: 769, lifeSeq: 2 }] },
    { radarPlayerIds: [768, 768] }, { playerAlive: false }, { serverPlayerAlive: false },
  ]) assert.equal(botEncounterStageState({ ...m, ...patch }, [768, 769], options).ready, false);
});

for (const file of ["bot_authority.mjs", "bot_authority_firefox.mjs"]) {
  test(`${file} stages Chaos before its original initial radar assertion`, async () => {
    const source = fs.readFileSync(new URL(file, import.meta.url), "utf8");
    const start = source.indexOf('    const initialReadinessEncounter =');
    const end = source.indexOf('}) : null;', start) + '}) : null;'.length;
    assert(start > 0 && end > start);
    const fragment = source.slice(start, end);
    const radarWait = source.indexOf('await waitUntil(', end);
    assert(radarWait > end, "encounter setup must precede initial radar wait");
    let calls = [];
    const ctx = vm.createContext({ arena: "chaos-zone", expectedBotCount: 2,
      actor: {}, server: {}, serverBotIdBase: 768, botIdBase: 768,
      approachLiveBotEncounter: async options => { calls.push(options); return { keysOnly: true }; },
    });
    const run = () => vm.runInContext(`(async () => { ${fragment}; return initialReadinessEncounter; })()`, ctx);
    const result = await run(); assert.equal(result.keysOnly, true);
    assert.equal(calls.length, 1); assert.deepEqual(Array.from(calls[0].botIds), [768, 769]);
    assert.equal(calls[0].requireEveryBot, true); assert.equal(calls[0].allowSafeSource, true);
    assert.equal(calls[0].readiness, "radar");
    ctx.arena = "extreme"; calls = [];
    assert.equal(await run(), null); assert.equal(calls.length, 0,
      "Extreme's protected initial observation stays unchanged");
    ctx.arena = "chaos-zone"; ctx.expectedBotCount = 0;
    assert.equal(await run(), null); assert.equal(calls.length, 0);
  });

  test(`${file} separates clean facing encounter setup from the original collection window`, async () => {
    const source = fs.readFileSync(new URL(file, import.meta.url), "utf8");
    const start = source.indexOf("    const facingEncounterPreparation =");
    const end = source.indexOf('    reportStage("facing_encounter_preparation"', start);
    assert(start > 0 && end > start);
    assert(source.indexOf("const minimumFacingShotServerTick", end) > end);
    assert(source.indexOf("await collectBotFacingEvidence", end) > end);
    const calls = [];
    const ctx = vm.createContext({ networkProfile: "", arena: "chaos-zone", actor: {}, server: {},
      observedBotIds: [768, 769], orderedBotIds: [768, 769], positionIsOnAuthoredSafeTile: () => false,
      approachLiveBotEncounter: async options => { calls.push(options); return { keysOnly: true }; },
    });
    const run = () => vm.runInContext(`(async () => { ${source.slice(start, end)}; return facingEncounterPreparation; })()`, ctx);
    assert.equal((await run()).keysOnly, true);
    assert.equal(calls.length, 1);
    assert.deepEqual({ ...calls[0].searchPoint }, { x: 8200, y: 8200 });
    assert.deepEqual(Array.from(calls[0].botIds), [768, 769]);
    ctx.networkProfile = "mild";
    assert.equal(await run(), null);
    assert.equal(calls.length, 1, "impaired lanes keep their explicit non-decidable exact-tick scope");
  });
}

async function replaySearch({ searchPoint = { x: 8200, y: 8200 }, acquire = true } = {}) {
  let now = 0;
  const m = { ...pilot(), playerX: 5514, playerY: 8348 };
  const calls = [];
  const actor = { metricsState: async () => ({ uiMetrics: structuredClone(m) }) };
  const source = fs.readFileSync(new URL("./bot_encounter_staging.mjs", import.meta.url), "utf8");
  const ctx = vm.createContext({ Number, Set, Math, Error, Date: { now: () => now },
    exactLiveGameplayScope: () => true,
    waitUntil: async (_label, predicate) => {
      for (let i = 0; i < 4; i++) {
        if (await predicate()) return;
        now += 300;
      }
      throw new Error("virtual wait expired");
    },
    navigateAuthoredTarget: async (_actor, destination, _server, options) => {
      calls.push({ destination: { ...destination }, speed: options.maximumDesiredSpeed });
      assert.equal(await options.complete(), false, "authored coordinates alone are never a live encounter");
      now += 50;
      if (!acquire) {
        m.playerX = 8200; m.playerY = 8200;
        assert.equal(await options.complete(), false);
        return { keysOnly: true, status: "complete" };
      }
      if (calls.length === 1) {
        m.playerX = 7500;
        if (acquire) m.remotePlayers = [bot(768, 8000)];
        assert.equal(await options.complete(), acquire, "search stops only when actual public membership arrives");
      } else {
        m.playerX = 7800;
        assert.equal(await options.complete(), true);
      }
      return { keysOnly: true, status: "complete" };
    },
  });
  vm.runInContext(source.slice(source.indexOf("export function")).replaceAll("export ", ""), ctx);
  let result, error;
  try { result = await ctx.approachLiveBotEncounter({ actor, botIds: [768], label: "virtual search", searchPoint, timeoutMs: 1000 }); }
  catch (e) { error = e; }
  return { result, error, calls };
}

test("declared region search reacquires public membership before routing to an actual live bot", async () => {
  const { result, error, calls } = await replaySearch();
  assert.ifError(error);
  assert.deepEqual(calls, [
    { destination: { x: 8200, y: 8200 }, speed: 250 },
    { destination: { x: 8000, y: 8200 }, speed: 100 },
  ]);
  assert.deepEqual(Array.from(result.approaches, leg => leg.purpose), ["authored-region-search", "live-bot-approach"]);
  assert.equal(result.keysOnly, true);
});

test("a missing public target without a declared search point never invents a destination", async () => {
  const { result, error, calls } = await replaySearch({ searchPoint: null });
  assert.equal(result, undefined);
  assert.match(error.message, /virtual wait expired/);
  assert.equal(calls.length, 0);
});

test("arriving in the declared region without a live bot cannot satisfy staging", async () => {
  const { result, error, calls } = await replaySearch({ acquire: false });
  assert.equal(result, undefined);
  assert.match(error.message, /budget|expired/);
  assert(calls.length > 0);
  assert(calls.every(call => call.destination.x === 8200 && call.destination.y === 8200));
});

test("invalid search coordinates fail before navigation", async () => {
  for (const searchPoint of [{ x: NaN, y: 8200 }, { x: -1, y: 8200 }, { x: 8200, y: 16384 }]) {
    const { result, error, calls } = await replaySearch({ searchPoint });
    assert.equal(result, undefined);
    assert.match(error.message, /invalid authored bot encounter search point/);
    assert.equal(calls.length, 0);
  }
});
