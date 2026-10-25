import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import { authoredMapGeometry, hullOverlapsOnlyOpenDoors } from './authored_map_geometry.mjs';

const geometry = authoredMapGeometry(fs.readFileSync(new URL('../../assets/zones/chaos-zone/bdhadar.lvl', import.meta.url)));
const origin = { playerX: 7119.9, playerY: 8267.1, doorSeed: 230, doorClockFresh: true };

test('retained Chaos origin overlaps a naturally open door, not static clear terrain', () => {
  assert.equal(geometry.authoredTerrainHullIsTraversable(origin.playerX, origin.playerY), false);
  assert.equal(hullOverlapsOnlyOpenDoors(geometry, origin), true);
});

test('closed, stale or missing door evidence never excuses a blocked route origin', () => {
  for (const change of [{ doorSeed: 255 }, { doorClockFresh: false }, { doorSeed: undefined }]) {
    assert.equal(hullOverlapsOnlyOpenDoors(geometry, { ...origin, ...change }), false);
  }
  assert.equal(hullOverlapsOnlyOpenDoors({ tileAt: () => 1 }, origin), false);
  assert.equal(hullOverlapsOnlyOpenDoors({ tileAt: () => 0 }, origin), false);
  assert.equal(hullOverlapsOnlyOpenDoors(geometry, { ...origin, playerX: NaN }), false);
});
