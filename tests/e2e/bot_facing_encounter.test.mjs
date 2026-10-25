import test from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { facingLaunchStep, facingBotAimIsClear } from './bot_facing_encounter.mjs';
import { loadRendezvousMap } from './player_rendezvous.mjs';

const open = {authoredHullIsOpen: () => true, authoredHullIsTraversable: () => true,
  authoredLineIsClear: () => true};
const target = {id: 769, lifeSeq: 2, positionX: 8157, positionY: 7883};
const step = (metrics, cycle = 0, geometry = open) => facingLaunchStep({metrics, target, cycle, geometry});

test('retained aged stationary hunter needs approach before any quarter-turn bait', () => {
  const metrics = {playerX: 8820.69, playerY: 7851.78, inSafeZone: false};
  for (const cycle of [0, 1, 2, 3]) {
    const plan = step(metrics, cycle);
    assert.equal(plan.phase, 'approach');
    assert.ok(plan.distance > 660);
    assert.ok(Math.hypot(plan.waypoint.x - target.positionX, plan.waypoint.y - target.positionY) <= 180.001);
  }
});

test('near exposed pilot alternates stable observation and clear bounded-radius diversity', () => {
  const metrics = {playerX: target.positionX + 150, playerY: target.positionY, inSafeZone: false};
  assert.equal(step(metrics, 0).phase, 'observe');
  const orbit = step(metrics, 1);
  assert.equal(orbit.phase, 'diversity');
  assert.equal(Math.hypot(orbit.waypoint.x-target.positionX, orbit.waypoint.y-target.positionY), 150);
  assert.equal(step({...metrics, inSafeZone: true}, 0).phase, 'approach');
});

test('diversity cannot cut a wall or put the pilot behind a blocked native aim corridor', () => {
  const metrics = {playerX: target.positionX + 150, playerY: target.positionY, inSafeZone: false};
  const wall = {...open, authoredHullIsTraversable: (x, y) => Math.hypot(x-(target.positionX+75), y-(target.positionY+75))>10};
  assert.equal(step(metrics, 1, wall).phase, 'observe');
  assert.equal(facingBotAimIsClear({...open, authoredHullIsOpen: () => false}, 0, 0, 0, -150), false);
  assert.equal(facingBotAimIsClear(open, 0, 0, 0, 0), false);
  assert.equal(facingBotAimIsClear(open, NaN, 0, 0, 150), false);
});

test('the actual Extreme map offers a hull-clear firing approach for the retained aged pair', () => {
  const {geometry} = loadRendezvousMap('extreme', [path.resolve('assets/zones')]);
  const metrics = {playerX: 8820.69, playerY: 7851.78, inSafeZone: false};
  const plan = step(metrics, 1, geometry);
  assert.equal(plan.phase, 'approach');
  assert.ok(plan.waypoint && geometry.authoredHullIsOpen(plan.waypoint.x, plan.waypoint.y));
  assert.ok(facingBotAimIsClear(geometry, target.positionX, target.positionY, plan.waypoint.x, plan.waypoint.y));
});
