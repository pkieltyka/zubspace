import { sleep } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { brakeClientVelocity, waitUntil } from "./combat_replay.mjs";
import { authoredSafetyMargin } from "./authored_map_geometry.mjs";
import { keyboardTraceRecords } from "./keyboard_trace_reader.mjs";

const PRIMARY_WEAPON_TYPES = new Set([1, 2]);
const CONTINUOUS_FIRE_KEY = "Control";
const CONTINUOUS_FIRE_CODE = "ControlLeft";
const TURN = {
  ArrowLeft: { bit: 1 << 2, sign: -1 },
  ArrowRight: { bit: 1 << 3, sign: 1 },
};

const CASES = [
  { first: CONTINUOUS_FIRE_KEY, second: "ArrowLeft", release: ["ArrowLeft", CONTINUOUS_FIRE_KEY] },
  { first: CONTINUOUS_FIRE_KEY, second: "ArrowRight", release: [CONTINUOUS_FIRE_KEY, "ArrowRight"] },
  { first: "ArrowLeft", second: CONTINUOUS_FIRE_KEY, release: [CONTINUOUS_FIRE_KEY, "ArrowLeft"] },
  { first: "ArrowRight", second: CONTINUOUS_FIRE_KEY, release: ["ArrowRight", CONTINUOUS_FIRE_KEY] },
  { first: CONTINUOUS_FIRE_KEY, second: "ArrowLeft", release: [CONTINUOUS_FIRE_KEY, "ArrowLeft"] },
  { first: CONTINUOUS_FIRE_KEY, second: "ArrowRight", release: ["ArrowRight", CONTINUOUS_FIRE_KEY] },
  { first: "ArrowLeft", second: CONTINUOUS_FIRE_KEY, release: ["ArrowLeft", CONTINUOUS_FIRE_KEY] },
  { first: "ArrowRight", second: CONTINUOUS_FIRE_KEY, release: [CONTINUOUS_FIRE_KEY, "ArrowRight"] },
];

const AFTERBURNER_CASES = [
  { first: "Shift", second: "ArrowUp", release: ["ArrowUp", "Shift"] },
  { first: "ArrowUp", second: "Shift", release: ["Shift", "ArrowUp"] },
  { first: "Shift", second: "ArrowUp", release: ["Shift", "ArrowUp"] },
  { first: "ArrowUp", second: "Shift", release: ["ArrowUp", "Shift"] },
];

function traceRecords(server, marker) {
  return keyboardTraceRecords(server, marker);
}

function safeZoneThrustEvidence({ state, records, playerId, lifeSeq, isAuthoredSafe }) {
  const metrics = state?.uiMetrics;
  const codes = state?.gameKeyBridge?.trackedCodes;
  const accepted = records.filter(record => record.player_id === playerId && record.life_seq === lifeSeq);
  if (metrics?.playerAlive !== true || metrics.serverPlayerAlive !== true ||
      metrics.playerLifeSeq !== lifeSeq || metrics.inSafeZone !== true ||
      codes?.length !== 2 || !codes.includes("ControlLeft") || !codes.includes("ArrowUp") ||
      ![1, 65].includes(metrics.localGameplayState?.inputBits) || accepted.length < 64 ||
      !accepted.every(record => [1, 65].includes(record.input_bits) &&
        record.accepted_weapon === 0 && record.reported_weapon === 0 &&
        isAuthoredSafe(record.x, record.y)) ||
      accepted.filter(record => record.input_bits === 65).length < 4) return null;
  // Duplicated packets or a stale sampled state cannot stand in for sustained
  // accepted input. Sequence order is the server's committed order, not arrival.
  for (let i = 1; i < accepted.length; i += 1) {
    const advance = (accepted[i].input_sequence - accepted[i - 1].input_sequence) & 0xffff;
    if (advance === 0 || advance >= 0x8000) return null;
  }
  return { lifeSeq, acceptedCount: accepted.length,
    stopCount: accepted.filter(record => record.input_bits === 65).length,
    authoritativeInputs: accepted, trackedCodes: codes };
}

async function proveSafeZoneFireThrust({ actor, server, playerId, isAuthoredSafe }) {
  await actor.keyUp("Control");
  await actor.keyUp("ArrowUp");
  const before = await captureWeaponReadyMetrics(actor, "safe-zone pilot ready before input");
  if (!weaponInputReady(before) || before.inSafeZone !== true ||
      !isAuthoredSafe(before.playerX, before.playerY)) {
    throw new Error(`safe-zone fire/thrust requires a live pilot inside authored safety: ${JSON.stringify({
      alive: before.playerAlive, serverAlive: before.serverPlayerAlive,
      lifeSeq: before.playerLifeSeq, inSafeZone: before.inSafeZone,
      x: before.playerX, y: before.playerY, combatView: before.combatViewTimeline,
    })}`);
  }
  const lifeSeq = before.playerLifeSeq;
  let held = null;
  let thrustOnly = null;
  try {
    await actor.keyDown("Control");
    await actor.keyDown("ArrowUp");
    // Exclude the first-key-only admission, but retain every subsequent record.
    await waitUntil("safe-zone chord reaches authority", async () => {
      const last = traceRecords(server, "human authority input trace")
        .filter(record => record.player_id === playerId && record.life_seq === lifeSeq).at(-1);
      return [1, 65].includes(last?.input_bits);
    }, 3_000);
    const offset = traceRecords(server, "human authority input trace").length;
    await waitUntil("repeated safe-zone fire/thrust stays authoritative", async () => {
      held = safeZoneThrustEvidence({ state: await actor.keyState(),
        records: traceRecords(server, "human authority input trace").slice(offset),
        playerId, lifeSeq, isAuthoredSafe });
      return held !== null;
    }, 10_000);
    await actor.keyUp("Control");
    const releaseOffset = traceRecords(server, "human authority input trace").length;
    await waitUntil("release fire preserves forward thrust", async () => {
      thrustOnly = singleHeldKeyEvidence({ state: await actor.keyState(),
        records: traceRecords(server, "human authority input trace").slice(releaseOffset),
        playerId, lifeSeq, remainingKey: "ArrowUp" });
      return thrustOnly !== null;
    }, 3_000);
  } finally {
    try {
      await actor.keyUp("Control");
    } finally {
      await actor.keyUp("ArrowUp");
    }
  }
  const neutralOffset = traceRecords(server, "human authority input trace").length;
  await waitUntil("safe-zone chord releases all physical and authoritative input", async () => {
    const state = await actor.keyState();
    const tail = traceRecords(server, "human authority input trace").slice(neutralOffset)
      .filter(record => record.player_id === playerId && record.life_seq === lifeSeq).slice(-16);
    return state.gameKeyBridge?.trackedCodes?.length === 0 &&
      state.uiMetrics?.localGameplayState?.inputBits === 0 && tail.length === 16 &&
      tail.every(record => record.input_bits === 0 && record.accepted_weapon === 0);
  }, 3_000);
  return { held, thrustOnly };
}

function orientationDelta(from, to) {
  return ((to - from + 1.5) % 1) - 0.5;
}

function primaryCount(metrics) {
  return metrics?.semanticInputCounts?.primary ?? 0;
}

const SINGLE_KEY_STATE = {
  Control: { code: "ControlLeft", bits: 0, fires: true },
  ArrowLeft: { code: "ArrowLeft", bits: 4, fires: false },
  ArrowRight: { code: "ArrowRight", bits: 8, fires: false },
  ArrowUp: { code: "ArrowUp", bits: 1, fires: false },
  Shift: { code: "ShiftLeft", bits: 16, fires: false },
};

function singleHeldKeyEvidence({ state, records, playerId, lifeSeq, remainingKey }) {
  const expected = SINGLE_KEY_STATE[remainingKey];
  if (!expected) throw new Error(`unsupported held key ${remainingKey}`);
  const metrics = state?.uiMetrics;
  const codes = state?.gameKeyBridge?.trackedCodes;
  const tail = records.filter((record) =>
    record.player_id === playerId && record.life_seq === lifeSeq
  ).slice(-16);
  if (
    metrics?.playerAlive !== true || metrics.serverPlayerAlive !== true ||
    metrics.playerLifeSeq !== lifeSeq ||
    codes?.length !== 1 || codes[0] !== expected.code ||
    metrics.localGameplayState?.inputBits !== expected.bits ||
    metrics.localGameplayState?.afterburnerActive !== false ||
    tail.length !== 16 ||
    !tail.every((record) => record.input_bits === expected.bits && Number.isInteger(record.reported_weapon)) ||
    (expected.fires
      ? !tail.some((record) => PRIMARY_WEAPON_TYPES.has(record.reported_weapon & 0x1f))
      : !tail.every((record) => (record.reported_weapon & 0x1f) === 0))
  ) return null;
  return {
    remainingKey,
    lifeSeq,
    trackedCodes: codes,
    localInputBits: metrics.localGameplayState.inputBits,
    authoritativeTail: tail,
  };
}

