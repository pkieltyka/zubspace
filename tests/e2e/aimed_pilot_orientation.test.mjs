import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(new URL('./aimed_bullet_damage.mjs', import.meta.url), 'utf8');
const start = source.indexOf('async function rotateToOrientation(');
const end = source.indexOf('\n}\n', start) + 2;
const controllerSource = source.slice(start, end);
const signed = (from, to) => ((to - from + 0.5) % 1 + 1) % 1 - 0.5;

async function replay(initialOrientation, target, tolerance, framesForPulse = ms => Math.max(1, Math.floor(ms / (1000 / 60)))) {
  // Native model only: the actual captured Terrier advances about0.0095833
  // turns per canonical input step. This does not inject state into a browser.
  let orientation = Math.fround(initialOrientation);
  const pulses = [];
  const client = {
    metricsState: async () => ({ uiMetrics: { playerOrientation: orientation } }),
    pulseOnFreshInputStep: async (key, ms) => {
      pulses.push({ key, ms });
      for (let i = 0; i < framesForPulse(ms); i++) {
        orientation = Math.fround(orientation + (key === 'ArrowRight' ? 1 : -1) * Math.fround(0.575 / 60));
        if (orientation >= 1) orientation = Math.fround(orientation - 1);
        if (orientation < 0) orientation = Math.fround(orientation + 1);
      }
    },
  };
  const controller = vm.runInNewContext(`(${controllerSource})`, {
    signedOrientationDelta: signed,
    rotationDirection: async () => ({ increase: 'ArrowRight', decrease: 'ArrowLeft' }),
    sleep: async () => {},
  });
  const result = await controller(client, target, 'native retained aiming orbit', tolerance);
  assert.ok(Math.abs(signed(result.playerOrientation, target)) <= tolerance);
  assert.equal(Math.floor(Math.fround(result.playerOrientation) * 40) % 40, Math.round(target * 40) % 40);
  assert.ok(pulses.length <= 60, 'do not extend the original attempt budget');
  return { result, pulses };
}

test('the captured Terrier aiming lattice can enter the unchanged south tolerance and wire cell', async () => {
  const { pulses } = await replay(0.49833319, 0.5, 0.007);
  assert.ok(pulses.some(p => p.ms > 20), 'escape the adjacent-heading orbit through a real forward turn');
});

test('the same pilot retains the north wire cell across the orientation wrap', async () => {
  await replay(0.99833319, 0, 0.007);
});

test('an already eligible heading needs no additional turn', async () => {
  const { pulses } = await replay(0.503, 0.5, 0.007);
  assert.equal(pulses.length, 0);
});

test('an unresponsive pilot still fails within the original sixty pulses', async () => {
  await assert.rejects(() => replay(0.49833319, 0.5, 0.007, () => 0), /could not rotate/);
});
