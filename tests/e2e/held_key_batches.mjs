import { INPUT_LATENCY_CAPTURE } from "./input_latency_evidence.mjs";
import { traceRecords } from "./keyboard_chords.mjs";
import { waitUntil } from "./combat_replay.mjs";

const KEYS = ["Control", "ArrowLeft", "ArrowRight", "ArrowUp", "Shift"];
const CODES = ["ControlLeft", "ArrowLeft", "ArrowRight", "ArrowUp", "ShiftLeft"];
const BITS = [0, 4, 8, 1, 16];
export const BATCH_CAPTURE = INPUT_LATENCY_CAPTURE;
export const heldBits = mask => BITS.reduce((bits, bit, i) => bits | ((mask & (1 << i)) ? bit : 0), 0);
export const heldCodes = mask => CODES.filter((_, i) => mask & (1 << i)).sort();
export const batchChanges = (before, after) => KEYS.flatMap((key, i) =>
  ((before ^ after) & (1 << i)) ? [{ key, type: after & (1 << i) ? "keyDown" : "keyUp" }] : []);

export function assertBatchDraw(capture, afterSerial, before, after, lifeSeq) {
  const events = capture.inputs.filter(event => event.serial > afterSerial);
  const expected = batchChanges(before, after).map(({ key, type }) =>
    [type.toLowerCase(), CODES[KEYS.indexOf(key)]]);
  if (JSON.stringify(events.map(event => [event.kind, event.code])) !== JSON.stringify(expected)) {
    throw new Error(`batch physical events differ: ${JSON.stringify({ before, after, expected, events })}`);
  }
  if (events.length === 0) throw new Error("empty batch is not input evidence");
  const last = events.at(-1);
  const frame = capture.frames.find(frame => frame.renderOrdinal === last.renderOrdinal + 1);
  if (!events.every((event, i) => event.serial === afterSerial + i + 1 && event.playerLifeSeq === lifeSeq && Number.isInteger(event.renderOrdinal)) ||
      !frame || frame.playerLifeSeq !== lifeSeq || !frame.playerAlive || !frame.serverPlayerAlive ||
      frame.localRenderPrediction?.inputBits !== heldBits(after)) {
    throw new Error(`batch next-draw state differs: ${JSON.stringify({ before, after, lifeSeq, events, frame })}`);
  }
  return { before, after, events, frame,
    sameFrame: events.length > 1 && events.every(event => event.renderOrdinal === last.renderOrdinal) };
}

export function heldAuthorityEvidence(state, records, playerId, lifeSeq, mask) {
  const metrics = state?.uiMetrics;
  const expected = heldBits(mask);
  const tail = records.filter(record => record.player_id === playerId && record.life_seq === lifeSeq).slice(-16);
  // Bit 64 is the authored safe-zone stop marker, not a physical held key.
  // This test does not claim firing in safety or weapon/Afterburner admission.
  if (!metrics?.playerAlive || !metrics.serverPlayerAlive || metrics.playerLifeSeq !== lifeSeq ||
      JSON.stringify(state.gameKeyBridge?.trackedCodes) !== JSON.stringify(heldCodes(mask)) ||
      !Number.isInteger(metrics.localGameplayState?.inputBits) ||
      (metrics.localGameplayState?.inputBits & ~64) !== expected ||
      tail.length !== 16 || !tail.every(record => (record.input_bits & ~64) === expected && Number.isInteger(record.input_bits) && Number.isInteger(record.input_sequence))) return null;
  for (let i = 1; i < tail.length; i += 1) {
    const advance = (tail[i].input_sequence - tail[i - 1].input_sequence) & 0xffff;
    if (!Number.isInteger(tail[i].input_sequence) || advance === 0 || advance >= 0x8000) return null;
  }
  return { trackedCodes: state.gameKeyBridge.trackedCodes, inputBits: metrics.localGameplayState.inputBits, tail };
}

