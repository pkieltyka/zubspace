import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

// Exercise the actual optional DataChannel filters. These byte fixtures are
// protocol units, not browser gameplay injection or a physics substitute.
const html = fs.readFileSync(new URL("../../index.html", import.meta.url), "utf8");
function fixture(mode) {
  const start = html.indexOf("          function releaseMovementCorrectionPhase("),
    end = html.indexOf("          function readLeU16(", start);
  assert(start >= 0 && end > start);
  const phased = mode === "phased", configured = phased || mode === "armed";
  const state = { movementCorrectionOffersSeen: 0, movementCorrectionOffersDropped: 0,
    movementCorrectionOffersRecovered: 0, movementCorrectionOffersHeld: 0,
    movementCorrectionOfferReleased: !phased, movementCorrectionOfferReliableId: null,
    movementCorrectionAcceptancesSeen: 0, movementCorrectionAcceptancesDropped: 0,
    movementCorrectionAcceptancesRecovered: 0, movementCorrectionAcceptancesHeld: 0,
    movementCorrectionAcceptanceReleased: !phased, movementCorrectionAcceptanceReliableId: null };
  const logs = [], window = { __zubspaceNetworkStats: {} };
  const ctx = vm.createContext({ state, window,
    movementCorrectionPhaseControlled: phased, movementCorrectionImpairmentConfigured: configured,
    readLeU32: (p, offset) => new DataView(p.buffer, p.byteOffset, p.byteLength).getUint32(offset, true),
    networkLog: (...args) => logs.push(args), packetLog: (...args) => logs.push(args) });
  vm.runInContext(html.slice(start, end), ctx);
  return { state, window, ctx, logs,
    offer: packet => ctx.maybeImpairMovementCorrectionOffer(packet, "zubspace-reliable"),
    terminal: packet => ctx.maybeImpairMovementCorrectionAcceptance(packet, "zubspace-reliable") };
}
function packet(kind, id) {
  const bytes = new Uint8Array(kind === "offer" ? 53 : 55);
  bytes.set([0, 3]); new DataView(bytes.buffer).setUint32(2, id, true);
  bytes[6] = kind === "offer" ? 0x7b : 0x7c;
  if (kind !== "offer") bytes[7] = 3;
  return bytes;
}

test("phased offer loss survives setup until release, then requires an actual retransmission", () => {
  const f = fixture("phased"), offer = packet("offer", 17), original = offer.slice();
  assert.equal(f.offer(offer), true); // original lost copy
  assert.equal(f.offer(offer), true); assert.equal(f.offer(offer), true); // retries held before measured phase
  assert.equal(f.state.movementCorrectionOffersDropped, 1);
  assert.equal(f.state.movementCorrectionOffersHeld, 2);
  assert.equal(f.state.movementCorrectionOffersRecovered, 0);
  assert.equal(f.offer(packet("offer", 999)), false, "another reliable fact is not swallowed");
  assert.equal(f.window.__zubspaceReleaseMovementCorrectionOfferForTest(), true);
  assert.equal(f.window.__zubspaceReleaseMovementCorrectionOfferForTest(), false, "release is one-shot");
  assert.equal(f.state.movementCorrectionOffersRecovered, 0, "release cannot invent packet delivery");
  assert.equal(f.offer(offer), false); assert.equal(f.offer(offer), false);
  assert.equal(f.state.movementCorrectionOffersRecovered, 1);
  assert.deepEqual(offer, original);
});

test("the missing terminal remains observable until the queued-action assertion releases recovery", () => {
  const f = fixture("phased"), terminal = packet("terminal", 18);
  assert.equal(f.terminal(terminal), true); assert.equal(f.terminal(terminal), true);
  assert.equal(f.state.movementCorrectionAcceptancesDropped, 1);
  assert.equal(f.state.movementCorrectionAcceptancesHeld, 1);
  assert.equal(f.state.movementCorrectionAcceptancesRecovered, 0);
  assert.equal(f.ctx.releaseMovementCorrectionPhase("unknown"), false);
  assert.equal(f.window.__zubspaceReleaseMovementCorrectionAcceptanceForTest(), true);
  assert.equal(f.state.movementCorrectionAcceptancesRecovered, 0);
  assert.equal(f.terminal(terminal), false);
  assert.equal(f.state.movementCorrectionAcceptancesRecovered, 1);
});

test("legacy armed loss preserves its original automatic recovery and exposes no phase control", () => {
  const f = fixture("armed"), offer = packet("offer", 17), terminal = packet("terminal", 18);
  assert.equal(f.offer(offer), true); assert.equal(f.offer(offer), false);
  assert.equal(f.terminal(terminal), true); assert.equal(f.terminal(terminal), false);
  assert.equal(f.state.movementCorrectionOffersHeld, 0);
  assert.equal(f.state.movementCorrectionAcceptancesHeld, 0);
  assert.equal(f.window.__zubspaceReleaseMovementCorrectionOfferForTest, undefined);
  assert.equal(f.ctx.releaseMovementCorrectionPhase("offer"), false);
});

test("ordinary URLs and unrelated or malformed packets cannot enter the phase fixture", () => {
  const f = fixture(null), before = structuredClone(f.state);
  assert.equal(f.offer(packet("offer", 17)), false); assert.equal(f.terminal(packet("terminal", 18)), false);
  assert.equal(f.window.__zubspaceReleaseMovementCorrectionAcceptanceForTest, undefined);
  assert.equal(f.ctx.releaseMovementCorrectionPhase("acceptance"), false);
  assert.deepEqual(f.state, before); assert.equal(f.logs.length, 0);
  const phased = fixture("phased"), wrong = packet("terminal", 18); wrong[7] = 4;
  assert.equal(phased.terminal(wrong), false); assert.equal(phased.offer(new Uint8Array(52)), false);
  assert.equal(phased.state.movementCorrectionAcceptancesSeen, 0);
});
