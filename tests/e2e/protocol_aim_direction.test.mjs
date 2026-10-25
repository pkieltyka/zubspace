import assert from "node:assert/strict";
import test from "node:test";
import { orientClientForProtocolDirection } from "./combat_replay.mjs";

function aimingClient({ direction = 1, changeOnReleasedRead = false } = {}) {
  let sequence = 0;
  let orientation = 0.025;
  let nativeDirection = direction;
  const turns = [];
  return {
    turns,
    metricsState: async () => {
      if (changeOnReleasedRead && sequence === 1) nativeDirection = 1;
      return { uiMetrics: { playerOrientation: orientation,
        playerProtocolDirection: nativeDirection, clientInputSeq: sequence++ } };
    },
    press: async key => {
      turns.push(key);
      assert.equal(key, "a", "correct the observed bin 1 toward north bin 0");
      orientation = 0.0125;
      nativeDirection = 0;
    },
  };
}

test("protocol aiming uses the native direction at a rounded display boundary", async () => {
  // Retained attacker: raw f32 orientation 0.02500071, display 0.025.
  // The old JS positive wrap places the display just below the bin boundary.
  assert.equal(Math.floor((((0.025 % 1) + 1) % 1) * 40), 0);
  assert.equal(Math.trunc(Math.fround(Math.fround(0.02500071) * 40)), 1);
  const client = aimingClient();
  const aim = await orientClientForProtocolDirection(client, 0, -1, 500);
  assert.equal(aim.oriented, true);
  assert.deepEqual(client.turns, ["a"], "display rounding cannot certify the wrong physical bin");
  assert.equal(aim.direction, 0);
  assert.equal(aim.metrics.playerProtocolDirection, 0);
});

test("released-step aiming rechecks the native direction", async () => {
  const client = aimingClient({ direction: 0, changeOnReleasedRead: true });
  const aim = await orientClientForProtocolDirection(client, 0, -1, 500);
  assert.deepEqual(client.turns, ["a"], "a released step in bin 1 must not pass as bin 0");
  assert.equal(aim.metrics.playerProtocolDirection, 0);
});

test("an already correct native bin needs no correction despite ambiguous display text", async () => {
  const client = aimingClient({ direction: 0 });
  const aim = await orientClientForProtocolDirection(client, 0, -1, 500);
  assert.equal(aim.oriented, true);
  assert.deepEqual(client.turns, []);
});

for (const value of [undefined, null, -1, 40, 1.5, "0"]) {
  test(`protocol aiming rejects missing/invalid native direction: ${String(value)}`, async () => {
    const client = aimingClient({ direction: 0 });
    const original = client.metricsState;
    client.metricsState = async () => {
      const state = await original();
      state.uiMetrics.playerProtocolDirection = value;
      return state;
    };
    await assert.rejects(orientClientForProtocolDirection(client, 0, -1, 500),
      /exact native protocol direction/);
  });
}

test("released-step aiming cannot replace invalid native evidence with the display", async () => {
  const client = aimingClient({ direction: 0 });
  const original = client.metricsState;
  let calls = 0;
  client.metricsState = async () => {
    const state = await original();
    if (++calls > 1) state.uiMetrics.playerProtocolDirection = 40;
    return state;
  };
  await assert.rejects(orientClientForProtocolDirection(client, 0, -1, 500),
    /exact native protocol direction/);
});