export async function proveHeldKeyBatches({ actor, server, playerId, capture }) {
  const state = await actor.keyState();
  if (state.gameKeyBridge?.trackedCodes?.length !== 0) throw new Error("batch matrix requires released physical keys");
  const lifeSeq = state.uiMetrics.playerLifeSeq;
  // Visit every held set, alternating complements to exercise multi-key
  // press/release batches instead of disguising a one-key Gray-code walk.
  const masks = Array.from({ length: 16 }, (_, i) => [31 - i, i]).flat();
  masks.push(0);
  const evidence = [];
  let held = 0;
  try {
    for (const mask of masks) {
      if (mask === held) continue;
      const before = await capture();
      const serial = before.inputs.at(-1)?.serial ?? 0;
      const offset = traceRecords(server, "human authority input trace").length;
      const previous = held;
      held = mask; // Cleanup releases the requested set even if dispatch fails.
      await actor.keyBatch(batchChanges(previous, mask));
      let authority = null;
      await waitUntil(`held-set ${mask} reaches physical/local/server state`, async () => {
        authority = heldAuthorityEvidence(await actor.keyState(),
          traceRecords(server, "human authority input trace").slice(offset), playerId, lifeSeq, mask);
        return authority !== null;
      }, 5_000);
      evidence.push({ ...assertBatchDraw(await capture(), serial, previous, mask, lifeSeq), authority });
    }
  } finally {
    // Release all source ids, including a partially dispatched failing batch.
    await actor.keyBatch(KEYS.map(key => ({ type: "keyUp", key })));
  }
  const simultaneous = evidence.filter(proof => proof.sameFrame);
  if (!simultaneous.some(proof => proof.events.filter(event => event.kind === "keydown").length > 1) ||
      !simultaneous.some(proof => proof.events.filter(event => event.kind === "keyup").length > 1)) {
    throw new Error(`browser did not realize simultaneous press and release coverage: ${JSON.stringify(evidence)}`);
  }
  return { lifeSeq, heldSets: [...new Set(evidence.map(proof => proof.after))].length,
    sameFrameBatches: simultaneous.length, evidence };
}

export function heldRespawnSuppressionEvidence(state, records, playerId, lifeSeq, mask) {
  const metrics = state?.uiMetrics;
  const tail = records.filter(record => record.player_id === playerId && record.life_seq === lifeSeq).slice(-16);
  if (metrics?.playerAlive !== true || metrics.serverPlayerAlive !== true || metrics.playerLifeSeq !== lifeSeq ||
      JSON.stringify(state.gameKeyBridge?.trackedCodes) !== JSON.stringify(heldCodes(mask)) ||
      metrics.localGameplayState?.inputBits !== 0 || metrics.pendingWeapon !== 0 || tail.length !== 16 ||
      !tail.every(record => record.input_bits === 0 && record.reported_weapon === 0 && record.accepted_weapon === 0 && Number.isInteger(record.input_sequence))) return null;
  for (let i = 1; i < tail.length; i += 1) {
    const advance = (tail[i].input_sequence - tail[i - 1].input_sequence) & 0xffff;
    if (advance === 0 || advance >= 0x8000) return null;
  }
  return { trackedCodes: state.gameKeyBridge.trackedCodes, tail };
}

export async function beginHeldDeath({ actor, server, playerId, lifeSeq }) {
  const offset = traceRecords(server, "human authority input trace").length;
  await actor.keyBatch(batchChanges(0, 7));
  let held = null;
  await waitUntil("fire and opposing turns reach authority before death", async () => {
    held = heldAuthorityEvidence(await actor.keyState(),
      traceRecords(server, "human authority input trace").slice(offset), playerId, lifeSeq, 7);
    return held !== null;
  }, 5_000);
  const capture = await actor.captureInput();
  return { held, serial: capture.inputs.at(-1)?.serial, lifeSeq };
}

export async function finishHeldDeath({ actor, server, playerId, lifeSeq, before }) {
  const records = () => traceRecords(server, "human authority input trace");
  const capture = await actor.captureInput();
  if (!Number.isInteger(before.serial) || capture.inputs.at(-1)?.serial !== before.serial || lifeSeq === before.lifeSeq) {
    throw new Error(`physical keys changed before across-death proof: ${JSON.stringify({ before, lifeSeq, inputs: capture.inputs })}`);
  }
  let suppressed = null;
  let partiallyReleased = null;
  let fresh = null;
  try {
    await waitUntil("across-death held keys remain suppressed on the new life", async () => {
      suppressed = heldRespawnSuppressionEvidence(await actor.keyState(), records(), playerId, lifeSeq, 7);
      return suppressed !== null;
    }, 5_000);
    await actor.keyBatch(batchChanges(7, 1));
    const offset = records().length;
    await waitUntil("releasing old turns does not resume old fire", async () => {
      partiallyReleased = heldRespawnSuppressionEvidence(await actor.keyState(), records().slice(offset), playerId, lifeSeq, 1);
      return partiallyReleased !== null;
    }, 5_000);
    await actor.keyBatch(batchChanges(1, 3));
    const freshOffset = records().length;
    await waitUntil("fresh turn recovers input while old fire stays physically held", async () => {
      fresh = heldAuthorityEvidence(await actor.keyState(), records().slice(freshOffset), playerId, lifeSeq, 3);
      return fresh !== null;
    }, 5_000);
  } finally {
    await actor.keyBatch(["Control", "ArrowLeft", "ArrowRight"].map(key => ({ type: "keyUp", key })));
  }
  let released = null;
  const releaseOffset = records().length;
  await waitUntil("across-death keys release completely", async () => {
    released = heldAuthorityEvidence(await actor.keyState(), records().slice(releaseOffset), playerId, lifeSeq, 0);
    return released !== null;
  }, 5_000);
  return { before, lifeSeq, suppressed, partiallyReleased, fresh, released };
}