async function releaseChordWithIntermediateProof({ actor, server, playerId, lifeSeq, releaseOrder, phase }) {
  const [releasedKey, remainingKey] = releaseOrder;
  await actor.keyUp(releasedKey);
  const offset = traceRecords(server, "human authority input trace").length;
  let evidence = null;
  try {
    await waitUntil(`${phase} releasing ${releasedKey} preserves only ${remainingKey}`, async () => {
      const state = await actor.keyState();
      evidence = singleHeldKeyEvidence({
        state,
        records: traceRecords(server, "human authority input trace").slice(offset),
        playerId, lifeSeq, remainingKey,
      });
      return evidence !== null;
    }, 3_000);
  } finally {
    await actor.keyUp(remainingKey);
  }
  return { releasedKey, ...evidence };
}

function turnKeyFor(testCase) {
  return testCase.first === CONTINUOUS_FIRE_KEY ? testCase.second : testCase.first;
}

function weaponInputReady(metrics) {
  return metrics.playerAlive === true &&
    metrics.serverPlayerAlive === true &&
    metrics.combatViewTimeline?.weaponReady === true;
}

async function captureWeaponReadyMetrics(actor, description, timeoutMs = 3_000) {
  let captured = null;
  await waitUntil(description, async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    if (weaponInputReady(metrics) && Number.isInteger(metrics.playerLifeSeq)) {
      captured = metrics;
      return true;
    }
    return false;
  }, timeoutMs);
  return captured;
}

async function waitForNeutral(actor, server, playerId, lifeSeq, releaseOffset) {
  const requiredNeutralRecords = 16;
  await waitUntil("released chord returns to a neutral authoritative input", async () => {
    const records = traceRecords(server, "human authority input trace").slice(releaseOffset)
      .filter((record) => record.player_id === playerId && record.life_seq === lifeSeq);
    const tail = records.slice(-requiredNeutralRecords);
    const metrics = (await actor.metricsState()).uiMetrics || {};
    return tail.length === requiredNeutralRecords &&
      tail.every((record) =>
        (record.input_bits & ((1 << 2) | (1 << 3))) === 0 &&
        (record.reported_weapon & 0x1f) === 0
      ) &&
      ((metrics.localGameplayState?.inputBits ?? 0) & ((1 << 2) | (1 << 3))) === 0;
  }, 3_000);
}

async function proveHeldChordSurvivesRepeatPause({ actor, server, playerId, phase }) {
  if (typeof actor.keyDownRepeat !== "function") {
    return { phase, supported: false };
  }
  await actor.releaseKeys?.().catch(() => {});
  const before = await actor.keyState();
  const metrics = before.uiMetrics || {};
  if (
    !weaponInputReady(metrics) ||
    !Number.isInteger(metrics.playerLifeSeq)
  ) {
    throw new Error(`${phase} held-repeat-pause proof started outside a live authority scope`);
  }
  const lifeSeq = metrics.playerLifeSeq;
  const forcedBefore = before.gameKeyBridge?.forcedReleaseBatches ?? 0;
  const repeatsBefore = before.gameKeyBridge?.repeatEvents ?? 0;
  const traceOffset = traceRecords(server, "human authority input trace").length;
  const pauseStartedAt = Date.now();
  let paused = null;
  let resumed = null;
  let acceptedDuringPause = [];
  let acceptedAfterResume = [];
  let releaseOffset = traceOffset;
  await actor.keyDown(CONTINUOUS_FIRE_KEY);
  try {
    await sleep(160);
    await actor.keyDown("ArrowUp");
    for (let index = 0; index < 8; index += 1) {
      await sleep(70);
      await actor.keyDownRepeat(CONTINUOUS_FIRE_KEY);
    }
    await waitUntil(`${phase} held Ctrl+Forward reaches server authority`, async () => {
      const accepted = traceRecords(server, "human authority input trace").slice(traceOffset)
        .filter((record) =>
          record.player_id === playerId &&
          record.life_seq === lifeSeq &&
          (record.input_bits & 1) !== 0 &&
          PRIMARY_WEAPON_TYPES.has(record.reported_weapon & 0x1f)
        );
      const state = await actor.keyState();
      return accepted.length > 0 &&
        (state.gameKeyBridge?.repeatEvents ?? 0) > repeatsBefore &&
        state.gameKeyBridge?.trackedCodes?.includes(CONTINUOUS_FIRE_CODE) &&
        state.gameKeyBridge?.trackedCodes?.includes("ArrowUp");
    }, 2_000);
    const authoritativePauseOffset = traceRecords(server, "human authority input trace").length;
    // A browser/desktop compositor may pause repeat delivery while both keys
    // remain physically held. No key-up or focus-loss event occurred, so the
    // client must retain both keys instead of inferring a release from silence.
    await sleep(1_750);
    await waitUntil(`${phase} held chord survives repeat silence`, async () => {
      const state = await actor.keyState();
      const bridge = state.gameKeyBridge || {};
      acceptedDuringPause = traceRecords(server, "human authority input trace")
        .slice(authoritativePauseOffset)
        .filter((record) => record.player_id === playerId && record.life_seq === lifeSeq);
      const tail = acceptedDuringPause.slice(-24);
      const localBits = state.uiMetrics?.localGameplayState?.inputBits ?? 0;
      if (
        bridge.forcedReleaseBatches === forcedBefore &&
        bridge.trackedCodes?.includes(CONTINUOUS_FIRE_CODE) &&
        bridge.trackedCodes?.includes("ArrowUp") &&
        (localBits & 1) !== 0 &&
        tail.length === 24 &&
        tail.every((record) => (record.input_bits & 1) !== 0)
      ) {
        paused = {
          ...bridge,
          elapsedMs: Date.now() - pauseStartedAt,
          localInputBits: localBits,
          authoritativeTail: tail,
        };
        return true;
      }
      return false;
    }, 2_500);
    const resumeOffset = traceRecords(server, "human authority input trace").length;
    await actor.keyDownRepeat(CONTINUOUS_FIRE_KEY);
    await waitUntil(`${phase} held chord remains complete after repeats resume`, async () => {
      const state = await actor.keyState();
      const bridge = state.gameKeyBridge || {};
      acceptedAfterResume = traceRecords(server, "human authority input trace")
        .slice(resumeOffset)
        .filter((record) => record.player_id === playerId && record.life_seq === lifeSeq);
      const tail = acceptedAfterResume.slice(-12);
      const localBits = state.uiMetrics?.localGameplayState?.inputBits ?? 0;
      if (
        bridge.forcedReleaseBatches === forcedBefore &&
        bridge.trackedCodes?.includes(CONTINUOUS_FIRE_CODE) &&
        bridge.trackedCodes?.includes("ArrowUp") &&
        (localBits & 1) !== 0 &&
        tail.length === 12 &&
        tail.every((record) => (record.input_bits & 1) !== 0)
      ) {
        resumed = {
          ...bridge,
          localInputBits: localBits,
          authoritativeTail: tail,
        };
        return true;
      }
      return false;
    }, 2_000);
  } finally {
    await actor.keyUp("ArrowUp").catch(() => {});
    await actor.keyUp(CONTINUOUS_FIRE_KEY).catch(() => {});
    releaseOffset = traceRecords(server, "human authority input trace").length;
    await actor.releaseKeys?.().catch(() => {});
  }
  await waitForNeutral(actor, server, playerId, lifeSeq, releaseOffset);
  return {
    phase,
    supported: true,
    lifeSeq,
    paused,
    resumed,
    acceptedDuringPause: acceptedDuringPause.slice(-24),
    acceptedAfterResume: acceptedAfterResume.slice(-12),
  };
}

