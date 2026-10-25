import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {authoredMapGeometry,authoredSafeTiles,authoredSafetyDistance} from './authored_map_geometry.mjs';
import {extremeChordStageRoute} from './extreme_chord_stage.mjs';
const bytes=fs.readFileSync(new URL('../../assets/zones/extreme/doomsy8.lvl',import.meta.url));
const geometry=authoredMapGeometry(bytes),safetyDistance=authoredSafetyDistance(authoredSafeTiles(bytes));

test('retained Extreme wall contact is not a direct route back to the old stage',()=>{
  assert.equal(geometry.authoredLineIsClear(9694.19,8233.08,9000,8200),false);
  assert.equal(geometry.authoredLineIsClear(9694.19,8233.08,8420,8200),false);
});
test('retained positions and ordinary spawn have reachable full-contract stages',()=>{
  for(const from of [{x:9694.19,y:8233.08},{x:9768.12,y:8220.2},{x:8200,y:8200}]){
    const route=extremeChordStageRoute({geometry,safetyDistance,from,requiresRunway:true});
    assert.ok(route,JSON.stringify(from));
    assert.ok(route.distancePx<=512);
    assert.equal(geometry.authoredLineIsClear(from.x,from.y,route.target.x,route.target.y),true);
    assert.ok(safetyDistance(route.target.x,route.target.y)>192);
    assert.ok(route.heading.runwayPx>=512);
    assert.ok(route.heading.clearancePx>=128);
  }
});
test('no reachable stage fails explicitly instead of inventing a runway',()=>{
  const closed={authoredHullIsTraversable:()=>false,authoredLineIsClear:()=>false};
  assert.equal(extremeChordStageRoute({geometry:closed,safetyDistance:()=>999,
    from:{x:100,y:100},requiresRunway:true,maxDistancePx:32}),null);
  assert.throws(()=>extremeChordStageRoute({geometry,safetyDistance,
    from:{x:NaN,y:100},requiresRunway:true}),/finite bounded/);
});
