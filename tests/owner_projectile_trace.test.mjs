import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

const html = fs.readFileSync(new URL("../index.html", import.meta.url), "utf8");

function bridge(name, next, enabled = true) {
  const start = html.indexOf(`importObject.env.${name} =`);
  const nextStart = html.indexOf(next, start);
  const end = html.lastIndexOf("};", nextStart) + 2;
  assert.ok(start >= 0 && nextStart > start && end > start, "extract the actual shipped bridge");
  let payload;
  let now = 0;
  const context = vm.createContext({
    importObject: { env: {} },
    window: { __zubspaceRemoteSpriteTrace: [], __zubspaceMetricsPublishCount: 0 },
    traceEnabled: enabled,
    qaTraceTotals: { remoteSprites: 0 },
    decoder: new TextDecoder(),
    heapBytes: () => payload,
    Date: { now: () => 1000 + now },
    performance: { now: () => now },
    console,
  });
  vm.runInContext(html.slice(start, end), context);
  return {
    window: context.window,
    send(value) {
      now += 0.125;
      payload = new TextEncoder().encode(JSON.stringify(value));
      assert.equal(context.importObject.env[name](0, payload.length), 0);
    },
  };
}

test("admission trace preserves exact shot identities and reports lost ring entries", () => {
  const capture = bridge("zub_weapon_admission_event", "importObject.env.zub_qa_trace_context =");
  for (let i = 0; i < 105; i++) {
    capture.send({ phase: "owner-bomb-handoff", activation: "18446744073709551615", inputSequence: i });
  }
  const { window } = capture;
  assert.equal(window.__zubspaceWeaponAdmissionTraceTotal, 105);
  assert.equal(window.__zubspaceWeaponAdmissionTraceDropped, 5);
  assert.equal(window.__zubspaceWeaponAdmissionTrace.length, 100);
  const first = window.__zubspaceWeaponAdmissionTrace[0];
  const last = window.__zubspaceWeaponAdmissionTrace.at(-1);
  assert.equal(first.traceOrdinal, 6);
  assert.equal(first.inputSequence, 5);
  assert.equal(last.traceOrdinal, 105);
  assert.equal(last.activation, "18446744073709551615");
  assert.ok(last.monotonicMs > first.monotonicMs);
});

test("sprite frame ordinals expose overwritten frames even when HUD metrics do not advance", () => {
  const capture = bridge("zub_remote_sprite_frame", "function installZubspaceLoadingBridge()");
  for (let i = 0; i < 1202; i++) {
    capture.send({ predictedBombs: { total: 1, overflow: 0, entries: [{ activation: "17", inputSequence: 42 }] } });
  }
  const frames = capture.window.__zubspaceRemoteSpriteTrace;
  assert.equal(frames.length, 1200);
  assert.equal(frames[0].spriteFrameOrdinal, 3);
  assert.equal(frames.at(-1).spriteFrameOrdinal, 1202);
  assert.equal(frames.at(-1).renderOrdinal, 0, "HUD publications are a separate clock");
  assert.equal(frames.at(-1).predictedBombs.entries[0].inputSequence, 42);
});

test("ordinary play does not retain admission or sprite payloads", () => {
  const admission = bridge("zub_weapon_admission_event", "importObject.env.zub_qa_trace_context =", false);
  admission.send({ phase: "owner-bomb-handoff" });
  assert.equal(admission.window.__zubspaceWeaponAdmissionTrace, undefined);
  assert.equal(admission.window.__zubspaceWeaponAdmissionTraceTotal, undefined);
  const sprite = bridge("zub_remote_sprite_frame", "function installZubspaceLoadingBridge()", false);
  sprite.send({ predictedBombs: { entries: [] } });
  assert.equal(sprite.window.__zubspaceRemoteSpriteTrace.length, 0);
});