async function proveFocusLossReleasesHeldChord({ actor, server, playerId, phase }) {
  if (typeof actor.cycleFocus !== "function") {
    return { phase, supported: false };
  }
  await actor.releaseKeys?.().catch(() => {});
  await waitUntil(`${phase} neutral input before focus-loss proof`, async () => {
    const state = await actor.keyState();
    const metrics = state.uiMetrics || {};
    return weaponInputReady(metrics) &&
      (metrics.localGameplayState?.inputBits ?? 0) === 0 &&
      (metrics.pendingWeapon ?? 0) === 0 &&
      (state.gameKeyBridge?.trackedCodes?.length ?? 0) === 0;
  }, 3_000);

  const before = await actor.keyState();
  const lifeSeq = before.uiMetrics?.playerLifeSeq;
  const bridgeBefore = before.gameKeyBridge || {};
  const traceOffset = traceRecords(server, "human authority input trace").length;
  let heldTail = [];
  let focusCycle = null;
  let released = null;
  let releaseOffset = traceOffset;
  await actor.keyDown(CONTINUOUS_FIRE_KEY);
  try {
    await sleep(160);
    await actor.keyDown("ArrowUp");
    await waitUntil(`${phase} focus-loss chord reaches exact server authority`, async () => {
      const state = await actor.keyState();
      heldTail = traceRecords(server, "human authority input trace").slice(traceOffset)
        .filter((record) => record.player_id === playerId && record.life_seq === lifeSeq)
        .filter((record) =>
          (record.input_bits & 1) !== 0 &&
          PRIMARY_WEAPON_TYPES.has(record.reported_weapon & 0x1f)
        );
      return heldTail.length >= 4 &&
        state.gameKeyBridge?.trackedCodes?.includes(CONTINUOUS_FIRE_CODE) &&
        state.gameKeyBridge?.trackedCodes?.includes("ArrowUp") &&
        ((state.uiMetrics?.localGameplayState?.inputBits ?? 0) & 1) !== 0;
    }, 2_000);

    focusCycle = await actor.cycleFocus(250);
    releaseOffset = traceRecords(server, "human authority input trace").length;
    await waitUntil(`${phase} real focus loss publishes neutral authority`, async () => {
      const state = await actor.keyState();
      const bridge = state.gameKeyBridge || {};
      const records = traceRecords(server, "human authority input trace")
        .slice(traceOffset)
        .filter((record) => record.player_id === playerId && record.life_seq === lifeSeq);
      const tail = records.slice(-16);
      const forcedReason = bridge.lastForcedReleaseReason;
      if (
        bridge.forcedReleaseBatches === (bridgeBefore.forcedReleaseBatches ?? 0) + 1 &&
        bridge.forcedReleaseKeys === (bridgeBefore.forcedReleaseKeys ?? 0) + 2 &&
        new Set(["window-blur", "document-hidden"])
          .has(forcedReason) &&
        (bridge.trackedCodes?.length ?? 0) === 0 &&
        (state.uiMetrics?.localGameplayState?.inputBits ?? 0) === 0 &&
        (state.uiMetrics?.pendingWeapon ?? 0) === 0 &&
        tail.length === 16 &&
        tail.every((record) => record.input_bits === 0 && (record.reported_weapon & 0x1f) === 0)
      ) {
        released = {
          ...bridge,
          authoritativeTail: tail,
          localInputBits: state.uiMetrics?.localGameplayState?.inputBits,
          pendingWeapon: state.uiMetrics?.pendingWeapon,
        };
        return true;
      }
      return false;
    }, 3_000);
  } finally {
    await actor.keyUp("ArrowUp").catch(() => {});
    await actor.keyUp(CONTINUOUS_FIRE_KEY).catch(() => {});
    await actor.releaseKeys?.().catch(() => {});
  }

  const stoppedPrimary = primaryCount((await actor.metricsState()).uiMetrics || {});
  await sleep(350);
  const settled = (await actor.metricsState()).uiMetrics || {};
  if (
    primaryCount(settled) !== stoppedPrimary ||
    (settled.localGameplayState?.inputBits ?? 0) !== 0 ||
    (settled.pendingWeapon ?? 0) !== 0
  ) {
    throw new Error(`${phase} focus loss left a stuck gameplay key: ${JSON.stringify({
      stoppedPrimary,
      settledPrimary: primaryCount(settled),
      inputBits: settled.localGameplayState?.inputBits,
      pendingWeapon: settled.pendingWeapon,
      released,
    })}`);
  }

  const freshOffset = traceRecords(server, "human authority input trace").length;
  await actor.press("Control", 180);
  let fresh = null;
  await waitUntil(`${phase} fresh Control edge works after focus recovery`, async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    const records = traceRecords(server, "human authority input trace").slice(freshOffset)
      .filter((record) =>
        record.player_id === playerId &&
        record.life_seq === lifeSeq &&
        PRIMARY_WEAPON_TYPES.has(record.reported_weapon & 0x1f)
      );
    if (primaryCount(metrics) > stoppedPrimary && records.length > 0) {
      fresh = {
        semanticPrimary: primaryCount(metrics),
        authoritative: records.slice(0, 8),
      };
      return true;
    }
    return false;
  }, 3_000);
  await waitForNeutral(actor, server, playerId, lifeSeq, releaseOffset);
  return {
    phase,
    supported: true,
    lifeSeq,
    focusCycle,
    heldTail: heldTail.slice(-8),
    released,
    stoppedPrimary,
    fresh,
  };
}

function speed(metrics) {
  return Math.hypot(metrics?.playerVelocityX ?? 0, metrics?.playerVelocityY ?? 0);
}

function distance(left, right) {
  return Math.hypot(
    (right?.playerX ?? 0) - (left?.playerX ?? 0),
    (right?.playerY ?? 0) - (left?.playerY ?? 0),
  );
}

