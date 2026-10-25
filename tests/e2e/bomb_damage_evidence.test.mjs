import assert from "node:assert/strict";
import test from "node:test";
import { shooterRemoteBombEvidence } from "./bomb_damage.mjs";

// Reduced from the retained fa7420e failure: both clients received damage 731,
// but the first fresh rendered snapshot already included two energy of recharge.
function fixture() {
  const event = {
    damageSeq: 8, projectileId: 8, serverTick: 2568,
    victimId: 0, shooterId: 1, victimLifeSeq: 2, shooterLifeSeq: 1,
    weapon: 8868, damage: 731, energyBefore: 1700, energyAfter: 969,
  };
  const receipt = { ...event, attackerId: 1, forSelf: false, selfPlayerId: 1, remoteEnergy: 971 };
  const { damage, ...identity } = receipt;
  return {
    event,
    state: {
      damageTrace: [receipt],
      // A later same-tick self-splash replaces this transient field.
      uiMetrics: { lastNetworkDamageEnergy: 1224 },
      damagePresentationTrace: [{
        damage: { ...identity, amount: damage },
        sameLifeEnergySnapshot: {
          playerId: 0, lifeSeq: 2, serverTick: 2570, energy: 971, serverEnergy: 971,
        },
      }],
    },
  };
}

test("bomb receipt and fresh rendered energy remain valid after authoritative recharge and self-splash", () => {
  const { event, state } = fixture();
  assert.equal(shooterRemoteBombEvidence(state, event), true);
});

test("bomb display evidence rejects missing, stale, wrong-life, and contradictory energy snapshots", () => {
  for (const override of [
    null, { playerId: 9 }, { lifeSeq: 1 }, { serverTick: 2567 },
    { serverTick: undefined }, { energy: 1700 }, { energy: 967 },
    { serverEnergy: 967 }, { energy: 975, serverEnergy: 971 }, { energy: NaN },
  ]) {
    const { event, state } = fixture();
    const witness = state.damagePresentationTrace[0];
    witness.sameLifeEnergySnapshot = override === null ? null : {
      ...witness.sameLifeEnergySnapshot, ...override,
    };
    assert.equal(shooterRemoteBombEvidence(state, event), false, JSON.stringify(override));
  }
});

test("bomb evidence cannot substitute another damage identity or a victim/self-splash receipt", () => {
  for (const field of ["damageSeq", "projectileId", "serverTick", "victimId", "attackerId", "victimLifeSeq", "shooterLifeSeq", "weapon", "damage", "energyAfter"]) {
    for (const target of ["receipt", "presentation"]) {
      const { event, state } = fixture();
      const record = target === "receipt" ? state.damageTrace[0] : state.damagePresentationTrace[0].damage;
      const key = target === "presentation" && field === "damage" ? "amount" : field;
      record[key] += 10;
      assert.equal(shooterRemoteBombEvidence(state, event), false, `${target}.${key}`);
    }
  }
  for (const override of [{ forSelf: true }, { selfPlayerId: 0 }]) {
    const { event, state } = fixture();
    Object.assign(state.damageTrace[0], override);
    assert.equal(shooterRemoteBombEvidence(state, event), false);
  }
  const { event, state } = fixture();
  state.damageTrace = [];
  assert.equal(shooterRemoteBombEvidence(state, event), false);
});

test("bomb snapshot freshness follows the existing 31-bit server tick wrap", () => {
  const { event, state } = fixture();
  event.serverTick = 0x7fff_ffff;
  state.damageTrace[0].serverTick = event.serverTick;
  state.damagePresentationTrace[0].damage.serverTick = event.serverTick;
  state.damagePresentationTrace[0].sameLifeEnergySnapshot.serverTick = 1;
  assert.equal(shooterRemoteBombEvidence(state, event), true);
});
