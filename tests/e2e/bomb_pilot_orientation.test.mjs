import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = fs.readFileSync(new URL("./bomb_damage.mjs", import.meta.url), "utf8");
const start = source.indexOf("async function rotateToOrientation("), end = source.indexOf("\n}\n", start) + 2;
const signed = (from, to) => ((to - from + 0.5) % 1 + 1) % 1 - 0.5;

async function replay({ unresponsive = false, initial = 0.50749981 } = {}) {
  let orientation = Math.fround(initial);
  const pulses = [];
  // Captured released states alternate0.50749981/0.48333317. Every30ms
  // ordinary hold spans two logical steps; a fresh20ms pulse admits one.
  const turn = (key, ms, frames) => {
    pulses.push({ key, ms, frames });
    for (let n = 0; !unresponsive && n < frames; n++) {
      orientation = Math.fround(orientation + (key === "ArrowRight" ? 1 : -1) * Math.fround(0.725 / 60));
      if (orientation < 0) orientation = Math.fround(orientation + 1);
      if (orientation >= 1) orientation = Math.fround(orientation - 1);
    }
  };
  const client = {
    combatState: async () => ({ uiMetrics: { playerOrientation: orientation } }),
    hold: async (key, ms) => turn(key, ms, Math.ceil(ms / (1000 / 60))),
    pulseOnFreshInputStep: async (key, ms) => turn(key, ms, Math.max(1, Math.floor(ms / (1000 / 60)))),
  };
  const controller = vm.runInNewContext(`(${source.slice(start, end)})`, {
    signedOrientationDelta: signed, sleep: async () => {},
    rotationDirection: async () => ({ increase: "ArrowRight", decrease: "ArrowLeft" }),
  });
  const result = await controller(client, 0.5, "retained Warbird bomb aim", 0.006);
  assert(Math.abs(signed(result.playerOrientation, 0.5)) <= 0.006);
  assert(pulses.length <= 50, "preserve the original attempt ceiling");
  return pulses;
}

test("captured Warbird aim can enter the unchanged bomb tolerance with a fresh logical pulse", async () => {
  const pulses = await replay();
  assert(pulses.some(p => p.frames === 1));
});

test("an already aimed bomb pilot needs no turn, and an unresponsive one still fails", async () => {
  assert.deepEqual(await replay({ initial: 0.5 }), []);
  await assert.rejects(replay({ unresponsive: true }), /could not rotate shooter/);
});