async function proveAfterburnerMatrix({
  actor,
  server,
  playerId,
  phase,
  arena,
  prepareCase = null,
  chordDurationMs = 900,
}) {
  const evidence = [];
  for (const testCase of AFTERBURNER_CASES) {
    await actor.releaseKeys?.().catch(() => {});
    // The previous case's boost leaves the pilot wherever that chord carried
    // it, which can be inside an authored safe surface or facing a wall. Ask
    // the caller to re-stage before every measurement, not once per attempt.
    if (prepareCase) await prepareCase({ testCase, attempt: null, afterburner: true });
    await waitUntil(`${phase} authoritative energy reserve before ${testCase.first}->${testCase.second}`, async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      const reserve = Math.min((metrics.playerMaxEnergy ?? 0) - 1, 300);
      return metrics.playerAlive === true && metrics.serverPlayerAlive === true &&
        Number.isFinite(metrics.playerEnergy) && Number.isFinite(metrics.serverPlayerEnergy) &&
        Number.isFinite(metrics.playerMaxEnergy) && reserve > 0 &&
        metrics.playerEnergy >= reserve && metrics.serverPlayerEnergy >= reserve &&
        Math.abs(metrics.playerEnergy - metrics.serverPlayerEnergy) <= 18 &&
        (metrics.localGameplayState?.inputBits ?? 0) === 0;
    }, 8_000);
    const brake = await brakeClientVelocity(actor, 4_000);
    const before = (await actor.metricsState()).uiMetrics || {};
    const afterburnerCueBefore = before.afterburnerSoundPlayCount ?? 0;
    const traceOffset = traceRecords(server, "human authority input trace").length;
    const prizeTraceOffset = traceRecords(server, "server prize claimed").length;
    const doorRelocationTraceOffset = traceRecords(
      server,
      "canonical door-close relocation observed",
    ).length;
    let during = null;
    let accepted = [];
    let intermediateRelease = null;
    await actor.keyDown(testCase.first);
    try {
      await sleep(180);
      await actor.keyDown(testCase.second);
      try {
        // Sample across the hold, not only at its end. The chord's own thrust can pass through zero
        // inside the window (the nose may start opposite the residual brake velocity), so an end-only
        // sample can read near-zero speed on a ship the afterburner just moved. The peak is a stronger
        // motion assertion than the end sample, and `distance(before, during)` below still proves the
        // ship actually travelled.
        let maxSpeedDuring = 0;
        let maxDisplacementDuring = 0;
        const holdUntil = Date.now() + chordDurationMs;
        while (Date.now() < holdUntil) {
          await sleep(100);
          const sample = (await actor.metricsState()).uiMetrics || {};
          if (
            Number.isFinite(sample.playerVelocityX) &&
            Number.isFinite(sample.playerVelocityY)
          ) {
            maxSpeedDuring = Math.max(
              maxSpeedDuring,
              Math.hypot(sample.playerVelocityX, sample.playerVelocityY),
            );
          }
          if (Number.isFinite(sample.playerX) && Number.isFinite(sample.playerY)) {
            maxDisplacementDuring = Math.max(
              maxDisplacementDuring,
              Math.hypot(sample.playerX - before.playerX, sample.playerY - before.playerY),
            );
          }
        }
        during = (await actor.metricsState()).uiMetrics || {};
        accepted = traceRecords(server, "human authority input trace").slice(traceOffset)
          .filter((record) =>
            record.player_id === playerId &&
            record.life_seq === before.playerLifeSeq &&
            (record.input_bits & 0x11) === 0x11
          );
        const localBits = during.localGameplayState?.inputBits ?? 0;
        const movement = during.localGameplayState || {};
        const selectedAuthoredMaximum =
          movement.afterburnerActive === true &&
          Number.isFinite(movement.movementThrustAcceleration) &&
          Number.isFinite(movement.movementTopSpeed) &&
          Number.isFinite(movement.afterburnerThrustAcceleration) &&
          Number.isFinite(movement.afterburnerTopSpeed) &&
          Math.abs(
            movement.movementThrustAcceleration - movement.afterburnerThrustAcceleration,
          ) <= 0.01 &&
          Math.abs(movement.movementTopSpeed - movement.afterburnerTopSpeed) <= 0.01;
        const authoredBoostHeadroom =
          movement.afterburnerThrustAcceleration > movement.baseThrustAcceleration + 0.01 ||
          movement.afterburnerTopSpeed > movement.baseTopSpeed + 0.01;
        const acceptedStartEnergy = accepted[0]?.energy;
        const acceptedEndEnergy = accepted.at(-1)?.energy;
        const authoritativeCostObserved =
          Number.isFinite(acceptedStartEnergy) && Number.isFinite(acceptedEndEnergy) &&
          acceptedEndEnergy <= acceptedStartEnergy - 2;
        const browserConvergedToAuthority =
          Number.isFinite(acceptedEndEnergy) &&
          Math.abs(during.playerEnergy - acceptedEndEnergy) <= 18;
        const prizeClaims = traceRecords(server, "server prize claimed").slice(prizeTraceOffset)
          .filter((record) => record.player_id === playerId);
        const doorRelocations = traceRecords(
          server,
          "canonical door-close relocation observed",
        ).slice(doorRelocationTraceOffset)
          .filter((record) =>
            record.player_id === playerId && record.life_seq === before.playerLifeSeq
          );
        const damageAdvancedDuring =
          during.lastSelfDamageServerSeq !== before.lastSelfDamageServerSeq;
        if (damageAdvancedDuring || prizeClaims.length > 0 || doorRelocations.length > 0) {
          const interruption = new Error(`${phase} ${testCase.first}->${testCase.second} Afterburner measurement was overtaken by authoritative gameplay: ${JSON.stringify({
            damageBefore: before.lastSelfDamageServerSeq,
            damageDuring: during.lastSelfDamageServerSeq,
            prizeClaims,
            doorRelocations,
          })}`);
          interruption.afterburnerGameplayInterruption = {
            damageAdvancedDuring,
            prizeClaims,
            doorRelocations,
          };
          throw interruption;
        }
        if (
          during.playerAlive !== true ||
          during.serverPlayerAlive !== true ||
          during.playerLifeSeq !== before.playerLifeSeq ||
          (localBits & 0x11) !== 0x11 ||
          accepted.length < 8 ||
          !selectedAuthoredMaximum ||
          (during.afterburnerSoundPlayCount ?? 0) <= afterburnerCueBefore ||
          maxDisplacementDuring < 12 ||
          maxSpeedDuring < 12 ||
          (arena === "chaos-zone" &&
            (!authoritativeCostObserved || !browserConvergedToAuthority)) ||
          (during.localGameplayState?.rocketTimer ?? 0) > 0 ||
          during.pendingRocketActivationSequence != null
        ) {
          throw new Error(`${phase} ${testCase.first}->${testCase.second} lacked exact Afterburner ownership/motion/cost: ${JSON.stringify({
            before: {
              lifeSeq: before.playerLifeSeq,
              energy: before.playerEnergy,
              x: before.playerX,
              y: before.playerY,
              speed: speed(before),
            },
            during: {
              lifeSeq: during.playerLifeSeq,
              energy: during.playerEnergy,
              x: during.playerX,
              y: during.playerY,
              speed: speed(during),
              maxSpeedDuring,
              maxDisplacementDuring,
              inputBits: localBits,
              rocketTimer: during.localGameplayState?.rocketTimer,
              pendingRocketActivationSequence: during.pendingRocketActivationSequence,
              afterburnerActive: movement.afterburnerActive,
              movementThrustAcceleration: movement.movementThrustAcceleration,
              movementTopSpeed: movement.movementTopSpeed,
              baseThrustAcceleration: movement.baseThrustAcceleration,
              baseTopSpeed: movement.baseTopSpeed,
              afterburnerThrustAcceleration: movement.afterburnerThrustAcceleration,
              afterburnerTopSpeed: movement.afterburnerTopSpeed,
              authoredBoostHeadroom,
              selectedAuthoredMaximum,
              afterburnerSoundPlayCount: during.afterburnerSoundPlayCount,
              afterburnerSoundPlayCountBefore: afterburnerCueBefore,
            },
            authoritativeCost: {
              startEnergy: acceptedStartEnergy,
              endEnergy: acceptedEndEnergy,
              observed: authoritativeCostObserved,
              browserConverged: browserConvergedToAuthority,
            },
            accepted,
          })}`);
        }
        intermediateRelease = await releaseChordWithIntermediateProof({
          actor, server, playerId, lifeSeq: before.playerLifeSeq,
          releaseOrder: testCase.release, phase,
        });
      } finally {
        if (!intermediateRelease) {
          for (const key of testCase.release) await actor.keyUp(key).catch(() => {});
        }
      }
    } catch (error) {
      await actor.releaseKeys?.().catch(() => {});
      throw error;
    }
    await waitUntil(`${phase} ${testCase.first}->${testCase.second} releases cleanly`, async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      const records = traceRecords(server, "human authority input trace").slice(traceOffset)
        .filter((record) => record.player_id === playerId && record.life_seq === before.playerLifeSeq);
      return (metrics.localGameplayState?.inputBits ?? 0) === 0 &&
        metrics.localGameplayState?.afterburnerActive === false &&
        records.some((record) => (record.input_bits & 0x11) === 0);
    }, 3_000);
    await sleep(300);
    const after = (await actor.metricsState()).uiMetrics || {};
    const authoritativeDamageAdvanced =
      Number.isInteger(after.lastSelfDamageServerSeq) &&
      after.lastSelfDamageServerSeq !== during.lastSelfDamageServerSeq;
    if (
      arena === "chaos-zone" &&
      after.playerEnergy <= during.playerEnergy &&
      !authoritativeDamageAdvanced
    ) {
      throw new Error(`${phase} ${testCase.first}->${testCase.second} kept the Afterburner cost after release: ${JSON.stringify({
        duringEnergy: during.playerEnergy,
        afterEnergy: after.playerEnergy,
        duringDamageSeq: during.lastSelfDamageServerSeq,
        afterDamageSeq: after.lastSelfDamageServerSeq,
      })}`);
    }
    evidence.push({
      phase,
      firstKey: testCase.first,
      secondKey: testCase.second,
      releaseOrder: testCase.release,
      intermediateRelease,
      lifeSeq: before.playerLifeSeq,
      brake,
      before: {
        energy: before.playerEnergy,
        x: before.playerX,
        y: before.playerY,
        speed: speed(before),
      },
      during: {
        energy: during.playerEnergy,
        x: during.playerX,
        y: during.playerY,
        speed: speed(during),
        distance: distance(before, during),
        inputBits: during.localGameplayState?.inputBits,
        active: during.localGameplayState?.afterburnerActive,
        soundPlayCountBefore: afterburnerCueBefore,
        soundPlayCountDuring: during.afterburnerSoundPlayCount,
        selectedThrustAcceleration:
          during.localGameplayState?.movementThrustAcceleration,
        selectedTopSpeed: during.localGameplayState?.movementTopSpeed,
        baseThrustAcceleration:
          during.localGameplayState?.baseThrustAcceleration,
        baseTopSpeed: during.localGameplayState?.baseTopSpeed,
        authoredMaximumThrustAcceleration:
          during.localGameplayState?.afterburnerThrustAcceleration,
        authoredMaximumTopSpeed:
          during.localGameplayState?.afterburnerTopSpeed,
        authoredBoostHeadroom:
          (during.localGameplayState?.afterburnerThrustAcceleration ?? 0) >
            (during.localGameplayState?.baseThrustAcceleration ?? 0) + 0.01 ||
          (during.localGameplayState?.afterburnerTopSpeed ?? 0) >
            (during.localGameplayState?.baseTopSpeed ?? 0) + 0.01,
      },
      after: {
        energy: after.playerEnergy,
        inputBits: after.localGameplayState?.inputBits,
        active: after.localGameplayState?.afterburnerActive,
        authoritativeDamageAdvanced,
        damageSeq: after.lastSelfDamageServerSeq,
      },
      acceptedInputSamples: accepted.slice(0, 24),
      authoritativeCost: {
        startEnergy: accepted[0]?.energy,
        endEnergy: accepted.at(-1)?.energy,
      },
    });
  }
  return evidence;
}

