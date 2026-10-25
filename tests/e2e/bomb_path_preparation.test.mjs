import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import test from 'node:test';
import { loadRendezvousMap, planPlayerRendezvous } from './player_rendezvous.mjs';
import { movingHullPathIsClear } from './bot_combat_evidence.mjs';

const source = fs.readFileSync(new URL('./bomb_damage.mjs', import.meta.url), 'utf8');
const start = source.indexOf('async function placeVictimInBombPath(');
const end = source.indexOf('\n}\n', start) + 2;
const geometry = loadRendezvousMap('chaos-zone', ['assets/zones']).geometry;
const plan = planPlayerRendezvous(geometry,
  [{ x: 8488.57, y: 7666.27 }, { x: 7128, y: 9064 }],
  { hullRadiusPx: 20, stageRadiusPx: 128 });

async function replay({ wrongView = false, retireLife = false, stage = plan.target } = {}) {
  const shooterMetrics = { selfPlayerId: 0, playerLifeSeq: 1, playerX: stage.x, playerY: stage.y,
    playerAlive: true, serverPlayerAlive: true, playerVelocityX: 0, playerVelocityY: 0, remotePositionX: stage.x + 3.3, remotePositionY: stage.y - 1.2 };
  const victimMetrics = { selfPlayerId: 1, playerLifeSeq: 2, playerX: stage.x + 3.3, playerY: stage.y - 1.2,
    playerAlive: true, serverPlayerAlive: true, playerVelocityX: 2.48, playerVelocityY: -0.12, playerOrientation: 0.84292 };
  const shooter = { combatState: async () => ({ uiMetrics: { ...shooterMetrics } }) };
  const victim = { combatState: async () => ({ uiMetrics: { ...victimMetrics } }) };
  const driven = [];
  const controller = vm.runInNewContext(`(${source.slice(start, end)})`, {
    Date, Math, Number, JSON, process, path, repoRoot: process.cwd(), arena: 'chaos-zone',
    loadRendezvousMap, movingHullPathIsClear,
    verticalKeyMap: async () => { throw new Error('old placement assumes a world vertical thrust axis'); },
    sleep: async () => {},
    driveClientToWorldTarget: async (client, target, timeoutMs, settle, tolerance, complete) => {
      assert.equal(client, victim); assert.equal(settle, true); assert.equal(tolerance, 1);
      assert.ok(timeoutMs > 0 && timeoutMs <= 20_000);
      driven.push({ ...target });
      Object.assign(victimMetrics, { playerX: target.x, playerY: target.y, playerVelocityX: 0, playerVelocityY: 0 });
      Object.assign(shooterMetrics, { remotePositionX: target.x, remotePositionY: target.y + (wrongView ? 20 : 0) });
      if (retireLife) victimMetrics.playerLifeSeq++;
      const completed = await complete();
      if (wrongView) {
        assert.equal(completed, false, 'the real two-view placement predicate must remain closed');
        throw new Error('native driver cannot complete with the wrong observer placement');
      }
      assert.equal(completed, true);
      return { reached: true, completed: true };
    },
  });
  return { placement: await controller(shooter, victim, 'native diagonal victim'), driven };
}

test('bomb preparation uses a two-axis real-key driver after a diagonal-heading rendezvous', async () => {
  const result = await replay();
  assert.deepEqual(result.driven, [{ x: plan.target.x, y: plan.target.y + 94 }]);
  assert.equal(result.placement.victim.playerLifeSeq, 2);
});

test('a displaced observer view cannot satisfy the unchanged bomb placement contract', async () => {
  await assert.rejects(() => replay({ wrongView: true }), /cannot complete with the wrong observer placement/);
});

test('the captured small rendezvous stage does not certify the94px bomb corridor', () => {
  assert.equal(movingHullPathIsClear({ x: 8491.31, y: 9047.67, velocityX: 1.95, velocityY: 81.27,
    seconds: 1, hullIsOpen: (x, y) => geometry.authoredHullIsTraversable(x, y, 20) }), false);
  assert.equal(movingHullPathIsClear({ x: plan.target.x, y: plan.target.y, velocityX: 0, velocityY: 94,
    seconds: 1, hullIsOpen: (x, y) => geometry.authoredHullIsTraversable(x, y, 20) }), true);
});


test('a changed victim life cannot complete the original placement attempt', async () => {
  await assert.rejects(() => replay({ retireLife: true }), /crossed a player lifecycle boundary/);
});

test('the actual controller refuses the old stage corridor before steering into its wall', async () => {
  await assert.rejects(() => replay({ stage: { x: 8493.26, y: 9034.94 } }),
    /lacks a full-hull clear corridor/);
});
