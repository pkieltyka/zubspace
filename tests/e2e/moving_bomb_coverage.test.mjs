import test from 'node:test';
import assert from 'node:assert/strict';
import {movingBombShotDisposition} from './moving_bomb_coverage.mjs';

test('retained 29px same-life bomb pair cannot satisfy 64px shot-spacing precondition',()=>{
  const anchor={target_id:768,target_life_seq:2,collision_anchor_x:8308,collision_anchor_y:8226};
  const target={id:768,lifeSeq:2,displayX:8287,displayY:8206};
  assert.deepEqual(movingBombShotDisposition(anchor,target),{action:'separate',ready:false,travel:29});
  assert.equal(movingBombShotDisposition(anchor,{...target,displayX:8372,displayY:8226}).ready,true);
  assert.equal(movingBombShotDisposition(anchor,{...target,displayX:8371.99,displayY:8226}).ready,false);
  assert.equal(movingBombShotDisposition(anchor,{...target,id:769,displayX:8500}).ready,false);
  assert.equal(movingBombShotDisposition(anchor,{...target,lifeSeq:3,displayX:8500}).ready,false);
  assert.equal(movingBombShotDisposition(anchor,{...target,displayX:NaN}).ready,false);
  assert.equal(movingBombShotDisposition(null,target).action,'first-shot');
});