async function proveAfterburnerMatrixWithLifecycleRetries({
  actor,
  server,
  playerId,
  phase,
  arena,
  prepareAttempt = null,
  chordDurationMs = 900,
}) {
  let lastInterruption = null;
  for (let attempt = 0; attempt < 8; attempt += 1) {
    await waitUntil(`${phase} live authority before Afterburner matrix`, async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      return metrics.playerAlive === true && metrics.serverPlayerAlive === true;
    }, 10_000);
    if (prepareAttempt) {
      await prepareAttempt({ attempt, afterburner: true });
      await waitUntil(`${phase} prepared live authority before Afterburner matrix`, async () => {
        const metrics = (await actor.metricsState()).uiMetrics || {};
        return metrics.playerAlive === true && metrics.serverPlayerAlive === true;
      }, 10_000);
    }
    await actor.releaseKeys?.().catch(() => {});
    const before = (await actor.metricsState()).uiMetrics || {};
    try {
      return await proveAfterburnerMatrix({
        actor,
        server,
        playerId,
        phase,
        arena,
        prepareCase: prepareAttempt,
        chordDurationMs,
      });
    } catch (error) {
      await actor.releaseKeys?.().catch(() => {});
      const after = (await actor.metricsState()).uiMetrics || {};
      const lifecycleInterrupted =
        after.playerAlive !== true ||
        after.serverPlayerAlive !== true ||
        after.playerLifeSeq !== before.playerLifeSeq ||
        after.playerRespawnSeq !== before.playerRespawnSeq;
      const gameplayInterrupted = error.afterburnerGameplayInterruption != null;
      if (!lifecycleInterrupted && !gameplayInterrupted) throw error;
      lastInterruption = {
        attempt,
        message: error.message,
        gameplay: error.afterburnerGameplayInterruption ?? null,
        before: {
          lifeSeq: before.playerLifeSeq,
          respawnSeq: before.playerRespawnSeq,
        },
        after: {
          lifeSeq: after.playerLifeSeq,
          respawnSeq: after.playerRespawnSeq,
          alive: after.playerAlive,
          serverAlive: after.serverPlayerAlive,
        },
      };
      await sleep(150);
    }
  }
  throw new Error(`${phase} Afterburner matrix was repeatedly interrupted by authoritative lifecycle changes: ${JSON.stringify(lastInterruption)}`);
}

async function proveOneChord({ actor, server, playerId, phase, testCase }) {
  // Use the exact sample that satisfied readiness. Firefox WebDriver can
  // cross a short observer hold between two back-to-back script commands;
  // re-polling and then rejecting that second sample races the gate without
  // exercising any keyboard input.
  const before = await captureWeaponReadyMetrics(
    actor,
    `${phase} ${testCase.first}->${testCase.second} exact live start`,
  );
  const turnKey = turnKeyFor(testCase);
  const turn = TURN[turnKey];
  const traceOffset = traceRecords(server, "human authority input trace").length;
  const semanticBefore = primaryCount(before);
  let during = null;
  let accepted = [];
  let releaseOffset = null;
  let intermediateRelease = null;
  if (actor.performChord) {
    const startedAt = Date.now();
    await actor.performChord(
      testCase.first,
      testCase.second,
      180,
      520,
      [], // Keep both genuine keys down until the staged release proof below.
    );
    const endedAt = Date.now();
    const state = await actor.heldWindowState(startedAt, endedAt);
    const heldFrames = (state.metricsTrace || []).filter((frame) =>
      frame.at >= startedAt + 180 && frame.at <= endedAt &&
      frame.playerAlive === true && frame.serverPlayerAlive === true
    );
    during = heldFrames
      .map((frame) => ({
        ...frame,
        signedDelta: orientationDelta(before.playerOrientation, frame.playerOrientation),
      }))
      .filter((frame) => frame.signedDelta * turn.sign > 0)
      .sort((left, right) =>
        right.signedDelta * turn.sign - left.signedDelta * turn.sign
      )[0] || null;
    accepted = traceRecords(server, "human authority input trace").slice(traceOffset)
      .filter((record) =>
        record.player_id === playerId &&
        record.life_seq === before.playerLifeSeq &&
        (record.input_bits & turn.bit) !== 0 &&
        PRIMARY_WEAPON_TYPES.has(record.reported_weapon & 0x1f)
      );
    if (
      !during ||
      during.signedDelta * turn.sign <= 0.025 ||
      accepted.length === 0 ||
      primaryCount(state.uiMetrics) <= semanticBefore
    ) {
      throw new Error(`${phase} ${testCase.first}->${testCase.second} lacked Firefox held-window evidence: ${JSON.stringify({
        heldFrames: heldFrames.slice(-20),
        accepted,
        semanticBefore,
        semanticAfter: primaryCount(state.uiMetrics),
      })}`);
    }
    intermediateRelease = await releaseChordWithIntermediateProof({
      actor, server, playerId, lifeSeq: before.playerLifeSeq,
      releaseOrder: testCase.release, phase,
    });
    releaseOffset = traceRecords(server, "human authority input trace").length;
  } else {
  await actor.keyDown(testCase.first);
  try {
    await sleep(180);
    await actor.keyDown(testCase.second);
    try {
      await waitUntil(`${phase} ${testCase.first}->${testCase.second} applies while both keys remain down`, async () => {
        during = (await actor.metricsState()).uiMetrics || {};
        accepted = traceRecords(server, "human authority input trace").slice(traceOffset)
          .filter((record) =>
            record.player_id === playerId &&
            record.life_seq === before.playerLifeSeq &&
            (record.input_bits & turn.bit) !== 0 &&
            PRIMARY_WEAPON_TYPES.has(record.reported_weapon & 0x1f)
          );
        const delta = orientationDelta(before.playerOrientation, during.playerOrientation);
        return accepted.length > 0 &&
          primaryCount(during) > semanticBefore &&
          delta * turn.sign > 0.025;
      }, 2_000);
      intermediateRelease = await releaseChordWithIntermediateProof({
        actor, server, playerId, lifeSeq: before.playerLifeSeq,
        releaseOrder: testCase.release, phase,
      });
    } finally {
      if (!intermediateRelease) {
        for (const key of testCase.release) await actor.keyUp(key);
      }
      releaseOffset = traceRecords(server, "human authority input trace").length;
    }
  } catch (error) {
    await actor.releaseKeys?.().catch(() => {});
    throw error;
  }
  }
  if (!Number.isInteger(releaseOffset)) {
    throw new Error(`${phase} ${testCase.first}->${testCase.second} did not record its release boundary`);
  }
  await waitForNeutral(actor, server, playerId, before.playerLifeSeq, releaseOffset);
  const after = (await actor.metricsState()).uiMetrics || {};
  if (primaryCount(after) <= semanticBefore) {
    throw new Error(`${phase} ${testCase.first}->${testCase.second} lost its semantic fire action`);
  }
  const semanticAfter = primaryCount(after);
  await sleep(200);
  const settled = (await actor.metricsState()).uiMetrics || {};
  if (primaryCount(settled) !== semanticAfter) {
    throw new Error(`${phase} ${testCase.first}->${testCase.second} kept firing after both keys were released`);
  }
  return {
    phase,
    firstKey: testCase.first,
    secondKey: testCase.second,
    releaseOrder: testCase.release,
    intermediateRelease,
    lifeSeq: before.playerLifeSeq,
    semanticPrimaryBefore: semanticBefore,
    semanticPrimaryAfter: semanticAfter,
    orientationBefore: before.playerOrientation,
    orientationWhileHeld: during.playerOrientation,
    orientationDeltaWhileHeld: orientationDelta(before.playerOrientation, during.playerOrientation),
    acceptedInputSamples: accepted.slice(0, 24),
  };
}

