import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { createSpawnRegionFixture } from './fixture_spawn_region.mjs';
import { loadRendezvousMap } from './player_rendezvous.mjs';
import { planNaturalPrizeNavigation } from './natural_prize_navigation.mjs';

const zoneRoot = path.resolve(new URL('../../assets/zones', import.meta.url).pathname);

test('the visual lane declares only its private Spawn fields and can reach the original natural prizes in time', t => {
  const source = fs.readFileSync(new URL('./timed_prize_visuals.mjs', import.meta.url), 'utf8');
  const start = source.indexOf('  const spawnFixture = createSpawnRegionFixture(');
  const end = source.indexOf('  const loadedMap =', start);
  assert.ok(start > 0 && end > start);
  const fixture = vm.runInNewContext(`(() => { ${source.slice(start, end)} return spawnFixture; })()`,
    { arena: 'chaos-zone', createSpawnRegionFixture });
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  const declared = loadRendezvousMap('chaos-zone', [fixture.root]);
  const shipping = loadRendezvousMap('chaos-zone', [zoneRoot]);
  assert.equal(declared.provenance.mapSha256, shipping.provenance.mapSha256);
  const withoutSpawn = text => text.replace(/^Team[0-3]-(X|Y|Radius)=.*$/gm, '<declared Spawn>');
  assert.equal(withoutSpawn(fs.readFileSync(path.join(declared.provenance.directory, 'arena.conf'), 'utf8')),
    withoutSpawn(fs.readFileSync(path.join(shipping.provenance.directory, 'arena.conf'), 'utf8')));
  assert.deepEqual(fixture.spawnRegion, { x: 512, y: 512, radius: 2 });
  for (const x of [8168, 8184, 8200, 8216]) for (const y of [8168, 8184, 8200, 8216]) {
    for (const prize of [{ x: 8248, y: 8200, prizeId: 18, collectibleId: 10, expiresAtTick: 6233 },
      { x: 8120, y: 8168, prizeId: 17, collectibleId: 3, expiresAtTick: 6233 }]) {
      const original = structuredClone(prize);
      const route = planNaturalPrizeNavigation(declared.geometry, { x, y }, prize, 2000);
      assert.ok(route && route.requiredTtlTicks <= 2000);
      assert.ok(route.remainingTicks >= route.requiredTtlTicks);
      assert.deepEqual(prize, original, 'the preparation may not alter natural prize identity or TTL');
      assert.ok(Math.hypot(route.plan.target.x - prize.x, route.plan.target.y - prize.y) <= 16);
    }
  }
});

test('the retained far random origin exceeded the old20s navigator at its42px/s cap', () => {
  const { geometry } = loadRendezvousMap('chaos-zone', [zoneRoot]);
  const route = planNaturalPrizeNavigation(geometry, { x: 9032, y: 7816 },
    { x: 8248, y: 8200, expiresAtTick: 6233 }, 1433);
  assert.ok(route);
  // A lower bound, excluding turns/braking, already exceeds the failed setup.
  assert.ok(route.plan.routes[0].distancePx / 42 > 20);
  assert.equal(planNaturalPrizeNavigation(geometry, { x: 8200, y: 8200 },
    { x: 8248, y: 8200, expiresAtTick: 6233 }, 6233), null);
});