async function proveSustainedReversal({ actor, server, playerId, phase }) {
  const before = await captureWeaponReadyMetrics(
    actor,
    `${phase} sustained reversal exact live start`,
  );
  const lifeSeq = before.playerLifeSeq;
  const traceOffset = traceRecords(server, "human authority input trace").length;
  const semanticBefore = primaryCount(before);
  const phases = [];
  let releaseOffset = null;
  await actor.keyDown(CONTINUOUS_FIRE_KEY);
  try {
    await sleep(180);
    for (const [index, turnKey] of ["ArrowLeft", "ArrowRight", "ArrowLeft"].entries()) {
      const turn = TURN[turnKey];
      const otherKey = turnKey === "ArrowLeft" ? "ArrowRight" : "ArrowLeft";
      await actor.keyUp(otherKey);
      await sleep(80);
      const phaseStart = (await actor.metricsState()).uiMetrics || {};
      const recordOffset = traceRecords(server, "human authority input trace").length;
      await actor.keyDown(turnKey);
      let during = null;
      let accepted = [];
      try {
        await waitUntil(`${phase} sustained reversal phase ${index + 1} ${turnKey}`, async () => {
          during = (await actor.metricsState()).uiMetrics || {};
          accepted = traceRecords(server, "human authority input trace").slice(recordOffset)
            .filter((record) =>
              record.player_id === playerId &&
              record.life_seq === lifeSeq &&
              (record.input_bits & turn.bit) !== 0 &&
              PRIMARY_WEAPON_TYPES.has(record.reported_weapon & 0x1f)
            );
          const localBits = during.localGameplayState?.inputBits ?? 0;
          return during.playerAlive === true &&
            during.serverPlayerAlive === true &&
            during.playerLifeSeq === lifeSeq &&
            (localBits & turn.bit) !== 0 &&
            orientationDelta(phaseStart.playerOrientation, during.playerOrientation) * turn.sign > 0.04 &&
            accepted.length > 0;
        }, 3_000);
      } finally {
        await actor.keyUp(turnKey);
      }
      phases.push({
        turnKey,
        orientationBefore: phaseStart.playerOrientation,
        orientationWhileHeld: during.playerOrientation,
        orientationDelta: orientationDelta(phaseStart.playerOrientation, during.playerOrientation),
        localInputBits: during.localGameplayState?.inputBits,
        acceptedInputSamples: accepted.slice(0, 24),
      });
    }
  } catch (error) {
    await actor.releaseKeys?.().catch(() => {});
    throw error;
  } finally {
    await actor.keyUp("ArrowLeft").catch(() => {});
    await actor.keyUp("ArrowRight").catch(() => {});
    await actor.keyUp(CONTINUOUS_FIRE_KEY).catch(() => {});
    releaseOffset = traceRecords(server, "human authority input trace").length;
  }
  await waitForNeutral(actor, server, playerId, lifeSeq, releaseOffset);
  const after = (await actor.metricsState()).uiMetrics || {};
  if (primaryCount(after) <= semanticBefore) {
    throw new Error(`${phase} sustained reversal lost its held-Ctrl fire action`);
  }
  const semanticAfter = primaryCount(after);
  await sleep(200);
  const settled = (await actor.metricsState()).uiMetrics || {};
  if (primaryCount(settled) !== semanticAfter) {
    throw new Error(`${phase} sustained reversal kept firing after Ctrl was released`);
  }
  return {
    phase,
    sequence: "Ctrl-held Left->Right->Left",
    lifeSeq,
    semanticPrimaryBefore: semanticBefore,
    semanticPrimaryAfter: semanticAfter,
    phases,
  };
}

async function proveKeyboardChordMatrix({
  actor,
  server,
  playerId,
  phase,
  prepareAttempt = null,
}) {
  const evidence = [];
  for (const testCase of CASES) {
    let completed = null;
    let lastInterruption = null;
    for (let attempt = 0; attempt < 8 && completed == null; attempt += 1) {
      await waitUntil(`${phase} live authority before ${testCase.first}->${testCase.second}`, async () => {
        const metrics = (await actor.metricsState()).uiMetrics || {};
        return weaponInputReady(metrics);
      }, 10_000);
      if (prepareAttempt) {
        await prepareAttempt({ testCase, attempt });
        await waitUntil(`${phase} prepared live authority before ${testCase.first}->${testCase.second}`, async () => {
          const metrics = (await actor.metricsState()).uiMetrics || {};
          return weaponInputReady(metrics);
        }, 10_000);
      }
      // A lifecycle transition can occur while a fire/turn key is held. Send
      // a genuine browser key release and require a rendered neutral sample
      // before the next chord, so the respawn latch is cleared by the same
      // input path a player uses rather than carrying a stale phase key into
      // the next ordering proof.
      await actor.releaseKeys?.().catch(() => {});
      await waitUntil(`${phase} neutral input before ${testCase.first}->${testCase.second}`, async () => {
        const metrics = (await actor.metricsState()).uiMetrics || {};
        const gameplay = metrics.localGameplayState || {};
        return weaponInputReady(metrics) &&
          (gameplay.inputBits ?? 0) === 0 &&
          (metrics.pendingWeapon ?? 0) === 0;
      }, 2_000);
      await sleep(50);
      const before = (await actor.metricsState()).uiMetrics || {};
      try {
        completed = await proveOneChord({ actor, server, playerId, phase, testCase });
      } catch (error) {
        await actor.releaseKeys?.().catch(() => {});
        const after = (await actor.metricsState()).uiMetrics || {};
        const lifecycleInterrupted =
          after.playerAlive !== true ||
          after.serverPlayerAlive !== true ||
          after.playerLifeSeq !== before.playerLifeSeq ||
          after.playerRespawnSeq !== before.playerRespawnSeq;
        if (!lifecycleInterrupted) throw error;
        lastInterruption = {
          attempt,
          message: error.message,
          before: {
            lifeSeq: before.playerLifeSeq,
            respawnSeq: before.playerRespawnSeq,
          },
          after: {
            lifeSeq: after.playerLifeSeq,
            respawnSeq: after.playerRespawnSeq,
            alive: after.playerAlive,
            serverAlive: after.serverPlayerAlive,
          },
        };
        await sleep(150);
      }
    }
    if (completed == null) {
      throw new Error(`${phase} ${testCase.first}->${testCase.second} was repeatedly interrupted by authoritative lifecycle changes: ${JSON.stringify(lastInterruption)}`);
    }
    evidence.push(completed);
    await sleep(200);
  }
  let sustained = null;
  let sustainedInterruption = null;
  for (let attempt = 0; attempt < 8 && sustained == null; attempt += 1) {
    await waitUntil(`${phase} live authority before sustained reversal`, async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      return weaponInputReady(metrics);
    }, 10_000);
    if (prepareAttempt) {
      await prepareAttempt({ testCase: null, attempt, sustained: true });
      await waitUntil(`${phase} prepared live authority before sustained reversal`, async () => {
        const metrics = (await actor.metricsState()).uiMetrics || {};
        return weaponInputReady(metrics);
      }, 10_000);
    }
    await actor.releaseKeys?.().catch(() => {});
    await waitUntil(`${phase} neutral input before sustained reversal`, async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      const gameplay = metrics.localGameplayState || {};
      return weaponInputReady(metrics) &&
        (gameplay.inputBits ?? 0) === 0 &&
        (metrics.pendingWeapon ?? 0) === 0;
    }, 2_000);
    await sleep(50);
    const before = (await actor.metricsState()).uiMetrics || {};
    try {
      sustained = await proveSustainedReversal({ actor, server, playerId, phase });
    } catch (error) {
      await actor.releaseKeys?.().catch(() => {});
      const after = (await actor.metricsState()).uiMetrics || {};
      const lifecycleInterrupted =
        after.playerAlive !== true ||
        after.serverPlayerAlive !== true ||
        after.playerLifeSeq !== before.playerLifeSeq ||
        after.playerRespawnSeq !== before.playerRespawnSeq;
      if (!lifecycleInterrupted) throw error;
      sustainedInterruption = {
        attempt,
        message: error.message,
        before: {
          lifeSeq: before.playerLifeSeq,
          respawnSeq: before.playerRespawnSeq,
        },
        after: {
          lifeSeq: after.playerLifeSeq,
          respawnSeq: after.playerRespawnSeq,
          alive: after.playerAlive,
          serverAlive: after.serverPlayerAlive,
        },
      };
      await sleep(150);
    }
  }
  if (sustained == null) {
    throw new Error(`${phase} sustained reversal was repeatedly interrupted by authoritative lifecycle changes: ${JSON.stringify(sustainedInterruption)}`);
  }
  evidence.push(sustained);
  return evidence;
}

export {
  traceRecords,
  safeZoneThrustEvidence,
  proveSafeZoneFireThrust,
  singleHeldKeyEvidence,
  CASES as FIRE_TURN_CHORD_CASES,
  AFTERBURNER_CASES,
  proveAfterburnerMatrix,
  proveAfterburnerMatrixWithLifecycleRetries,
  proveKeyboardChordMatrix,
  proveFocusLossReleasesHeldChord,
  proveHeldChordSurvivesRepeatPause,
  proveChaosSafeZoneHeldSets,
};
// Chaos authored safety is not its spawn. `tests/e2e/chaos_safe_route.mjs` measures that
// only two of the map's 353 authored safe tiles are reachable by the ship hull from the
// canonical spawn, so until now the primary-fire suppression path had only ever been
// observed in Extreme, where the green happens to be the spawn.
//
// The key semantics matter for what can be staged at all: `held_primary_fire_down` is
// `!shift_down && ctrl_down` (src/game/player/ship.rs:887), because Shift is the bomb/mine
// modifier. A held set that adds Shift therefore stops being a held primary fire - no gun
// delay, no authored stop, and the ship accelerates out of the green, which is exactly how
// the first version of this phase read its own failure. The three-key set here is therefore
// fire + thrust + turn, which keeps primary fire held.
//
// The stop is a real, frequent witness in Chaos: the retained first run recorded 47
// `input_bits & 64` records for this pilot, every one of them at velocity exactly 0/0 about
// every 25 ticks, against `accepted_weapon=66` for the same player firing outside safety.
// Both halves are asserted below.
function chaosHeldSetEvidence({
  state, window, playerId, lifeSeq, expectedBits, minimumRecords, minimumStops, isAuthoredSafe, why,
}) {
  const metrics = state?.uiMetrics;
  const codes = state?.gameKeyBridge?.trackedCodes;
  const mine = window.filter((record) => record.player_id === playerId && record.life_seq === lifeSeq);
  // Records from the staggered presses carry a partial set, so the sample is the records
  // taken while the whole set was held. Weapon admission, by contrast, is judged over every
  // record in the window: nothing may fire at any point of a suppressed hold.
  const held = mine.filter((record) => (record.input_bits & ~64) === expectedBits);
  const stops = held.filter((record) => (record.input_bits & 64) !== 0);
  const fail = (clause, detail) => {
    why.clause = clause;
    why.detail = detail;
    return null;
  };
  if (metrics?.playerAlive !== true || metrics.serverPlayerAlive !== true) {
    return fail("pilot is not live on both sides", { client: metrics?.playerAlive,
      server: metrics?.serverPlayerAlive });
  }
  if (metrics.playerLifeSeq !== lifeSeq) return fail("life sequence changed", metrics.playerLifeSeq);
  if (metrics.inSafeZone !== true) {
    return fail("pilot left authored safety", { x: metrics.playerX, y: metrics.playerY });
  }
  if (!Array.isArray(codes) || codes.length < 2) return fail("held key tracking", codes);
  const bits = metrics.localGameplayState?.inputBits;
  if (((bits ?? -1) & ~64) !== expectedBits) return fail("client input bits", bits);
  if (held.length < minimumRecords) return fail("held records", held.length);
  if (stops.length < minimumStops) return fail("stop-marker records", stops.length);
  const fired = mine.find((record) => record.accepted_weapon !== 0 || record.reported_weapon !== 0);
  if (fired !== undefined) {
    return fail("a weapon was admitted during the hold", { server_tick: fired.server_tick,
      accepted_weapon: fired.accepted_weapon, reported_weapon: fired.reported_weapon });
  }
  const drifted = held.find((record) => !isAuthoredSafe(record.x, record.y));
  if (drifted !== undefined) {
    return fail("a held record left the authored safe tile", { server_tick: drifted.server_tick,
      x: drifted.x, y: drifted.y });
  }
  const moving = stops.find((record) => record.velocity_x !== 0 || record.velocity_y !== 0);
  if (moving !== undefined) {
    return fail("an authored stop record carried velocity", { server_tick: moving.server_tick,
      velocity_x: moving.velocity_x, velocity_y: moving.velocity_y });
  }
  // The server's committed order, so a duplicated or reordered packet cannot inflate the
  // sample the assertion is counting.
  for (let i = 1; i < held.length; i += 1) {
    const advance = (held[i].input_sequence - held[i - 1].input_sequence) & 0xffff;
    if (advance === 0 || advance >= 0x8000) {
      return fail("input sequence is not monotonic", { at: held[i].input_sequence,
        previous: held[i - 1].input_sequence });
    }
  }
  why.clause = null;
  return {
    expectedBits,
    heldRecords: held.length,
    windowRecords: mine.length,
    stopCount: stops.length,
    stopCadenceTicks: stops.length > 1
      ? stops.at(-1).server_tick - stops[0].server_tick
      : null,
    trackedCodes: codes,
    orientation: metrics.playerOrientation,
    position: { x: metrics.playerX, y: metrics.playerY },
  };
}

async function proveChaosHeldSetInSafety({
  actor, server, playerId, lifeSeq, keys, expectedBits, minimumRecords, minimumStops,
  isAuthoredSafe, label, orient, holdHeading,
}) {
  for (const key of keys) await actor.keyUp(key);
  const staged = await captureWeaponReadyMetrics(actor, `${label} Chaos safe-zone stage ready`);
  if (staged.inSafeZone !== true || !isAuthoredSafe(staged.playerX, staged.playerY) ||
      (await actor.keyState()).gameKeyBridge?.trackedCodes?.length !== 0) {
    throw new Error(`${label} requires a live, released pilot on authored Chaos safety: ` +
      JSON.stringify({ inSafeZone: staged.inSafeZone, x: staged.playerX, y: staged.playerY,
        orientation: staged.playerOrientation }));
  }
  // An unarmed pilot would make "no weapon accepted" meaningless, which is how the first
  // Chaos run misread itself. `captureWeaponReadyMetrics` already gates on the client's own
  // `combatViewTimeline.weaponReady`; this restates it where the claim is made.
  if (!weaponInputReady(staged)) {
    throw new Error(`${label} needs an armed pilot inside authored Chaos safety: ` +
      JSON.stringify({ weaponReady: staged.combatViewTimeline?.weaponReady,
        x: staged.playerX, y: staged.playerY, lifeSeq: staged.playerLifeSeq }));
  }
  // Thrust on a four-tile-deep green has to run *along* the green. The authored stop caps
  // speed but not position - measured at about 21px of travel per 0.4s cooldown cycle - so
  // the heading is taken from the map's safe run, not from wherever the ship happens to be
  // pointing, and the sample below is sized from that depth.
  if (orient !== null && holdHeading !== null) {
    await orient(holdHeading.x, holdHeading.y);
  }
  const startedOrientation = (await actor.metricsState()).uiMetrics.playerOrientation;
  // The steering claim is travel *during* the hold, so orientations are sampled across the whole held
  // window rather than comparing one pre-press baseline against whichever instant the witness happened to
  // admit on. A single pair can be satisfied - or defeated - by admission timing: the hold must last long
  // enough to collect its records and stops, and orientation in this engine is turns per continuum, so a
  // short window can measure a fraction of a turn even while the pilot is turning the whole time. Sampling
  // the window makes the quantity the case actually asserts: the pilot kept steering under suppression.
  const heldOrientations = [startedOrientation];
  const offset = traceRecords(server, "human authority input trace").length;
  const why = { clause: "not started", detail: null };
  let evidence = null;
  try {
    // Staggered presses, so this stays a test of held simultaneity and not of same-frame
    // event ordering (which the batch matrix owns).
    for (const key of keys) {
      await actor.keyDown(key);
      await sleep(180);
    }
    const deadline = Date.now() + 15_000;
    while (evidence === null) {
      heldOrientations.push((await actor.metricsState()).uiMetrics.playerOrientation);
      evidence = chaosHeldSetEvidence({
        state: await actor.keyState(),
        window: traceRecords(server, "human authority input trace").slice(offset),
        playerId, lifeSeq, expectedBits, minimumRecords, minimumStops, isAuthoredSafe, why,
      });
      if (evidence !== null) break;
      if (Date.now() > deadline) {
        const metrics = (await actor.metricsState()).uiMetrics || {};
        throw new Error(`${label} held set was not admitted inside authored Chaos safety: ` +
          `stalled on "${why.clause}" with ${JSON.stringify(why.detail)}; ` +
          JSON.stringify({ keys, expectedBits, minimumRecords, minimumStops,
            x: metrics.playerX, y: metrics.playerY, inSafeZone: metrics.inSafeZone,
            // The margin, because a held-set failure 2 px inside a two-tile green is a different finding from a
            // held-set failure 20 px inside: the first is boundary noise, the second is the pilot moving.
            authoredMarginPx: authoredSafetyMargin(isAuthoredSafe ?? (() => false))(
              metrics.playerX ?? 0, metrics.playerY ?? 0),
            // Read from localRenderPrediction (src/game/render/hud/metrics.rs:71). Both metric blocks carry
            // the field (localGameplayState at metrics.rs:477), so this is a single-source choice, not a repair
            // of a field that never arrived.
            inputBits: metrics.localRenderPrediction?.inputBits ?? "absent",
            energy: metrics.energy, lifeSeq: metrics.playerLifeSeq }));
      }
      await sleep(120);
    }
  } finally {
    for (const key of keys) {
      try {
        await actor.keyUp(key);
      } catch { /* report the held-set failure, not the cleanup error */ }
    }
  }
  // Sum of successive wrapped deltas: rotation traveled, not net displacement. Net displacement would let
  // a pilot that turned a full circle satisfy the claim without ever steering *through* the suppression,
  // and it would understate an opposing pair that returns to its heading.
  let travel = 0;
  for (let index = 1; index < heldOrientations.length; index += 1) {
    travel += Math.abs(orientationDelta(heldOrientations[index - 1], heldOrientations[index]));
  }
  const turn = travel;
  // Both quantities are stated because the difference between them is the whole point: net displacement
  // wraps, so a pilot that turns close to a full circle looks nearly motionless to a net-only witness.
  console.log(`held-set ${label}: traveled ${travel.toFixed(3)} turns across `
    + `${heldOrientations.length} orientation samples; net `
    + `${Math.abs(orientationDelta(startedOrientation, evidence.orientation)).toFixed(3)} turns `
    + `(${evidence.heldRecords} records, ${evidence.stopCount} suppression stops)`);
  await waitForNeutral(actor, server, playerId, lifeSeq,
    traceRecords(server, "human authority input trace").length - 1);
  return { keys, turn, heldOrientationSamples: heldOrientations.length, ...evidence };
}

async function proveChaosSafeZoneHeldSets({ actor, server, playerId, isAuthoredSafe, orient,
  thrustHeading, safeHeading }) {
  const staged = await captureWeaponReadyMetrics(actor, "Chaos safe-zone pilot staged");
  const lifeSeq = staged.playerLifeSeq;
  // Thrust holds are sized by the green's authored depth: about 21px of drift per 0.4s
  // stop cycle against a run of a few tiles means a long sample can only end by leaving the
  // surface, which is a physics fact about Chaos' narrow green and not a licence to weaken
  // the witness. The zero-drift turn case therefore carries the strict 16-record / 4-stop
  // sample (attempt 4 held 329 records without moving a pixel), and the release witness
  // below carries the travel claim.
  // Sized from the map and the trace, not chosen to pass: the reachable green runs 64px
  // along its safe axis, the stop stamps every 25 ticks (0.4s at 60tps) and inputs arrive
  // about every 2.4 ticks, so two stops needs roughly twelve records, and a stair-stepping
  // thrust travels about 21px per stop. Twelve records therefore stays inside with margin;
  // a longer sample can only end by leaving the surface.
  const thrustHold = { orient, holdHeading: safeHeading, minimumRecords: 12, minimumStops: 2 };
  // A three-key held set. No case table in either engine contains one. It is the opposing
  // turn pair rather than thrust+turn because turning on a four-tile green rotates the
  // thrust vector off the safe axis and walks the ship out of the authored surface; the
  // opposing pair keeps the ship still, so it can carry the strict sample, and the claim it
  // makes is the one that matters at three keys: the client reports all three bits at once
  // and the suppression stop keeps stamping underneath them.
  const fireBothTurns = await proveChaosHeldSetInSafety({
    actor, server, playerId, lifeSeq,
    keys: ["Control", "ArrowLeft", "ArrowRight"],
    expectedBits: 4 | 8, minimumRecords: 16, minimumStops: 4, isAuthoredSafe,
    label: "fire+both-turns", orient: null, holdHeading: null,
  });
  // Fire held *through* a turn: the pair the matrix only ever fires as an instantaneous
  // weapon-and-aim chord. Suppression is about the weapon edge, so steering has to survive.
  const fireThroughTurn = await proveChaosHeldSetInSafety({
    actor, server, playerId, lifeSeq,
    keys: ["Control", "ArrowLeft"],
    expectedBits: 4, minimumRecords: 16, minimumStops: 4, isAuthoredSafe,
    label: "fire-through-turn", orient: null, holdHeading: null,
  });
  if (fireThroughTurn.turn <= 0.04) {
    throw new Error("held turn did not happen while the weapon edge was suppressed inside " +
      "authored Chaos safety (turn is rotation traveled in turns of a full circle, not radians): " +
      `${JSON.stringify(fireThroughTurn)}`);
  }
  // Fire + thrust on the green, and the release: holding the thrust key must carry the
  // pilot out of the green under real travel, which is only meaningful because the route's
  // stage heading was chosen for authored clearance.
  const fireWithThrust = await proveChaosHeldSetInSafety({
    actor, server, playerId, lifeSeq,
    keys: ["Control", "ArrowUp"],
    expectedBits: 1, isAuthoredSafe, label: "fire+thrust", ...thrustHold,
  });
  // The release witness wants the opposite heading: authored terrain clearance, so the
  // travel it measures is travel and not a wall bounce.
  await orient(thrustHeading.x, thrustHeading.y);
  await actor.keyDown("Control");
  await actor.keyDown("ArrowUp");
  await actor.keyUp("Control");
  let departed = null;
  // Recorded at the release instant, because a 10 s timeout cannot be judged without knowing what the pilot had
  // to cross. boundaryDistanceAlongHeadingPx walks the heading actually faced in 8 px steps until authored
  // safety ends - a straight-line approximation that ignores walls, which is exactly the quantity the budget
  // should be stated against. A failing pass that shows a small distance and no departure means thrust was not
  // doing what the case assumes; a large distance means the pilot was staged deeper inside the green than the
  // budget was tuned on.
  const releaseSample = (await actor.metricsState()).uiMetrics || {};
  let boundaryDistancePx = null;
  if (typeof isAuthoredSafe === "function") {
    for (let step = 0; step <= 240; step += 1) {
      const probeX = (releaseSample.playerX ?? 0) + thrustHeading.x * step * 8;
      const probeY = (releaseSample.playerY ?? 0) + thrustHeading.y * step * 8;
      if (!isAuthoredSafe(probeX, probeY)) { boundaryDistancePx = step * 8; break; }
    }
  }
  console.log(`Chaos safe-zone release: position ${releaseSample.playerX ?? "?"},${releaseSample.playerY ?? "?"} `
    + `speed ${Math.hypot(releaseSample.playerVelocityX ?? 0, releaseSample.playerVelocityY ?? 0).toFixed(2)} px/s `
    + `inSafeZone=${releaseSample.inSafeZone} boundaryDistanceAlongHeadingPx=${boundaryDistancePx} `
    + `heading=${thrustHeading.x},${thrustHeading.y}`);
  // A start-only report cannot say why a release failed. This samples the whole 10 s window once a second:
  // position, speed, the engine's own inSafeZone, the input bits actually reaching the client, and how far the
  // authored boundary still is along the heading faced. Those separate the three shapes a failure can have -
  // thrust not arriving (bits 0), thrust arriving but not moving the ship (speed 0 with bits set), and the ship
  // travelling while the engine still calls it safe (distance shrinking slowly, or the authored predicate
  // disagreeing with the engine's). waitUntil's own message carries none of that, so the trace is rethrown with
  // it rather than left in a log line the failure payload never sees.
  const releaseStartedAt = Date.now();
  const releaseTrace = [];
  let releaseTraceAt = 0;
  const boundaryAlongHeading = (metrics) => {
    if (typeof isAuthoredSafe !== "function") return null;
    const headingSpeed = Math.hypot(metrics.playerVelocityX ?? 0, metrics.playerVelocityY ?? 0);
    // Walk the velocity direction when there is one, since that is the direction travel actually takes;
    // fall back to the commanded heading when the ship is momentarily still.
    const ux = headingSpeed > 0.5 ? (metrics.playerVelocityX ?? 0) / headingSpeed : thrustHeading.x;
    const uy = headingSpeed > 0.5 ? (metrics.playerVelocityY ?? 0) / headingSpeed : thrustHeading.y;
    for (let step = 0; step <= 240; step += 1) {
      if (!isAuthoredSafe((metrics.playerX ?? 0) + ux * step * 8, (metrics.playerY ?? 0) + uy * step * 8)) {
        return step * 8;
      }
    }
    return ">1920";
  };
  try {
    await waitUntil("releasing fire leaves authored Chaos safety under real thrust", async () => {
      departed = (await actor.metricsState()).uiMetrics || {};
      const now = Date.now();
      if (now - releaseTraceAt >= 1000) {
        releaseTraceAt = now;
        releaseTrace.push({
          ms: now - releaseStartedAt,
          x: departed.playerX ?? null, y: departed.playerY ?? null,
          speed: Number(Math.hypot(departed.playerVelocityX ?? 0, departed.playerVelocityY ?? 0).toFixed(2)),
          inSafeZone: departed.inSafeZone ?? "absent",
          inputBits: (departed.localRenderPrediction ?? departed).inputBits ?? "absent",
          boundaryPx: boundaryAlongHeading(departed),
          marginPx: authoredSafetyMargin(isAuthoredSafe ?? (() => false))(
            departed.playerX ?? 0, departed.playerY ?? 0),
        });
      }
      return departed.inSafeZone === false;
    }, 10_000);
  } catch (error) {
    throw new Error(`${error.message}; release trace ${JSON.stringify(releaseTrace)}`, { cause: error });
  }
  await actor.keyUp("ArrowUp");
  if (departed?.playerAlive !== true ||
      Math.hypot(departed.playerVelocityX ?? 0, departed.playerVelocityY ?? 0) < 8) {
    throw new Error("Chaos safe-zone release produced no real travel out of the green: " +
      JSON.stringify({ inSafeZone: departed?.inSafeZone, x: departed?.playerX,
        y: departed?.playerY, vx: departed?.playerVelocityX, vy: departed?.playerVelocityY,
        stage: fireWithThrust.position, thrustHeading }));
  }
  await waitForNeutral(actor, server, playerId, lifeSeq,
    traceRecords(server, "human authority input trace").length - 1);
  return { fireBothTurns, fireThroughTurn, fireWithThrust, departedTo: {
    x: departed.playerX, y: departed.playerY, inSafeZone: departed.inSafeZone } };
}
